# Device matrix

Per-device, per-firmware results for the **CONFIRM** items in `CLAUDE.md` §5.4.
**No release without every row in the v1 set filled in from real hardware.** Nothing here may be
inferred from documentation — the whole point is that vendor docs do not cover a chain that does
not exist to them.

Status key: `✅ pass` · `⚠️ works with caveat` · `❌ blocked` · `— untested`

## What to test

For each device, on **mainnet / the Bitcoin app** (never testnet — ECX is coin type `0'`, §3),
build a sweep through `finalize_ecx_psbt` and record:

| # | Check | Why it matters |
|---|---|---|
| 1 | Device enumerates and reports the expected **master fingerprint** | Descriptor↔device mismatch must be a hard error (§5.4) |
| 2 | Account xpub reads at `m/84'/0'/0'` | Coin type is `0'`; the real BTC account is where the coins are |
| 3 | Device **accepts `nLockTime = 499999999`** and signs | If a device refuses, that device cannot split. This is the load-bearing check |
| 4 | What the device **displays** about the locktime | Trezor is expected to show a locktime/blockheight warning — the only on-device ECX marker we get (§5, Golden Rule 5) |
| 5 | Signs with **all inputs at `nSequence = 0xFFFFFFFD`** | Sequence `0xFFFFFFFF` makes locktime ignored → **replays onto Bitcoin** (§8.2) |
| 6 | Requires `non_witness_utxo` on segwit inputs? | Trezor ≥1.9.0/2.3.0 does; drives the "never `only_witness_utxo()`" rule (§5.4) |
| 7 | Signs a **default wallet policy** (`wpkh(@0/**)`) without registration | Ledger-specific; anything non-default needs `register_wallet` + HMAC persistence |
| 8 | Change output to a **second account** (`m/84'/0'/1'`) shown as change or as external? | The ECX destination is a fresh account (§7.2); an external-looking change output is a UX problem, not a safety one |
| 9 | `verify_signed` passes on the returned bytes | Golden Rule 3 — re-verified, not trusted |
| 10 | Reported fingerprint matches the descriptor's | A mismatch is a hard error, never a warning (§5.4) |
| 11 | **PIN is entered on-device** (not via a host prompt) | Golden Rule 1. A device that demands host entry is unsupported — this is why Trezor Model One is out (§5.5) |
| 12 | **Does unlock require network access?** | Jade relays to Blockstream's blind oracle — required by its security model, and a documented Golden Rule 8 carve-out. Verify it works behind a proxy / on a restricted network |
| 13 | **Passphrase is entered on-device** | Same rule. Verify `ApplySettings` on-device-only can be honoured; host-cleartext `PassphraseAck` is a fail |
| 14 | Time for 12 sequential `get_extended_pubkey` calls, no display | Sets the discovery progress UX (§5.6). Confirm no button press is required per xpub |

Then broadcast to ECX and confirm the transaction is **accepted as final by ECX `IsFinalTx`** and
**rejected as non-final by Bitcoin Core** (§11). Untested until both halves are observed.

## Results

### USB path

| Device | Library | Firmware | 1 fp | 2 xpub | 3 locktime | 4 display | 5 seq | 6 prevtx | 7 policy | 8 change | 9 verify | Tested |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Ledger Nano S+ | `async-hwi` | — | — | — | — | — | — | — | — | — | — | — |
| Ledger Nano X | `async-hwi` | — | — | — | — | — | — | — | — | — | — | — |
| Trezor Model T | `trezor-client` | — | — | — | — | — | — | — | n/a | — | — | — |
| Trezor Safe 3/5 | `trezor-client` | — | — | — | — | — | — | — | n/a | — | — | — |
| BitBox02 | `async-hwi` | — | — | — | — | — | — | — | — | — | — | — |
| Coldcard (USB) | `async-hwi` | — | — | — | — | — | — | — | — | — | — | — |
| Jade | `async-hwi` | — | — | — | — | — | — | — | — | — | — | — |

### Unlock & session (checks 11–14)

| Device | 11 PIN entry | 12 offline unlock | 13 passphrase entry | 14 12× xpub | Notes |
|---|---|---|---|---|---|
| Ledger Nano S+/X | on-device | — | on-device | — | `display_xpub(false)` expected to avoid button presses |
| Trezor Model T / Safe 3 / Safe 5 | on-device | — | on-device | — | |
| BitBox02 | on-device | — | on-device | — | Pairing code confirmation on first connect |
| Coldcard (USB) | on-device | — | on-device | — | |
| **Jade** | on-device | **needs internet** | on-device | — | Unlock relays to the blind PIN oracle; `auth()` is a no-op if already unlocked |

### Air-gap path (PSBT file / QR)

| Device | Transport | Firmware | 3 locktime | 4 display | 5 seq | 6 prevtx | 9 verify | Tested |
|---|---|---|---|---|---|---|---|---|
| Coldcard | SD card `.psbt` | — | — | — | — | — | — | — |
| SeedSigner | animated QR | — | — | — | — | — | — | — |
| Krux | animated QR | — | — | — | — | — | — | — |
| Passport | SD / QR | — | — | — | — | — | — | — |
| Jade | QR | — | — | — | — | — | — | — |

## Excluded devices

**Trezor Model One — unsupported, decided 2026-08-20** (`CLAUDE.md` §5.5/§12). It cannot take a
PIN or a passphrase on-device, so supporting it means putting both into the app's memory, against
Golden Rule 1. It has no SD card or camera, so there is no air-gap fallback. Model One holders
are directed to `ecash-electrum`. If `trezor-client` returns `PinMatrixRequest`, that is an
unsupported-device error naming the model — do not build a matrix screen.

## Notes

Record one subsection per device as it is tested: exact firmware version, exact app version
(Ledger Bitcoin app version separately from the device firmware), the literal on-screen text for
check 4, and the txid on ECX. Screenshots of the locktime screen are worth keeping — they are the
evidence behind Golden Rule 5.

## Host platform

Separate from the devices, and also blocking for release (§5.4 "USB plumbing"):

- **Linux** — udev rules per vendor, shipped with the package + a setup doc. Record which distros verified.
- **macOS** — whether hardened runtime / notarization needs the USB device entitlement, and what `cargo-packager` actually emits (`CLAUDE.md` §4/§11). **CONFIRM.**
- **Windows** — WinUSB/HID driver behaviour per device.
