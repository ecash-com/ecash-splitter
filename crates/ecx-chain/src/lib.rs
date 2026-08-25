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
    /// The node looked at the transaction and said no. Carries its reason verbatim.
    #[error("{0}")]
    Rejected(String),
    #[error(
        "refusing to broadcast: this endpoint is Bitcoin, not ECX \
         (its block hash at {ECASH_HEIGHT} matches Bitcoin's)"
    )]
    EndpointIsBitcoin,
    #[error("refusing to broadcast: endpoint has not synced past the fork (tip {tip})")]
    NotSyncedToFork { tip: u32 },
    #[error(
        "refusing to broadcast: no Bitcoin reference hash is configured, so this endpoint cannot \
         be told apart from Bitcoin. Set ECX_BITCOIN_FORK_HASH to Bitcoin's block hash at the \
         fork height"
    )]
    NoBitcoinReference,
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
    /// No Bitcoin reference hash is configured, so there is nothing to compare against.
    ///
    /// Distinct from every other variant: this is a gap in *our* configuration, not a fact about
    /// the endpoint. It is the state before Bitcoin reaches the fork height, and it stays the
    /// state afterwards until [`BITCOIN_HASH_AT_FORK`] or [`ENV_BITCOIN_FORK_HASH`] is set.
    NoBitcoinReference,
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
            ForkProbe::NoBitcoinReference => Some(ChainError::NoBitcoinReference),
        }
    }
}

/// Environment override for [`bitcoin_hash_at_fork`], so the probe can be armed the moment the
/// block exists, without waiting for a release.
pub const ENV_BITCOIN_FORK_HASH: &str = "ECX_BITCOIN_FORK_HASH";

/// Bitcoin mainnet's block hash at [`ECASH_HEIGHT`], compiled in.
///
/// Read 2026-08-25 from blockstream.info and mempool.space independently, both agreeing, with
/// Bitcoin's tip at 964,003 — comfortably past the fork height.
///
/// For comparison, ECX alphanet's block at the same height is
/// `0000000000b360c17636b7a6c366e3effbe91a847eb5d61b7a7b29476439e924` — note the shorter run of
/// leading zeros, which is the difficulty reset at the fork showing through.
///
/// **Update this per phase.** Each launch forks at its own height, and a stale value here means
/// the probe compares against the wrong block.
pub const BITCOIN_HASH_AT_FORK: Option<&str> =
    Some("00000000000000000001769d9a327f5b455aa8a2dd407b1b63040d2a9f832d32");

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
        // Nothing to compare against. Note this says nothing about the endpoint — asking it for
        // its own tip here would only invite reporting one chain's height as the other's.
        return Ok(ForkProbe::NoBitcoinReference);
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

    /// A chain that answers however the test needs, so the gate can be exercised without a
    /// network. `hash_at_fork = None` models an endpoint that has not reached the fork height.
    struct FakeChain {
        tip: u32,
        hash_at_fork: Option<BlockHash>,
    }

    fn hash(byte: u8) -> BlockHash {
        use bitcoin::hashes::Hash;
        BlockHash::from_byte_array([byte; 32])
    }

    #[async_trait::async_trait]
    impl ChainSource for FakeChain {
        async fn tip_height(&self) -> Result<u32, ChainError> {
            Ok(self.tip)
        }
        async fn tip(&self) -> Result<TipInfo, ChainError> {
            Ok(TipInfo {
                height: self.tip,
                time: 0,
            })
        }
        async fn block_hash_at(&self, _height: u32) -> Result<Option<BlockHash>, ChainError> {
            Ok(self.hash_at_fork)
        }
        async fn raw_tx(&self, _txid: Txid) -> Result<Option<Transaction>, ChainError> {
            Ok(None)
        }
        async fn tx_state(&self, _txid: Txid) -> Result<TxState, ChainError> {
            Ok(TxState::Unknown)
        }
        async fn broadcast(
            &self,
            tx: &Transaction,
            _permit: &BroadcastPermit,
        ) -> Result<Txid, ChainError> {
            Ok(tx.compute_txid())
        }
    }

    /// **The one that matters.** An endpoint whose block at the fork height matches Bitcoin's *is*
    /// Bitcoin, and must never yield a permit — this is what stops a signed sweep being published
    /// to Bitcoin mainnet, where it would be a real spend of real BTC.
    #[tokio::test]
    async fn an_endpoint_that_is_bitcoin_never_mints_a_permit() {
        let bitcoin = hash(0xbb);
        let chain = FakeChain {
            tip: 970_000,
            hash_at_fork: Some(bitcoin),
        };

        let probe = probe_fork_against(&chain, Some(bitcoin)).await.unwrap();
        assert_eq!(probe, ForkProbe::IsBitcoin);
        assert!(
            probe.permit().is_none(),
            "Bitcoin must never be broadcastable to"
        );
        assert!(!probe.may_broadcast());
    }

    #[tokio::test]
    async fn only_a_chain_that_diverged_from_bitcoin_mints_a_permit() {
        let bitcoin = hash(0xbb);
        let ecash = hash(0xec);
        let chain = FakeChain {
            tip: 970_000,
            hash_at_fork: Some(ecash),
        };

        let probe = probe_fork_against(&chain, Some(bitcoin)).await.unwrap();
        assert_eq!(probe, ForkProbe::ConfirmedEcx { hash: ecash });
        assert!(probe.permit().is_some());
    }

    #[tokio::test]
    async fn an_endpoint_short_of_the_fork_mints_no_permit() {
        let chain = FakeChain {
            tip: 963_500,
            hash_at_fork: None,
        };
        let probe = probe_fork_against(&chain, Some(hash(0xbb))).await.unwrap();
        assert_eq!(probe, ForkProbe::NotSyncedToFork { tip: 963_500 });
        assert!(probe.permit().is_none());
    }

    /// With no independent reference there is nothing to compare against, so nothing can be
    /// proven and nothing may be published. This is the state before the fork.
    #[tokio::test]
    async fn no_reference_hash_means_no_permit() {
        let chain = FakeChain {
            tip: 963_500,
            hash_at_fork: Some(hash(0xec)),
        };
        let probe = probe_fork_against(&chain, None).await.unwrap();
        assert_eq!(probe, ForkProbe::NoBitcoinReference);
        assert!(probe.permit().is_none());
    }

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
        assert!(ForkProbe::NoBitcoinReference.permit().is_none());
    }

    #[test]
    fn only_confirmed_ecx_may_broadcast() {
        let hash = BlockHash::from_raw_hash(bitcoin::hashes::Hash::all_zeros());
        assert!(ForkProbe::ConfirmedEcx { hash }.may_broadcast());
        assert!(!ForkProbe::IsBitcoin.may_broadcast());
        assert!(!ForkProbe::NotSyncedToFork { tip: 1 }.may_broadcast());
        assert!(!ForkProbe::NoBitcoinReference.may_broadcast());
    }

    #[test]
    fn the_compiled_in_reference_parses_and_is_bitcoins_not_ecashs() {
        let hash = bitcoin_hash_at_fork().expect("a reference is compiled in");
        // Guards a paste error: ECX alphanet's block at the same height, which would make the
        // probe clear Bitcoin and reject eCash — exactly backwards.
        assert_ne!(
            hash.to_string(),
            "0000000000b360c17636b7a6c366e3effbe91a847eb5d61b7a7b29476439e924",
            "that is ECX's hash at the fork height, not Bitcoin's"
        );
        assert_eq!(
            hash.to_string(),
            "00000000000000000001769d9a327f5b455aa8a2dd407b1b63040d2a9f832d32"
        );
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
