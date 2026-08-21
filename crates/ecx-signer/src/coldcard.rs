//! Coldcard over USB, via `async-hwi`.
//!
//! Coldcard takes its PIN on-device, so there is nothing for the app to prompt for.

use async_hwi::{
    HWI,
    coldcard::{Coldcard, api},
};
use bitcoin::bip32::{DerivationPath, Fingerprint, Xpub};
use ecx_core::EcxPsbt;

use crate::{DeviceInfo, DeviceKind, SignedTx, Signer, SignerError, ledger::map_hwi};

/// List connected Coldcards.
pub fn enumerate() -> Result<Vec<DeviceInfo>, SignerError> {
    let mut detector = match api::Api::new() {
        Ok(d) => d,
        // No HID access at all is a platform problem, not "no Coldcard attached".
        Err(e) => return Err(SignerError::Transport(e.to_string())),
    };
    let serials = detector
        .detect()
        .map_err(|e| SignerError::Transport(e.to_string()))?;
    Ok(serials
        .into_iter()
        .map(|serial| DeviceInfo {
            kind: DeviceKind::Coldcard,
            label: format!("Coldcard {serial:?}"),
        })
        .collect())
}

pub struct ColdcardSigner {
    inner: Coldcard,
}

impl std::fmt::Debug for ColdcardSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ColdcardSigner")
    }
}

impl ColdcardSigner {
    /// Connect to the first Coldcard found.
    pub fn connect() -> Result<Self, SignerError> {
        let mut detector = api::Api::new().map_err(|e| SignerError::Transport(e.to_string()))?;
        let serial = detector
            .detect()
            .map_err(|e| SignerError::Transport(e.to_string()))?
            .into_iter()
            .next()
            .ok_or(SignerError::NoDevice)?;
        let (device, _) = detector
            .open(&serial, None)
            .map_err(|e| SignerError::Transport(e.to_string()))?;
        Ok(Self {
            inner: device.into(),
        })
    }
}

#[async_trait::async_trait]
impl Signer for ColdcardSigner {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Coldcard
    }

    async fn version(&self) -> Result<String, SignerError> {
        let v = self.inner.get_version().await.map_err(map_hwi)?;
        Ok(format!("{}.{}.{}", v.major, v.minor, v.patch))
    }

    async fn master_fingerprint(&self) -> Result<Fingerprint, SignerError> {
        self.inner.get_master_fingerprint().await.map_err(map_hwi)
    }

    async fn extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, SignerError> {
        self.inner.get_extended_pubkey(path).await.map_err(map_hwi)
    }

    async fn display_address(&self, _path: &DerivationPath) -> Result<(), SignerError> {
        Err(SignerError::Unsupported {
            what: "on-device address display".into(),
        })
    }

    async fn sign(&self, psbt: &EcxPsbt) -> Result<SignedTx, SignerError> {
        let mut working = psbt.psbt().clone();
        self.inner.sign_tx(&mut working).await.map_err(map_hwi)?;
        Ok(SignedTx::Psbt(Box::new(working)))
    }
}
