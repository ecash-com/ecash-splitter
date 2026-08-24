//! Chain sources and the two safety gates that guard them.
//!
//! Backends are Esplora (default) and Electrum (`CLAUDE.md` §6). BIP157/158 SPV was considered
//! and dropped; do not add it back without reading §6 first.

use bitcoin::{BlockHash, Transaction, Txid};

pub mod bitcoin_ref;
pub mod esplora;
pub mod profile;

pub use bitcoin_ref::{BitcoinReference, SplitVerdict};
use ecx_core::ECASH_HEIGHT;
pub use esplora::EsploraChain;
pub use profile::{ChainProfile, ProfileKind};

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
    /// Height **and timestamp** of the newest block the indexer knows about. The timestamp is
    /// what makes the sync gate work before the fork — see [`ScanReadiness::assess`].
    async fn tip(&self) -> Result<TipInfo, ChainError>;
    async fn block_hash_at(&self, height: u32) -> Result<Option<BlockHash>, ChainError>;
    async fn raw_tx(&self, txid: Txid) -> Result<Option<Transaction>, ChainError>;
    /// Where a transaction stands: unseen, in the mempool, or buried under confirmations.
    async fn tx_state(&self, txid: Txid) -> Result<TxState, ChainError>;

    /// Broadcast. Requires a [`BroadcastPermit`], which only a [`ForkProbe::ConfirmedEcx`] can
    /// mint — so broadcasting to a Bitcoin endpoint is a compile error, not a runtime check.
    async fn broadcast(
        &self,
        tx: &Transaction,
        permit: &BroadcastPermit,
    ) -> Result<Txid, ChainError>;
}

/// Proof that the endpoint was verified as ECX. Unforgeable outside this module: the only
/// constructor is [`ForkProbe::permit`], and the inner field is private.
#[derive(Debug)]
pub struct BroadcastPermit(());

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

    /// Mint a [`BroadcastPermit`]. `None` for every state except a confirmed ECX endpoint.
    pub fn permit(&self) -> Option<BroadcastPermit> {
        matches!(self, ForkProbe::ConfirmedEcx { .. }).then_some(BroadcastPermit(()))
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

/// Environment override for [`bitcoin_hash_at_fork`], so the probe can be armed the moment the
/// block exists, without waiting for a release.
pub const ENV_BITCOIN_FORK_HASH: &str = "ECX_BITCOIN_FORK_HASH";

/// Bitcoin mainnet's block hash at [`ECASH_HEIGHT`], compiled in.
///
/// **`None` until Bitcoin mines that block** — at 2026-08-21 its tip was ~963,480, still short of
/// 963,648. Fill this in from a trusted Bitcoin source once the height is reached, and refresh it
/// per phase.
pub const BITCOIN_HASH_AT_FORK: Option<&str> = None;

/// Bitcoin's hash at the fork height: from the environment if set, otherwise compiled in.
///
/// **Never derive this from the endpoint under test.** The probe works by comparing that
/// endpoint's answer against an independent one; taking both from the same place would compare a
/// chain against itself and clear anything.
pub fn bitcoin_hash_at_fork() -> Option<BlockHash> {
    if let Ok(text) = std::env::var(ENV_BITCOIN_FORK_HASH) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<BlockHash>() {
                Ok(hash) => return Some(hash),
                Err(e) => tracing::warn!(%e, "ignoring unparseable {ENV_BITCOIN_FORK_HASH}"),
            }
        }
    }
    BITCOIN_HASH_AT_FORK.and_then(|h| h.parse().ok())
}

/// Prove an endpoint is ECX, using the configured Bitcoin reference hash.
pub async fn probe_fork(source: &dyn ChainSource) -> Result<ForkProbe, ChainError> {
    probe_fork_against(source, bitcoin_hash_at_fork()).await
}

/// As [`probe_fork`], with the reference supplied explicitly. Tests use this.
pub async fn probe_fork_against(
    source: &dyn ChainSource,
    bitcoin_reference: Option<BlockHash>,
) -> Result<ForkProbe, ChainError> {
    let Some(reference) = bitcoin_reference else {
        // No reference means the chains have not diverged yet, so nothing can be proven.
        return Ok(ForkProbe::ChainsNotYetDiverged {
            bitcoin_tip: source.tip_height().await.unwrap_or(0),
        });
    };
    match source.block_hash_at(ECASH_HEIGHT).await? {
        None => Ok(ForkProbe::NotSyncedToFork {
            tip: source.tip_height().await?,
        }),
        Some(hash) if hash == reference => Ok(ForkProbe::IsBitcoin),
        Some(hash) => Ok(ForkProbe::ConfirmedEcx { hash }),
    }
}

/// Where a transaction stands on a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxState {
    /// The endpoint has never seen it. Either not broadcast, or dropped.
    Unknown,
    /// Accepted into the mempool but not mined.
    InMempool,
    /// Mined, and buried under this many blocks (1 = in the tip block).
    Confirmed { height: u32, confirmations: u32 },
}

impl TxState {
    pub fn confirmations(&self) -> u32 {
        match self {
            TxState::Confirmed { confirmations, .. } => *confirmations,
            _ => 0,
        }
    }

    /// Post-fork difficulty resets to minimum, so reorg risk is elevated and six is not enough.
    pub fn is_deep_enough(&self, required: u32) -> bool {
        self.confirmations() >= required
    }
}

// ---------------------------------------------------------------------------
// Golden Rule 9 — the sync gate
// ---------------------------------------------------------------------------

/// Height and timestamp of an indexer's newest block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TipInfo {
    pub height: u32,
    /// Unix seconds, from the block header.
    pub time: u32,
}

/// How stale an indexer's tip may be before we stop trusting an empty result. Bitcoin averages a
/// block every ten minutes, so three hours is roughly eighteen blocks of slack.
pub const MAX_TIP_AGE_SECS: u64 = 3 * 60 * 60;

/// Whether a scan result may be shown to the user at all.
///
/// A lagging indexer is indistinguishable from an empty wallet, and "you have no coins" is the
/// one wrong answer a user acts on immediately and irreversibly (Golden Rule 9).
///
/// **The question is "has this indexer caught up?", not "is it past the fork height".** Those are
/// different, and conflating them is wrong: before the fork block exists, an indexer sitting on
/// the current chain tip is perfectly synced and its empty results are trustworthy, even though
/// it is below [`ecx_core::ECASH_HEIGHT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanReadiness {
    /// Caught up. Results, including empty ones, can be trusted.
    Ready { tip: u32 },
    /// Behind. Show progress, state no balance.
    Behind { tip: u32, age_secs: u64 },
}

impl ScanReadiness {
    /// `now_unix` is threaded in rather than read from the clock so this stays testable.
    ///
    /// Known limitation: an indexer stalled *above* the fork height still reads as ready. Post-
    /// fork difficulty resets to minimum, so ECX block intervals will be erratic and a pure
    /// freshness test would produce constant false alarms. Revisit once real block times exist.
    pub fn assess(tip: TipInfo, now_unix: u64) -> Self {
        if tip.height >= ecx_core::ECASH_HEIGHT {
            return ScanReadiness::Ready { tip: tip.height };
        }
        let age = now_unix.saturating_sub(tip.time as u64);
        if age <= MAX_TIP_AGE_SECS {
            ScanReadiness::Ready { tip: tip.height }
        } else {
            ScanReadiness::Behind {
                tip: tip.height,
                age_secs: age,
            }
        }
    }

    pub fn may_report_balance(&self) -> bool {
        matches!(self, ScanReadiness::Ready { .. })
    }

    pub fn tip(&self) -> u32 {
        match self {
            ScanReadiness::Ready { tip } | ScanReadiness::Behind { tip, .. } => *tip,
        }
    }
}

/// Current unix time, for [`ScanReadiness::assess`].
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_787_000_000;

    #[test]
    fn a_mid_sync_indexer_is_not_trusted() {
        // explorer.alpha.ecash.ninja was here earlier on 2026-08-21, replaying old blocks.
        let stale = TipInfo {
            height: 458_330,
            time: (NOW - 5 * 365 * 24 * 3600) as u32,
        };
        let readiness = ScanReadiness::assess(stale, NOW);
        assert!(!readiness.may_report_balance());
        assert!(matches!(
            readiness,
            ScanReadiness::Behind { tip: 458_330, .. }
        ));
    }

    #[test]
    fn a_caught_up_pre_fork_indexer_is_trusted() {
        // The same host later the same day: below ECASH_HEIGHT only because the fork block does
        // not exist yet, but sitting on the real chain tip. Empty results here are trustworthy.
        let fresh = TipInfo {
            height: 963_466,
            time: (NOW - 600) as u32,
        };
        assert!(ScanReadiness::assess(fresh, NOW).may_report_balance());
    }

    #[test]
    fn past_the_fork_height_is_always_ready() {
        let old = TipInfo {
            height: ECASH_HEIGHT,
            time: 1,
        };
        assert!(ScanReadiness::assess(old, NOW).may_report_balance());
    }

    #[test]
    fn only_a_confirmed_ecx_probe_mints_a_permit() {
        let hash = BlockHash::from_raw_hash(bitcoin::hashes::Hash::all_zeros());
        assert!(ForkProbe::ConfirmedEcx { hash }.permit().is_some());
        assert!(ForkProbe::IsBitcoin.permit().is_none());
        assert!(ForkProbe::NotSyncedToFork { tip: 1 }.permit().is_none());
        assert!(
            ForkProbe::ChainsNotYetDiverged {
                bitcoin_tip: 963_465
            }
            .permit()
            .is_none()
        );
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
        // Nothing is compiled in until Bitcoin mines block 963,648. The env override is what
        // arms the probe the moment it does, without waiting for a release.
        assert!(BITCOIN_HASH_AT_FORK.is_none());
    }

    #[test]
    fn confirmation_depth_reads_off_tx_state() {
        assert_eq!(TxState::Unknown.confirmations(), 0);
        assert_eq!(TxState::InMempool.confirmations(), 0);
        assert!(!TxState::InMempool.is_deep_enough(1));
        let deep = TxState::Confirmed {
            height: 963_700,
            confirmations: 30,
        };
        assert!(deep.is_deep_enough(30));
        assert!(!deep.is_deep_enough(31));
    }
}
