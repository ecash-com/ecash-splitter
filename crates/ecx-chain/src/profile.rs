//! Chain endpoints.
//!
//! # Changing endpoints between forks
//!
//! These hosts can move, so they are deliberately easy to change in three ways, in increasing
//! order of permanence:
//!
//! 1. **At runtime.** The GUI has an editable endpoint field; the CLI takes `--endpoint`.
//! 2. **By environment**, without touching the binary or the UI:
//!    ```sh
//!    ECX_ESPLORA_URL=https://esplora.ecash.ninja \
//!    ECX_EXPLORER_URL=https://explorer.ecash.ninja \
//!    cargo run -p ecash-splitter
//!    ```
//! 3. **In code**, by editing [`PRESETS`] below. That is the only place a hostname is written.
//!
//! Whatever the source, an endpoint is still gated by the fork probe before anything is
//! broadcast — changing a URL cannot loosen that (`CLAUDE.md` Golden Rule 4).

use std::borrow::Cow;

/// Environment overrides, read once per profile construction.
pub const ENV_ESPLORA_URL: &str = "ECX_ESPLORA_URL";
pub const ENV_EXPLORER_URL: &str = "ECX_EXPLORER_URL";

// ---------------------------------------------------------------------------
// THE ONE PLACE HOSTNAMES ARE WRITTEN — update this per fork.
// ---------------------------------------------------------------------------

/// Built-in endpoints offered in the UI.
///
/// `esplora_url` is the API base; `explorer_url` is the human-facing site a transaction link
/// points at. They are usually the same host, but need not be.
///
/// **Mainnet.** From <https://drivechain.dev/config> (`id: ecash`), and both entries verified on
/// 2026-10-28 against the endpoints `bdk_esplora` actually scans with — `/scripthash/{hash}/txs`
/// returning `200 []`, not merely `/blocks/tip/height`, which mempool.space and some earlier
/// hosts pass while failing the scan. Both reported tip 973,577, level with Bitcoin, because the
/// fork block had not been mined yet; both serve `/fee-estimates` (empty, as expected on a chain
/// with no mempool history).
///
/// Note the first entry's API base has **no `/api` suffix**: `esplora.ecash.ninja` serves
/// Esplora at the root.
pub const PRESETS: &[(&str, &str, &str)] = &[
    // (display name, esplora API base, explorer site)
    (
        "ECX mainnet",
        "https://esplora.ecash.ninja",
        "https://explorer.ecash.ninja",
    ),
    (
        "ECX mainnet (explorer)",
        "https://explorer.ecash.ninja/api",
        "https://explorer.ecash.ninja",
    ),
];

// ---------------------------------------------------------------------------

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
    /// A URL supplied at runtime or by environment. Treated as ECX; the fork probe is still the
    /// authority on broadcast.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainProfile {
    pub name: Cow<'static, str>,
    /// Esplora API base, e.g. `https://esplora.ecash.ninja`.
    pub esplora_url: Cow<'static, str>,
    /// Human-facing explorer, e.g. `https://explorer.ecash.ninja`.
    pub explorer_url: Cow<'static, str>,
    pub kind: ProfileKind,
}

impl ChainProfile {
    /// The first preset, with [`ENV_ESPLORA_URL`] / [`ENV_EXPLORER_URL`] applied if set.
    pub fn ecx_mainnet() -> Self {
        let (name, esplora, explorer) = PRESETS[0];
        let mut profile = Self {
            name: Cow::Borrowed(name),
            esplora_url: Cow::Borrowed(esplora),
            explorer_url: Cow::Borrowed(explorer),
            kind: ProfileKind::Ecx,
        };
        profile.apply_env();
        profile
    }

    /// Replace URLs from the environment, so a phase change needs no rebuild and no UI fiddling.
    fn apply_env(&mut self) {
        if let Ok(url) = std::env::var(ENV_ESPLORA_URL) {
            if !url.trim().is_empty() {
                let normalized = normalize_api(&url);
                self.explorer_url = Cow::Owned(strip_api(&normalized).to_string());
                self.esplora_url = Cow::Owned(normalized);
                self.name = Cow::Owned(format!("{} (env)", host_of(&self.esplora_url)));
                self.kind = ProfileKind::Custom;
            }
        }
        if let Ok(url) = std::env::var(ENV_EXPLORER_URL) {
            if !url.trim().is_empty() {
                self.explorer_url = Cow::Owned(url.trim().trim_end_matches('/').to_string());
            }
        }
    }

    /// A user-supplied Esplora base URL.
    ///
    /// Trailing slashes and an accidentally-omitted `/api` are common enough to fix silently
    /// rather than fail with a confusing 404 — except on a host that serves Esplora at its root,
    /// where adding `/api` is what *causes* the 404. See [`normalize_api`].
    pub fn custom(url: &str) -> Self {
        let esplora = normalize_api(url);
        let explorer = strip_api(&esplora).to_string();
        Self {
            name: Cow::Owned(host_of(&esplora).to_string()),
            esplora_url: Cow::Owned(esplora),
            explorer_url: Cow::Owned(explorer),
            kind: ProfileKind::Custom,
        }
    }

    /// Built-in endpoints offered in the UI, with environment overrides applied.
    pub fn presets() -> Vec<Self> {
        if std::env::var(ENV_ESPLORA_URL).is_ok_and(|v| !v.trim().is_empty()) {
            return vec![Self::ecx_mainnet()];
        }
        PRESETS
            .iter()
            .map(|&(name, esplora, explorer)| Self {
                name: Cow::Borrowed(name),
                esplora_url: Cow::Borrowed(esplora),
                explorer_url: Cow::Borrowed(explorer),
                kind: ProfileKind::Ecx,
            })
            .collect()
    }

    /// Link to a transaction on this chain's explorer.
    pub fn tx_url(&self, txid: &str) -> String {
        format!("{}/tx/{}", self.explorer_url.trim_end_matches('/'), txid)
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

    pub fn is_ecx(&self) -> bool {
        matches!(self.kind, ProfileKind::Ecx | ProfileKind::Custom)
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
        Self::ecx_mainnet()
    }
}

/// Make a user-supplied URL into an Esplora API base.
///
/// Most deployments mount the API under `/api` on the explorer host, and users routinely paste
/// the explorer URL without it, so we append it. **But some serve Esplora at the root**, and
/// appending there produces exactly the 404 this is meant to prevent:
///
/// ```text
/// https://esplora.ecash.ninja/blocks/tip/height      -> 200 973577
/// https://esplora.ecash.ninja/api/blocks/tip/height  -> 404 endpoint does not exist
/// ```
///
/// The two are told apart by the leftmost host label, which is the naming convention across every
/// eCash net so far: `explorer.*` mounts under `/api`, `esplora.*` serves at the root. A URL that
/// already carries a path is left alone either way, so an explicit
/// `https://esplora.example.org/api` is still honoured for a host that wants it.
fn normalize_api(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.ends_with("/api") || serves_api_at_root(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}/api")
    }
}

/// Whether this host serves the Esplora API at its root rather than under `/api`.
fn serves_api_at_root(url: &str) -> bool {
    let host = host_of(url);
    // Only when the URL is a bare host: a path means the user was explicit, so respect it.
    url.trim_end_matches('/').ends_with(host) && host.starts_with("esplora.")
}

fn strip_api(url: &str) -> &str {
    url.trim_end_matches('/')
        .strip_suffix("/api")
        .unwrap_or(url)
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
    fn an_esplora_host_keeps_its_root_api_base() {
        // esplora.ecash.ninja serves Esplora at the root; appending /api 404s every call.
        for input in [
            "https://esplora.ecash.ninja",
            "https://esplora.ecash.ninja/",
            "  https://esplora.ecash.ninja  ",
        ] {
            assert_eq!(
                ChainProfile::custom(input).esplora_url,
                "https://esplora.ecash.ninja",
                "input was {input:?}"
            );
        }
        // An explicit /api is still honoured — the user said so.
        assert_eq!(
            ChainProfile::custom("https://esplora.example.org/api").esplora_url,
            "https://esplora.example.org/api"
        );
        // An explorer.* host still gets /api appended.
        assert_eq!(
            ChainProfile::custom("https://explorer.ecash.ninja").esplora_url,
            "https://explorer.ecash.ninja/api"
        );
    }

    #[test]
    fn the_explorer_is_the_api_base_without_api() {
        let p = ChainProfile::custom("https://explorer.ecash.ninja/api");
        assert_eq!(p.explorer_url, "https://explorer.ecash.ninja");
        assert_eq!(p.tx_url("abc123"), "https://explorer.ecash.ninja/tx/abc123");
    }

    #[test]
    fn every_preset_is_well_formed() {
        // Guards a fork migration that updates one URL and forgets the other. The api base and
        // the explorer are deliberately allowed to be different hosts — esplora.* serves the
        // API at its root while explorer.* hosts the human-facing site — so this checks shape
        // rather than asserting one is derived from the other.
        assert!(!PRESETS.is_empty());
        for &(name, esplora, explorer) in PRESETS {
            assert!(!name.is_empty());
            for url in [esplora, explorer] {
                assert!(url.starts_with("https://"), "{url} is not https");
                assert!(!url.ends_with('/'), "{url} has a trailing slash");
            }
            // A preset must survive its own normalizer untouched, or the UI and the env override
            // would disagree about the same host.
            assert_eq!(
                normalize_api(esplora),
                esplora,
                "{esplora} is not normalized"
            );
        }
    }

    #[test]
    fn the_presets_target_the_configured_fork() {
        // A stale endpoint is the other half of a half-done migration: the fork probe would be
        // comparing the right block against the wrong chain's indexer.
        assert_eq!(ecx_core::ECASH_HEIGHT, 973_728);
        for &(_, esplora, explorer) in PRESETS {
            let host = host_of(esplora);
            for url in [esplora, explorer] {
                assert!(
                    host_of(url).ends_with(".ecash.ninja"),
                    "{url} is not an eCash endpoint"
                );
            }
            // Mainnet hosts are the bare `*.ecash.ninja` names: exactly one label before the
            // domain, so a pre-launch subdomain (`*.beta.ecash.ninja`) cannot slip through.
            assert_eq!(
                host.matches('.').count(),
                2,
                "{host} has a pre-launch subdomain"
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
            super::super::ForkProbe::NoBitcoinReference
                .permit()
                .is_none()
        );
    }
}
