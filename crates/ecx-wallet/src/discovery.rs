//! BIP44 account discovery.
//!
//! The device is a pubkey oracle and a signer: it does not know your balance and cannot tell you
//! which accounts you use (`CLAUDE.md` §5.6). Every derivation path yields a valid key, so
//! "which accounts have coins" is a question for the chain. We ask the device for xpubs and the
//! indexer for history.

use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::{KeychainKind, Wallet};
use bitcoin::{
    Network,
    bip32::{Fingerprint, Xpub},
};
use ecx_chain::{EsploraChain, ScanReadiness};

/// "3 hours", "2 days" — for error text.
pub(crate) fn humanize(secs: u64) -> String {
    match secs {
        s if s < 120 => format!("{s} seconds"),
        s if s < 7_200 => format!("{} minutes", s / 60),
        s if s < 172_800 => format!("{} hours", s / 3_600),
        s => format!("{} days", s / 86_400),
    }
}

use crate::{AccountCandidate, DiscoveredAccount, STOP_GAP, descriptor_pair};

/// Concurrent requests per scan. Esplora hosts are shared infrastructure; be a good citizen.
pub(crate) const PARALLEL_REQUESTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WalletError {
    #[error("descriptor is invalid: {0}")]
    Descriptor(String),
    #[error("scan failed: {0}")]
    Scan(String),
    #[error(
        "refusing to report results: the indexer is at block {tip}, whose timestamp is {} old, \
         so it has not caught up and an empty result would be indistinguishable from an empty \
         wallet", humanize(*age_secs)
    )]
    NotSynced { tip: u32, age_secs: u64 },
}

/// How far to look.
///
/// Two independent depths, easy to confuse:
///
/// - `accounts` — how many **account indices** to probe per script type. `m/84'/0'/0'`,
///   `.../1'`, `.../2'` and so on. Three covers the overwhelming majority; someone who created
///   an account further out needs this raised or their coins are invisible.
/// - `stop_gap` — how many consecutive **unused addresses** inside one account end the scan.
///   Twenty is the BIP44 convention. Raising it helps a wallet that skipped addresses; it does
///   not help find a missing account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryDepth {
    pub accounts: u32,
    pub stop_gap: usize,
}

impl Default for DiscoveryDepth {
    fn default() -> Self {
        Self {
            accounts: crate::DEFAULT_ACCOUNTS_PROBED,
            stop_gap: STOP_GAP,
        }
    }
}

impl DiscoveryDepth {
    /// Candidates this depth produces: four script types × `accounts`.
    pub fn candidate_count(&self) -> usize {
        crate::ScriptKind::ALL.len() * self.accounts as usize
    }
}

/// Progress for the UI. Discovery takes tens of seconds, so it must be narratable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryProgress {
    pub scanned: usize,
    pub total: usize,
    pub current: AccountCandidate,
}

/// Scan one account and report what is there.
///
/// Returns `None` when the account has no history at all — the common case, since most of the
/// twelve candidates are empty for any given user.
async fn scan_one(
    chain: &EsploraChain,
    candidate: &AccountCandidate,
    fingerprint: Fingerprint,
    xpub: &Xpub,
    stop_gap: usize,
) -> Result<Option<DiscoveredAccount>, WalletError> {
    let (external, internal) = descriptor_pair(candidate, fingerprint, xpub);

    let mut wallet = Wallet::create::<String>(external.clone(), internal.clone())
        .network(Network::Bitcoin)
        .create_wallet_no_persist()
        .map_err(|e| WalletError::Descriptor(e.to_string()))?;

    let request = wallet.start_full_scan().build();
    // Route through the chain so the error is a phrase, not a paragraph of reqwest debug.
    let update = chain
        .client()
        .full_scan::<KeychainKind, _>(request, stop_gap, PARALLEL_REQUESTS)
        .await
        .map_err(|e| WalletError::Scan(chain.describe_error(*e).to_string()))?;
    wallet
        .apply_update(update)
        .map_err(|e| WalletError::Scan(e.to_string()))?;

    let tx_count = wallet.transactions().count();
    if tx_count == 0 {
        return Ok(None);
    }

    let utxos: Vec<crate::Utxo> = wallet
        .list_unspent()
        .map(|u| crate::Utxo {
            outpoint: u.outpoint,
            value: u.txout.value,
            height: match u.chain_position {
                bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } => {
                    Some(anchor.block_id.height)
                }
                bdk_wallet::chain::ChainPosition::Unconfirmed { .. } => None,
            },
        })
        .collect();

    Ok(Some(DiscoveredAccount {
        candidate: candidate.clone(),
        descriptor: external,
        change_descriptor: internal,
        utxo_count: utxos.len(),
        utxos,
        balance: wallet.balance().total(),
        tx_count,
    }))
}

/// Scan every candidate, reporting progress as it goes.
///
/// **Golden Rule 9**: gated on [`guard_synced`], so an empty return value always means "we
/// looked at a caught-up chain and found nothing", never "we could not see far enough".
pub async fn discover(
    chain: &EsploraChain,
    readiness: ScanReadiness,
    fingerprint: Fingerprint,
    xpubs: &[(AccountCandidate, Xpub)],
    depth: DiscoveryDepth,
    mut on_progress: impl FnMut(DiscoveryProgress),
) -> Result<Vec<DiscoveredAccount>, WalletError> {
    guard_synced(readiness)?;

    let total = xpubs.len();
    let mut found = Vec::new();

    for (scanned, (candidate, xpub)) in xpubs.iter().enumerate() {
        on_progress(DiscoveryProgress {
            scanned,
            total,
            current: candidate.clone(),
        });
        if let Some(account) = scan_one(chain, candidate, fingerprint, xpub, depth.stop_gap).await?
        {
            found.push(account);
        }
    }

    // Largest balance first: the account a user means is almost always their biggest.
    found.sort_by(|a, b| b.balance.cmp(&a.balance));
    Ok(found)
}

/// Golden Rule 9, as a guard.
///
/// A partially-synced indexer is indistinguishable from an empty wallet, and "you have no coins"
/// is the one wrong answer a user acts on immediately and irreversibly.
pub fn guard_synced(readiness: ScanReadiness) -> Result<(), WalletError> {
    match readiness {
        ScanReadiness::Ready { .. } => Ok(()),
        ScanReadiness::Behind { tip, age_secs } => Err(WalletError::NotSynced { tip, age_secs }),
    }
}

/// Keychains a discovered account exposes, for later PSBT construction.
pub const KEYCHAINS: [KeychainKind; 2] = [KeychainKind::External, KeychainKind::Internal];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_gated_on_sync() {
        assert_eq!(
            guard_synced(ScanReadiness::Behind {
                tip: 458_330,
                age_secs: 90_000
            }),
            Err(WalletError::NotSynced {
                tip: 458_330,
                age_secs: 90_000
            })
        );
        assert!(guard_synced(ScanReadiness::Ready { tip: 963_466 }).is_ok());
    }

    #[test]
    fn humanize_reads_naturally() {
        assert_eq!(humanize(30), "30 seconds");
        assert_eq!(humanize(600), "10 minutes");
        assert_eq!(humanize(9_000), "2 hours");
        assert_eq!(humanize(500_000), "5 days");
    }
}
