//! Invariant tests.
//!
//! `CLAUDE.md` §11: "Round-trip a built PSBT through a mock signer and confirm `verify_signed`
//! catches every mutation — flipped sequence, swapped output, altered locktime, injected input.
//! These tests are the reason the app can be trusted."

use super::*;
use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    absolute::LockTime, hashes::Hash, transaction::Version,
};

const IN_VALUE: Amount = Amount::from_sat(100_000);
const OUT_VALUE: Amount = Amount::from_sat(99_000);
const FEE_CAP: Amount = Amount::from_sat(10_000);

fn txid(seed: u8) -> Txid {
    Txid::from_byte_array([seed; 32])
}

/// Raw P2WPKH scriptPubKey: OP_0 <20 bytes>.
fn p2wpkh(seed: u8) -> ScriptBuf {
    let mut v = vec![0x00, 0x14];
    v.extend_from_slice(&[seed; 20]);
    ScriptBuf::from_bytes(v)
}

/// Raw P2TR scriptPubKey: OP_1 <32 bytes>.
fn p2tr(seed: u8) -> ScriptBuf {
    let mut v = vec![0x51, 0x20];
    v.extend_from_slice(&[seed; 32]);
    ScriptBuf::from_bytes(v)
}

/// A previous transaction paying `IN_VALUE` to `spk` at vout 0.
fn prev_tx(spk: ScriptBuf) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: IN_VALUE,
            script_pubkey: spk,
        }],
    }
}

/// An unsigned PSBT spending one P2WPKH input, with `non_witness_utxo` populated.
fn psbt_with(script: ScriptBuf, taproot: bool) -> Psbt {
    let prev = prev_tx(script.clone());
    let unsigned = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO, // deliberately wrong; finalize must stamp it
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: prev.compute_txid(),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX, // deliberately final; finalize must fix it
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: OUT_VALUE,
            script_pubkey: p2wpkh(0xbb),
        }],
    };
    let mut psbt = Psbt::from_unsigned_tx(unsigned).expect("valid unsigned tx");
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: IN_VALUE,
        script_pubkey: script,
    });
    if !taproot {
        psbt.inputs[0].non_witness_utxo = Some(prev);
    }
    psbt
}

fn good_psbt() -> Psbt {
    psbt_with(p2wpkh(0xaa), false)
}

/// Stand-in for a hardware wallet: returns the finalized transaction unchanged.
fn mock_sign(psbt: &EcxPsbt) -> Transaction {
    psbt.psbt().unsigned_tx.clone()
}

// -------------------------------------------------------------------------
// finalize_ecx_psbt
// -------------------------------------------------------------------------

#[test]
fn finalize_stamps_locktime_and_sequences() {
    let ecx = finalize_ecx_psbt(good_psbt(), FEE_CAP).unwrap();
    let tx = &ecx.psbt().unsigned_tx;
    assert_eq!(tx.lock_time.to_consensus_u32(), ECX_MAGIC_LOCKTIME);
    assert!(tx.input.iter().all(|i| i.sequence == ECX_SEQUENCE));
    assert!(tx.input.iter().all(|i| i.sequence != Sequence::MAX));
}

#[test]
fn finalize_rejects_missing_prev_tx_for_non_taproot() {
    let mut psbt = good_psbt();
    psbt.inputs[0].non_witness_utxo = None;
    assert_eq!(
        finalize_ecx_psbt(psbt, FEE_CAP),
        Err(InvariantError::MissingPrevTx { index: 0 })
    );
}

#[test]
fn finalize_allows_taproot_without_prev_tx() {
    // Taproot inputs are exempt: Trezor verifies their amounts from the witness UTXO.
    let psbt = psbt_with(p2tr(0xcc), true);
    assert!(psbt.inputs[0].non_witness_utxo.is_none());
    assert!(finalize_ecx_psbt(psbt, FEE_CAP).is_ok());
}

#[test]
fn finalize_rejects_absurd_fee() {
    let cap = Amount::from_sat(500);
    match finalize_ecx_psbt(good_psbt(), cap) {
        Err(InvariantError::FeeTooHigh { found, cap: c }) => {
            assert_eq!(found, IN_VALUE - OUT_VALUE);
            assert_eq!(c, cap);
        }
        other => panic!("expected FeeTooHigh, got {other:?}"),
    }
}

#[test]
fn finalize_rejects_empty_inputs() {
    let unsigned = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: OUT_VALUE,
            script_pubkey: p2wpkh(0xbb),
        }],
    };
    let psbt = Psbt::from_unsigned_tx(unsigned).unwrap();
    assert_eq!(
        finalize_ecx_psbt(psbt, FEE_CAP),
        Err(InvariantError::NoInputs)
    );
}

#[test]
fn intent_captures_inputs_outputs_and_fee() {
    let ecx = finalize_ecx_psbt(good_psbt(), FEE_CAP).unwrap();
    let intent = ecx.intent().unwrap();
    assert_eq!(intent.inputs.len(), 1);
    assert_eq!(intent.outputs.len(), 1);
    assert_eq!(intent.fee, IN_VALUE - OUT_VALUE);
}

// -------------------------------------------------------------------------
// verify_signed — the happy path, then every mutation it must catch
// -------------------------------------------------------------------------

#[test]
fn verify_accepts_an_untampered_round_trip() {
    let ecx = finalize_ecx_psbt(good_psbt(), FEE_CAP).unwrap();
    let intent = ecx.intent().unwrap();
    assert_eq!(verify_signed(&mock_sign(&ecx), &intent), Ok(()));
}

/// Finalize, take the intent, then hand the transaction to `tamper` and expect rejection.
fn assert_caught(tamper: impl FnOnce(&mut Transaction), expected: InvariantError) {
    let ecx = finalize_ecx_psbt(good_psbt(), FEE_CAP).unwrap();
    let intent = ecx.intent().unwrap();
    let mut tx = mock_sign(&ecx);
    tamper(&mut tx);
    assert_eq!(verify_signed(&tx, &intent), Err(expected));
}

#[test]
fn verify_catches_altered_locktime() {
    assert_caught(
        |tx| tx.lock_time = LockTime::from_consensus(800_000),
        InvariantError::WrongLockTime { found: 800_000 },
    );
}

#[test]
fn verify_catches_zeroed_locktime() {
    assert_caught(
        |tx| tx.lock_time = LockTime::ZERO,
        InvariantError::WrongLockTime { found: 0 },
    );
}

/// The most expensive possible bug: a final sequence makes `IsFinalTx` ignore the locktime,
/// and the transaction replays onto Bitcoin.
#[test]
fn verify_catches_final_sequence() {
    assert_caught(
        |tx| tx.input[0].sequence = Sequence::MAX,
        InvariantError::FinalSequence { index: 0 },
    );
}

#[test]
fn verify_catches_injected_input() {
    let injected = OutPoint {
        txid: txid(0x99),
        vout: 7,
    };
    assert_caught(
        |tx| {
            tx.input.push(TxIn {
                previous_output: injected,
                script_sig: ScriptBuf::new(),
                sequence: ECX_SEQUENCE,
                witness: Witness::new(),
            })
        },
        InvariantError::UnexpectedInput { outpoint: injected },
    );
}

#[test]
fn verify_catches_dropped_input() {
    let ecx = finalize_ecx_psbt(good_psbt(), FEE_CAP).unwrap();
    let intent = ecx.intent().unwrap();
    let missing = *intent.inputs.keys().next().unwrap();
    let mut tx = mock_sign(&ecx);
    tx.input.clear();
    // No inputs at all is caught earlier, by the shared invariant check.
    assert_eq!(verify_signed(&tx, &intent), Err(InvariantError::NoInputs));

    // Substituting one input for another is caught as unexpected-then-missing.
    let mut tx = mock_sign(&ecx);
    tx.input[0].previous_output = OutPoint {
        txid: txid(0x99),
        vout: 0,
    };
    match verify_signed(&tx, &intent) {
        Err(InvariantError::UnexpectedInput { .. }) => {}
        other => panic!("expected UnexpectedInput, got {other:?}"),
    }
    assert!(intent.inputs.contains_key(&missing));
}

#[test]
fn verify_catches_swapped_output_script() {
    assert_caught(
        |tx| tx.output[0].script_pubkey = p2wpkh(0xee),
        InvariantError::UnexpectedOutput { index: 0 },
    );
}

#[test]
fn verify_catches_reduced_output_amount() {
    assert_caught(
        |tx| tx.output[0].value = Amount::from_sat(1),
        InvariantError::UnexpectedOutput { index: 0 },
    );
}

#[test]
fn verify_catches_extra_output() {
    assert_caught(
        |tx| {
            tx.output.push(TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: p2wpkh(0xff),
            })
        },
        InvariantError::OutputCountMismatch {
            found: 2,
            expected: 1,
        },
    );
}

#[test]
fn verify_catches_removed_output() {
    assert_caught(
        |tx| tx.output.clear(),
        InvariantError::OutputCountMismatch {
            found: 0,
            expected: 1,
        },
    );
}

// -------------------------------------------------------------------------
// Phases
// -------------------------------------------------------------------------

#[test]
fn broadcast_refuses_a_transaction_that_lost_its_replay_protection() {
    // The broadcast path may be handed a transaction from elsewhere — a pasted hex, a saved
    // run — with no intent to compare against. It must still refuse to publish one that would
    // replay onto Bitcoin.
    let ecx = finalize_ecx_psbt(good_psbt(), FEE_CAP).unwrap();
    let good = mock_sign(&ecx);
    assert_eq!(assert_broadcastable(&good), Ok(()));

    let mut final_sequence = good.clone();
    final_sequence.input[0].sequence = Sequence::MAX;
    assert_eq!(
        assert_broadcastable(&final_sequence),
        Err(InvariantError::FinalSequence { index: 0 })
    );

    let mut wrong_locktime = good;
    wrong_locktime.lock_time = LockTime::from_consensus(800_000);
    assert_eq!(
        assert_broadcastable(&wrong_locktime),
        Err(InvariantError::WrongLockTime { found: 800_000 })
    );
}

#[test]
fn phase_boundaries() {
    assert_eq!(Phase::at_height(ECASH_HEIGHT - 1), Phase::PreFork);
    assert_eq!(Phase::at_height(ALPHA_HEIGHT), Phase::Alpha);
    assert_eq!(Phase::at_height(BETA_HEIGHT - 1), Phase::Alpha);
    assert_eq!(Phase::at_height(BETA_HEIGHT), Phase::Beta);
    assert_eq!(Phase::at_height(FULL_HEIGHT - 1), Phase::Beta);
    assert_eq!(Phase::at_height(FULL_HEIGHT), Phase::Full);
}

#[test]
fn only_full_launch_coins_are_durable() {
    // Alpha and beta coins are destroyed and re-issued at full launch.
    assert!(!Phase::PreFork.coins_are_durable());
    assert!(!Phase::Alpha.coins_are_durable());
    assert!(!Phase::Beta.coins_are_durable());
    assert!(Phase::Full.coins_are_durable());
}

#[test]
fn magic_locktime_is_below_the_threshold() {
    // Must be interpreted as a block height, not a timestamp, on both chains.
    assert_eq!(ECX_MAGIC_LOCKTIME, 500_000_000 - 1);
    assert!(LockTime::from_consensus(ECX_MAGIC_LOCKTIME).is_block_height());
}
