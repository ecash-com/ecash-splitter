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
