//! The split plan, orchestrating the other crates in the order `CLAUDE.md` §7 requires.
//!
//! Golden Rule 6: the ECX sweep happens first, always. Replay protection is one-directional, and
//! split ordering is what protects the other direction — it is not an optimization.

use bitcoin::{Address, Amount, Txid};
use ecx_wallet::DiscoveredAccount;

/// Where the swept coins go.
///
/// An ECX address *is* a Bitcoin address (`CLAUDE.md` §3) — nothing in the string identifies the
/// chain, so a pasted exchange deposit address is unrecoverable and undetectable. Device-derived
/// is the default for that reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// A fresh account on the connected device, verified on its screen via `display_address`.
    DeviceDerived { account: u32, address: Address },
    /// Typed or pasted. Requires a typed acknowledgement naming the chain (§7.5).
    Pasted {
        address: Address,
        acknowledged: bool,
    },
}

impl Destination {
    /// Golden Rule 7: never broadcast without explicit confirmation.
    pub fn is_confirmed(&self) -> bool {
        match self {
            Destination::DeviceDerived { .. } => true,
            Destination::Pasted { acknowledged, .. } => *acknowledged,
        }
    }
}

/// Where the user is in the §7 flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Connect,
    Discover,
    SelectAccount { found: Vec<DiscoveredAccount> },
    ChooseDestination { account: Box<DiscoveredAccount> },
    Review,
    Signing,
    Broadcasting,
    AwaitingDepth { txid: Txid, confirmations: u32 },
    Done { txid: Txid },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SplitError {
    #[error("the destination has not been confirmed by the user")]
    DestinationUnconfirmed,
    #[error("nothing to split: the selected account has no spendable UTXOs")]
    NothingToSplit,
    #[error("chain: {0}")]
    Chain(#[from] ecx_chain::ChainError),
    #[error("device: {0}")]
    Signer(#[from] ecx_signer::SignerError),
    #[error("invariant: {0}")]
    Invariant(#[from] ecx_core::InvariantError),
}

/// Post-fork difficulty resets to minimum, so reorg risk is elevated and six confirmations is
/// not enough. **CONFIRM against observed alpha block times before release.**
pub const MIN_CONFIRMATIONS: u32 = 30;

/// Build, sign, verify, and broadcast the sweep.
pub async fn execute_split(
    _account: &DiscoveredAccount,
    destination: &Destination,
    _max_fee: Amount,
) -> Result<Txid, SplitError> {
    if !destination.is_confirmed() {
        return Err(SplitError::DestinationUnconfirmed);
    }
    todo!("build via ecx-wallet, finalize via ecx-core, sign via ecx-signer, verify, broadcast")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_destinations_start_unconfirmed() {
        let addr: Address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"
            .parse::<Address<_>>()
            .unwrap()
            .assume_checked();
        assert!(
            !Destination::Pasted {
                address: addr.clone(),
                acknowledged: false
            }
            .is_confirmed()
        );
        assert!(
            Destination::Pasted {
                address: addr,
                acknowledged: true
            }
            .is_confirmed()
        );
    }
}
