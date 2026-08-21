//! Blockstream Jade over serial, via `async-hwi`.
//!
//! **Unlocking Jade contacts Blockstream's blind PIN oracle over HTTPS.** That is not incidental
//! and not something to design around: Jade has no secure element, so the oracle is what makes a
//! short PIN resistant to physical extraction. The oracle is *blind* — it never learns the PIN or
//! any key material — and the host is a relay for an encrypted handshake between device and
//! oracle, not a participant in it (`async-hwi`'s `jade::pinserver`).
//!
//! This is the one place in the app that talks to a host other than the chain source, and it is
//! reached only when a user plugs in a Jade. See `CLAUDE.md` Golden Rule 8.

use async_hwi::{
    HWI,
    jade::{Jade, SerialTransport},
};
use bitcoin::{
    Network,
    bip32::{DerivationPath, Fingerprint, Xpub},
};
use ecx_core::EcxPsbt;

use crate::{DeviceInfo, DeviceKind, Signer, SignerError, ledger::map_hwi};

/// Probe serial ports for a Jade.
pub async fn enumerate() -> Result<Vec<DeviceInfo>, SignerError> {
    match Jade::<SerialTransport>::enumerate().await {
        Ok(devices) => Ok(devices
            .into_iter()
            .map(|_| DeviceInfo {
                kind: DeviceKind::Jade,
                label: "Jade".to_string(),
            })
            .collect()),
        Err(_) => Ok(Vec::new()),
    }
}

pub struct JadeSigner {
    inner: Jade<SerialTransport>,
}

impl std::fmt::Debug for JadeSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JadeSigner")
    }
}

impl JadeSigner {
    /// Connect to the first Jade found and unlock it.
    ///
    /// `auth()` is the step that relays to the oracle. It is a no-op if the device is already
    /// unlocked, so a user who unlocked via the Blockstream app first never triggers a request.
    pub async fn connect() -> Result<Self, SignerError> {
        let device = Jade::<SerialTransport>::enumerate()
            .await
            .map_err(|e| SignerError::Transport(format!("{e:?}")))?
            .into_iter()
            .next()
            .ok_or(SignerError::NoDevice)?
            .with_network(Network::Bitcoin);

        device
            .auth()
            .await
            .map_err(|e| SignerError::JadeUnlock(flatten(format!("{e:?}"))))?;

        Ok(Self { inner: device })
    }
}

fn flatten(text: String) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[async_trait::async_trait]
impl Signer for JadeSigner {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Jade
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

    async fn sign(&self, psbt: &mut EcxPsbt) -> Result<(), SignerError> {
        self.inner.sign_tx(psbt.psbt_mut()).await.map_err(map_hwi)
    }
}
