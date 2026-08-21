//! Device abstraction.
//!
//! BDK never talks to a device (`CLAUDE.md` §5.2): its output is a PSBT and the device's input is
//! a PSBT, so the seam is a byte buffer. This crate owns that seam.
//!
//! Backends: `async-hwi` (Ledger, BitBox02, Coldcard, Jade, Specter), `trezor-client`
//! (Trezor Model T / Safe 3 / Safe 5), and air-gapped PSBT over file or QR.

use bitcoin::{
    Psbt, Transaction,
    bip32::{DerivationPath, Fingerprint, Xpub},
};
use ecx_core::EcxPsbt;

pub mod airgap;
pub mod coldcard;
pub mod jade;
pub mod ledger;
pub mod specter;
pub mod trezor;

pub use coldcard::ColdcardSigner;
pub use jade::JadeSigner;
pub use ledger::LedgerSigner;
pub use specter::SpecterSigner;
pub use trezor::TrezorSigner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Ledger,
    BitBox02,
    Coldcard,
    Jade,
    Specter,
    /// Trezor Model T / Safe 3 / Safe 5. **Model One is unsupported** — see [`SignerError`].
    Trezor,
    /// PSBT over SD card or animated QR. No USB, no vendor library.
    AirGap,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignerError {
    #[error("no device found")]
    NoDevice,

    #[error(
        "{model} is not supported: it cannot take a PIN or passphrase on-device, and this app has no PIN or passphrase screen (CLAUDE.md §5.5)"
    )]
    UnsupportedDevice { model: String },

    /// Trezor Model One asks the host to render the blind PIN matrix. We refuse (Golden Rule 1).
    #[error(
        "this device requires host-side PIN entry, which this app does not implement; Trezor Model One holders should use ecash-electrum"
    )]
    HostPinRequested,

    /// Model One cannot take a passphrase on-device and would send it in cleartext.
    #[error(
        "this device requires host-side passphrase entry; enter the passphrase on the device instead"
    )]
    HostPassphraseRequested,

    #[error(
        "device fingerprint {found} does not match the descriptor's {expected}; wrong device, or a different passphrase"
    )]
    FingerprintMismatch {
        found: Fingerprint,
        expected: Fingerprint,
    },

    #[error("the device declined to sign")]
    Declined,

    #[error("the device is not ready — unlock it and open the Bitcoin app, then try again")]
    NotReady,

    #[error("device communication failed: {0}")]
    Transport(String),

    #[error("{what} is not supported on this device yet")]
    Unsupported { what: String },

    #[error("air-gap: {0}")]
    AirGap(String),

    /// Jade's unlock relays an encrypted handshake to Blockstream's blind PIN oracle. The most
    /// common cause of failure is simply having no route to it.
    #[error(
        "Jade unlock failed: {0} — unlocking relays an encrypted handshake to Blockstream's \
         blind PIN oracle, so this needs a working internet connection"
    )]
    JadeUnlock(String),
}

/// What a device must do. Mirrors `async-hwi`'s `HWI` trait so wrapping it is mechanical.
///
/// Note [`Signer::sign`] takes an [`EcxPsbt`], not a `Psbt`: a PSBT that has not passed through
/// `ecx_core::finalize_ecx_psbt` cannot reach a device, enforced by the type system rather than
/// by review (Golden Rule 2).
#[async_trait::async_trait]
pub trait Signer: Send + Sync {
    fn kind(&self) -> DeviceKind;
    async fn version(&self) -> Result<String, SignerError>;
    async fn master_fingerprint(&self) -> Result<Fingerprint, SignerError>;
    async fn extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, SignerError>;
    /// Show an address on the device screen, for verifying a device-derived destination (§7.5).
    ///
    /// Ledger needs a registered wallet policy for anything but BIP86 taproot, so this is
    /// unimplemented for single-sig segwit until `register_wallet` lands (§12).
    async fn display_address(&self, path: &DerivationPath) -> Result<(), SignerError>;
    async fn sign(&self, psbt: &EcxPsbt) -> Result<SignedTx, SignerError>;
}

/// What a device hands back after signing.
///
/// Devices genuinely differ here, and flattening the difference would mean lying to one of them:
///
/// - Most fill signatures into the PSBT, which then needs finalizing and extracting.
/// - **Trezor returns the fully serialized signed transaction instead.** It emits a raw signature
///   rather than a scriptSig, and `trezor-client` deliberately dropped its `apply_signature`
///   helper because putting one back into a PSBT requires pubkey and script inspection. So it
///   streams the finished transaction and we take it as-is.
///
/// Both shapes converge at `ecx_core::verify_signed`, which is what actually matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedTx {
    /// Signatures filled into the PSBT. Still needs finalizing and extracting.
    Psbt(Box<Psbt>),
    /// A complete signed transaction, ready to verify.
    Transaction(Box<Transaction>),
}

/// A device the app can see but has not connected to yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub kind: DeviceKind,
    pub label: String,
}

/// Enumerate connected devices.
///
/// There is no global "list all hardware wallets" call — `async-hwi` enumerates per module
/// (`Ledger::enumerate(&HidApi)`, …) and `trezor-client` has its own. This is where we write the
/// fan-out (`CLAUDE.md` §5.5).
pub async fn enumerate() -> Result<Vec<DeviceInfo>, SignerError> {
    let mut found = Vec::new();

    // Each backend enumerates on its own transport; a failure in one must not hide the others,
    // so nothing here is `?`. A user with a working Ledger should not see an empty list because
    // their machine has no serial ports.
    match ledger::enumerate() {
        Ok(devices) => found.extend(devices),
        Err(e) => tracing::debug!(error = %e, "ledger enumeration failed"),
    }
    match coldcard::enumerate() {
        Ok(devices) => found.extend(devices),
        Err(e) => tracing::debug!(error = %e, "coldcard enumeration failed"),
    }
    match specter::enumerate().await {
        Ok(devices) => found.extend(devices),
        Err(e) => tracing::debug!(error = %e, "specter enumeration failed"),
    }
    match jade::enumerate().await {
        Ok(devices) => found.extend(devices),
        Err(e) => tracing::debug!(error = %e, "jade enumeration failed"),
    }
    match trezor::enumerate() {
        Ok(devices) => found.extend(devices),
        Err(e) => tracing::debug!(error = %e, "trezor enumeration failed"),
    }

    Ok(found)
}

/// Connect to the first device of the given kind.
pub async fn connect(kind: DeviceKind) -> Result<Box<dyn Signer>, SignerError> {
    match kind {
        DeviceKind::Ledger => Ok(Box::new(LedgerSigner::connect()?)),
        DeviceKind::Coldcard => Ok(Box::new(ColdcardSigner::connect()?)),
        DeviceKind::Specter => Ok(Box::new(SpecterSigner::connect().await?)),
        DeviceKind::Trezor => {
            // Connecting spawns the device thread and blocks on its handshake, so keep it off
            // the async executor.
            tokio_blocking(TrezorSigner::connect).await
        }
        DeviceKind::BitBox02 => Err(SignerError::Unsupported {
            what: "BitBox02 (needs the pairing-code confirmation flow)".into(),
        }),
        DeviceKind::Jade => Ok(Box::new(JadeSigner::connect().await?)),
        DeviceKind::AirGap => Err(SignerError::Unsupported {
            what: "air-gapped signing (export the PSBT instead)".into(),
        }),
    }
}

/// Run a blocking constructor without stalling the async executor.
async fn tokio_blocking(
    f: impl FnOnce() -> Result<TrezorSigner, SignerError> + Send + 'static,
) -> Result<Box<dyn Signer>, SignerError> {
    let signer = tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| SignerError::Transport(e.to_string()))??;
    Ok(Box::new(signer))
}

/// Connect for **signing**, supplying the wallet policy devices that need one require.
///
/// Ledger refuses to sign without a policy, and it must be set at construction. Every other
/// backend ignores the argument, so callers do not have to branch on device kind.
pub async fn connect_for_signing(
    kind: DeviceKind,
    policy: &str,
) -> Result<Box<dyn Signer>, SignerError> {
    match kind {
        DeviceKind::Ledger => {
            let policy = policy.to_owned();
            let signer =
                tokio::task::spawn_blocking(move || LedgerSigner::connect_with_policy(&policy))
                    .await
                    .map_err(|e| SignerError::Transport(e.to_string()))??;
            Ok(Box::new(signer))
        }
        other => connect(other).await,
    }
}

/// Connect to whatever is attached, preferring the first device found.
pub async fn connect_any() -> Result<Box<dyn Signer>, SignerError> {
    let devices = enumerate().await?;
    let first = devices.first().ok_or(SignerError::NoDevice)?;
    connect(first.kind).await
}
