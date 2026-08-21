//! Building the sweep PSBT.
//!
//! A split is a **sweep**: every UTXO in the account moves to one destination, no change. That
//! is what `drain_wallet` + `drain_to` express, and it is why the invariant check in
//! `ecx_core::finalize_ecx_psbt` expects exactly one output.

use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::{KeychainKind, Wallet};
use bitcoin::{
    Address, Amount, FeeRate, Network, Psbt,
    absolute::LockTime,
    bip32::{DerivationPath, Fingerprint, Xpub},
    secp256k1::Secp256k1,
};
use ecx_chain::{EsploraChain, ScanReadiness};
use ecx_core::{ECX_MAGIC_LOCKTIME, EcxPsbt, InvariantError};

use crate::discovery::{PARALLEL_REQUESTS, guard_synced};
use crate::{DiscoveredAccount, STOP_GAP};

/// ECX will have a near-empty mempool and reset difficulty, so `estimatefee` is absent or noise
/// (`CLAUDE.md` §6). Static floor, deliberately.
pub const DEFAULT_FEERATE_SAT_PER_VB: u64 = 1;

/// A fee this large is a bug, not a preference (§8.6).
pub const MAX_FEE: Amount = Amount::from_sat(200_000);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("scan failed: {0}")]
    Scan(String),
    #[error("descriptor is invalid: {0}")]
    Descriptor(String),
    #[error("nothing to split: this account has no spendable UTXOs")]
    NothingToSplit,
    #[error("could not build the transaction: {0}")]
    Build(String),
    #[error(transparent)]
    Invariant(#[from] InvariantError),
    #[error("chain is not caught up, so the UTXO set cannot be trusted")]
    NotSynced,
}

/// Re-scan the account and build its sweep.
///
/// Rescans rather than caching a `Wallet` across the UI boundary: it costs a second or two and
/// keeps chain state out of the render layer entirely (§10).
pub async fn build_sweep(
    chain: &EsploraChain,
    readiness: ScanReadiness,
    account: &DiscoveredAccount,
    destination: &Address,
    feerate_sat_per_vb: u64,
) -> Result<EcxPsbt, BuildError> {
    guard_synced(readiness).map_err(|_| BuildError::NotSynced)?;

    let mut wallet = Wallet::create::<String>(
        account.descriptor.clone(),
        account.change_descriptor.clone(),
    )
    .network(Network::Bitcoin)
    .create_wallet_no_persist()
    .map_err(|e| BuildError::Descriptor(e.to_string()))?;

    let request = wallet.start_full_scan().build();
    let update = chain
        .client()
        .full_scan::<KeychainKind, _>(request, STOP_GAP, PARALLEL_REQUESTS)
        .await
        .map_err(|e| BuildError::Scan(chain.describe_error(*e).to_string()))?;
    wallet
        .apply_update(update)
        .map_err(|e| BuildError::Scan(e.to_string()))?;

    if wallet.list_unspent().next().is_none() {
        return Err(BuildError::NothingToSplit);
    }

    let feerate = FeeRate::from_sat_per_vb(feerate_sat_per_vb)
        .ok_or_else(|| BuildError::Build("fee rate out of range".into()))?;

    let psbt = {
        let mut builder = wallet.build_tx();
        builder
            // Sweep: everything in, one output out, no change.
            .drain_wallet()
            .drain_to(destination.script_pubkey())
            .fee_rate(feerate)
            // Stamped again by finalize_ecx_psbt, which is the authority. Setting it here too
            // keeps BDK's own fee and size estimates consistent with the transaction we ship.
            .nlocktime(LockTime::from_consensus(ECX_MAGIC_LOCKTIME));
        // NB: never call `only_witness_utxo()`. Trezor needs the full previous transaction for
        // every non-taproot input, and `finalize_ecx_psbt` rejects a PSBT that lacks it (§5.4).
        builder
            .finish()
            .map_err(|e| BuildError::Build(e.to_string()))?
    };

    Ok(ecx_core::finalize_ecx_psbt(psbt, MAX_FEE)?)
}

/// Derive the destination address for a device-derived ECX account.
///
/// `xpub` is the account key at `m/84'/0'/{account}'`; this returns its first receive address,
/// `.../0/0`. Deriving locally is safe — it is public-key math — but it is **not** the same as
/// the user verifying the address on the device screen, which needs a registered Ledger wallet
/// policy and is not implemented yet (§5.3, §12).
pub fn device_destination(xpub: &Xpub) -> Result<Address, BuildError> {
    let secp = Secp256k1::verification_only();
    let path: DerivationPath = "m/0/0".parse().expect("static path");
    let derived = xpub
        .derive_pub(&secp, &path)
        .map_err(|e| BuildError::Descriptor(e.to_string()))?;
    let pubkey = bitcoin::CompressedPublicKey(derived.public_key);
    Ok(Address::p2wpkh(&pubkey, Network::Bitcoin))
}

/// The account index we put ECX in: a fresh account, never used on Bitcoin (§7.2).
pub const ECX_DESTINATION_ACCOUNT: u32 = 1;

/// Path of the ECX destination account for a given script kind.
pub fn destination_account_path() -> DerivationPath {
    crate::ScriptKind::P2wpkh.account_path(ECX_DESTINATION_ACCOUNT)
}

/// Everything the review screen needs, derived from the PSBT the user is about to approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepSummary {
    pub input_count: usize,
    pub total_in: Amount,
    pub sending: Amount,
    pub fee: Amount,
    pub locktime: u32,
    pub destination: String,
    pub fingerprint: Fingerprint,
    /// Base64 PSBT, for inspection and for the air-gap path.
    pub psbt_base64: String,
    /// True when every non-taproot input carries its previous transaction (§5.4).
    pub has_prev_txs: bool,
}

pub fn summarize(
    psbt: &EcxPsbt,
    destination: &Address,
    fingerprint: Fingerprint,
) -> Result<SweepSummary, InvariantError> {
    let intent = psbt.intent()?;
    let inner: &Psbt = psbt.psbt();
    let total_in: Amount = intent.inputs.values().map(|o| o.value).sum();
    let sending: Amount = intent.outputs.iter().map(|o| o.value).sum();

    let has_prev_txs = inner.inputs.iter().enumerate().all(|(i, input)| {
        let is_taproot = input.tap_internal_key.is_some()
            || input
                .witness_utxo
                .as_ref()
                .is_some_and(|o| o.script_pubkey.is_p2tr());
        let _ = i;
        is_taproot || input.non_witness_utxo.is_some()
    });

    Ok(SweepSummary {
        input_count: intent.inputs.len(),
        total_in,
        sending,
        fee: intent.fee,
        locktime: inner.unsigned_tx.lock_time.to_consensus_u32(),
        destination: destination.to_string(),
        fingerprint,
        psbt_base64: inner.to_string(),
        has_prev_txs,
    })
}
