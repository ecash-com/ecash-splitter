//! Importing accounts from an air-gapped device.
//!
//! **Discovery needs xpubs, and xpubs come from the device.** Over USB we simply ask for the
//! twelve candidates. Air-gapped, there is nothing to ask — so the device has to hand them over
//! first, as a file or a QR, before any scan can happen and therefore before any PSBT can exist.
//! That makes the air-gap flow three hops, not one:
//!
//! 1. device exports its account xpubs → **this module**
//! 2. we scan, the user picks an account and a destination, we build the PSBT → export it
//! 3. device signs → import the signature, verify, broadcast
//!
//! What arrives here is public data — extended *public* keys and derivation paths. Nothing
//! secret crosses the gap, which is why this is safe to read from a USB stick.

use bitcoin::bip32::{DerivationPath, Fingerprint, Xpub};

use crate::{AccountCandidate, ScriptKind};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("could not read this as an account export: {0}")]
    Unrecognized(String),
    #[error("the export contained no usable accounts")]
    Empty,
}

/// One account a device told us about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAccount {
    pub candidate: AccountCandidate,
    pub fingerprint: Fingerprint,
    pub xpub: Xpub,
}

/// Parse whatever the device exported.
///
/// Accepts a Coldcard-style generic JSON export, or one output descriptor per line. Both are
/// what the common air-gapped devices actually produce.
pub fn parse_account_export(text: &str) -> Result<Vec<ImportedAccount>, ImportError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ImportError::Empty);
    }

    let accounts = if trimmed.starts_with('{') {
        parse_coldcard_json(trimmed)?
    } else {
        parse_descriptors(trimmed)?
    };

    if accounts.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(accounts)
}

/// Coldcard's generic export: a top-level `xfp` plus a `bip44`/`bip49`/`bip84`/`bip86` object
/// each carrying `deriv` and `xpub`. Passport and others emit the same shape.
fn parse_coldcard_json(text: &str) -> Result<Vec<ImportedAccount>, ImportError> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ImportError::Unrecognized(e.to_string()))?;

    let root_xfp = root.get("xfp").and_then(|v| v.as_str());
    let mut accounts = Vec::new();

    for (key, kind) in [
        ("bip44", ScriptKind::P2pkh),
        ("bip49", ScriptKind::P2shP2wpkh),
        ("bip84", ScriptKind::P2wpkh),
        ("bip86", ScriptKind::P2tr),
    ] {
        let Some(section) = root.get(key) else {
            continue;
        };
        let Some(xpub_text) = section.get("xpub").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(xpub) = xpub_text.parse::<Xpub>() else {
            continue;
        };

        // Prefer the section's own fingerprint, then the root's, then the xpub's parent.
        let fingerprint = section
            .get("xfp")
            .and_then(|v| v.as_str())
            .or(root_xfp)
            .and_then(|s| s.parse::<Fingerprint>().ok())
            .unwrap_or(xpub.parent_fingerprint);

        let account = section
            .get("deriv")
            .and_then(|v| v.as_str())
            .and_then(account_index_from_path)
            .unwrap_or(0);

        accounts.push(ImportedAccount {
            candidate: AccountCandidate {
                kind,
                account,
                path: kind.account_path(account),
            },
            fingerprint,
            xpub,
        });
    }

    Ok(accounts)
}

/// One output descriptor per line, e.g.
/// `wpkh([73c5da0a/84'/0'/0']xpub6.../0/*)`.
fn parse_descriptors(text: &str) -> Result<Vec<ImportedAccount>, ImportError> {
    let mut accounts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(account) = parse_descriptor_line(line) {
            // The same account appears twice when a device exports receive and change
            // descriptors separately; keep one.
            if !accounts.contains(&account) {
                accounts.push(account);
            }
        }
    }
    Ok(accounts)
}

fn parse_descriptor_line(line: &str) -> Option<ImportedAccount> {
    let kind = if line.starts_with("wpkh(") {
        ScriptKind::P2wpkh
    } else if line.starts_with("sh(wpkh(") {
        ScriptKind::P2shP2wpkh
    } else if line.starts_with("pkh(") {
        ScriptKind::P2pkh
    } else if line.starts_with("tr(") {
        ScriptKind::P2tr
    } else {
        return None;
    };

    // Key origin: [fingerprint/derivation]
    let origin_start = line.find('[')?;
    let origin_end = line.find(']')?;
    let origin = line.get(origin_start + 1..origin_end)?;
    let (fp_text, path_text) = origin.split_once('/')?;
    let fingerprint = fp_text.parse::<Fingerprint>().ok()?;

    // The xpub runs from the ']' to the next '/' that begins the child path.
    let after = line.get(origin_end + 1..)?;
    let xpub_text: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let xpub = xpub_text.parse::<Xpub>().ok()?;

    let account = account_index_from_path(path_text).unwrap_or(0);
    Some(ImportedAccount {
        candidate: AccountCandidate {
            kind,
            account,
            path: kind.account_path(account),
        },
        fingerprint,
        xpub,
    })
}

/// Third hardened component of `purpose'/coin'/account'`.
fn account_index_from_path(path: &str) -> Option<u32> {
    let normalized = path.trim().trim_start_matches("m/").trim_start_matches('/');
    let component = normalized.split('/').nth(2)?;
    component.trim_end_matches(['\'', 'h', 'H']).parse().ok()
}

/// Shape imported accounts the way `discover` wants them.
pub fn to_candidates(accounts: &[ImportedAccount]) -> Vec<(AccountCandidate, Xpub)> {
    accounts
        .iter()
        .map(|a| (a.candidate.clone(), a.xpub))
        .collect()
}

/// Every imported account should share one fingerprint — they come from one seed.
///
/// A mismatch means the export was stitched together from two devices or two passphrases, and
/// scanning it would show a blend of wallets.
pub fn common_fingerprint(accounts: &[ImportedAccount]) -> Option<Fingerprint> {
    let first = accounts.first()?.fingerprint;
    accounts
        .iter()
        .all(|a| a.fingerprint == first)
        .then_some(first)
}

/// Derivation paths the user can be told to export, when their device asks.
pub fn suggested_paths() -> Vec<DerivationPath> {
    ScriptKind::ALL.iter().map(|k| k.account_path(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XPUB: &str = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL";

    #[test]
    fn a_coldcard_style_export_yields_one_account_per_script_type() {
        let json = format!(
            r#"{{"chain":"BTC","xfp":"73C5DA0A","account":0,
                 "bip44":{{"deriv":"m/44'/0'/0'","xpub":"{XPUB}"}},
                 "bip49":{{"deriv":"m/49'/0'/0'","xpub":"{XPUB}"}},
                 "bip84":{{"deriv":"m/84'/0'/0'","xpub":"{XPUB}"}},
                 "bip86":{{"deriv":"m/86'/0'/0'","xpub":"{XPUB}"}}}}"#
        );
        let accounts = parse_account_export(&json).unwrap();
        assert_eq!(accounts.len(), 4);
        assert_eq!(
            common_fingerprint(&accounts).unwrap().to_string(),
            "73c5da0a"
        );
        let kinds: Vec<_> = accounts.iter().map(|a| a.candidate.kind).collect();
        assert!(kinds.contains(&ScriptKind::P2wpkh));
        assert!(kinds.contains(&ScriptKind::P2tr));
    }

    #[test]
    fn a_non_zero_account_index_is_read_from_the_derivation() {
        let json =
            format!(r#"{{"xfp":"73C5DA0A","bip84":{{"deriv":"m/84'/0'/2'","xpub":"{XPUB}"}}}}"#);
        let accounts = parse_account_export(&json).unwrap();
        assert_eq!(accounts[0].candidate.account, 2);
        assert_eq!(accounts[0].candidate.path.to_string(), "84'/0'/2'");
    }

    #[test]
    fn descriptors_are_parsed_and_deduplicated() {
        // Devices commonly export receive and change lines for the same account.
        let text =
            format!("wpkh([73c5da0a/84'/0'/0']{XPUB}/0/*)\nwpkh([73c5da0a/84'/0'/0']{XPUB}/1/*)\n");
        let accounts = parse_account_export(&text).unwrap();
        assert_eq!(accounts.len(), 1, "receive and change are one account");
        assert_eq!(accounts[0].candidate.kind, ScriptKind::P2wpkh);
    }

    #[test]
    fn wrapped_segwit_descriptors_are_recognized() {
        let text = format!("sh(wpkh([73c5da0a/49'/0'/0']{XPUB}/0/*))");
        let accounts = parse_account_export(&text).unwrap();
        assert_eq!(accounts[0].candidate.kind, ScriptKind::P2shP2wpkh);
    }

    #[test]
    fn a_mixed_fingerprint_export_is_flagged() {
        // Two seeds stitched together would scan as a blend of wallets.
        let mut accounts = parse_account_export(&format!(
            r#"{{"xfp":"73C5DA0A","bip84":{{"deriv":"m/84'/0'/0'","xpub":"{XPUB}"}}}}"#
        ))
        .unwrap();
        accounts.push(ImportedAccount {
            fingerprint: "0f056943".parse().unwrap(),
            ..accounts[0].clone()
        });
        assert!(common_fingerprint(&accounts).is_none());
    }

    #[test]
    fn junk_is_rejected() {
        assert!(parse_account_export("").is_err());
        assert!(parse_account_export("hello").is_err());
        assert!(parse_account_export("{}").is_err());
    }
}
