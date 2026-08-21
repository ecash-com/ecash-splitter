# Support status

What works today, what does not, and what each missing piece needs. Written 2026-08-21; check
`git log` before trusting it. Section references are to [`CLAUDE.md`](../CLAUDE.md).

Key: ✅ works · 🟡 implemented, untested on hardware · ⛔ deliberately excluded · ❌ not built

---

## At a glance

| | |
|---|---|
| **Frontends** | Desktop (GPUI) ✅ · CLI (`ecx`) ✅ — both share `ecx-split`, neither owns the flow |
| **Flow** | Connect → discover → select → destination → build PSBT → review ✅ · signing and broadcast ❌ |
| **Devices** | Ledger ✅ · Coldcard 🟡 · Specter 🟡 · Jade 🟡 · BitBox02 ❌ · Trezor ❌ |
| **Chain** | Esplora ✅ · Electrum ❌ · compact-filter SPV ⛔ |
| **Tests** | 37, all passing. 19 of them are the `ecx-core` invariant suite |

The app **stops before signing on purpose**: eCash activates at block 963,648 and until then the
chains are identical, no endpoint can pass the fork probe, and a signed transaction would have
nowhere valid to go.

---

## Hardware wallets

### Supported

| Device | Transport | Library | State |
|---|---|---|---|
| **Ledger** Nano S+ / X | USB HID | `async-hwi` | ✅ Verified against a real Nano X: connect, fingerprint, version, 12 account xpubs, full discovery |
| **Coldcard** | USB HID | `async-hwi` | 🟡 Implemented, never run against hardware |
| **Specter DIY** | Serial | `async-hwi` | 🟡 Implemented, never run against hardware |
| **Jade** | Serial | `async-hwi` | 🟡 Implemented, never run against hardware. Unlock relays to Blockstream's blind PIN oracle — see below |

`sign()` is wired for all four but **never called**, because the flow stops at review. It is
untested on every device including Ledger.

**Jade and the blind oracle.** Unlocking a Jade makes an HTTPS request to Blockstream's PIN
oracle. This is required, not incidental: Jade has no secure element, so the oracle is what makes
a short PIN resistant to physical extraction. The oracle is *blind* — it never learns the PIN or
any key — and the host relays an encrypted handshake rather than participating in it. Recorded as
an explicit carve-out to Golden Rule 8. `auth()` is a no-op on an already-unlocked device, so a
user who unlocked in the Blockstream app never triggers a request.

### Not supported

| Device | Why | What it needs | Rough effort |
|---|---|---|---|
| **Trezor** Model T / Safe 3 / Safe 5 | `trezor-client` is **blocking** over `rusb`, and every call is a recursive `TxRequest` ack loop rather than a one-shot | A dedicated thread with a channel pair, plus an adapter onto our async `Signer` trait. **No Blockbook — see below.** | ~half a day |
| **BitBox02** | Connecting is a pairing handshake, not a `connect()` | `PairingBitbox02::connect` → display `pairing_code()` → user confirms on device → `wait_confirm()`. A new UI step in both frontends | ~1–2 hours |
| **Trezor Model One** | ⛔ Deliberate. It cannot take a PIN or passphrase on-device, so supporting it would put both in this app's memory, against Golden Rule 1. No SD card or camera either, so no air-gap fallback | Nothing — holders are directed to `../ecash-electrum` | n/a |
| **Air-gapped** (SeedSigner, Passport, Krux, Keystone, Coldcard Q, Jade via QR) | Not built | See below | ~1 hour (file) / days (QR) |

Trezor has the largest install base after Ledger, so it is the highest-value next device.

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
support is done. What remains is purely transport plumbing.

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
| 3. Discover accounts | ✅ 12 candidates — 4 script types × 3 account indices, gap limit 20 |
| 4. Select account | ✅ |
| 5. Destination | ✅ Pasted (default, behind a typed acknowledgement) or device-derived at `m/84'/0'/1'` |
| 6. Build + review | ✅ Sweep PSBT, full breakdown, unsigned PSBT shown and copyable |
| 7. Sign | ❌ **Deliberately disabled** until the fork activates |
| 8. Verify signed bytes | 🟡 `verify_signed` is implemented and covered by 19 tests, but **nothing calls it yet** — it is wired in at `ecx_split::sign_and_broadcast`, which is `todo!()` |
| 9. Broadcast | ❌ No UI. `ChainSource::broadcast` exists and requires a `BroadcastPermit` |
| 10. Wait for depth | ❌ Not built. `MIN_CONFIRMATIONS = 30` is a placeholder pending real alpha block times |
| 11. Hand off descriptor | ❌ Not built |

### Known gaps inside working steps

- **`BITCOIN_HASH_AT_FORK` is `None`.** The fork probe cannot run until Bitcoin mines block
  963,648 — below that height the two chains are byte-identical and no probe can distinguish
  them. Fill it in from a trusted Bitcoin source once the height is reached.
- **`display_address` is unimplemented on every device.** Ledger needs a registered wallet policy
  (`register_wallet` + persisted HMAC) for anything but BIP86. Until then a device-derived
  destination cannot be confirmed on the device screen, which is why pasting is the default
  (§7.5).
- **Fee rate is fixed at 1 sat/vB**, with a 200,000 sat absolute cap. No user override.
- **Passphrase wallets** are not surfaced. A passphrase yields a different fingerprint and a
  completely different account set; the app neither asks about one nor supports rescanning per
  passphrase (§5.6).

---

## The air-gapped path

Not built. The shape:

1. App builds the unsigned PSBT — **already works**, shown and copyable on the review screen
2. **Export** — `.psbt` file, or an animated QR
3. Device signs, fully offline
4. **Import** — file back, or scan the device's QR
5. `verify_signed` → broadcast

Two transports with very different costs:

- **File** (~1 hour) — covers Coldcard SD mode and Passport. No help for Jade or SeedSigner,
  which have no card slot.
- **QR** (days) — animated QR encoding, camera capture, and `ur:psbt` decoding. Covers
  *everything*: Jade, SeedSigner, Passport, Krux, Keystone, Coldcard Q. No vendor library at all.

---

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

1. **Trezor** — largest remaining install base, and the data half is already done
2. **Air-gap by file** — cheapest broad win, and it is the same export the QR path will need
3. **Sign → verify → broadcast** — blocked on the fork anyway, but `verify_signed` sitting
   uncalled is the biggest gap between what is tested and what runs
4. **BitBox02** — small, self-contained
5. **`display_address`** via Ledger wallet policies — makes device-derived destinations
   verifiable, at which point the §7.5 default should be revisited
6. **Linux and Windows** — required before anyone but us can run this
