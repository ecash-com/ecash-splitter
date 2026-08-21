//! Chain endpoints.
//!
//! All pre-launch and volatile — re-check every phase (`CLAUDE.md` §6). Because they move, the
//! URL is editable at runtime rather than being a fixed menu of hosts.

use std::borrow::Cow;

/// What an endpoint is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// An ECX endpoint. Broadcast is still gated on the fork probe passing.
    Ecx,
    /// Bitcoin mainnet, **discovery only**. No preset currently uses this.
    ///
    /// The idea is sound — below `ECASH_HEIGHT` the two chains are identical, so a Bitcoin
    /// indexer returns the same pre-fork UTXO set, and broadcast stays impossible because the
    /// fork probe can never return `ConfirmedEcx` for it. What is missing is a *host*.
    ///
    /// **Do not point this at mempool.space.** It looks like Esplora but is not: it rejects the
    /// `/scripthash/{hash}/txs` endpoints `bdk_esplora` scans with (HTTP 400 "Invalid
    /// scripthash"), so every discovery run fails on the scan step while the tip request
    /// succeeds — which reads as "the endpoint works, the wallet is broken". `blockstream.info`
    /// does implement them but was unreachable from our network on 2026-08-21. Only add a host
    /// verified against a real `full_scan`, not just `/blocks/tip/height`.
    BitcoinReadOnly,
    /// A URL the user typed. Treated as ECX; the fork probe is still the authority on broadcast.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainProfile {
    pub name: Cow<'static, str>,
    /// Esplora API base, e.g. `https://explorer.alpha.ecash.ninja/api`.
    pub esplora_url: Cow<'static, str>,
    pub kind: ProfileKind,
}

impl ChainProfile {
    /// ECX alpha. Live, Esplora-compatible, and serves the `/scripthash/` endpoints BDK scans
    /// with — which is the check that matters (see [`ProfileKind::BitcoinReadOnly`]).
    pub fn ecx_alpha() -> Self {
        Self {
            name: Cow::Borrowed("ECX alpha"),
            esplora_url: Cow::Borrowed("https://explorer.alpha.ecash.ninja/api"),
            kind: ProfileKind::Ecx,
        }
    }

    /// A user-supplied Esplora base URL.
    ///
    /// Trailing slashes and an accidentally-omitted `/api` are common enough to be worth fixing
    /// silently rather than failing with a confusing 404.
    pub fn custom(url: &str) -> Self {
        let trimmed = url.trim().trim_end_matches('/');
        let normalized = if trimmed.ends_with("/api") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/api")
        };
        Self {
            name: Cow::Owned(host_of(&normalized).to_string()),
            esplora_url: Cow::Owned(normalized),
            kind: ProfileKind::Custom,
        }
    }

    /// Built-in endpoints offered in the UI.
    ///
    /// Only one: `esplora.alpha.ecash.ninja` was listed here until 2026-08-21 but never answered
    /// (502, then 404), and an endpoint that cannot serve a request is worse than no button.
    pub fn presets() -> Vec<Self> {
        vec![Self::ecx_alpha()]
    }

    pub fn is_ecx(&self) -> bool {
        matches!(self.kind, ProfileKind::Ecx | ProfileKind::Custom)
    }

    /// Ticker for amounts on this chain.
    ///
    /// Lives here rather than in a frontend so the GUI and the CLI cannot disagree about what
    /// they are counting.
    pub fn ticker(&self) -> &'static str {
        match self.kind {
            ProfileKind::Ecx | ProfileKind::Custom => "ECX",
            ProfileKind::BitcoinReadOnly => "BTC",
        }
    }

    /// Format an amount with this chain's ticker.
    pub fn format(&self, amount: bitcoin::Amount) -> String {
        format!("{:.8} {}", amount.to_btc(), self.ticker())
    }

    pub fn is_custom(&self) -> bool {
        matches!(self.kind, ProfileKind::Custom)
    }

    /// Bare hostname, for error messages and labels.
    pub fn host(&self) -> &str {
        host_of(&self.esplora_url)
    }
}

impl Default for ChainProfile {
    fn default() -> Self {
        Self::ecx_alpha()
    }
}

fn host_of(url: &str) -> &str {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_urls_are_normalized() {
        for input in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/api",
            "https://example.com/api/",
            "  https://example.com/api  ",
        ] {
            assert_eq!(
                ChainProfile::custom(input).esplora_url,
                "https://example.com/api",
                "input was {input:?}"
            );
        }
    }

    #[test]
    fn custom_profiles_are_named_by_host() {
        assert_eq!(
            ChainProfile::custom("https://esplora.example.org/api").name,
            "esplora.example.org"
        );
    }

    #[test]
    fn a_custom_endpoint_is_still_gated_on_the_fork_probe() {
        // is_ecx() only says "may be treated as an ECX chain source", never "may broadcast".
        let custom = ChainProfile::custom("https://anything.invalid");
        assert!(custom.is_ecx());
        assert!(
            super::super::ForkProbe::ChainsNotYetDiverged { bitcoin_tip: 1 }
                .permit()
                .is_none()
        );
    }
}
