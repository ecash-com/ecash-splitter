//! Background work: device I/O and chain I/O, off the render thread.
//!
//! Everything here runs on a Tokio runtime on its own threads. The UI learns about it only
//! through [`Progress`] messages — it never awaits a device (`CLAUDE.md` §10).

use std::sync::OnceLock;

use ecx_chain::{ChainProfile, ChainSource, EsploraChain, ScanReadiness, TipInfo, now_unix};
use ecx_signer::{LedgerSigner, Signer};
use ecx_wallet::{AccountCandidate, DEFAULT_ACCOUNTS_PROBED, candidates, discover};
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
    let tip = chain.tip().await.map_err(|e| e.to_string())?;
    Ok((tip, ScanReadiness::assess(tip, now_unix())))
}

/// Connect and read identity. Ledger takes its PIN on-device, so there is nothing to prompt for.
pub async fn connect() -> Result<DeviceSession, String> {
    let signer = LedgerSigner::connect().map_err(|e| e.to_string())?;
    let fingerprint = signer
        .master_fingerprint()
        .await
        .map_err(|e| e.to_string())?;
    let version = signer.version().await.unwrap_or_else(|_| "unknown".into());
    Ok(DeviceSession {
        kind: signer.kind(),
        label: "Ledger".into(),
        version,
        fingerprint,
    })
}

/// The full discovery run: connect, read twelve xpubs, scan each account.
///
/// Reconnects rather than holding a device handle across the UI boundary — the connection is
/// cheap and this keeps USB state out of the render layer entirely.
pub async fn run_discovery(profile: ChainProfile, tx: UnboundedSender<Progress>) {
    if let Err(message) = discovery_inner(profile, &tx).await {
        let _ = tx.send(Progress::Failed(message));
    }
}

async fn discovery_inner(
    profile: ChainProfile,
    tx: &UnboundedSender<Progress>,
) -> Result<(), String> {
    let signer = LedgerSigner::connect().map_err(|e| e.to_string())?;
    let fingerprint = signer
        .master_fingerprint()
        .await
        .map_err(|e| e.to_string())?;
    let version = signer.version().await.unwrap_or_else(|_| "unknown".into());

    let session = DeviceSession {
        kind: signer.kind(),
        label: "Ledger".into(),
        version,
        fingerprint,
    };
    let _ = tx.send(Progress::Connected(session));

    // Read every candidate xpub. `display_xpub(false)` means no button press per read (§5.6).
    let candidates: Vec<AccountCandidate> = candidates(DEFAULT_ACCOUNTS_PROBED);
    let total = candidates.len();
    let mut xpubs = Vec::with_capacity(total);
    for (scanned, candidate) in candidates.iter().enumerate() {
        let _ = tx.send(Progress::Step {
            phase: DiscoveryPhase::ReadingKeys,
            scanned,
            total,
            label: format!("{} {}", candidate.kind.label(), candidate.path),
        });
        let xpub = signer
            .extended_pubkey(&candidate.path)
            .await
            .map_err(|e| format!("reading {}: {e}", candidate.path))?;
        xpubs.push((candidate.clone(), xpub));
    }

    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    let tip = chain.tip().await.map_err(|e| e.to_string())?;
    let readiness = ScanReadiness::assess(tip, now_unix());

    let tx_progress = tx.clone();
    let accounts = discover(&chain, readiness, fingerprint, &xpubs, move |p| {
        let _ = tx_progress.send(Progress::Step {
            phase: DiscoveryPhase::Scanning,
            scanned: p.scanned,
            total: p.total,
            label: format!("{} {}", p.current.kind.label(), p.current.path),
        });
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = tx.send(Progress::Done(accounts));
    Ok(())
}

/// Derive the ECX destination address from the device.
///
/// This is public-key math done locally — the device is only asked for an xpub. It is **not**
/// the same as the user verifying the address on the device screen, which needs a registered
/// Ledger wallet policy (`CLAUDE.md` §5.3).
pub async fn derive_destination() -> Result<(bitcoin::Address, String), String> {
    let signer = LedgerSigner::connect().map_err(|e| e.to_string())?;
    let path = ecx_wallet::build::destination_account_path();
    let xpub = signer
        .extended_pubkey(&path)
        .await
        .map_err(|e| format!("reading {path}: {e}"))?;
    let address = ecx_wallet::device_destination(&xpub).map_err(|e| e.to_string())?;
    Ok((address, format!("{path}/0/0")))
}

/// Build the sweep and summarize it for the review screen. Stops short of signing.
pub async fn build_sweep_summary(
    profile: ChainProfile,
    account: ecx_wallet::DiscoveredAccount,
    destination: bitcoin::Address,
    fingerprint: bitcoin::bip32::Fingerprint,
) -> Result<ecx_wallet::SweepSummary, String> {
    let chain = EsploraChain::new(profile).map_err(|e| e.to_string())?;
    let tip = chain.tip().await.map_err(|e| e.to_string())?;
    let readiness = ScanReadiness::assess(tip, now_unix());

    let psbt = ecx_wallet::build_sweep(
        &chain,
        readiness,
        &account,
        &destination,
        ecx_wallet::build::DEFAULT_FEERATE_SAT_PER_VB,
    )
    .await
    .map_err(|e| e.to_string())?;

    ecx_wallet::summarize(&psbt, &destination, fingerprint).map_err(|e| e.to_string())
}
