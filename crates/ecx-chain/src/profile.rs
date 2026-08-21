//! Named chain endpoints.
//!
//! All pre-launch and volatile — re-check every phase (`CLAUDE.md` §6).

/// What a profile is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// An ECX endpoint. Broadcast is still gated on the fork probe passing.
    Ecx,
    /// Bitcoin mainnet, **discovery only**.
    ///
    /// Legitimate before the fork: ECX and Bitcoin share every block below `ECASH_HEIGHT`, so a
    /// Bitcoin indexer returns exactly the same pre-fork UTXO set — and unlike the ECX indexers,
    /// it is fully synced today. Broadcast is impossible regardless, because the fork probe can
    /// never return `ConfirmedEcx` for it.
    BitcoinReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainProfile {
    pub name: &'static str,
    /// Esplora API base, e.g. `https://blockstream.info/api`.
    pub esplora_url: &'static str,
    pub kind: ProfileKind,
}

impl ChainProfile {
    /// ECX alpha. Live and Esplora-compatible, but syncing from genesis — it was at height
    /// 458,330 on 2026-08-21, so the sync gate will block discovery until it catches up.
    pub const ECX_ALPHA: Self = Self {
        name: "ECX alpha",
        esplora_url: "https://explorer.alpha.ecash.ninja/api",
        kind: ProfileKind::Ecx,
    };

    /// ECX alpha, dedicated Esplora host. DNS resolves but was returning 502 on 2026-08-21.
    pub const ECX_ALPHA_ESPLORA: Self = Self {
        name: "ECX alpha (esplora)",
        esplora_url: "https://esplora.alpha.ecash.ninja/api",
        kind: ProfileKind::Ecx,
    };

    /// Bitcoin mainnet, discovery only. A fallback for exercising discovery if the ECX
    /// endpoints are down — `blockstream.info` was unreachable on 2026-08-21, so this points at
    /// mempool.space.
    pub const BITCOIN_READ_ONLY: Self = Self {
        name: "Bitcoin (discovery only)",
        esplora_url: "https://mempool.space/api",
        kind: ProfileKind::BitcoinReadOnly,
    };

    pub const ALL: [Self; 3] = [
        Self::ECX_ALPHA,
        Self::ECX_ALPHA_ESPLORA,
        Self::BITCOIN_READ_ONLY,
    ];

    pub fn is_ecx(&self) -> bool {
        matches!(self.kind, ProfileKind::Ecx)
    }
}
