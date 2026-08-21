//! Air-gapped signing: the device never touches this process.
//!
//! **Export is cheap, import is the constrained half.** Our screen or a file reaches the device
//! for free; getting the signed result *back* is where the work is. Three return routes, only
//! one of which needs a camera:
//!
//! | Route | Devices | Camera |
//! |---|---|---|
//! | File — device writes a signed PSBT to SD | Coldcard, Passport, Krux | no |
//! | Paste — signed PSBT or raw transaction as text | anything | no |
//! | Scan — read the device's animated QR directly | SeedSigner, Keystone, Jade-QR | yes |
//!
//! This module implements the two that need no camera. Whichever route is used, the result must
//! come back here: skipping it means `ecx_core::verify_signed` never runs, and re-checking the
//! device's bytes is the last thing standing between a bug and someone's BTC (Golden Rule 3).

use std::path::Path;

use bitcoin::{
    Psbt, Transaction,
    consensus::{Decodable, encode::deserialize_hex},
};
use ecx_core::EcxPsbt;

use crate::{SignedTx, SignerError};

/// Serialize the unsigned PSBT for transport to an offline device.
///
/// Binary BIP174, which is what every device expects on an SD card. For QR the same bytes get
/// fountain-encoded as `ur:crypto-psbt`.
pub fn export_bytes(psbt: &EcxPsbt) -> Vec<u8> {
    psbt.psbt().serialize()
}

/// Base64 PSBT, for pasting into another tool or into a QR encoder.
pub fn export_base64(psbt: &EcxPsbt) -> String {
    psbt.psbt().to_string()
}

/// Write the unsigned PSBT to a file, typically on removable media.
pub fn export_to_file(psbt: &EcxPsbt, path: &Path) -> Result<(), SignerError> {
    std::fs::write(path, export_bytes(psbt))
        .map_err(|e| SignerError::AirGap(format!("could not write {}: {e}", path.display())))
}

/// Read whatever the device gave back.
///
/// Deliberately permissive about *shape*, because devices differ and users will paste whatever
/// their device produced: a binary PSBT, a base64 PSBT, or a hex raw transaction. It is not
/// permissive about *content* — every route lands on [`SignedTx`], which the caller must still
/// put through `verify_signed`.
pub fn import_bytes(data: &[u8]) -> Result<SignedTx, SignerError> {
    // Binary PSBT: magic "psbt\xff".
    if data.starts_with(b"psbt\xff") {
        let psbt = Psbt::deserialize(data)
            .map_err(|e| SignerError::AirGap(format!("not a valid PSBT: {e}")))?;
        return Ok(SignedTx::Psbt(Box::new(psbt)));
    }

    // Otherwise treat it as text and fall through to the string parser.
    let text = std::str::from_utf8(data)
        .map_err(|_| SignerError::AirGap("file is neither a PSBT nor text".into()))?;
    import_text(text)
}

/// Parse a pasted signed PSBT (base64) or raw transaction (hex).
pub fn import_text(text: &str) -> Result<SignedTx, SignerError> {
    let trimmed: String = text.split_whitespace().collect();
    if trimmed.is_empty() {
        return Err(SignerError::AirGap("nothing to import".into()));
    }

    // A base64 PSBT always starts with the magic bytes, which encode to "cHNidP".
    if trimmed.starts_with("cHNidP") {
        let psbt: Psbt = trimmed
            .parse()
            .map_err(|e| SignerError::AirGap(format!("not a valid PSBT: {e}")))?;
        return Ok(SignedTx::Psbt(Box::new(psbt)));
    }

    // Raw transaction as hex.
    if trimmed.chars().all(|c| c.is_ascii_hexdigit()) && trimmed.len() % 2 == 0 {
        let tx: Transaction = deserialize_hex(&trimmed)
            .map_err(|e| SignerError::AirGap(format!("not a valid transaction: {e}")))?;
        return Ok(SignedTx::Transaction(Box::new(tx)));
    }

    Err(SignerError::AirGap(
        "unrecognized — expected a base64 PSBT (starting \"cHNidP\") or a hex transaction".into(),
    ))
}

/// Read a signed PSBT or transaction from a file.
pub fn import_from_file(path: &Path) -> Result<SignedTx, SignerError> {
    let data = std::fs::read(path)
        .map_err(|e| SignerError::AirGap(format!("could not read {}: {e}", path.display())))?;
    import_bytes(&data)
}

/// Read a raw transaction from consensus bytes. Used by the QR path once it exists.
pub fn transaction_from_bytes(data: &[u8]) -> Result<Transaction, SignerError> {
    Transaction::consensus_decode(&mut &data[..])
        .map_err(|e| SignerError::AirGap(format!("not a valid transaction: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        Amount, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Txid, Witness, absolute::LockTime,
        hashes::Hash, transaction::Version,
    };

    /// A real PSBT, built rather than pasted, so the test cannot drift from what the library
    /// actually accepts.
    fn sample_psbt() -> Psbt {
        let prev = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf::from_bytes([vec![0x00, 0x14], vec![0xaa; 20]].concat()),
            }],
        };
        let unsigned = Transaction {
            version: Version::TWO,
            lock_time: LockTime::from_consensus(499_999_999),
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: prev.compute_txid(),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0xFFFF_FFFD),
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: ScriptBuf::from_bytes([vec![0x00, 0x14], vec![0xbb; 20]].concat()),
            }],
        };
        let _ = Txid::all_zeros();
        let mut psbt = Psbt::from_unsigned_tx(unsigned).expect("valid unsigned tx");
        psbt.inputs[0].non_witness_utxo = Some(prev);
        psbt
    }

    #[test]
    fn a_base64_psbt_is_recognized() {
        let text = sample_psbt().to_string();
        assert!(
            text.starts_with("cHNidP"),
            "base64 PSBTs start with the magic: {text:.10}"
        );
        match import_text(&text).unwrap() {
            SignedTx::Psbt(_) => {}
            other => panic!("expected a PSBT, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_and_newlines_are_tolerated() {
        // Pasted text routinely arrives wrapped across lines.
        let text = sample_psbt().to_string();
        let wrapped = format!("  {}\n{}\n ", &text[..30], &text[30..]);
        assert!(import_text(&wrapped).is_ok());
    }

    #[test]
    fn a_binary_psbt_is_recognized_by_its_magic() {
        let raw = sample_psbt().serialize();
        assert!(raw.starts_with(b"psbt\xff"));
        match import_bytes(&raw).unwrap() {
            SignedTx::Psbt(_) => {}
            other => panic!("expected a PSBT, got {other:?}"),
        }
    }

    #[test]
    fn a_hex_transaction_is_recognized() {
        let tx = sample_psbt().unsigned_tx;
        let hex = bitcoin::consensus::encode::serialize_hex(&tx);
        match import_text(&hex).unwrap() {
            SignedTx::Transaction(got) => assert_eq!(*got, tx),
            other => panic!("expected a transaction, got {other:?}"),
        }
    }

    #[test]
    fn export_round_trips_through_import() {
        let psbt = sample_psbt();
        let ecx = ecx_core::finalize_ecx_psbt(psbt.clone(), Amount::from_sat(10_000)).unwrap();
        match import_bytes(&export_bytes(&ecx)).unwrap() {
            SignedTx::Psbt(got) => assert_eq!(got.unsigned_tx, ecx.psbt().unsigned_tx),
            other => panic!("expected a PSBT, got {other:?}"),
        }
        assert!(import_text(&export_base64(&ecx)).is_ok());
    }

    #[test]
    fn junk_is_rejected_with_a_useful_message() {
        let err = import_text("hello world").unwrap_err().to_string();
        assert!(
            err.contains("cHNidP"),
            "message should say what was expected: {err}"
        );
        assert!(import_text("").is_err());
    }
}
