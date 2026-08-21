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
) -> Result<ecx_wallet::SweepSummary, String> {
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    // The UI already required its own confirmation before getting here (§7.5).
    let destination = ecx_split::Destination::Pasted {
        address: destination,
        acknowledged: true,
    };
    ecx_split::build_sweep(
        &chain,
        &account,
        &destination,
        fingerprint,
        ecx_wallet::build::DEFAULT_FEERATE_SAT_PER_VB,
    )
    .await
    .map_err(|e| e.to_string())
}
