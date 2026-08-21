//! UI state. Data only — no I/O, no device handles (`CLAUDE.md` Golden Rule 8).
//!
//! Note what is *not* here: a `LedgerSigner`. Device handles never cross into the UI. Each
//! operation reconnects inside its own task and hands back plain data, which keeps the render
//! layer free of anything that could block or fail on a USB cable.

use bitcoin::{Address, bip32::Fingerprint};
use ecx_chain::{ChainProfile, ProfileKind, ScanReadiness, TipInfo};
use ecx_signer::DeviceKind;
use ecx_split::BuiltSweep;
use ecx_wallet::DiscoveredAccount;

/// What we know about the chain source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStatus {
    Unknown,
    Checking,
    Up {
        tip: TipInfo,
        readiness: ScanReadiness,
    },
    Down {
        message: String,
    },
}

impl ChainStatus {
    pub fn tip(&self) -> Option<u32> {
        match self {
            ChainStatus::Up { tip, .. } => Some(tip.height),
            _ => None,
        }
    }

    pub fn readiness(&self) -> Option<ScanReadiness> {
        match self {
            ChainStatus::Up { readiness, .. } => Some(*readiness),
            _ => None,
        }
    }

    pub fn is_checking(&self) -> bool {
        matches!(self, ChainStatus::Unknown | ChainStatus::Checking)
    }

    /// Golden Rule 9 — may we state a balance at all?
    pub fn may_report_balance(&self) -> bool {
        matches!(self, ChainStatus::Up { readiness, .. } if readiness.may_report_balance())
    }
}

/// A connected device, reduced to what the UI needs to display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSession {
    pub kind: DeviceKind,
    pub label: String,
    pub version: String,
    pub fingerprint: Fingerprint,
}

/// Two-phase discovery: read keys from the device, then scan each account against the chain.
///
/// The user sees which phase they are in, because the two take very different amounts of time
/// and only the first one involves the device at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPhase {
    ReadingKeys,
    Scanning,
}

impl DiscoveryPhase {
    pub fn step(self) -> usize {
        match self {
            DiscoveryPhase::ReadingKeys => 1,
            DiscoveryPhase::Scanning => 2,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            DiscoveryPhase::ReadingKeys => "Reading account keys from the device",
            DiscoveryPhase::Scanning => "Scanning accounts against the chain",
        }
    }
}

/// Where the user is in the §7 flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    NeedsDevice,
    Connecting,
    Connected(DeviceSession),
    Discovering {
        session: DeviceSession,
        phase: DiscoveryPhase,
        scanned: usize,
        total: usize,
        current: String,
    },
    Accounts {
        session: DeviceSession,
        accounts: Vec<DiscoveredAccount>,
        selected: Option<usize>,
    },
    /// Step 5 of §7 — where the coins go.
    ChoosingDestination {
        session: DeviceSession,
        account: Box<DiscoveredAccount>,
        choice: DestinationChoice,
    },
    Building {
        session: DeviceSession,
        account: Box<DiscoveredAccount>,
    },
    /// Step 6 — the confirmation screen, which `CLAUDE.md` §10 calls "the product".
    Review {
        session: DeviceSession,
        account: Box<DiscoveredAccount>,
        built: Box<BuiltSweep>,
    },
    /// Step 7 — on the device. The user is looking at their Ledger, not at us.
    Signing {
        session: DeviceSession,
        account: Box<DiscoveredAccount>,
        built: Box<BuiltSweep>,
    },
    /// Step 8 — signed and **verified**, but deliberately not broadcast.
    Signed {
        session: DeviceSession,
        account: Box<DiscoveredAccount>,
        built: Box<BuiltSweep>,
        txid: String,
        raw_hex: String,
    },
}

/// Where the swept coins go.
///
/// An ECX address *is* a Bitcoin address (§3), so a pasted exchange deposit address is
/// unrecoverable and undetectable. Device-derived is the default for exactly that reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationChoice {
    /// A fresh account on the connected device, derived locally from its xpub.
    Device { address: Address, path: String },
    /// Typed or pasted. Requires a typed acknowledgement naming the chain (§7.5).
    Pasted {
        parsed: Option<Address>,
        acknowledged: bool,
    },
    /// The device xpub for the destination account has not been read yet.
    Pending,
}

impl DestinationChoice {
    pub fn address(&self) -> Option<&Address> {
        match self {
            DestinationChoice::Device { address, .. } => Some(address),
            DestinationChoice::Pasted {
                parsed,
                acknowledged,
            } => parsed.as_ref().filter(|_| *acknowledged),
            DestinationChoice::Pending => None,
        }
    }

    pub fn is_pasted(&self) -> bool {
        matches!(self, DestinationChoice::Pasted { .. })
    }
}

impl Stage {
    pub fn session(&self) -> Option<&DeviceSession> {
        match self {
            Stage::Connected(s)
            | Stage::Discovering { session: s, .. }
            | Stage::Accounts { session: s, .. }
            | Stage::ChoosingDestination { session: s, .. }
            | Stage::Building { session: s, .. }
            | Stage::Review { session: s, .. }
            | Stage::Signing { session: s, .. }
            | Stage::Signed { session: s, .. } => Some(s),
            _ => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Stage::Connecting | Stage::Discovering { .. } | Stage::Building { .. }
        )
    }
}

/// Progress messages from the discovery task.
#[derive(Debug, Clone)]
pub enum Progress {
    Connected(DeviceSession),
    Step {
        phase: DiscoveryPhase,
        scanned: usize,
        total: usize,
        label: String,
    },
    Done(Vec<DiscoveredAccount>),
    Failed(String),
}

/// Result of building the sweep.
#[derive(Debug, Clone)]
pub enum BuildOutcome {
    Ready(Box<BuiltSweep>),
    Failed(String),
}

/// Result of signing on the device and re-verifying what it returned.
#[derive(Debug, Clone)]
pub enum SignOutcome {
    /// Signed **and** verified against the reviewed intent. Not broadcast.
    Verified {
        txid: String,
        raw_hex: String,
    },
    Failed(String),
}

/// Result of deriving the device destination address.
#[derive(Debug, Clone)]
pub enum DestinationOutcome {
    Derived { address: Address, path: String },
    Failed(String),
}

/// What the profile banner should say, if anything.
pub fn profile_notice(profile: &ChainProfile) -> Option<&'static str> {
    match profile.kind {
        ProfileKind::Ecx | ProfileKind::Custom => None,
        ProfileKind::BitcoinReadOnly => Some(
            "Discovery only. This endpoint is Bitcoin, so the fork probe can never clear it \
             and broadcasting from here is impossible by construction.",
        ),
    }
}
