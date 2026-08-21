//! The split flow, orchestrating the other crates in the order `CLAUDE.md` §7 requires.
//!
//! **This crate is the seam that makes a second frontend possible.** It owns every step of the
//! flow — connect, read keys, scan, build — and knows nothing about how it will be presented.
//! Progress arrives as [`SplitEvent`]s through a callback, so a GUI can turn them into state
//! updates and a CLI can print them, without either duplicating the sequence.
//!
//! Golden Rule 6: the ECX sweep happens first, always. Replay protection is one-directional, and
//! split ordering is what protects the other direction — it is not an optimization.

use bitcoin::{Address, Amount, Transaction, Txid, bip32::Fingerprint};
use ecx_chain::{BroadcastPermit, ChainSource, EsploraChain, ScanReadiness, TipInfo, now_unix};
use ecx_core::{EcxPsbt, TxIntent};
use ecx_signer::{DeviceKind, SignedTx, Signer};
use ecx_wallet::{
    AccountCandidate, DiscoveredAccount, DiscoveryDepth, SweepSummary, candidates,
    discover as scan_accounts,
};

/// Where the swept coins go.
///
/// An ECX address *is* a Bitcoin address (`CLAUDE.md` §3) — nothing in the string identifies the
/// chain, so a pasted exchange deposit address is unrecoverable and undetectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Derived from the connected device at a fresh account.
    DeviceDerived { account: u32, address: Address },
    /// Typed or pasted. Requires a typed acknowledgement naming the chain (§7.5).
    Pasted {
        address: Address,
        acknowledged: bool,
    },
}

impl Destination {
    /// Golden Rule 7: never broadcast without explicit confirmation.
    pub fn is_confirmed(&self) -> bool {
        match self {
            Destination::DeviceDerived { .. } => true,
            Destination::Pasted { acknowledged, .. } => *acknowledged,
        }
    }

    pub fn address(&self) -> &Address {
        match self {
            Destination::DeviceDerived { address, .. } | Destination::Pasted { address, .. } => {
                address
            }
        }
    }
}

/// Identity of a connected device, with no handle attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub kind: DeviceKind,
    pub label: String,
    pub version: String,
    pub fingerprint: Fingerprint,
}

/// Progress from a long-running step. Both frontends render these; neither interprets the order.
#[derive(Debug, Clone)]
pub enum SplitEvent {
    Connected(DeviceIdentity),
    /// Reading candidate account keys from the device. Fast, no button presses.
    ReadingKeys {
        done: usize,
        total: usize,
        label: String,
    },
    /// Scanning each candidate account against the chain. The slow half.
    Scanning {
        done: usize,
        total: usize,
        label: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SplitError {
    #[error("the destination has not been confirmed")]
    DestinationUnconfirmed,
    #[error("nothing to split: the selected account has no spendable UTXOs")]
    NothingToSplit,
    #[error("chain: {0}")]
    Chain(#[from] ecx_chain::ChainError),
    #[error("device: {0}")]
    Signer(#[from] ecx_signer::SignerError),
    #[error("wallet: {0}")]
    Wallet(#[from] ecx_wallet::WalletError),
    #[error("import: {0}")]
    Import(#[from] ecx_wallet::ImportError),
    #[error(
        "the exported accounts do not all share one master fingerprint — they came from \
         different seeds or different passphrases, and scanning them together would show a \
         blend of wallets"
    )]
    MixedFingerprints,
    #[error("build: {0}")]
    Build(#[from] ecx_wallet::BuildError),
    #[error("invariant: {0}")]
    Invariant(#[from] ecx_core::InvariantError),
}

/// Post-fork difficulty resets to minimum, so reorg risk is elevated and six confirmations is
/// not enough. **CONFIRM against observed alpha block times before release.**
pub const MIN_CONFIRMATIONS: u32 = 30;

/// Read the device's identity.
pub async fn identify(signer: &dyn Signer, label: String) -> Result<DeviceIdentity, SplitError> {
    let fingerprint = signer.master_fingerprint().await?;
    let version = signer.version().await.unwrap_or_else(|_| "unknown".into());
    Ok(DeviceIdentity {
        kind: signer.kind(),
        label,
        version,
        fingerprint,
    })
}

/// Chain status: tip plus whether its results can be trusted (Golden Rule 9).
pub async fn chain_status(chain: &EsploraChain) -> Result<(TipInfo, ScanReadiness), SplitError> {
    let tip = chain.tip().await?;
    Ok((tip, ScanReadiness::assess(tip, now_unix())))
}

/// The full discovery run: read every candidate xpub, then scan each account.
///
/// `CLAUDE.md` §5.6 — the device is a pubkey oracle and cannot tell us which accounts are used;
/// only the chain can answer that.
pub async fn discover(
    chain: &EsploraChain,
    signer: &dyn Signer,
    device_label: String,
    depth: DiscoveryDepth,
    mut on_event: impl FnMut(SplitEvent),
) -> Result<(DeviceIdentity, Vec<DiscoveredAccount>), SplitError> {
    let identity = identify(signer, device_label).await?;
    on_event(SplitEvent::Connected(identity.clone()));

    let candidates: Vec<AccountCandidate> = candidates(depth.accounts);
    let total = candidates.len();
    let mut xpubs = Vec::with_capacity(total);
    for (done, candidate) in candidates.iter().enumerate() {
        on_event(SplitEvent::ReadingKeys {
            done,
            total,
            label: format!("{} {}", candidate.kind.label(), candidate.path),
        });
        let xpub = signer.extended_pubkey(&candidate.path).await?;
        xpubs.push((candidate.clone(), xpub));
    }

    let (_, readiness) = chain_status(chain).await?;
    let accounts = scan_accounts(chain, readiness, identity.fingerprint, &xpubs, depth, |p| {
        on_event(SplitEvent::Scanning {
            done: p.scanned,
            total: p.total,
            label: format!("{} {}", p.current.kind.label(), p.current.path),
        });
    })
    .await?;

    Ok((identity, accounts))
}

/// Discover accounts from an **air-gapped** export, with no device attached.
///
/// The USB [`discover`] asks the device for twelve candidate xpubs. Air-gapped there is nothing
/// to ask, so the device exports its account keys first and we scan whatever it gave us. That is
/// why the air-gap flow has three hops rather than one: keys out, PSBT out, signature back.
///
/// Coverage differs, and the caller should say so. Over USB we probe four script types across
/// three account indices; an export typically carries account 0 for each script type — four of
/// the twelve. Anything the device did not export is invisible to us, so an empty result here
/// means "nothing in what you exported", not "nothing in your wallet".
pub async fn discover_from_export(
    chain: &EsploraChain,
    accounts: &[ecx_wallet::ImportedAccount],
    mut on_event: impl FnMut(SplitEvent),
) -> Result<(Fingerprint, Vec<DiscoveredAccount>), SplitError> {
    let fingerprint =
        ecx_wallet::import::common_fingerprint(accounts).ok_or(SplitError::MixedFingerprints)?;

    let xpubs = ecx_wallet::import::to_candidates(accounts);
    let (_, readiness) = chain_status(chain).await?;
    let found = scan_accounts(
        chain,
        readiness,
        fingerprint,
        &xpubs,
        DiscoveryDepth::default(),
        |p| {
            on_event(SplitEvent::Scanning {
                done: p.scanned,
                total: p.total,
                label: format!("{} {}", p.current.kind.label(), p.current.path),
            });
        },
    )
    .await?;

    Ok((fingerprint, found))
}

/// Derive the ECX destination from the device: a fresh account, never used on Bitcoin (§7.2).
pub async fn device_destination(signer: &dyn Signer) -> Result<Destination, SplitError> {
    let path = ecx_wallet::build::destination_account_path();
    let xpub = signer.extended_pubkey(&path).await?;
    let address = ecx_wallet::device_destination(&xpub)?;
    Ok(Destination::DeviceDerived {
        account: ecx_wallet::build::ECX_DESTINATION_ACCOUNT,
        address,
    })
}

/// A built sweep, with everything the signing step needs kept together.
///
/// The `intent` matters as much as the PSBT. `verify_signed` compares the device's output
/// against what the **user confirmed** — so the intent must be derived here, from the PSBT that
/// was actually displayed, and carried forward. Re-deriving it later from the returned bytes
/// would compare a transaction against itself and turn the check into theatre.
#[derive(Debug, Clone)]
pub struct BuiltSweep {
    pub psbt: EcxPsbt,
    pub intent: TxIntent,
    pub summary: SweepSummary,
}

/// Build the sweep, keeping the PSBT and its intent alongside the summary.
pub async fn build_sweep_full(
    chain: &EsploraChain,
    account: &DiscoveredAccount,
    destination: &Destination,
    fingerprint: Fingerprint,
    feerate_sat_per_vb: u64,
) -> Result<BuiltSweep, SplitError> {
    if !destination.is_confirmed() {
        return Err(SplitError::DestinationUnconfirmed);
    }
    let (_, readiness) = chain_status(chain).await?;
    let psbt = ecx_wallet::build_sweep(
        chain,
        readiness,
        account,
        destination.address(),
        feerate_sat_per_vb,
    )
    .await?;
    let intent = psbt.intent()?;
    let summary = ecx_wallet::summarize(&psbt, destination.address(), fingerprint)?;
    Ok(BuiltSweep {
        psbt,
        intent,
        summary,
    })
}

/// Build the sweep and summarize it for review. Stops short of signing.
pub async fn build_sweep(
    chain: &EsploraChain,
    account: &DiscoveredAccount,
    destination: &Destination,
    fingerprint: Fingerprint,
    feerate_sat_per_vb: u64,
) -> Result<SweepSummary, SplitError> {
    Ok(
        build_sweep_full(chain, account, destination, fingerprint, feerate_sat_per_vb)
            .await?
            .summary,
    )
}

/// Turn whatever the device produced into a transaction.
///
/// Devices differ: most fill signatures into the PSBT and it still needs finalizing, while
/// Trezor streams back a finished transaction. `SignedTx` keeps that difference honest; this is
/// where the two paths converge.
pub fn resolve_signed(signed: SignedTx) -> Result<Transaction, SplitError> {
    match signed {
        SignedTx::Psbt(psbt) => Ok(ecx_wallet::build::finalize_and_extract(*psbt)?),
        SignedTx::Transaction(tx) => Ok(*tx),
    }
}

/// Sign on the device, then **re-verify the bytes it returned** before anything else happens.
///
/// Golden Rule 3: the device's output is untrusted. `verify_signed` re-checks the locktime, every
/// input's sequence, every outpoint, and every output against the intent the user actually
/// confirmed. A mismatch aborts here, with nothing broadcast.
pub async fn sign_and_verify(
    signer: &dyn Signer,
    psbt: &EcxPsbt,
    intent: &TxIntent,
) -> Result<Transaction, SplitError> {
    let signed = signer.sign(psbt).await?;
    let tx = resolve_signed(signed)?;
    ecx_core::verify_signed(&tx, intent)?;
    Ok(tx)
}

/// Verify a signature that came back from an **air-gapped** device.
///
/// The USB path has [`sign_and_verify`], which owns the whole sequence. Air-gap has no signer to
/// call — the bytes arrive from a file or a paste — so this is its equivalent, and it exists so
/// that path cannot be composed without the check. Both routes end at the same
/// `ecx_core::verify_signed`; there is no shorter way to a broadcastable transaction.
///
/// `intent` must be the one derived from the PSBT the user actually confirmed, not one recomputed
/// from the returned bytes — otherwise the comparison is a transaction against itself.
pub fn verify_imported(signed: SignedTx, intent: &TxIntent) -> Result<Transaction, SplitError> {
    let tx = resolve_signed(signed)?;
    ecx_core::verify_signed(&tx, intent)?;
    Ok(tx)
}

/// Broadcast a verified transaction to a chain proven to be ECX.
///
/// The [`BroadcastPermit`] is the proof, and only [`ecx_chain::ForkProbe::ConfirmedEcx`] can mint
/// one — so this cannot be called against a Bitcoin endpoint, or against ECX before the chains
/// diverge. That is a type-level guarantee, not a runtime check.
pub async fn broadcast(
    chain: &EsploraChain,
    tx: &Transaction,
    permit: &BroadcastPermit,
) -> Result<Txid, SplitError> {
    Ok(chain.broadcast(tx, permit).await?)
}

/// The whole tail of §7: sign, verify, broadcast.
pub async fn sign_verify_broadcast(
    chain: &EsploraChain,
    signer: &dyn Signer,
    psbt: &EcxPsbt,
    intent: &TxIntent,
    permit: &BroadcastPermit,
) -> Result<Txid, SplitError> {
    let tx = sign_and_verify(signer, psbt, intent).await?;
    broadcast(chain, &tx, permit).await
}

/// A fee this large is a bug, not a preference (§8.6).
pub const MAX_FEE: Amount = ecx_wallet::build::MAX_FEE;

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> Address {
        "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"
            .parse::<Address<bitcoin::address::NetworkUnchecked>>()
            .unwrap()
            .assume_checked()
    }

    #[test]
    fn a_pasted_destination_starts_unconfirmed() {
        assert!(
            !Destination::Pasted {
                address: addr(),
                acknowledged: false
            }
            .is_confirmed()
        );
        assert!(
            Destination::Pasted {
                address: addr(),
                acknowledged: true
            }
            .is_confirmed()
        );
    }

    #[test]
    fn a_device_derived_destination_is_confirmed_by_construction() {
        assert!(
            Destination::DeviceDerived {
                account: 1,
                address: addr()
            }
            .is_confirmed()
        );
    }

    #[test]
    fn a_device_supplied_transaction_passes_straight_through() {
        // Trezor's shape: already finished, nothing to finalize.
        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::from_consensus(499_999_999),
            input: vec![],
            output: vec![],
        };
        let resolved = resolve_signed(SignedTx::Transaction(Box::new(tx.clone()))).unwrap();
        assert_eq!(resolved, tx);
    }

    #[test]
    fn an_unsigned_psbt_fails_to_finalize_rather_than_producing_a_transaction() {
        // The other shape, with no signatures in it. This must be an error: silently extracting
        // an unfinalized transaction would produce something the network rejects, after we had
        // already told the user it was signed.
        let unsigned = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::from_consensus(499_999_999),
            input: vec![bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::null(),
                script_sig: bitcoin::ScriptBuf::new(),
                sequence: bitcoin::Sequence(0xFFFF_FFFD),
                witness: bitcoin::Witness::new(),
            }],
            output: vec![],
        };
        let psbt = bitcoin::Psbt::from_unsigned_tx(unsigned).unwrap();
        assert!(resolve_signed(SignedTx::Psbt(Box::new(psbt))).is_err());
    }

    #[test]
    fn an_imported_signature_is_verified_against_the_confirmed_intent() {
        use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Witness};
        use bitcoin::{absolute::LockTime, transaction::Version};

        let spk = |b: u8| ScriptBuf::from_bytes([vec![0x00, 0x14], vec![b; 20]].concat());
        let prev = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: spk(0xaa),
            }],
        };
        let outpoint = OutPoint {
            txid: prev.compute_txid(),
            vout: 0,
        };
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::from_consensus(ecx_core::ECX_MAGIC_LOCKTIME),
            input: vec![TxIn {
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0xFFFF_FFFD),
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: spk(0xbb),
            }],
        };

        let intent = TxIntent {
            inputs: [(outpoint, prev.output[0].clone())].into_iter().collect(),
            outputs: tx.output.clone(),
            fee: Amount::from_sat(1_000),
        };

        // Matching bytes verify.
        assert!(verify_imported(SignedTx::Transaction(Box::new(tx.clone())), &intent).is_ok());

        // A file swapped for one paying somewhere else must not get through, which is the whole
        // reason the air-gap path routes through here rather than straight to broadcast.
        let mut tampered = tx;
        tampered.output[0].script_pubkey = spk(0xee);
        assert!(verify_imported(SignedTx::Transaction(Box::new(tampered)), &intent).is_err());
    }

    #[tokio::test]
    async fn building_refuses_an_unconfirmed_destination() {
        // Guard order matters: this must fail before any chain or device work happens, so an
        // unacknowledged paste can never reach a scan, let alone a signer.
        let chain = EsploraChain::new(ecx_chain::ChainProfile::custom("https://invalid.test"))
            .expect("client builds without contacting anything");
        let account = DiscoveredAccount {
            candidate: candidates(1).remove(0),
            descriptor: String::new(),
            change_descriptor: String::new(),
            utxo_count: 0,
            balance: Amount::ZERO,
            tx_count: 0,
        };
        let destination = Destination::Pasted {
            address: addr(),
            acknowledged: false,
        };
        let err = build_sweep(&chain, &account, &destination, Fingerprint::default(), 1)
            .await
            .unwrap_err();
        assert!(
            matches!(err, SplitError::DestinationUnconfirmed),
            "got {err:?}"
        );
    }
}
