//! Named chain endpoints.
//!
//! All pre-launch and volatile — re-check every phase (`CLAUDE.md` §6).

/// What a profile is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// An ECX endpoint. Broadcast is still gated on the fork probe passing.
    Ecx,
    /// Bitcoin mainnet, **discovery only**. No profile currently uses this.
    ///
    /// Kept because the idea is sound — below `ECASH_HEIGHT` the two chains are identical, so a
    /// Bitcoin indexer returns the same pre-fork UTXO set, and broadcast stays impossible because
    /// the fork probe can never return `ConfirmedEcx` for it. What is missing is a *host*.
    ///
    /// **Do not point this at mempool.space.** It looks like Esplora but is not: it rejects the
    /// `/scripthash/{hash}/txs` endpoints `bdk_esplora` scans with (HTTP 400 "Invalid
    /// scripthash"), so every discovery run fails on the scan step while the tip request
    /// succeeds — which reads as "the endpoint works, the wallet is broken". `blockstream.info`
    /// does implement them but was unreachable from our network on 2026-08-21. Re-add only with
    /// a host verified against a real `full_scan`, not just `/blocks/tip/height`.
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

    pub const ALL: [Self; 2] = [Self::ECX_ALPHA, Self::ECX_ALPHA_ESPLORA];

    pub fn is_ecx(&self) -> bool {
        matches!(self.kind, ProfileKind::Ecx)
    }
}
