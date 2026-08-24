//! Asking Bitcoin about a coin.
//!
//! Height alone cannot tell you whether a coin is shared, and the mistake runs in **both**
//! directions. eCash's replay marker is permissive, not mandatory — the fork only makes the magic
//! locktime *count as* final, it never requires it — so eCash still accepts ordinary
//! Bitcoin-valid transactions. Spend an eCash coin from Sparrow, Electrum, or an exchange and
//! that transaction is valid on Bitcoin too, putting its outputs on **both chains at post-fork
//! heights**.
//!
//! So:
//!
//! - **Height under-reports, dangerously.** A post-fork coin from an unprotected spend is shared,
//!   and height calls it safe. The user believes they are separated and their next spend moves
//!   their BTC.
//! - **Height over-reports.** A pre-fork coin already spent on Bitcoin cannot be replayed onto,
//!   so it needs no split — but height insists it does.
//!
//! The sound invariant is inductive rather than a height comparison:
//!
//! > A UTXO is chain-specific **iff the transaction that created it could not appear on the other
//! > chain** — it carried the marker, or every one of its inputs was already chain-specific.
//!
//! Asking Bitcoin directly about the outpoint decides it in one step.
//!
//! Credit: this mirrors `../ecash-wallet-mobile/docs/coin-splitting.md`, which worked it out first
//! and paid for the trap in §[`BitcoinReference::verdict`].

use bitcoin::{OutPoint, Txid};
use esplora_client::AsyncClient;

use crate::ChainError;

/// Environment override for the Bitcoin endpoint used by the check.
pub const ENV_BITCOIN_ESPLORA_URL: &str = "ECX_BITCOIN_ESPLORA_URL";

/// Default Bitcoin Esplora. Only `/tx` and `/outspend` are used, so a host that cannot serve
/// `/scripthash` (mempool.space, say) is still fine here — unlike for wallet scanning.
pub const DEFAULT_BITCOIN_ESPLORA: &str = "https://blockstream.info/api";

/// What Bitcoin knows about one of our coins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitVerdict {
    /// Bitcoin has never seen the creating transaction, so the coin exists only on eCash.
    ChainSpecific,
    /// Bitcoin knows it and the output is unspent: the same coin is live on both chains.
    NeedsSplit,
    /// Bitcoin knows it but the output is already spent there, so a replay would be a
    /// double-spend and cannot land. Already separated.
    Separated,
    /// We could not reach a backend.
    ///
    /// **Deliberately distinct from safe.** "We could not check" and "you are fine" must never be
    /// the same answer on a screen about money.
    Unverified,
}

impl SplitVerdict {
    /// Decide from the two facts, with no I/O. Kept pure because the I/O is exactly what kept the
    /// wrong version of this logic untested for so long elsewhere.
    pub fn decide(exists_on_bitcoin: bool, spent_on_bitcoin: bool) -> Self {
        match (exists_on_bitcoin, spent_on_bitcoin) {
            (false, _) => SplitVerdict::ChainSpecific,
            (true, false) => SplitVerdict::NeedsSplit,
            (true, true) => SplitVerdict::Separated,
        }
    }

    /// Would splitting this coin actually achieve anything?
    pub fn needs_split(&self) -> bool {
        matches!(self, SplitVerdict::NeedsSplit)
    }

    /// True only for a decided, settled answer — never for [`SplitVerdict::Unverified`].
    pub fn is_decided(&self) -> bool {
        !matches!(self, SplitVerdict::Unverified)
    }

    pub fn label(&self) -> &'static str {
        match self {
            SplitVerdict::ChainSpecific => "eCash only",
            SplitVerdict::NeedsSplit => "shared with Bitcoin",
            SplitVerdict::Separated => "already separated",
            SplitVerdict::Unverified => "could not check",
        }
    }
}

/// A read-only Bitcoin backend, used purely to ask about outpoints.
///
/// Read-only by construction: this type has no broadcast method and never sees a key. It does
/// reveal the wallet's transaction ids to whoever runs the endpoint, which is why the check is
/// something a user asks for rather than something that happens on every scan.
pub struct BitcoinReference {
    url: String,
    client: AsyncClient,
}

impl std::fmt::Debug for BitcoinReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitcoinReference")
            .field("url", &self.url)
            .finish()
    }
}

impl BitcoinReference {
    /// Build from [`ENV_BITCOIN_ESPLORA_URL`], falling back to [`DEFAULT_BITCOIN_ESPLORA`].
    pub fn from_env() -> Result<Self, ChainError> {
        let url = std::env::var(ENV_BITCOIN_ESPLORA_URL)
            .ok()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| DEFAULT_BITCOIN_ESPLORA.to_string());
        Self::new(&url)
    }

    pub fn new(url: &str) -> Result<Self, ChainError> {
        let client = esplora_client::Builder::new(url)
            .build_async()
            .map_err(|e| ChainError::Unreachable(e.to_string()))?;
        Ok(Self {
            url: url.to_string(),
            client,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn host(&self) -> &str {
        self.url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or(&self.url)
    }

    /// Ask Bitcoin about one outpoint.
    ///
    /// **Order matters, and getting it wrong is silent.** `/tx/{txid}/outspend/{vout}` does *not*
    /// 404 for a transaction Bitcoin has never seen — it answers from the spend index and returns
    /// `200 {"spent": false}` for any txid at all, real or invented. Verified against both
    /// blockstream.info and mempool.space. Reading that as "Bitcoin never saw it" classifies
    /// **every eCash-only coin as shared**, including the output a split just created, so the
    /// warning never clears.
    ///
    /// Ask `/tx/{txid}` first for existence, and only then `/outspend`. It is also cheaper: an
    /// eCash-only coin resolves in a single request.
    pub async fn verdict(&self, outpoint: OutPoint) -> SplitVerdict {
        let exists = match self.tx_exists(outpoint.txid).await {
            Ok(exists) => exists,
            Err(e) => {
                tracing::debug!(%e, "bitcoin existence check failed");
                return SplitVerdict::Unverified;
            }
        };
        if !exists {
            return SplitVerdict::ChainSpecific;
        }
        match self.output_spent(outpoint).await {
            Ok(spent) => SplitVerdict::decide(true, spent),
            Err(e) => {
                tracing::debug!(%e, "bitcoin spentness check failed");
                SplitVerdict::Unverified
            }
        }
    }

    async fn tx_exists(&self, txid: Txid) -> Result<bool, ChainError> {
        match self.client.get_tx(&txid).await {
            Ok(found) => Ok(found.is_some()),
            Err(esplora_client::Error::HttpResponse { status: 404, .. }) => Ok(false),
            Err(e) => Err(ChainError::Unreachable(format!("{}: {e}", self.host()))),
        }
    }

    async fn output_spent(&self, outpoint: OutPoint) -> Result<bool, ChainError> {
        self.client
            .get_output_status(&outpoint.txid, outpoint.vout as u64)
            .await
            .map(|status| status.is_some_and(|s| s.spent))
            .map_err(|e| ChainError::Unreachable(format!("{}: {e}", self.host())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_cases_are_decided_without_io() {
        // Bitcoin never saw the creating transaction: eCash-only.
        assert_eq!(
            SplitVerdict::decide(false, false),
            SplitVerdict::ChainSpecific
        );
        assert_eq!(
            SplitVerdict::decide(false, true),
            SplitVerdict::ChainSpecific
        );
        // Bitcoin knows it and it is unspent there: the same coin is live on both chains.
        assert_eq!(SplitVerdict::decide(true, false), SplitVerdict::NeedsSplit);
        // Spent on Bitcoin: a replay would double-spend, so it cannot land.
        assert_eq!(SplitVerdict::decide(true, true), SplitVerdict::Separated);
    }

    #[test]
    fn only_shared_coins_need_splitting() {
        assert!(SplitVerdict::NeedsSplit.needs_split());
        assert!(!SplitVerdict::ChainSpecific.needs_split());
        assert!(!SplitVerdict::Separated.needs_split());
        assert!(!SplitVerdict::Unverified.needs_split());
    }

    #[test]
    fn unverified_is_never_mistaken_for_a_settled_answer() {
        // "We could not check" must not read as "you are fine".
        assert!(!SplitVerdict::Unverified.is_decided());
        assert!(SplitVerdict::ChainSpecific.is_decided());
        assert!(SplitVerdict::NeedsSplit.is_decided());
        assert!(SplitVerdict::Separated.is_decided());
    }
}
