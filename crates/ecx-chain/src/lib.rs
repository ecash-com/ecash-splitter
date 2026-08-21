//! Chain sources and the two safety gates that guard them.
//!
//! Backends are Esplora (default) and Electrum (`CLAUDE.md` §6). BIP157/158 SPV was considered
//! and dropped; do not add it back without reading §6 first.

use bitcoin::{BlockHash, Transaction, Txid};
use ecx_core::ECASH_HEIGHT;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("chain source is unreachable: {0}")]
    Unreachable(String),
    #[error("chain source returned a malformed response: {0}")]
    Malformed(String),
    #[error(
        "refusing to broadcast: this endpoint is Bitcoin, not ECX \
         (its block hash at {ECASH_HEIGHT} matches Bitcoin's)"
    )]
    EndpointIsBitcoin,
    #[error("refusing to broadcast: endpoint has not synced past the fork (tip {tip})")]
    NotSyncedToFork { tip: u32 },
    #[error("refusing to broadcast: ECX and Bitcoin have not diverged yet (Bitcoin tip {tip})")]
    ChainsNotYetDiverged { tip: u32 },
}

/// Everything the app needs from a chain. Deliberately small so backends stay swappable
/// (`CLAUDE.md` §6) — and deliberately including `raw_tx`, because Trezor needs full previous
/// transactions for every non-taproot input (§5.4).
#[async_trait::async_trait]
pub trait ChainSource: Send + Sync {
    async fn tip_height(&self) -> Result<u32, ChainError>;
    async fn block_hash_at(&self, height: u32) -> Result<Option<BlockHash>, ChainError>;
    async fn raw_tx(&self, txid: Txid) -> Result<Option<Transaction>, ChainError>;
    async fn broadcast(&self, tx: &Transaction) -> Result<Txid, ChainError>;
}

// ---------------------------------------------------------------------------
// Golden Rule 4 — the fork probe
// ---------------------------------------------------------------------------

/// Result of proving an endpoint is ECX and not Bitcoin.
///
/// The probe works because ECX and Bitcoin share every block below [`ECASH_HEIGHT`] and diverge
/// from it onward. That is also its limitation: **below the fork height the two chains are
/// byte-identical and no probe can tell them apart.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkProbe {
    /// Hash at the fork height differs from Bitcoin's. Safe to broadcast.
    ConfirmedEcx { hash: BlockHash },
    /// Hash at the fork height equals Bitcoin's. Refuse permanently.
    IsBitcoin,
    /// Endpoint has not synced past the fork. Usable for scanning, never for broadcast.
    NotSyncedToFork { tip: u32 },
    /// Bitcoin itself has not reached the fork height, so there is nothing to compare against.
    ///
    /// True until Bitcoin mines block [`ECASH_HEIGHT`] — at 2026-08-21 its tip was 963,465, some
    /// 183 blocks short. Until then [`BITCOIN_HASH_AT_FORK`] cannot be filled in.
    ChainsNotYetDiverged { bitcoin_tip: u32 },
}

impl ForkProbe {
    /// Golden Rule 4: broadcast only to a chain we proved is ECX.
    pub fn may_broadcast(&self) -> bool {
        matches!(self, ForkProbe::ConfirmedEcx { .. })
    }

    pub fn as_error(&self) -> Option<ChainError> {
        match self {
            ForkProbe::ConfirmedEcx { .. } => None,
            ForkProbe::IsBitcoin => Some(ChainError::EndpointIsBitcoin),
            ForkProbe::NotSyncedToFork { tip } => Some(ChainError::NotSyncedToFork { tip: *tip }),
            ForkProbe::ChainsNotYetDiverged { bitcoin_tip } => {
                Some(ChainError::ChainsNotYetDiverged { tip: *bitcoin_tip })
            }
        }
    }
}

/// Bitcoin mainnet's block hash at [`ECASH_HEIGHT`].
///
/// **`None` until Bitcoin mines that block.** Fill it in from a trusted Bitcoin source once the
/// height is reached, refresh per phase, and never derive it at runtime from the endpoint under
/// test — that would defeat the entire probe.
pub const BITCOIN_HASH_AT_FORK: Option<BlockHash> = None;

/// Prove an endpoint is ECX. `bitcoin_reference` is [`BITCOIN_HASH_AT_FORK`], threaded in so
/// tests can supply one.
pub async fn probe_fork(
    source: &dyn ChainSource,
    bitcoin_reference: Option<BlockHash>,
    bitcoin_tip: u32,
) -> Result<ForkProbe, ChainError> {
    let Some(reference) = bitcoin_reference else {
        return Ok(ForkProbe::ChainsNotYetDiverged { bitcoin_tip });
    };
    match source.block_hash_at(ECASH_HEIGHT).await? {
        None => Ok(ForkProbe::NotSyncedToFork {
            tip: source.tip_height().await?,
        }),
        Some(hash) if hash == reference => Ok(ForkProbe::IsBitcoin),
        Some(hash) => Ok(ForkProbe::ConfirmedEcx { hash }),
    }
}

// ---------------------------------------------------------------------------
// Golden Rule 9 — the sync gate
// ---------------------------------------------------------------------------

/// Whether a scan result may be shown to the user at all.
///
/// A partially-synced indexer is indistinguishable from an empty wallet, and "you have no coins"
/// is the one wrong answer a user acts on immediately and irreversibly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanReadiness {
    /// Tip is at or past the fork height. Results may be shown.
    Ready,
    /// Show progress, state no balance.
    Syncing { tip: u32, target: u32 },
}

impl ScanReadiness {
    pub fn at_tip(tip: u32) -> Self {
        if tip >= ECASH_HEIGHT {
            ScanReadiness::Ready
        } else {
            ScanReadiness::Syncing {
                tip,
                target: ECASH_HEIGHT,
            }
        }
    }

    pub fn may_report_balance(&self) -> bool {
        matches!(self, ScanReadiness::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_gate_blocks_below_the_fork_height() {
        // explorer.alpha.ecash.ninja was here on 2026-08-21.
        let syncing = ScanReadiness::at_tip(458_330);
        assert!(!syncing.may_report_balance());
        assert_eq!(
            syncing,
            ScanReadiness::Syncing {
                tip: 458_330,
                target: ECASH_HEIGHT
            }
        );

        assert!(!ScanReadiness::at_tip(ECASH_HEIGHT - 1).may_report_balance());
        assert!(ScanReadiness::at_tip(ECASH_HEIGHT).may_report_balance());
    }

    #[test]
    fn only_confirmed_ecx_may_broadcast() {
        let hash = BlockHash::from_raw_hash(bitcoin::hashes::Hash::all_zeros());
        assert!(ForkProbe::ConfirmedEcx { hash }.may_broadcast());
        assert!(!ForkProbe::IsBitcoin.may_broadcast());
        assert!(!ForkProbe::NotSyncedToFork { tip: 1 }.may_broadcast());
        assert!(
            !ForkProbe::ChainsNotYetDiverged {
                bitcoin_tip: 963_465
            }
            .may_broadcast()
        );
    }

    #[test]
    fn probe_cannot_run_before_the_chains_diverge() {
        // BITCOIN_HASH_AT_FORK is None until Bitcoin mines block 963,648.
        assert!(BITCOIN_HASH_AT_FORK.is_none());
    }
}
