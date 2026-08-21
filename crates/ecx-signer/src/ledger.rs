//! Ledger, via `async-hwi`.
//!
//! Ledger takes its PIN on-device, so there is nothing for the app to prompt for — see
//! `CLAUDE.md` Golden Rule 1.

use async_hwi::{
    HWI,
    ledger::{HidApi, Ledger, TransportHID},
};
use bitcoin::bip32::{DerivationPath, Fingerprint, Xpub};
use ecx_core::EcxPsbt;

use crate::{DeviceInfo, DeviceKind, Signer, SignerError};

fn map(e: async_hwi::Error) -> SignerError {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("denied") || lower.contains("reject") || lower.contains("declin") {
        SignerError::Declined
    } else {
        SignerError::Transport(text)
    }
}

/// List connected Ledgers.
///
/// `async-hwi` enumerates per module — there is no global "list all hardware wallets" call, so
/// this is our fan-out point (`CLAUDE.md` §5.5).
pub fn enumerate() -> Result<Vec<DeviceInfo>, SignerError> {
    let api = HidApi::new().map_err(|e| SignerError::Transport(e.to_string()))?;
    Ok(Ledger::<TransportHID>::enumerate(&api)
        .map(|info| DeviceInfo {
            kind: DeviceKind::Ledger,
            label: info
                .product_string()
                .map(str::to_owned)
                .unwrap_or_else(|| "Ledger".to_owned()),
        })
        .collect())
}

pub struct LedgerSigner {
    inner: Ledger<TransportHID>,
}

impl std::fmt::Debug for LedgerSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LedgerSigner")
    }
}

impl LedgerSigner {
    /// Connect to the first Ledger found.
    ///
    /// `display_xpub(false)` matters for discovery: it reads the twelve candidate xpubs without a
    /// button press each (`CLAUDE.md` §5.6).
    pub fn connect() -> Result<Self, SignerError> {
        let ledger = Ledger::<TransportHID>::try_connect_hid().map_err(map)?;
        let ledger = ledger.display_xpub(false).map_err(map)?;
        Ok(Self { inner: ledger })
    }
}

#[async_trait::async_trait]
impl Signer for LedgerSigner {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Ledger
    }

    async fn version(&self) -> Result<String, SignerError> {
        let v = self.inner.get_version().await.map_err(map)?;
        Ok(format!("{}.{}.{}", v.major, v.minor, v.patch))
    }

    async fn master_fingerprint(&self) -> Result<Fingerprint, SignerError> {
        self.inner.get_master_fingerprint().await.map_err(map)
    }

    async fn extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, SignerError> {
        self.inner.get_extended_pubkey(path).await.map_err(map)
    }

    async fn display_address(&self, _path: &DerivationPath) -> Result<(), SignerError> {
        // Needs a registered wallet policy for single-sig segwit; see the trait doc.
        Err(SignerError::Transport(
            "on-device address display needs a registered wallet policy (not yet implemented)"
                .into(),
        ))
    }

    async fn sign(&self, psbt: &mut EcxPsbt) -> Result<(), SignerError> {
        // Takes EcxPsbt: an unstamped PSBT cannot reach a device (Golden Rule 2).
        self.inner.sign_tx(psbt.psbt_mut()).await.map_err(map)
    }
}
