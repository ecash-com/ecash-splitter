//! Specter DIY over serial, via `async-hwi`.

use async_hwi::{
    HWI,
    specter::{SerialTransport, Specter},
};
use bitcoin::bip32::{DerivationPath, Fingerprint, Xpub};
use ecx_core::EcxPsbt;

use crate::{DeviceInfo, DeviceKind, SignedTx, Signer, SignerError, ledger::map_hwi};

/// Probe serial ports for a Specter.
///
/// Unlike HID, there is no cheap way to identify one from port metadata — `async-hwi` opens each
/// candidate port and asks for a fingerprint. That makes enumeration slower than the others.
pub async fn enumerate() -> Result<Vec<DeviceInfo>, SignerError> {
    match Specter::<SerialTransport>::enumerate().await {
        Ok(devices) => Ok(devices
            .into_iter()
            .map(|_| DeviceInfo {
                kind: DeviceKind::Specter,
                label: "Specter".to_string(),
            })
            .collect()),
        // No serial ports, or none that answered. Not an error worth surfacing.
        Err(_) => Ok(Vec::new()),
    }
}

pub struct SpecterSigner {
    inner: Specter<SerialTransport>,
}

impl std::fmt::Debug for SpecterSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpecterSigner")
    }
}

impl SpecterSigner {
    pub async fn connect() -> Result<Self, SignerError> {
        let device = Specter::<SerialTransport>::enumerate()
            .await
            .map_err(|e| SignerError::Transport(format!("{e:?}")))?
            .into_iter()
            .next()
            .ok_or(SignerError::NoDevice)?;
        Ok(Self { inner: device })
    }
}

#[async_trait::async_trait]
impl Signer for SpecterSigner {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Specter
    }

    async fn version(&self) -> Result<String, SignerError> {
        let v = HWI::get_version(&self.inner).await.map_err(map_hwi)?;
        Ok(format!("{}.{}.{}", v.major, v.minor, v.patch))
    }

    async fn master_fingerprint(&self) -> Result<Fingerprint, SignerError> {
        HWI::get_master_fingerprint(&self.inner)
            .await
            .map_err(map_hwi)
    }

    async fn extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, SignerError> {
        HWI::get_extended_pubkey(&self.inner, path)
            .await
            .map_err(map_hwi)
    }

    async fn display_address(&self, _path: &DerivationPath) -> Result<(), SignerError> {
        Err(SignerError::Unsupported {
            what: "on-device address display".into(),
        })
    }

    async fn sign(&self, psbt: &EcxPsbt) -> Result<SignedTx, SignerError> {
        let mut working = psbt.psbt().clone();
        HWI::sign_tx(&self.inner, &mut working)
            .await
            .map_err(map_hwi)?;
        Ok(SignedTx::Psbt(Box::new(working)))
    }
}
