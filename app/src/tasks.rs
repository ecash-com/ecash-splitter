//! Background work: device I/O and chain I/O, off the render thread.
//!
//! Everything here runs on a Tokio runtime on its own threads. The UI learns about it only
//! through [`Progress`] messages — it never awaits a device (`CLAUDE.md` §10).

use std::sync::OnceLock;

use ecx_chain::{ChainProfile, EsploraChain, ScanReadiness, TipInfo};
use ecx_split::SplitEvent;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use crate::state::{DeviceSession, DiscoveryPhase, Progress};

/// Shared Tokio runtime. GPUI has its own executor, so this exists purely to host the async
/// device and HTTP clients.
pub fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("tokio runtime"))
}

/// Read the chain tip, height and timestamp. Feeds both the header status and the sync gate.
pub async fn chain_tip(profile: ChainProfile) -> Result<(TipInfo, ScanReadiness), String> {
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    ecx_split::chain_status(&chain)
        .await
        .map_err(|e| e.to_string())
}

/// Connect and read identity. Every supported device takes its PIN on-device, so there is
/// nothing for the app to prompt for.
pub async fn connect() -> Result<DeviceSession, String> {
    let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
    let label = device_label(signer.kind());
    let id = ecx_split::identify(signer.as_ref(), label)
        .await
        .map_err(|e| e.to_string())?;
    Ok(DeviceSession {
        kind: id.kind,
        label: id.label,
        version: id.version,
        fingerprint: id.fingerprint,
    })
}

fn device_label(kind: ecx_signer::DeviceKind) -> String {
    match kind {
        ecx_signer::DeviceKind::Ledger => "Ledger",
        ecx_signer::DeviceKind::Coldcard => "Coldcard",
        ecx_signer::DeviceKind::Specter => "Specter",
        ecx_signer::DeviceKind::BitBox02 => "BitBox02",
        ecx_signer::DeviceKind::Jade => "Jade",
        ecx_signer::DeviceKind::Trezor => "Trezor",
        ecx_signer::DeviceKind::AirGap => "Air-gapped",
    }
    .to_string()
}

/// The full discovery run. The sequence lives in `ecx-split`; this only adapts its events into
/// the channel the UI listens on, which is what keeps the CLI and the GUI in step.
pub async fn run_discovery(
    profile: ChainProfile,
    depth: ecx_wallet::DiscoveryDepth,
    tx: UnboundedSender<Progress>,
) {
    if let Err(message) = discovery_inner(profile, depth, &tx).await {
        let _ = tx.send(Progress::Failed(message));
    }
}

async fn discovery_inner(
    profile: ChainProfile,
    depth: ecx_wallet::DiscoveryDepth,
    tx: &UnboundedSender<Progress>,
) -> Result<(), String> {
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
    let label = device_label(signer.kind());

    let tx_events = tx.clone();
    let (_, accounts) = ecx_split::discover(&chain, signer.as_ref(), label, depth, move |event| {
        let progress = match event {
            SplitEvent::Connected(id) => Progress::Connected(DeviceSession {
                kind: id.kind,
                label: id.label,
                version: id.version,
                fingerprint: id.fingerprint,
            }),
            SplitEvent::ReadingKeys { done, total, label } => Progress::Step {
                phase: DiscoveryPhase::ReadingKeys,
                scanned: done,
                total,
                label,
            },
            SplitEvent::Scanning { done, total, label } => Progress::Step {
                phase: DiscoveryPhase::Scanning,
                scanned: done,
                total,
                label,
            },
        };
        let _ = tx_events.send(progress);
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = tx.send(Progress::Done(accounts));
    Ok(())
}

/// Derive the ECX destination address from the device.
pub async fn derive_destination() -> Result<(bitcoin::Address, String), String> {
    let signer = ecx_signer::connect_any().await.map_err(|e| e.to_string())?;
    let destination = ecx_split::device_destination(signer.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    let path = ecx_wallet::build::destination_account_path();
    Ok((destination.address().clone(), format!("{path}/0/0")))
}

/// Build the sweep and summarize it for the review screen. Stops short of signing.
pub async fn build_sweep_summary(
    profile: ChainProfile,
    account: ecx_wallet::DiscoveredAccount,
    destination: bitcoin::Address,
    fingerprint: bitcoin::bip32::Fingerprint,
) -> Result<ecx_split::BuiltSweep, String> {
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    // The UI already required its own confirmation before getting here (§7.5).
    let destination = ecx_split::Destination::Pasted {
        address: destination,
        acknowledged: true,
    };
    // The *full* build: the PSBT and its intent are kept, not just the summary. verify_signed
    // compares against the intent derived from the PSBT the user was shown; re-deriving it later
    // from the device's own output would compare a transaction against itself.
    ecx_split::build_sweep_full(
        &chain,
        &account,
        &destination,
        fingerprint,
        ecx_wallet::build::DEFAULT_FEERATE_SAT_PER_VB,
    )
    .await
    .map_err(|e| e.to_string())
}

/// Sign the reviewed sweep on the device and re-verify what comes back.
///
/// Stops there deliberately. Broadcasting needs a `BroadcastPermit`, which no probe can mint
/// until the chains diverge — and signing pre-fork is harmless, because the transaction carries
/// a locktime Bitcoin will never accept.
pub async fn sign_reviewed(
    kind: ecx_signer::DeviceKind,
    policy: String,
    built: ecx_split::BuiltSweep,
) -> Result<(String, String), String> {
    // A fresh connection: Ledger's wallet policy can only be set at construction, and the
    // account is not known until discovery has run. Nothing else holds the device here — every
    // task in this module connects and drops within its own scope.
    let signer = ecx_signer::connect_for_signing(kind, &policy)
        .await
        .map_err(|e| e.to_string())?;

    let tx = ecx_split::sign_and_verify(signer.as_ref(), &built.psbt, &built.intent)
        .await
        .map_err(|e| e.to_string())?;

    Ok((
        tx.compute_txid().to_string(),
        bitcoin::consensus::encode::serialize_hex(&tx),
    ))
}

/// Ask whether this endpoint may be broadcast to. Runs the fork probe.
pub async fn broadcast_readiness(
    profile: ChainProfile,
) -> Result<ecx_split::BroadcastReadiness, String> {
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    ecx_split::broadcast_readiness(&chain)
        .await
        .map_err(|e| e.to_string())
}

/// Publish a signed transaction, then read back where it stands.
///
/// `ecx_split::broadcast` runs the fork probe again immediately before publishing, so the
/// readiness check the UI made earlier informs the button but does not authorise anything.
pub async fn publish(
    profile: ChainProfile,
    raw_hex: String,
) -> Result<(String, ecx_chain::TxState), String> {
    let tx: bitcoin::Transaction = bitcoin::consensus::encode::deserialize_hex(raw_hex.trim())
        .map_err(|e| format!("not a valid transaction: {e}"))?;
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    let txid = ecx_split::broadcast(&chain, &tx)
        .await
        .map_err(|e| e.to_string())?;
    let state = ecx_split::track(&chain, txid)
        .await
        .map_err(|e| e.to_string())?;
    Ok((txid.to_string(), state))
}

/// Re-read how deeply buried a broadcast transaction is.
pub async fn track(profile: ChainProfile, txid: String) -> Result<ecx_chain::TxState, String> {
    let txid: bitcoin::Txid = txid.parse().map_err(|_| "invalid txid".to_string())?;
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    ecx_split::track(&chain, txid)
        .await
        .map_err(|e| e.to_string())
}
