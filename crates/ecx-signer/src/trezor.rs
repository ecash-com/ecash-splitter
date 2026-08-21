//! Trezor Model T / Safe 3 / Safe 5, via `trezor-client`.
//!
//! Trezor is the awkward one, for two structural reasons (`CLAUDE.md` §5.5):
//!
//! 1. **`trezor-client` is blocking**, over `rusb`. Its handle is neither `Send` across await
//!    points nor usable from an async context, so it lives on a dedicated thread and we talk to
//!    it over channels. That thread is created on connect and torn down on drop.
//! 2. **Every call is an interaction state machine, not a one-shot.** A request returns a
//!    `TrezorResponse` that may be a `ButtonRequest`, `PinMatrixRequest`, or `PassphraseRequest`,
//!    each of which must be acknowledged before the real answer arrives.
//!
//! How we answer those acknowledgements is policy, not convenience:
//!
//! - `ButtonRequest` — acknowledged. The user presses a button on the device.
//! - `PassphraseRequest` — acknowledged with `on_device = true`, so the passphrase is typed on
//!   the Trezor and never enters this process (Golden Rule 1).
//! - `PinMatrixRequest` — **refused.** Only Model One asks for host-side PIN entry, and Model One
//!   is unsupported. Anything else asking for it is a device we do not understand.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use bitcoin::{
    Network, Psbt, Transaction,
    bip32::{DerivationPath, Fingerprint, Xpub},
    consensus::Decodable,
};
use ecx_core::EcxPsbt;
use trezor_client::{Model, Trezor, TrezorMessage, TrezorResponse};

use crate::{DeviceInfo, DeviceKind, SignedTx, Signer, SignerError};

/// Trezor speaks Bitcoin; ECX is byte-identical to it (§3), so the device signs an ordinary
/// Bitcoin transaction and never learns which chain it is for.
const NETWORK: Network = Network::Bitcoin;

/// Model One reports as `TrezorLegacy`. It cannot take a PIN or a passphrase on-device, so
/// supporting it would put both in this app's memory — see `CLAUDE.md` §5.5 and §12.
fn reject_legacy(model: Model) -> Result<(), SignerError> {
    if matches!(model, Model::TrezorLegacy) {
        return Err(SignerError::UnsupportedDevice {
            model: "Trezor Model One".into(),
        });
    }
    Ok(())
}

/// List connected Trezors, excluding Model One.
pub fn enumerate() -> Result<Vec<DeviceInfo>, SignerError> {
    Ok(trezor_client::find_devices(false)
        .into_iter()
        .filter(|d| !matches!(d.model, Model::TrezorLegacy))
        .map(|d| DeviceInfo {
            kind: DeviceKind::Trezor,
            label: format!("Trezor {}", d.model),
        })
        .collect())
}

/// Walk the interaction state machine to the actual answer.
///
/// Recursive because each acknowledgement can itself return another request — a passphrase
/// prompt followed by a button press, say.
fn resolve<T, R: TrezorMessage>(response: TrezorResponse<'_, T, R>) -> Result<T, SignerError> {
    match response {
        TrezorResponse::Ok(value) => Ok(value),
        TrezorResponse::Failure(failure) => {
            let message = failure.message().to_string();
            if message.to_lowercase().contains("cancel") {
                Err(SignerError::Declined)
            } else {
                Err(SignerError::Transport(message))
            }
        }
        TrezorResponse::ButtonRequest(req) => resolve(req.ack().map_err(map_trezor)?),
        // Only Model One asks for this, and Model One is unsupported.
        TrezorResponse::PinMatrixRequest(_) => Err(SignerError::HostPinRequested),
        TrezorResponse::PassphraseRequest(req) => {
            // `true` = enter it on the device. The passphrase never reaches this process.
            resolve(req.ack(true).map_err(map_trezor)?)
        }
    }
}

fn map_trezor(e: trezor_client::Error) -> SignerError {
    tracing::debug!(error = ?e, "trezor error");
    let flat = e
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let lower = flat.to_lowercase();
    if lower.contains("cancel") || lower.contains("declin") {
        SignerError::Declined
    } else if lower.contains("not initialized") || lower.contains("pin") {
        SignerError::NotReady
    } else {
        SignerError::Transport(flat)
    }
}

/// Work sent to the device thread. Each carries the channel its answer goes back on.
enum Command {
    Version(Sender<Result<String, SignerError>>),
    Fingerprint(Sender<Result<Fingerprint, SignerError>>),
    Xpub(DerivationPath, Sender<Result<Xpub, SignerError>>),
    Sign(Box<Psbt>, Sender<Result<Transaction, SignerError>>),
}

pub struct TrezorSigner {
    commands: Sender<Command>,
}

impl std::fmt::Debug for TrezorSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrezorSigner")
    }
}

impl TrezorSigner {
    /// Connect to the first non-legacy Trezor and hand it to a dedicated thread.
    pub fn connect() -> Result<Self, SignerError> {
        let device = trezor_client::find_devices(false)
            .into_iter()
            .find(|d| !matches!(d.model, Model::TrezorLegacy))
            .ok_or(SignerError::NoDevice)?;
        reject_legacy(device.model)?;

        let (commands, rx) = channel::<Command>();
        let (ready_tx, ready_rx) = channel::<Result<(), SignerError>>();

        // The Trezor handle never leaves this thread.
        thread::Builder::new()
            .name("trezor".into())
            .spawn(move || {
                let mut trezor = match device.connect().map_err(map_trezor) {
                    Ok(t) => t,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                if let Err(e) = trezor.init_device(None).map_err(map_trezor) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                serve(&mut trezor, rx);
            })
            .map_err(|e| SignerError::Transport(e.to_string()))?;

        // Surface a connect failure here rather than on the first call.
        ready_rx
            .recv()
            .map_err(|_| SignerError::Transport("trezor thread died during connect".into()))??;

        Ok(Self { commands })
    }

    /// Send a command and wait for its reply. Blocking, so callers wrap it in `spawn_blocking`.
    fn call<T: Send + 'static>(
        &self,
        make: impl FnOnce(Sender<Result<T, SignerError>>) -> Command,
    ) -> Result<T, SignerError> {
        let (tx, rx) = channel();
        self.commands
            .send(make(tx))
            .map_err(|_| SignerError::Transport("trezor thread is gone".into()))?;
        rx.recv()
            .map_err(|_| SignerError::Transport("trezor thread stopped responding".into()))?
    }
}

/// The device thread's loop. Exits when every `TrezorSigner` handle is dropped.
fn serve(trezor: &mut Trezor, rx: Receiver<Command>) {
    while let Ok(command) = rx.recv() {
        match command {
            Command::Version(reply) => {
                let result = trezor
                    .features()
                    .map(|f| {
                        format!(
                            "{}.{}.{}",
                            f.major_version(),
                            f.minor_version(),
                            f.patch_version()
                        )
                    })
                    .ok_or_else(|| SignerError::Transport("device reported no features".into()));
                let _ = reply.send(result);
            }
            Command::Fingerprint(reply) => {
                let _ = reply.send(fingerprint(trezor));
            }
            Command::Xpub(path, reply) => {
                let _ = reply.send(xpub(trezor, &path));
            }
            Command::Sign(psbt, reply) => {
                let _ = reply.send(sign_psbt(trezor, *psbt));
            }
        }
    }
}

/// The master fingerprint is the parent fingerprint of `m/0'`.
///
/// Trezor has no "give me the master fingerprint" call, so derive it the way every other wallet
/// does: ask for a child xpub and read the parent fingerprint out of it.
fn fingerprint(trezor: &mut Trezor) -> Result<Fingerprint, SignerError> {
    let path: DerivationPath = "m/0'".parse().expect("static path");
    let xpub = resolve(
        trezor
            .get_public_key(&path, NETWORK, false)
            .map_err(map_trezor)?,
    )?;
    Ok(xpub.parent_fingerprint)
}

fn xpub(trezor: &mut Trezor, path: &DerivationPath) -> Result<Xpub, SignerError> {
    // `show_display = false`: no button press per xpub, which matters for the twelve-account
    // discovery sweep (§5.6).
    resolve(
        trezor
            .get_public_key(path, NETWORK, false)
            .map_err(map_trezor)?,
    )
}

/// Stream the PSBT through the device's `TxRequest` loop and reassemble the signed transaction.
///
/// The device pulls what it needs — inputs, outputs, and the **full previous transaction** for
/// every non-taproot input, which it reads out of the PSBT's `non_witness_utxo` (§5.4). This is
/// why `build.rs` never calls `only_witness_utxo()`.
///
/// What comes back is not an updated PSBT. Trezor emits the finished transaction in fragments
/// via `get_serialized_tx_part`, which we concatenate and deserialize.
fn sign_psbt(trezor: &mut Trezor, psbt: Psbt) -> Result<Transaction, SignerError> {
    let mut raw: Vec<u8> = Vec::new();
    let mut progress = resolve(trezor.sign_tx(&psbt, NETWORK).map_err(map_trezor)?)?;

    loop {
        if let Some(part) = progress.get_serialized_tx_part() {
            raw.extend_from_slice(part);
        }
        if progress.finished() {
            break;
        }
        progress = resolve(progress.ack_psbt(&psbt, NETWORK).map_err(map_trezor)?)?;
    }

    if raw.is_empty() {
        return Err(SignerError::Transport(
            "device finished without returning a signed transaction".into(),
        ));
    }

    Transaction::consensus_decode(&mut raw.as_slice()).map_err(|e| {
        SignerError::Transport(format!("device returned an undecodable transaction: {e}"))
    })
}

#[async_trait::async_trait]
impl Signer for TrezorSigner {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Trezor
    }

    async fn version(&self) -> Result<String, SignerError> {
        self.call(Command::Version)
    }

    async fn master_fingerprint(&self) -> Result<Fingerprint, SignerError> {
        self.call(Command::Fingerprint)
    }

    async fn extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, SignerError> {
        let path = path.clone();
        self.call(move |reply| Command::Xpub(path, reply))
    }

    async fn display_address(&self, _path: &DerivationPath) -> Result<(), SignerError> {
        Err(SignerError::Unsupported {
            what: "on-device address display".into(),
        })
    }

    async fn sign(&self, psbt: &EcxPsbt) -> Result<SignedTx, SignerError> {
        let input = psbt.psbt().clone();
        let signed = self.call(move |reply| Command::Sign(Box::new(input), reply))?;
        Ok(SignedTx::Transaction(Box::new(signed)))
    }
}
