//! Esplora backend.
//!
//! Chosen over Electrum because it serves raw transactions over plain HTTPS — which we need for
//! `non_witness_utxo` (`CLAUDE.md` §5.4) — and because `bdk_esplora` scans straight into BDK.

use bitcoin::{BlockHash, Transaction, Txid};
use esplora_client::AsyncClient;

use crate::{BroadcastPermit, ChainError, ChainProfile, ChainSource, TipInfo, TxState};

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
        // Keep the server's message. Esplora puts the node's actual rejection reason in the
        // body, and discarding it turns "your fee is too low" into "HTTP 400".
        E::HttpResponse { status, message } => {
            let detail = message.trim();
            if detail.is_empty() {
                ChainError::Unreachable(format!("{host} returned HTTP {status}"))
            } else {
                ChainError::Rejected(format!("{host}: {detail}"))
            }
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

    async fn tx_state(&self, txid: Txid) -> Result<TxState, ChainError> {
        let status = match self.client.get_tx_status(&txid).await {
            Ok(s) => s,
            // Esplora 404s on a transaction it has never seen, which is a state rather than a
            // failure: not broadcast, or dropped from the mempool.
            Err(esplora_client::Error::HttpResponse { status: 404, .. }) => {
                return Ok(TxState::Unknown);
            }
            Err(e) => return Err(self.err(e)),
        };

        let Some(height) = status.block_height.filter(|_| status.confirmed) else {
            return Ok(TxState::InMempool);
        };

        let tip = self.client.get_height().await.map_err(|e| self.err(e))?;
        Ok(TxState::Confirmed {
            height,
            // A transaction in the tip block has one confirmation, not zero.
            confirmations: tip.saturating_sub(height).saturating_add(1),
        })
    }

    async fn broadcast(
        &self,
        tx: &Transaction,
        _permit: &BroadcastPermit,
    ) -> Result<Txid, ChainError> {
        // The permit proves the fork probe returned ConfirmedEcx for this endpoint.
        //
        // Posted directly rather than through `esplora-client`, which sends no `Content-Type`
        // header at all. Some Esplora deployments require `text/plain` and, without it, never
        // hand the body to the node — explorer.alpha.ecash.ninja answers a bare POST with
        // `sendrawtransaction RPC error: {\"code\":-1}` for *any* input, valid or not, while
        // the same bytes with `text/plain` are parsed and judged on their merits.
        let hex = bitcoin::consensus::encode::serialize_hex(tx);
        let url = format!("{}/tx", self.profile.esplora_url.trim_end_matches('/'));

        let response = reqwest::Client::new()
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "text/plain")
            .body(hex)
            .send()
            .await
            .map_err(|e| ChainError::Unreachable(format!("{}: {e}", self.profile.host())))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            // The node's reason lives in the body; it is the whole point of the error.
            return Err(ChainError::Rejected(format!(
                "{} rejected the transaction: {}",
                self.profile.host(),
                body.trim()
            )));
        }

        // Esplora answers with the txid. Trust ours over a parsed echo, but flag a mismatch:
        // it would mean the endpoint published something other than what we sent.
        let expected = tx.compute_txid();
        if let Ok(returned) = body.trim().parse::<Txid>() {
            if returned != expected {
                return Err(ChainError::Malformed(format!(
                    "endpoint returned txid {returned}, but we sent {expected}"
                )));
            }
        }
        Ok(expected)
    }
}
