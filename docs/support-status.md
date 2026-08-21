# Support status

What works today, what does not, and what each missing piece needs. Written 2026-08-21, updated after signing was verified on hardware; check
`git log` before trusting it. Section references are to [`CLAUDE.md`](../CLAUDE.md).

Key: ✅ works · 🟡 implemented, untested on hardware · ⛔ deliberately excluded · ❌ not built

---

## At a glance

| | |
|---|---|
| **Frontends** | Desktop (GPUI) ✅ · CLI (`ecx`) ✅ — both share `ecx-split`, neither owns the flow |
| **Flow** | Connect → discover → select → destination → build → review ✅ · **sign + verify ✅ CLI, verified end to end on a Ledger** · ❌ GUI signing · broadcast ❌ |
| **Devices** | USB: Ledger ✅ · Coldcard 🟡 · Specter 🟡 · Jade 🟡 · Trezor 🟡 · BitBox02 ❌<br>Air-gap: **parked** — library exists, no UI |
| **Chain** | Esplora ✅ · Electrum ❌ · compact-filter SPV ⛔ |
| **Tests** | 52, all passing. 19 of them are the `ecx-core` invariant suite |
| **Frontend parity** | Both frontends stop at review; neither can sign |

The app **stops before signing on purpose**: eCash activates at block 963,648 and until then the
chains are identical, no endpoint can pass the fork probe, and a signed transaction would have
nowhere valid to go.

---

## Hardware wallets

### Supported

| Device | Transport | Library | State |
|---|---|---|---|
| **Ledger** Nano S+ / X | USB HID | `async-hwi` | ✅ **Verified end to end** on a Nano X (app 2.5.0): connect, fingerprint, 12 account xpubs, discovery, sweep construction, on-device signing, and verification |
| **Coldcard** | USB HID | `async-hwi` | 🟡 Implemented, never run against hardware |
| **Specter DIY** | Serial | `async-hwi` | 🟡 Implemented, never run against hardware |
| **Jade** | Serial | `async-hwi` | 🟡 Implemented, never run against hardware. Unlock relays to Blockstream's blind PIN oracle — see below |
| **Trezor** Model T / Safe 3 / Safe 5 | USB | `trezor-client` | 🟡 Implemented, never run against hardware. Runs on a dedicated thread — see below |

`sign()` is wired for all five. **Verified on Ledger** (see below); untested on Coldcard,
Specter, Jade and Trezor.

### Verified on hardware, 2026-08-21

A real sweep signed on a Nano X and decoded independently of the tool that produced it:

| Property | Value |
|---|---|
| `nLockTime` | **499999999** — trailing bytes `ff64cd1d` |
| Below `LOCKTIME_THRESHOLD` | yes, so Bitcoin reads it as a block height ~500M away and will never relay or mine it |
| `nSequence` | **`0xfffffffd`** — non-final, so the locktime is actually enforced |
| Outputs | 1, no change |
| Destination | decodes to the address passed on the command line |
| txid | recomputed from the stripped serialization, matches |

Both halves of the replay protection are present. The locktime alone would be inert with a final
sequence — that is the expensive bug the `ecx-core` suite exists to catch, and this transaction
has it right. The trailing `ff64cd1d` matches what `../ecash-electrum` independently predicts a
correct ECX transaction ends with.

**The Ledger wallet policy works.** `wpkh([fingerprint/84'/0'/2']xpub.../**)` with an empty name
was accepted as a *default* policy — no registration, no HMAC. That was the largest unknown, and
it is the same machinery `display_address` needs, so on-device address verification is now
reachable (§12).

**Jade and the blind oracle.** Unlocking a Jade makes an HTTPS request to Blockstream's PIN
oracle. This is required, not incidental: Jade has no secure element, so the oracle is what makes
a short PIN resistant to physical extraction. The oracle is *blind* — it never learns the PIN or
any key — and the host relays an encrypted handshake rather than participating in it. Recorded as
an explicit carve-out to Golden Rule 8. `auth()` is a no-op on an already-unlocked device, so a
user who unlocked in the Blockstream app never triggers a request.

### Not supported

| Device | Why | What it needs | Rough effort |
|---|---|---|---|
| **BitBox02** | Connecting is a pairing handshake, not a `connect()` | `PairingBitbox02::connect` → display `pairing_code()` → user confirms on device → `wait_confirm()`. A new UI step in both frontends | ~1–2 hours |
| **Trezor Model One** | ⛔ Deliberate. It cannot take a PIN or passphrase on-device, so supporting it would put both in this app's memory, against Golden Rule 1. No SD card or camera either, so no air-gap fallback | Nothing — holders are directed to `../ecash-electrum` | n/a |
| **Air-gapped by QR** (SeedSigner, Keystone, Jade-QR) | Not built | Animated QR display, camera capture, `ur` decode. See below | ~1 day |

### Trezor specifics

Two structural differences from the `async-hwi` devices, both handled:

- **Blocking transport.** `trezor-client` is synchronous over `rusb` and its handle is not usable
  from an async context, so it lives on a dedicated thread reached over channels. The thread is
  created on connect and exits when the last handle drops.
- **It returns a transaction, not a PSBT.** Every other device fills signatures into the PSBT.
  Trezor emits a *raw signature* rather than a scriptSig, and `trezor-client` deliberately dropped
  its `apply_signature` helper because putting one back into a PSBT needs pubkey and script
  inspection. So it streams the finished transaction in fragments, which we concatenate and
  deserialize. `Signer::sign` returns a `SignedTx` enum modelling both shapes rather than
  pretending they are the same; both converge at `verify_signed`.

Interaction requests are answered by policy, not convenience:

| Request | Response | Why |
|---|---|---|
| `ButtonRequest` | acknowledge | The user presses a button on the device |
| `PassphraseRequest` | `ack(on_device = true)` | The passphrase is typed on the Trezor and never enters this process (Golden Rule 1) |
| `PinMatrixRequest` | **refused** | Only Model One asks for host-side PIN entry, and Model One is unsupported |

Model One is filtered out at enumeration — it reports as `Model::TrezorLegacy`.

### Does Trezor need a Blockbook instance?

**No.** This comes up repeatedly, so, stated plainly:

Blockbook is **Trezor Suite's** backend, not the device's. The device has no network connection
and no opinion about where its data came from. Driving it ourselves over USB means no Blockbook
anywhere in this project.

The real requirement is different and easy to confuse with it: since firmware 1.9.0 / 2.3.0,
Trezor demands the **full previous transaction for every non-taproot input**, so it can verify
input amounts itself rather than trusting the host. Suite fetches those from Blockbook. We fetch
them from our own Esplora indexer and put them in the PSBT as `non_witness_utxo`.

**We already do this.** `build.rs` never calls `only_witness_utxo()`, `finalize_ecx_psbt` *rejects*
a PSBT whose non-taproot inputs lack a previous transaction (§8.5), and the review screen reports
the check on every build — it came back green on a real Ledger PSBT. So the data half of Trezor
support is done. That was the expensive half; the transport work is done too, as of this update.

---

## Chain endpoints

| | State |
|---|---|
| **Esplora** | ✅ Default and only backend. Tip, block hash at height, raw transactions, broadcast |
| **Custom endpoint** | ✅ Editable in the GUI header and via `--endpoint` in the CLI. URLs are normalized (trailing slash trimmed, `/api` appended) |
| **Electrum** | ❌ `bdk_electrum` is not wired up. `ChainSource` is a trait, so it is a constructor change plus an impl |
| **Compact-filter SPV** | ⛔ Dropped, see §6. Needs peers serving `getcfilters`, which Core has off by default |

**Verify any new endpoint with a real `full_scan`, not just `/blocks/tip/height`.** mempool.space
passes the tip check and then rejects the `/scripthash/{hash}/txs` endpoints BDK scans with,
which reads as "the endpoint works, the wallet is broken". That is why there is no Bitcoin
fallback preset.

---

## Flow

| Step (§7) | State |
|---|---|
| 1. Chain status + sync gate | ✅ Freshness-based; refuses to report a balance from a lagging indexer |
| 2. Connect device | ✅ Enumerates across all four backends |
| 3. Discover accounts | ✅ 4 script types × 3 account indices by default, gap limit 20. Depth adjustable in both frontends: `--accounts` / `--gap`, and preset buttons in the GUI |
| 4. Select account | ✅ |
| 5. Destination | ✅ Pasted (default, behind a typed acknowledgement) or device-derived at `m/84'/0'/1'` |
| 6. Build + review | ✅ Sweep PSBT, full breakdown, unsigned PSBT shown and copyable |
| 7. Sign | ✅ `ecx sign` does it end to end, verified on a Ledger. The GUI button stays **deliberately disabled** |
| 8. Verify signed bytes | ✅ Called by `ecx_split::sign_and_verify`, on both device shapes. `resolve_signed` finalizes a PSBT or takes Trezor's transaction as-is |
| 9. Broadcast | 🟡 `ecx_split::broadcast` implemented and requires a `BroadcastPermit`, which no probe can mint until the chains diverge. No UI |
| 10. Wait for depth | ❌ Not built. `MIN_CONFIRMATIONS = 30` is a placeholder pending real alpha block times |
| 11. Hand off descriptor | ❌ Not built |

### Known gaps inside working steps

- **A USB HID device can only be open once.** Signing needs a second connection, because Ledger's
  wallet policy can only be set at construction and the account is not known until discovery has
  run. The first connection must be dropped before the second is opened, or it fails with
  "device not found" — which reads as unplugged and actually means *already in use by us*. The
  GUI reconnects per operation and so has never hit this; anything holding a handle across steps
  will.

- **`BITCOIN_HASH_AT_FORK` is `None`.** The fork probe cannot run until Bitcoin mines block
  963,648 — below that height the two chains are byte-identical and no probe can distinguish
  them. Fill it in from a trusted Bitcoin source once the height is reached.
- **`display_address` is unimplemented on every device.** Ledger needs a registered wallet policy
  (`register_wallet` + persisted HMAC) for anything but BIP86. Until then a device-derived
  destination cannot be confirmed on the device screen, which is why pasting is the default
  (§7.5).
- **Fee rate is fixed at 1 sat/vB** in the GUI, with a 200,000 sat absolute cap. The CLI has
  `--feerate`.
- **A USB HID device can only be open once** — see the note above; signing needs a second
  connection and the first must be dropped.
- **Passphrase wallets** are not surfaced. A passphrase yields a different fingerprint and a
  completely different account set; the app neither asks about one nor supports rescanning per
  passphrase (§5.6).

---

## The air-gapped path — **parked (2026-08-21)**

> Not currently scoped. The library functions below exist and are tested, but nothing in either
> frontend reaches them, so this costs nothing at runtime and blocks nothing. Delete
> `ecx_signer::airgap`, `ecx_wallet::import`, and `ecx_split::discover_from_export` if it should
> go entirely — everything else they touch is shared with the USB path.

### Why it is bigger than it looks

**Discovery needs xpubs, and xpubs come from the device.** Over USB we ask for the twelve
candidates. Air-gapped there is nothing to ask, so the keys have to cross the gap first — before
any scan, and therefore before any PSBT can exist, since the amounts come from the scan. That
makes it three hops, not one:

```
1. device exports account xpubs   → import, build watch-only descriptors
2. scan chain, pick account + destination, build PSBT → export
   ── device signs, offline ──
3. import signed PSBT → verify → broadcast
```

Coverage is also narrower than USB: an export typically carries account 0 for each script type,
four of our twelve candidates. Anything not exported is invisible, so an empty result means
"nothing in what you exported", not "nothing in your wallet". Any UI must say that.

### What exists

**Export is cheap; import is the constrained half.** Our screen or a file reaches the device for
free. Getting the signed result *back* is where the cost is, and only one of the three routes
needs a camera:

| Route | Devices | Camera | State |
|---|---|---|---|
| **File** — device writes a signed PSBT to SD | Coldcard, Passport, Krux | no | 🟡 Implemented in `ecx_signer::airgap`, no UI yet |
| **Paste** — signed PSBT (base64) or raw transaction (hex) as text | anything | no | 🟡 Implemented, no UI yet |
| **Scan** — read the device's animated QR directly | SeedSigner, Keystone, Jade-QR | **yes** | ❌ Not built |

`import_text` and `import_bytes` accept a binary PSBT, a base64 PSBT, or a hex transaction, and
tolerate wrapped/whitespaced paste. Both land on `SignedTx`, and `ecx_split::verify_imported` is
the air-gap counterpart to `sign_and_verify` — it exists so the path cannot be composed without
the check. Both routes end at the same `ecx_core::verify_signed`.

**The return leg is not optional.** If the signed transaction never comes back to us,
`verify_signed` never runs and we never broadcast — and re-checking the device's bytes against
what the user confirmed is the last thing between a bug and their BTC (Golden Rule 3). A split
finished in another tool is a split with that check missing.

### SeedSigner specifically

SeedSigner has **no USB data path** — it is a Pi Zero where USB is power only, and everything
goes through its camera and screen. So it needs the QR round trip.

It is usable today without a camera, but awkwardly: we can display the PSBT for it to scan (once
QR export exists), and it displays the signed PSBT as animated QR, which you would need something
else — Sparrow with a webcam, a phone UR scanner — to turn into text you can paste back. That
works and keeps verification with us, but it is a hop.

### What the QR path needs

All crates exist and are maintained:

| Crate | Version | Role |
|---|---|---|
| `ur` | 0.5.2 | Fountain-encoded multi-part UR — the fiddly part, already solved |
| `qrcode` | 0.14.1 | Encode frames for display |
| `rqrr` | 0.10.1 | Detect and decode QR from captured frames |
| `nokhwa` | 0.10.11 | Cross-platform webcam capture |

Split by cost:

- **Export** (~2–3 hours, no new permissions) — encode as `ur:crypto-psbt`, animate frames on the
  review screen. Verify whether SeedSigner requires that exact type string or accepts `ur:bytes`;
  the `ur` crate's `bytewords` and `fountain` modules are public if the label needs setting by hand.
- **Import** (~1 day, **needs the camera**) — `NSCameraUsageDescription`, a hardened-runtime
  entitlement, and a macOS permission prompt. That is a real change to the app's capability
  profile and its notarization story, for a tool whose pitch is that it touches nothing it does
  not need to. Worth a deliberate decision.

Leverage is high: the same work covers SeedSigner, Keystone, Passport, Krux, Coldcard Q, and Jade
in QR mode — six devices, no vendor libraries.

## Platforms

| | State |
|---|---|
| **macOS** | ✅ Developed and run here. Needs the Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`, ~688 MB) for GPUI's shader build |
| **Linux** | ❌ Never built or run. Needs udev rules per vendor for HID and serial access |
| **Windows** | ❌ Never built or run. Needs WinUSB/HID driver verification per device |

Packaging (`cargo-packager` → `.dmg` / `.msi` / AppImage), code signing, and notarization are all
unstarted. See §11 — the signing burden is the real cost, not the code.

---

## Suggested order

1. **GUI parity** — the desktop app still stops at review, while the CLI signs and verifies
2. **The other four devices** — Coldcard, Specter, Jade and Trezor are implemented and have never
   touched hardware. Ledger proved the shape works; each of the others has its own quirks
3. **BitBox02** — small, self-contained; the last USB device
4. **`display_address`** via Ledger wallet policies — makes device-derived destinations
   verifiable, at which point the §7.5 default should be revisited
5. **Linux and Windows** — required before anyone but us can run this
6. **Air-gap by QR** — the big one, and the only thing that reaches SeedSigner and Keystone
