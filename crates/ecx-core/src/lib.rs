//! ECX consensus facts and the transaction invariants.
//!
//! This crate is **pure**: no I/O, no async, no UI, no `tokio`. That is deliberate — everything
//! here is exhaustively testable, and it is the only place allowed to stamp replay protection
//! (`CLAUDE.md` Golden Rule 2).
//!
//! The two functions that matter:
//!
//! - [`finalize_ecx_psbt`] — the single chokepoint. Stamps `nLockTime` and sequences, asserts
//!   the §8 invariants, and returns an [`EcxPsbt`]. A PSBT that did not pass through here cannot
//!   reach a signer, because [`EcxPsbt`] is not constructible any other way.
//! - [`verify_signed`] — re-checks the finalized transaction bytes coming back from the device
//!   (Golden Rule 3). The device's output is untrusted.

use std::collections::{BTreeMap, BTreeSet};

use bitcoin::{Amount, OutPoint, Psbt, Sequence, Transaction, TxOut, absolute::LockTime};

// ---------------------------------------------------------------------------
// Consensus facts. Verified against ecash-com/bitcoin @ alphanet on 2026-08-20.
// ---------------------------------------------------------------------------

/// `consensus.EcashHeight` — `src/kernel/chainparams.cpp:82`. `2016 * 478`, a retarget boundary.
pub const ECASH_HEIGHT: u32 = 963_648;

/// The magic `nLockTime` that makes a transaction final on ECX and permanently non-final on
/// Bitcoin. `LOCKTIME_THRESHOLD - 1`; see `IsFinalTx`, `src/consensus/tx_verify.cpp:19`.
pub const ECX_MAGIC_LOCKTIME: u32 = 499_999_999;

/// Sequence stamped on every input. Anything is fine except [`Sequence::MAX`], which would make
/// `IsFinalTx` ignore the locktime entirely — and the transaction would replay onto Bitcoin.
pub const ECX_SEQUENCE: Sequence = Sequence(0xFFFF_FFFD);

/// Launch phases. Alpha and beta coins are destroyed and re-issued at full launch, so the phase
/// is a product-level fact, not trivia: a user who splits during alpha is not done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Before the fork block. ECX and Bitcoin are byte-identical and indistinguishable.
    PreFork,
    Alpha,
    Beta,
    Full,
}

pub const ALPHA_HEIGHT: u32 = 963_648;
pub const BETA_HEIGHT: u32 = 967_680;
pub const FULL_HEIGHT: u32 = 973_728;

impl Phase {
    pub fn at_height(height: u32) -> Self {
        match height {
            h if h >= FULL_HEIGHT => Phase::Full,
            h if h >= BETA_HEIGHT => Phase::Beta,
            h if h >= ALPHA_HEIGHT => Phase::Alpha,
            _ => Phase::PreFork,
        }
    }

    /// Alpha and beta coins are destroyed and re-issued at full launch. Never present coins from
    /// those phases as durable value.
    pub fn coins_are_durable(self) -> bool {
        matches!(self, Phase::Full)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A violated invariant. These are `Result`, never `panic!` — a failed invariant is a
/// user-facing abort, not a crash (`CLAUDE.md` §8).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvariantError {
    #[error("locktime is {found}, must be {ECX_MAGIC_LOCKTIME} for ECX replay protection")]
    WrongLockTime { found: u32 },

    #[error(
        "input {index} has sequence 0xFFFFFFFF, so nLockTime would be ignored \
         and this transaction would replay onto Bitcoin"
    )]
    FinalSequence { index: usize },

    #[error("input {outpoint} is not in the confirmed intent")]
    UnexpectedInput { outpoint: OutPoint },

    #[error("intended input {outpoint} is missing from the signed transaction")]
    MissingInput { outpoint: OutPoint },

    #[error("output {index} does not match the confirmed intent")]
    UnexpectedOutput { index: usize },

    #[error("signed transaction has {found} outputs, the confirmed intent has {expected}")]
    OutputCountMismatch { found: usize, expected: usize },

    #[error(
        "input {index} has no non_witness_utxo; Trezor requires the full previous transaction \
         for every non-taproot input (CLAUDE.md §5.4)"
    )]
    MissingPrevTx { index: usize },

    #[error("input {index} has neither witness_utxo nor non_witness_utxo, so its value is unknown")]
    UnknownInputValue { index: usize },

    #[error("fee of {found} exceeds the cap of {cap}")]
    FeeTooHigh { found: Amount, cap: Amount },

    #[error("could not compute the fee: {0}")]
    Fee(String),

    #[error("transaction has no inputs")]
    NoInputs,
}

// ---------------------------------------------------------------------------
// Intent
// ---------------------------------------------------------------------------

/// Exactly what the user confirmed on the review screen. Derived from the [`EcxPsbt`] they were
/// shown, then used to re-verify the bytes the device returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxIntent {
    /// The UTXOs to be spent, with their values so the fee is determined.
    pub inputs: BTreeMap<OutPoint, TxOut>,
    /// Expected outputs, in order: destination and change, and nothing else.
    pub outputs: Vec<TxOut>,
    pub fee: Amount,
}

// ---------------------------------------------------------------------------
// The chokepoint
// ---------------------------------------------------------------------------

/// A PSBT that has passed [`finalize_ecx_psbt`]. Not constructible any other way — that is the
/// point. Signers accept only this type, so an unstamped PSBT cannot reach a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcxPsbt(Psbt);

impl EcxPsbt {
    pub fn psbt(&self) -> &Psbt {
        &self.0
    }

    /// Mutable access for signers, which fill in signatures in place.
    pub fn psbt_mut(&mut self) -> &mut Psbt {
        &mut self.0
    }

    pub fn into_inner(self) -> Psbt {
        self.0
    }

    /// The intent to show the user and later verify the signed bytes against.
    pub fn intent(&self) -> Result<TxIntent, InvariantError> {
        let mut inputs = BTreeMap::new();
        for (i, txin) in self.0.unsigned_tx.input.iter().enumerate() {
            let txout =
                input_txout(&self.0, i).ok_or(InvariantError::UnknownInputValue { index: i })?;
            inputs.insert(txin.previous_output, txout);
        }
        let fee = self
            .0
            .fee()
            .map_err(|e| InvariantError::Fee(e.to_string()))?;
        Ok(TxIntent {
            inputs,
            outputs: self.0.unsigned_tx.output.clone(),
            fee,
        })
    }
}

/// **The only place replay protection is stamped.** Sets `nLockTime` and every input's sequence,
/// then asserts the `CLAUDE.md` §8 invariants.
pub fn finalize_ecx_psbt(mut psbt: Psbt, max_fee: Amount) -> Result<EcxPsbt, InvariantError> {
    if psbt.unsigned_tx.input.is_empty() {
        return Err(InvariantError::NoInputs);
    }

    // §8.1 and §8.2 — stamped here and nowhere else.
    psbt.unsigned_tx.lock_time = LockTime::from_consensus(ECX_MAGIC_LOCKTIME);
    for txin in psbt.unsigned_tx.input.iter_mut() {
        txin.sequence = ECX_SEQUENCE;
    }

    // §8.5 — Trezor needs the full previous transaction for every non-taproot input.
    for (i, input) in psbt.inputs.iter().enumerate() {
        if !input_is_taproot(input) && input.non_witness_utxo.is_none() {
            return Err(InvariantError::MissingPrevTx { index: i });
        }
    }

    // §8.6 — an absurd fee is a bug, not a user preference.
    let fee = psbt.fee().map_err(|e| InvariantError::Fee(e.to_string()))?;
    if fee > max_fee {
        return Err(InvariantError::FeeTooHigh {
            found: fee,
            cap: max_fee,
        });
    }

    // Re-assert what we just stamped, so the invariant list has exactly one implementation.
    assert_tx_invariants(&psbt.unsigned_tx)?;
    Ok(EcxPsbt(psbt))
}

/// Re-check the finalized transaction coming back from a device (Golden Rule 3).
///
/// Inputs and outputs must match the confirmed intent **exactly**, which also pins the fee — so
/// there is no separate fee check here.
pub fn verify_signed(tx: &Transaction, intent: &TxIntent) -> Result<(), InvariantError> {
    assert_tx_invariants(tx)?;

    // §8.3 — no injected, dropped, or substituted inputs.
    let mut seen = BTreeSet::new();
    for txin in &tx.input {
        if !intent.inputs.contains_key(&txin.previous_output) {
            return Err(InvariantError::UnexpectedInput {
                outpoint: txin.previous_output,
            });
        }
        seen.insert(txin.previous_output);
    }
    for outpoint in intent.inputs.keys() {
        if !seen.contains(outpoint) {
            return Err(InvariantError::MissingInput {
                outpoint: *outpoint,
            });
        }
    }

    // §8.4 — no unexpected outputs, ever.
    if tx.output.len() != intent.outputs.len() {
        return Err(InvariantError::OutputCountMismatch {
            found: tx.output.len(),
            expected: intent.outputs.len(),
        });
    }
    for (i, (got, want)) in tx.output.iter().zip(&intent.outputs).enumerate() {
        if got != want {
            return Err(InvariantError::UnexpectedOutput { index: i });
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Last check before publishing: the locktime and sequences are still what make this an eCash
/// transaction rather than a Bitcoin one.
///
/// [`verify_signed`] already covers this, and more. This exists for the broadcast path, which may
/// be handed a transaction from somewhere else — a hex pasted back from an air-gapped device, or
/// one saved from an earlier run — where there is no intent to compare against. Cheap, and the
/// last chance to catch a transaction that lost its replay protection in transit.
pub fn assert_broadcastable(tx: &Transaction) -> Result<(), InvariantError> {
    assert_tx_invariants(tx)
}

/// §8.1 and §8.2. Applied to both the unsigned and the signed transaction.
fn assert_tx_invariants(tx: &Transaction) -> Result<(), InvariantError> {
    let found = tx.lock_time.to_consensus_u32();
    if found != ECX_MAGIC_LOCKTIME {
        return Err(InvariantError::WrongLockTime { found });
    }
    if tx.input.is_empty() {
        return Err(InvariantError::NoInputs);
    }
    for (index, txin) in tx.input.iter().enumerate() {
        if txin.sequence == Sequence::MAX {
            return Err(InvariantError::FinalSequence { index });
        }
    }
    Ok(())
}

/// Taproot inputs are exempt from the previous-transaction requirement.
fn input_is_taproot(input: &bitcoin::psbt::Input) -> bool {
    input.tap_internal_key.is_some()
        || input
            .witness_utxo
            .as_ref()
            .is_some_and(|o| o.script_pubkey.is_p2tr())
}

/// The `TxOut` being spent by input `index`, from either PSBT field.
fn input_txout(psbt: &Psbt, index: usize) -> Option<TxOut> {
    let input = psbt.inputs.get(index)?;
    if let Some(txout) = &input.witness_utxo {
        return Some(txout.clone());
    }
    let prev = input.non_witness_utxo.as_ref()?;
    let outpoint = psbt.unsigned_tx.input.get(index)?.previous_output;
    prev.output.get(outpoint.vout as usize).cloned()
}

#[cfg(test)]
mod tests;
