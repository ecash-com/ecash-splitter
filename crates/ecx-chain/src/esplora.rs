//! Esplora backend.
//!
//! Chosen over Electrum because it serves raw transactions over plain HTTPS — which we need for
//! `non_witness_utxo` (`CLAUDE.md` §5.4) — and because `bdk_esplora` scans straight into BDK.

use bitcoin::{BlockHash, Transaction, Txid};
use esplora_client::AsyncClient;

use crate::{BroadcastPermit, ChainError, ChainProfile, ChainSource, TipInfo};

pub struct EsploraChain {
    profile: ChainProfile,
    client: AsyncClient,
}

impl std::fmt::Debug for EsploraChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsploraChain")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

impl EsploraChain {
    pub fn new(profile: ChainProfile) -> Result<Self, ChainError> {
        let client = esplora_client::Builder::new(&profile.esplora_url)
            .build_async()
            .map_err(|e| ChainError::Unreachable(e.to_string()))?;
        Ok(Self { profile, client })
    }

    pub fn profile(&self) -> &ChainProfile {
        &self.profile
    }

    /// The underlying client, for `bdk_esplora`'s `full_scan` / `sync` extensions.
    pub fn client(&self) -> &AsyncClient {
        &self.client
    }
}

/// Short, human error text.
///
/// Raw `reqwest` errors are a paragraph of nested debug output. They are useless to a user and
/// they blow up the layout, so reduce them to a phrase and keep the detail in `tracing`.
fn describe(host: &str, e: esplora_client::Error) -> ChainError {
    use esplora_client::Error as E;
    tracing::debug!(%host, error = ?e, "esplora request failed");
    match e {
        E::HttpResponse { status: 404, .. } | E::HeaderHeightNotFound(_) => {
            ChainError::Malformed("not found".into())
        }
        E::HttpResponse { status, .. } => {
            ChainError::Unreachable(format!("{host} returned HTTP {status}"))
        }
        E::Reqwest(re) => {
            let what = if re.is_timeout() {
                "timed out"
            } else if re.is_connect() {
                "could not connect"
            } else if re.is_decode() {
                "sent an unreadable response"
            } else {
                "request failed"
            };
            ChainError::Unreachable(format!("{host} {what}"))
        }
        other => ChainError::Unreachable(format!("{host}: {other}")),
    }
}

impl EsploraChain {
    fn err(&self, e: esplora_client::Error) -> ChainError {
        describe(self.profile.host(), e)
    }

    /// Reduce an Esplora error to short, human text naming this host.
    pub fn describe_error(&self, e: esplora_client::Error) -> ChainError {
        self.err(e)
    }
}

#[async_trait::async_trait]
impl ChainSource for EsploraChain {
    async fn tip_height(&self) -> Result<u32, ChainError> {
        self.client.get_height().await.map_err(|e| self.err(e))
    }

    async fn tip(&self) -> Result<TipInfo, ChainError> {
        let hash = self.client.get_tip_hash().await.map_err(|e| self.err(e))?;
        let header = self
            .client
            .get_header_by_hash(&hash)
            .await
            .map_err(|e| self.err(e))?;
        let height = self.client.get_height().await.map_err(|e| self.err(e))?;
        Ok(TipInfo {
            height,
            time: header.time,
        })
    }

    async fn block_hash_at(&self, height: u32) -> Result<Option<BlockHash>, ChainError> {
        match self.client.get_block_hash(height).await {
            Ok(hash) => Ok(Some(hash)),
            // A height the chain has not reached yet is a 404, not a failure.
            Err(esplora_client::Error::HttpResponse { status: 404, .. }) => Ok(None),
            Err(esplora_client::Error::HeaderHeightNotFound(_)) => Ok(None),
            Err(e) => Err(self.err(e)),
        }
    }

    async fn raw_tx(&self, txid: Txid) -> Result<Option<Transaction>, ChainError> {
        self.client.get_tx(&txid).await.map_err(|e| self.err(e))
    }

    async fn broadcast(
        &self,
        tx: &Transaction,
        _permit: &BroadcastPermit,
    ) -> Result<Txid, ChainError> {
        // The permit proves the fork probe returned ConfirmedEcx for this endpoint.
        self.client.broadcast(tx).await.map_err(|e| self.err(e))?;
        Ok(tx.compute_txid())
    }
}
