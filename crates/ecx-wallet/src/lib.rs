//! Watch-only wallet: descriptors from device xpubs, BIP44 account discovery, PSBT construction.
//!
//! Watch-only by construction — this crate never holds or accepts a private key.

use bitcoin::Amount;
use bitcoin::bip32::{DerivationPath, Fingerprint, Xpub};

pub mod build;
pub mod discovery;
pub mod import;

pub use build::{BuildError, SweepSummary, build_sweep, device_destination, summarize};
pub use discovery::{DiscoveryProgress, WalletError, discover};
pub use import::{ImportError, ImportedAccount, parse_account_export};

/// ECX has no SLIP-44 coin type of its own, so everything derives under Bitcoin's `0'`
/// (`CLAUDE.md` §3). That is where the coins are; there is no alternative.
pub const COIN_TYPE: u32 = 0;

/// Standard gap limit for a full scan.
pub const STOP_GAP: usize = 20;

/// How many account indices to probe per script type by default. Twelve candidates total covers
/// the overwhelming majority of real users; "scan deeper" raises it (`CLAUDE.md` §5.6).
pub const DEFAULT_ACCOUNTS_PROBED: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    /// BIP84 native segwit, `bc1q…`
    P2wpkh,
    /// BIP49 wrapped segwit, `3…`
    P2shP2wpkh,
    /// BIP44 legacy, `1…`
    P2pkh,
    /// BIP86 taproot, `bc1p…`
    P2tr,
}

impl ScriptKind {
    pub const ALL: [ScriptKind; 4] = [
        ScriptKind::P2wpkh,
        ScriptKind::P2shP2wpkh,
        ScriptKind::P2pkh,
        ScriptKind::P2tr,
    ];

    pub fn purpose(self) -> u32 {
        match self {
            ScriptKind::P2wpkh => 84,
            ScriptKind::P2shP2wpkh => 49,
            ScriptKind::P2pkh => 44,
            ScriptKind::P2tr => 86,
        }
    }

    /// Descriptor fragment name, for building `wpkh(...)`, `sh(wpkh(...))`, and so on.
    pub fn wrap(self, inner: &str) -> String {
        match self {
            ScriptKind::P2wpkh => format!("wpkh({inner})"),
            ScriptKind::P2shP2wpkh => format!("sh(wpkh({inner}))"),
            ScriptKind::P2pkh => format!("pkh({inner})"),
            ScriptKind::P2tr => format!("tr({inner})"),
        }
    }

    /// Human label for the address style this account produces.
    pub fn label(self) -> &'static str {
        match self {
            ScriptKind::P2wpkh => "Native SegWit",
            ScriptKind::P2shP2wpkh => "Wrapped SegWit",
            ScriptKind::P2pkh => "Legacy",
            ScriptKind::P2tr => "Taproot",
        }
    }

    /// Example address prefix, for orienting the user.
    pub fn prefix(self) -> &'static str {
        match self {
            ScriptKind::P2wpkh => "bc1q…",
            ScriptKind::P2shP2wpkh => "3…",
            ScriptKind::P2pkh => "1…",
            ScriptKind::P2tr => "bc1p…",
        }
    }

    /// `m/{purpose}'/0'/{account}'`
    pub fn account_path(self, account: u32) -> DerivationPath {
        format!("m/{}'/{}'/{}'", self.purpose(), COIN_TYPE, account)
            .parse()
            .expect("well-formed derivation path")
    }
}

/// One account to probe during discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCandidate {
    pub kind: ScriptKind,
    pub account: u32,
    pub path: DerivationPath,
}

/// The default discovery grid: every script type × `accounts` indices.
pub fn candidates(accounts: u32) -> Vec<AccountCandidate> {
    ScriptKind::ALL
        .iter()
        .flat_map(|&kind| {
            (0..accounts).map(move |account| AccountCandidate {
                kind,
                account,
                path: kind.account_path(account),
            })
        })
        .collect()
}

/// Watch-only descriptor pair — external (receive) and internal (change) — for one account.
///
/// `kind([fingerprint/purpose'/0'/account']xpub/0/*)` and `.../1/*`.
pub fn descriptor_pair(
    candidate: &AccountCandidate,
    fingerprint: Fingerprint,
    xpub: &Xpub,
) -> (String, String) {
    let path = candidate.path.to_string();
    let path = path.trim_start_matches("m/");
    let origin = format!("[{fingerprint}/{path}]{xpub}");
    (
        candidate.kind.wrap(&format!("{origin}/0/*")),
        candidate.kind.wrap(&format!("{origin}/1/*")),
    )
}

/// An account discovery actually found history in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredAccount {
    pub candidate: AccountCandidate,
    /// External (receive) descriptor.
    pub descriptor: String,
    /// Internal (change) descriptor.
    pub change_descriptor: String,
    pub utxo_count: usize,
    pub balance: Amount,
    /// Total transactions seen. An account with history but a zero balance is still worth
    /// showing — it tells the user we looked and found nothing left to split.
    pub tx_count: usize,
}

impl DiscoveredAccount {
    pub fn label(&self) -> String {
        format!("{} · {}", self.candidate.kind.label(), self.candidate.path)
    }

    pub fn is_splittable(&self) -> bool {
        self.balance > Amount::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grid_is_twelve_candidates() {
        assert_eq!(candidates(DEFAULT_ACCOUNTS_PROBED).len(), 12);
    }

    #[test]
    fn paths_use_bitcoin_coin_type() {
        // ECX has no SLIP-44 of its own; everything is under 0'.
        assert_eq!(ScriptKind::P2wpkh.account_path(0).to_string(), "84'/0'/0'");
        assert_eq!(ScriptKind::P2tr.account_path(1).to_string(), "86'/0'/1'");
        assert_eq!(ScriptKind::P2pkh.account_path(2).to_string(), "44'/0'/2'");
        assert_eq!(
            ScriptKind::P2shP2wpkh.account_path(0).to_string(),
            "49'/0'/0'"
        );
    }

    #[test]
    fn descriptor_pair_splits_receive_and_change() {
        let fp: Fingerprint = "73c5da0a".parse().unwrap();
        let xpub: Xpub = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL".parse().unwrap();
        let c = AccountCandidate {
            kind: ScriptKind::P2wpkh,
            account: 0,
            path: ScriptKind::P2wpkh.account_path(0),
        };
        let (ext, int) = descriptor_pair(&c, fp, &xpub);
        assert!(ext.starts_with("wpkh([73c5da0a/84'/0'/0']xpub"), "{ext}");
        assert!(ext.ends_with("/0/*)"), "{ext}");
        assert!(int.ends_with("/1/*)"), "{int}");
    }

    #[test]
    fn descriptors_wrap_by_script_kind() {
        assert_eq!(ScriptKind::P2wpkh.wrap("K"), "wpkh(K)");
        assert_eq!(ScriptKind::P2shP2wpkh.wrap("K"), "sh(wpkh(K))");
        assert_eq!(ScriptKind::P2pkh.wrap("K"), "pkh(K)");
        assert_eq!(ScriptKind::P2tr.wrap("K"), "tr(K)");
    }
}
