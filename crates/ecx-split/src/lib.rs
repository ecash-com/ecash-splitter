//! The split flow, orchestrating the other crates in the order `CLAUDE.md` §7 requires.
//!
//! **This crate is the seam that makes a second frontend possible.** It owns every step of the
//! flow — connect, read keys, scan, build — and knows nothing about how it will be presented.
//! Progress arrives as [`SplitEvent`]s through a callback, so a GUI can turn them into state
//! updates and a CLI can print them, without either duplicating the sequence.
//!
//! Golden Rule 6: the ECX sweep happens first, always. Replay protection is one-directional, and
//! split ordering is what protects the other direction — it is not an optimization.

use bitcoin::{Address, Amount, Txid, bip32::Fingerprint};
use ecx_chain::{ChainSource, EsploraChain, ScanReadiness, TipInfo, now_unix};
use ecx_signer::{DeviceKind, Signer};
use ecx_wallet::{
    AccountCandidate, DEFAULT_ACCOUNTS_PROBED, DiscoveredAccount, SweepSummary, candidates,
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
    mut on_event: impl FnMut(SplitEvent),
) -> Result<(DeviceIdentity, Vec<DiscoveredAccount>), SplitError> {
    let identity = identify(signer, device_label).await?;
    on_event(SplitEvent::Connected(identity.clone()));

    let candidates: Vec<AccountCandidate> = candidates(DEFAULT_ACCOUNTS_PROBED);
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
    let accounts = scan_accounts(chain, readiness, identity.fingerprint, &xpubs, |p| {
        on_event(SplitEvent::Scanning {
            done: p.scanned,
            total: p.total,
            label: format!("{} {}", p.current.kind.label(), p.current.path),
        });
    })
    .await?;

    Ok((identity, accounts))
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

/// Build the sweep and summarize it for review. Stops short of signing.
pub async fn build_sweep(
    chain: &EsploraChain,
    account: &DiscoveredAccount,
    destination: &Destination,
    fingerprint: Fingerprint,
    feerate_sat_per_vb: u64,
) -> Result<SweepSummary, SplitError> {
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
    Ok(ecx_wallet::summarize(
        &psbt,
        destination.address(),
        fingerprint,
    )?)
}

/// Sign, re-verify, and broadcast.
///
/// Not reachable yet: the fork has not activated, so no endpoint can pass the probe and mint a
/// [`ecx_chain::BroadcastPermit`]. Wired here so the sequence lives in one place when it is.
pub async fn sign_and_broadcast(
    _chain: &EsploraChain,
    _signer: &dyn Signer,
    _summary: &SweepSummary,
) -> Result<Txid, SplitError> {
    todo!("sign via ecx-signer, verify_signed via ecx-core, broadcast with a BroadcastPermit")
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
