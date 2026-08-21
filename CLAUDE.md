# CLAUDE.md — eCash Splitter

> Project bible for Claude (and humans). Read this fully before writing or changing code.
> Keep it current: when an architectural decision changes, update this file in the same PR.
>
> **Product name:** eCash Splitter. **Code identifiers / crate prefix:** `ecx-`.
> Sibling projects in `../`: `ecash-electrum` (light-client wallet fork), `ecash-wallet-mobile`
> (Skip/BDK mobile wallet), `ecash-blockbook` (indexer notes). Read `../ecash-electrum/CLAUDE.md`
> before touching anything consensus- or replay-related — it is the verified source of truth for
> ECX chain facts and the split ordering, and this file deliberately does not duplicate all of it.

---

## 1. What this is

A **desktop** app whose single job is to let someone holding BTC on a **hardware wallet** claim
and separate their **ECX** ("eCash", the Layer Two Labs / Paul Sztorc Bitcoin hard fork) coins,
safely, without ever risking the BTC.

This is not a wallet. It does not hold keys, it does not do Lightning, it does not do BIP300
sidechains, and after the split it hands off to a real ECX wallet. It is a **one-task tool**:
build a replay-protected sweep, get a hardware device to sign it, broadcast it to ECX, verify it
confirmed, and get out of the way.

The entire product risk is one sentence: **a mistake here moves the user's real Bitcoin.**
Every design decision below is downstream of that.

**v1 scope**
- Connect a hardware device over USB (Ledger, Trezor, BitBox02, Coldcard, Jade) **or** work
  air-gapped via PSBT file / QR.
- Import the BTC account to split as a **watch-only descriptor** from the device's xpub.
- Scan an **ECX** chain source, show the UTXO set that is claimable on ECX.
- Pick/derive a **fresh ECX-dedicated destination account** (see §7).
- Build the sweep with `nLockTime = 499999999`, sign it on the device, **re-verify the signed
  bytes**, broadcast to an ECX endpoint only, track confirmation depth.
- Hand off: export the ECX destination descriptor for `ecash-electrum` / eCash.com Wallet.

**Explicitly out of scope for v1** — sending/receiving generally, multisig (design for it, don't
build it), Lightning, sidechain deposits, fiat, address book, mobile. See §12.

**Decisions already made (do not relitigate)**
- Rust workspace; **GPUI + `gpui-component`** for the UI, from **crates.io only** (decided
  2026-08-21 after measuring both options — see §4). No WebView anywhere in the product.
- **BDK** (`bdk_wallet`) owns descriptors, scanning, coin selection, PSBT construction.
- BDK does **not** talk to the hardware device. See §5 — this is the central architectural fact.
- Chain source is an **ECX Esplora/Electrum indexer**. BIP157/158 compact-filter SPV was
  considered and **dropped** — see §6 for the reasons so they don't get relitigated.

---

## 2. Golden rules (non-negotiable)

1. **No secret of any kind enters this app's memory.** Not a seed, not an xprv, not a PIN, not a
   passphrase. No "import a mnemonic" flow, not even as an advanced option; no PIN prompt; no
   passphrase prompt. The app is watch-only + PSBT, and every supported device takes its PIN and
   passphrase **on-device** (§5.5 — this is why Trezor Model One is unsupported). If a feature
   needs a secret from the user, the answer is no.
2. **One chokepoint stamps replay protection.** Exactly one function
   (`ecx_core::finalize_ecx_psbt`) sets `nLockTime = 499_999_999` and asserts the §8 invariants.
   No other code sets a locktime or a sequence. A PSBT that did not pass through it cannot reach
   a signer — enforce with a newtype (`EcxPsbt`), not a comment.
3. **Re-verify after signing.** The device returns bytes; treat them as untrusted. Before
   broadcast, deserialize the finalized transaction and re-check locktime, every input's
   sequence, every input outpoint against the intended set, and every output script against the
   intended set. Mismatch = hard abort, loud error, no broadcast. This is the last thing standing
   between a bug and the user's BTC.
4. **Broadcast only to a chain we proved is ECX.** Before any endpoint is usable, run the
   §6 fork-probe. Never fall back to a Bitcoin endpoint, never "retry elsewhere" on failure.
5. **The device screen cannot confirm the chain.** Ledger and Trezor will display *"Bitcoin"* and
   BTC amounts, because ECX addresses, xpubs, and serialization are byte-identical to Bitcoin
   (§3). The user cannot verify on-device that this is the ECX transaction. Therefore **our**
   confirmation screen carries the full weight: it must show the ECX badge, the fork phase, the
   locktime, the destination account path, and an explicit "this spends on eCash, not Bitcoin"
   statement, and require a typed confirmation for mainnet-value amounts.
6. **ECX first, always.** The sweep on the ECX side happens before any BTC-side movement.
   Replay protection is **one-directional** (§3) and split ordering is what protects the other
   direction. Never offer a "split on the BTC side first" flow.
7. **Fail loud, never silently.** No swallowed device errors, no auto-retry that re-prompts the
   device, no partial broadcast. Every failure surfaces the actual cause and the safe next step.
8. **The UI layer performs no I/O.** No network, no device, no filesystem beyond app config —
   all of it goes through the `ecx-*` crates behind channels, and the app ships no telemetry.
   **One carve-out, added 2026-08-21:** unlocking a Jade relays an encrypted handshake to
   Blockstream's blind PIN oracle. That is Jade's security model, not our choice — it has no
   secure element, so the oracle is what makes a short PIN resistant to physical extraction. The
   oracle never learns the PIN or any key, and the host is a relay rather than a participant.
   The rule forbids *the app* reaching out; it does not forbid a device protocol from doing so.
   With GPUI this is structural — there is no WebView and no HTML surface, so there is no CSP to
   get wrong and no remote-asset risk. **Do not undo it by embedding a webview** for a help page,
   a changelog, or anything else. See §4 and §10.
9. **Never report an empty result from a chain source that has not synced past the fork.** A
   partially-synced indexer is indistinguishable from an empty wallet, and "you have no coins" is
   the one wrong answer a user will act on immediately and irreversibly (they walk away). Below
   `EcashHeight`, the UI shows sync progress and refuses to state a balance at all. See §5.6.

---

## 3. ECX facts (verified against `ecash-com/bitcoin` @ `alphanet`, 2026-08-20)

Full treatment lives in `../ecash-electrum/CLAUDE.md`. The subset that binds this app:

| | |
|---|---|
| `consensus.EcashHeight` | **963648** (`src/kernel/chainparams.cpp:82`) — `2016 × 478`, a retarget boundary |
| Alpha / Beta / Full | 963,648 (2026-08-23) / 967,680 (2026-09-20) / **973,728 (2026-10-31)** |
| Magic locktime | **499999999** = `LOCKTIME_THRESHOLD - 1` |
| P2P magic / port | `0xeca5a104` / 8533 |
| Address & key encoding | **byte-identical to Bitcoin** — P2PKH `0x00`, P2SH `0x05`, WIF `128`, xpub `0x0488B21E`, bech32 HRP `bc`, Bitcoin's genesis hash |
| SLIP-44 coin type | none of its own → **`0'`**, the user's real Bitcoin account |

**Replay protection is a magic `nLockTime`, nothing more.** `IsFinalTx`
(`src/consensus/tx_verify.cpp:19` on `alphanet`) short-circuits:

```cpp
if (tx.nLockTime == 0 || tx.nLockTime == LOCKTIME_THRESHOLD - 1)
    return true;
```

Bitcoin Core reads `499999999` as a block height ~500M in the future and will neither relay nor
mine it. Three consequences that shape this whole app:

1. **Serialization and sighash are unchanged.** Any Bitcoin signer — every hardware wallet, every
   HSM, every library — can produce a valid replay-protected ECX transaction with no ECX support
   whatsoever. This is why the project is feasible at all.
2. **It only stops ECX→BTC replay.** A BTC transaction still replays onto ECX. Rule 6 (ECX
   first) is the mitigation, not an optimization.
3. **It requires a non-final input.** `IsFinalTx` ignores locktime when *every* input has
   `nSequence == 0xFFFFFFFF`. At least one input must differ; we set **all** inputs to
   `0xFFFFFFFD` and assert it (§8).

Two more, relevant but not to the transaction we build: difficulty resets to `bnPowLimit` at the
fork block (`src/pow.cpp:81-83`), and 220 reassigned Satoshi-era transactions are whitelisted in
`src/repo_txns.h`.

> **Re-verify the replay scheme per phase.** Earlier drynets used a magic tx *version*
> (`0x00BFBF3F`) instead of locktime. Before each phase, diff `tx_verify.cpp` on the launch branch
> and check https://drivechain.info/dev.txt. Bake the check into CI if possible.

**Phase gating is a product requirement, not a nicety.** Alpha and beta coins are destroyed and
re-issued at full launch (2026-10-31). The app must know the current phase, display it, and
refuse to present alpha/beta output as durable value. A user who splits during alpha and thinks
they are done has been actively misled.

---

## 4. Tech stack & versions

Everything below resolves to **`bitcoin 0.32`**. That is not a coincidence and it is the reason
this stack composes with no conversion glue — `bdk_wallet`, `async-hwi`, and `trezor-client` all
hand each other the same `bitcoin::psbt::Psbt`. **Protect this.** A dependency bump that splits
the `bitcoin` version is a breaking architectural change, not a routine update.

| Layer | Crate | Version | Notes |
|---|---|---|---|
| Language | Rust | edition 2024, **MSRV 1.85** | 1.85 is `async-hwi`'s floor with the `coldcard` feature |
| UI | `gpui` | **0.2.2** (crates.io) | Zed's GPU-accelerated framework. Native rendering, **no WebView** |
| UI components | `gpui-component` | **0.5.1** (crates.io) | 60+ components, shadcn-style defaults, Apache-2.0, Longbridge |
| Packaging | `cargo-packager` | **0.11.8** | `.dmg` / `.msi` / AppImage / deb, incl. macOS notarization + signing |
| Wallet engine | `bdk_wallet` | **3.1.0** | pulls `bitcoin ^0.32.8`, **`miniscript ^12.3.5`** |
| Chain (default) | `bdk_esplora` | **0.22.2** | |
| Chain (alt) | `bdk_electrum` | **0.24.0** | |
| Signer: Ledger/BitBox02/Coldcard/Jade/Specter | `async-hwi` | **0.0.32** | wizardsardine, powers Liana; actively maintained |
| Signer: Trezor | `trezor-client` | **0.1.6**, `features = ["bitcoin"]` | first-party, `trezor/trezor-firmware/rust/trezor-client` |
| Async | `tokio` | 1 | |

Pin exact versions in `Cargo.toml`. Commit `Cargo.lock`.

**Do not pull `miniscript` 13.** It is released, but `bdk_wallet 3.1.0` requires `^12.3.5`;
pulling 13 gives you two miniscript versions and type errors that look like nonsense.

**Do not use `hwi` / `rust-hwi`.** See §5.

### Frontend: GPUI + `gpui-component` (decided 2026-08-21)

**Known-good baseline, verified by building it** on 2026-08-21 with rustc 1.94.0:

```toml
gpui           = "0.2.2"   # crates.io — NOT a git dependency
gpui-component = "0.5.1"   # crates.io; pulls gpui_media 0.2.2
```

`cargo check` clean, zero errors, no git sources in the lockfile. **This is the configuration to
stay on.** If a future change makes the build require git dependencies on `zed-industries/zed`,
that is a significant architectural regression, not a routine update — see (b) below.

Chosen for the no-WebView property and for `gpui-component`'s coherent shadcn-style design
system, which is worth real effort on an app where *looking trustworthy is the product*. The
costs below are accepted, not overlooked. `app/` still sits on top of the `ecx-*` crates and
holds no consensus logic, no I/O, and no transaction construction (§9, Golden Rule 8) — so the
UI stays replaceable if this proves wrong. Build the `ecx-*` crates first regardless.

### Measured, not guessed (2026-08-21)

Resolved lockfiles, `cargo generate-lockfile`, crate counts on top of the same `ecx-*` base
(`bdk_wallet` + `bdk_esplora` + `async-hwi` + `trezor-client` + `tokio`):

| Configuration | crates | marginal |
|---|---|---|
| base only | 348 | — |
| base + `gpui-component` 0.5.1 + `gpui` 0.2.2 (crates.io) | **888** | **+540** |
| base + `dioxus` 0.7.10 desktop | **718** | **+370** |

**GPUI is the heavier of the two, by ~46% on marginal crate count.** It *feels* lighter — one
framework, no JS toolchain — but the tree underneath is bigger, and that is the lean
(crates.io-only) configuration. Record this because the intuition runs the other way and will
keep running the other way.

This matters more here than it usually would: §11's argument is that reproducible builds plus a
dependency tree a reviewer can actually read are the main defense against "this is malware." If
the threat model is a malicious dependency — which for an app rendering only local content it
should be — then 540 extra crates is a larger attack surface than the WebView GPUI removes.

**What we get**

- No WebView at all. For a Bitcoin app that is a real structural win — no HTML surface, no CSP to
  get wrong, no remote-asset risk. Golden Rule 8 stops being a discipline and becomes a property.
- `gpui-component` is 60+ components, Apache-2.0, extracted from Longbridge Pro (a shipped
  commercial desktop app), ~13.3k stars, pushed daily. Tag `v0.5.1` = `0f0ab3523321`.
- Two very different dependency shapes, and the difference is easy to trip over:
  - **(a) crates.io only.** `gpui-component = "0.5.1"` resolves `gpui 0.2.2` from crates.io.
    Semver, a lockfile, no git dependencies — a build §11's reproducibility requirement can stand
    behind. Cost: `gpui 0.2.2` was published 2025-10-22 and GPUI's living version is newer.
  - **(b) git-pinned.** `gpui-component`'s own workspace takes `gpui` by **unpinned git** to
    `zed-industries/zed`, plus a forked `reqwest`, tree-sitter, and LSP crates. Zed took ~450
    commits in the 30 days to 2026-08-20; no semver, no changelog across a bump.
  - **We are on (a), and staying.** Six screens do not need bleeding-edge GPUI. Move to (b) only
    for a concrete upstream fix we need, and then pin every dependency by explicit `rev`
    (mirroring `gpui-component`'s `Cargo.lock`, which pins `gpui` to `e0931d5a`) — never by
    branch, and each bump is a reviewed change, not a routine update.
- Cost: we hand-roll packaging. `cargo-packager` 0.11.8 covers `.dmg` / `.msi` / AppImage / deb
  including macOS notarization and signing; Zed's own `script/bundle-{mac,linux,windows.ps1}` is
  the prior art if it falls short.
- Cost: **GPUI needs the Metal Toolchain on macOS** — a separate ~688 MB Xcode component
  (`xcodebuild -downloadComponent MetalToolchain`), *not* part of Command Line Tools; check with
  `xcodebuild -showComponent MetalToolchain`. `gpui`'s build script compiles `shaders.metal` and
  hard-fails without it, before compiling a single Rust crate. Dev setup and CI must both
  provide and pin it, and §11's reproducibility story then includes a shader compiler. Dioxus
  needs no equivalent on macOS.

**What we gave up (Dioxus 0.7.10, the runner-up)**

- Stable semver on crates.io, small and contained audit surface, large ecosystem.
- `dx bundle` already produces the signed macOS / Windows / Linux packaging that §11 requires us
  to build regardless — a meaningful chunk of the release work done for us.
- Cost: it is a system WebView. "Load only bundled local assets, no CDN, no remote fonts, no
  telemetry, strict CSP" stays a rule we enforce rather than a guarantee the framework gives us.
- Cost: less comes free visually. `gpui-component` is a coherent design system out of the box;
  with Dioxus we write CSS, which means building a design system or adding Tailwind tooling.
  For six screens that need to look trustworthy, this is a real difference in effort.
- `0.8.0-alpha.1` exists — do not float.

**Why this closed the way it did.** The decisive risk was that `gpui-component` might only really
work against the git monorepo, which would have made §11's reproducibility promise very hard to
keep. Building it settled that: the crates.io-only configuration compiles clean. With that gone,
the remaining GPUI costs are all *known and bounded* — a bigger dependency tree, a macOS
toolchain component, and packaging we write ourselves — while its benefits (no WebView at all, a
design system we do not have to build) are exactly what this app needs.

Mobile was **not** a factor: this is a USB-HID desktop tool, and `../ecash-wallet-mobile` already
covers mobile in the family. Do not reopen this on crate count alone — the numbers above were
known when the decision was made.

### Claude Code tooling (GPUI skills)

Two upstream skills are **vendored into `.claude/skills/`**, fetched from
`longbridge/gpui-component@main` on 2026-08-21. They load automatically by topic:

- **`gpui`** — the framework: entities, elements, context (`App` / `Window` / `Context<T>` /
  `AsyncApp`), async and background tasks, actions and keybindings, events, focus handling,
  layout/styling, and testing. 22 reference files.
- **`gpui-component`** — the component library: setup, the component catalog, stateless vs
  stateful component patterns, theming, and a style guide. 2 reference files.

Two setup facts from the skill that everything else depends on: `gpui_component::init(cx)` must
be the first call in `app.run()`, and `Root::new(view, window, cx)` must be the first-level view
in every window.

Live docs, when the vendored copies are not enough:

- Full reference: `https://longbridge.github.io/gpui-component/llms-full.txt` (~1.2 MB, bilingual
  zh/en, every component duplicated per language — fetch per-component instead where possible)
- Per component: `https://longbridge.github.io/gpui-component/docs/components/{name}.md`
- Any site page renders as Markdown by appending `.md` to its URL

> **Version drift.** Upstream docs track `main` (0.5.2-dev); we build against **0.5.1** from
> crates.io (§4). Minor API drift is expected — trust the compiler over the docs, and re-vendor
> the skills whenever we bump `gpui-component`.

---

## 5. Hardware wallets — the actual answer

**Does BDK work with hardware wallets?** The question has the wrong shape, and answering it
literally leads to a dead end. Both halves matter:

### 5.1 The literal answer: the official path is dead

BDK's hardware-wallet story was `bitcoindevkit/rust-hwi`, a PyO3 wrapper around Bitcoin Core's
Python **HWI**. As of now:

- **The repository was archived on 2026-01-22.** Read-only, unmaintained.
- The `hwi` crate is stuck at **0.10.0 (2024-09-13)**.
- It **embedded a Python 3 runtime** — plus libusb and udev — into the process. For a signed,
  notarized, reproducibly-built desktop binary that is disqualifying on its own, archived or not.

Do not build on it. Do not "just vendor it."

### 5.2 The real answer: BDK never needed to talk to the device

BDK's job ends at a **PSBT**. The device's job starts at a **PSBT**. The seam between them is a
byte buffer, not an API. So there is no "BDK hardware wallet support" to be missing:

```
device xpub ──► bdk_wallet (watch-only descriptor, scan, coin select, build)
                     │
                     ▼   bitcoin::psbt::Psbt
              ecx_core::finalize_ecx_psbt   ← locktime + invariants (Golden Rule 2)
                     │
                     ▼
        ecx-signer ──► Ledger / Trezor / BitBox02 / Coldcard / Jade / air-gap
                     │
                     ▼   signed Psbt
              ecx_core::verify_signed       ← Golden Rule 3
                     │
                     ▼
              ecx-chain broadcast (ECX only)
```

And because ECX serialization is byte-identical to Bitcoin (§3), **every device signs this as an
ordinary Bitcoin transaction**. No firmware support, no coin definition, no vendor buy-in.

### 5.3 What to actually use

All on `bitcoin 0.32`:

- **`async-hwi 0.0.32`** — Ledger, BitBox02, Coldcard, Jade, Specter. Async, `HWI` trait
  (`get_master_fingerprint`, `get_extended_pubkey`, `register_wallet`, `sign_tx`). Pure Rust,
  no Python. **No Trezor.**
- **`trezor-client 0.1.6`** with `features = ["bitcoin"]` — first-party, maintained inside
  `trezor-firmware`. `Trezor::sign_tx(&psbt, Network::Bitcoin)` takes a `bitcoin::psbt::Psbt`
  directly and reads `lock_time` straight off `psbt.unsigned_tx`. **Blocking**, over `rusb`.
  **Model T / Safe 3 / Safe 5 only — Model One is unsupported** (§5.5).
- **`ledger_bitcoin_client 0.6.2`** directly, only if `async-hwi` proves too coarse.
- **Air-gapped PSBT** — `.psbt` file on SD card, and animated QR. Covers Coldcard, SeedSigner,
  Krux, Passport, Jade, and anything future. **Ship this in v1 as a first-class path, not a
  fallback** — for a fork-claim tool it is both the safest option and the one that needs no
  vendor library at all.

### 5.4 Per-device facts that will bite

- **Trezor does not need Blockbook.** Blockbook is Trezor *Suite*'s backend; the device has no
  network access and no opinion about where data came from. Driving the device ourselves over
  USB means no Blockbook anywhere in this project.
  **But**: since firmware 1.9.0 / 2.3.0, Trezor requires the **full previous transaction for
  every input, including segwit**, so it can verify input amounts itself. Suite fetches those
  from Blockbook; we must supply them as **`non_witness_utxo` in the PSBT**, sourced from our own
  ECX indexer. Concretely: the chain source must be able to serve raw transactions, and
  **never call `TxBuilder::only_witness_utxo()`**. Taproot inputs are exempt.
- **Ledger requires a wallet policy.** Standard single-sig on a standard path (`wpkh(@0/**)`,
  BIP84) is a *default* policy and signs with no registration. Anything else must be registered
  first via `register_wallet`, and the returned HMAC persisted — otherwise every signature
  re-prompts. v1 stays on default policies for this reason.
- **Coin type is `0'`.** ECX has no SLIP-44 of its own, so we derive at `m/84'/0'/…` — the user's
  *real Bitcoin account*. That is where the coins are; there is no alternative. It also means a
  bug here spends real BTC. Never let a device be in testnet mode.
- **Passphrase wallets change the master fingerprint.** The descriptor must be rebuilt per
  passphrase; a fingerprint mismatch between descriptor and device is a hard error, never a warning.
- **CONFIRM on real hardware, per device, before any release:** that the device accepts
  `nLockTime = 499999999`; what (if anything) it displays about it — Trezor is expected to show a
  locktime/blockheight warning, which is the only on-device ECX marker we get; and whether Coldcard
  or Jade warn or refuse on an unusual locktime. Record the results in `docs/device-matrix.md`.
- **USB plumbing:** `hidapi`/`rusb` need udev rules on Linux (ship them + a setup doc). macOS
  hardened-runtime builds need the USB device entitlement — **CONFIRM** what `dx bundle` emits
  and what notarization requires. All device I/O is blocking or slow; it runs on a dedicated
  thread or `spawn_blocking`, never on the UI thread (§10).

### 5.5 Device session model

Device differences leak into the UI whether we like it or not. `ecx-signer` normalizes what it
can behind one trait; three things it cannot hide:

| | `async-hwi` (Ledger, BitBox02, Coldcard, Jade, Specter) | `trezor-client` (Trezor) |
|---|---|---|
| Call style | async, one-shot | **blocking**, recursive ack loop |
| Transport | hidapi / serial / TCP | `rusb` |
| Enumeration | per-module (`Ledger::enumerate(&HidApi)`, …) — **no global list, we write the fan-out** | `find_devices()` / `unique()` |
| Mid-call UI callbacks | none | `ButtonRequest` / `PinMatrixRequest` / `PassphraseRequest` |

Trezor gets a dedicated thread with a channel pair, because it blocks *and* calls back into the
UI mid-operation. Everything else is `spawn_blocking` at worst.

Our trait surface — `async-hwi`'s `HWI` is already almost exactly this, so mirror it:

```rust
device_kind() · get_version() · get_master_fingerprint()
get_extended_pubkey(&DerivationPath) -> Xpub
sign_tx(&mut Psbt)
display_address(&AddressScript)                     // §7.5 destination verification
register_wallet(name, policy) -> Option<[u8; 32]>   // Ledger only in v1
```

**Unlock is not uniform, and the differences are UI work:**

- **Every supported device enters its PIN on-device** — Ledger (all models), Trezor Model T /
  Safe 3 / Safe 5, BitBox02, Coldcard, Jade. **This app has no PIN screen and never will**
  (Golden Rule 1).
- **Trezor Model One is unsupported** (decided 2026-08-20, §12). It has a 128×64 screen and two
  buttons and cannot take a PIN at all, so it needs the blind matrix: a blank grid rendered by
  the *host*, with the randomized digit layout visible only on the device. That is a sound
  scheme — the host learns positions, never digits — but it is the only device that would force a
  PIN screen into this app, and Model One also cannot take a passphrase on-device (below).
  Supporting it means owning both, for the one device that can do neither.
  **Handle `PinMatrixRequest` as an explicit "unsupported device" error** naming the model — never
  by building a matrix.
- **Jade — unlock relays to Blockstream's blind PIN oracle over HTTPS.** CONFIRMED 2026-08-21 by
  reading `async-hwi`'s `jade::pinserver`: `auth()` receives a `PinServerRequired { http_request }`
  from the device, forwards it, and passes the response back. This is **required, not optional** —
  Jade has no secure element and the oracle is its anti-brute-force guarantee. The oracle is
  blind and the host is a pipe. **Jade is supported over USB**; see the Golden Rule 8 carve-out.
  `auth()` is a no-op on an already-unlocked device, so a user who unlocked in the Blockstream
  app never triggers a request.
- **Passphrase entry is on-device on every supported model**, so no passphrase transits host
  memory either. Model One would have `PassphraseAck`'d it to us in cleartext, since it cannot
  take text input and cannot satisfy `ApplySettings`' on-device-only enforcement — the second
  reason it is out. **Handle `PassphraseRequest` by instructing the user to enter it on the
  device**; if a device demands host entry, that is an unsupported-device error.
  A passphrase changes the master fingerprint, which means **it is a different wallet** (§5.6) —
  the app still has to ask *whether* one is in use, and re-run discovery per fingerprint, but it
  never handles the passphrase itself.

> Dropping Model One is a real cost — it was the original Trezor with a large install base, and
> long-term holders skew toward old hardware, which is exactly this app's user. It has no SD card
> and no camera, so there is no air-gap fallback either: for Model One holders the answer is
> "use `ecash-electrum`." Taken deliberately, in exchange for Golden Rule 1 holding absolutely.

### 5.6 Account discovery — "find my coins"

The device is a pubkey oracle and a signer. **It does not know your balance and cannot tell you
which accounts you use.** Every derivation path always yields a valid key; there is no such thing
as an account that "does not exist" on a device. Whether an account holds coins is a question for
the chain, not the device. So discovery is ordinary BIP44 account discovery, run by us:

```
fingerprint = device.get_master_fingerprint()
for (purpose, kind) in [(84', wpkh), (49', sh(wpkh)), (44', pkh), (86', tr)]:
    for account in 0..3:
        path = m/{purpose}/0'/{account}'
        xpub = device.get_extended_pubkey(path)            # cached by fingerprint
        desc = "{kind}([{fingerprint}/{path}]{xpub}/<0;1>/*)"
        chain.full_scan(desc, stop_gap = 20)
→ list every account with history: script type, path, UTXO count, amount
```

12 candidate accounts by default, which covers the overwhelming majority of real users. Then a
**"scan deeper"** action for more account indices, and manual path entry as an advanced escape
hatch. Coin type is `0'` throughout (§3).

**Cost and caching.** The xpub reads are ~12 USB round trips — seconds, and **no button presses**
if we set `Ledger::display_xpub(false)`. The indexer scan dominates. **Cache xpubs keyed by
master fingerprint**, so a rescan never touches the device: after discovery the user can unplug
until signing.

Three failure modes to handle explicitly rather than paper over:

1. **A partially-synced indexer looks exactly like an empty wallet.** This is Golden Rule 9 and
   it is not hypothetical — `explorer.alpha.ecash.ninja` was at height 458,330 on 2026-08-20,
   less than halfway to the fork. Scanning it today returns "no history" for funded accounts.
   Gate every result on `tip >= EcashHeight`; below it show `syncing — 458,330 / 963,648` and
   state no balance at all.
2. **A passphrase wallet is invisible.** A BIP39 passphrase yields a different fingerprint and a
   completely different account set. We cannot enumerate them and cannot detect that they exist.
   Reporting "found 0.4 BTC" against the wrong wallet is a serious miss, so ask explicitly and
   support re-running discovery per passphrase, keyed by fingerprint.
3. **Non-standard paths get warnings or refusals** from the device. Auto-discovery stays on the
   four standard purposes; manual entry is advanced-only and may prompt on-device.

**Discovery must also work with no device connected** — paste an xpub or descriptor, scan, and
connect the device only to sign. Same code path the air-gap flow needs, and faster on repeat runs.

---

## 6. Chain source

**Esplora against an ECX indexer**, with Electrum as the alternate. `bdk_esplora` /
`bdk_electrum` both serve full raw transactions, which §5.4 requires for Trezor anyway.

Design `ecx-chain` as a trait over {full scan, sync, broadcast, fetch raw tx, fetch block hash at
height} so the backend is a constructor change — but do not add speculative implementations.

**BIP157/158 SPV (`bdk_kyoto`) is out.** Recorded so it doesn't come back around:

1. It needs peers serving `getcfilters`. Core only does that with `-blockfilterindex=1
   -peerblockfilters=1`, both **off by default**. On a days-old fork the number of ECX peers
   serving filters is plausibly zero, and we do not control them.
2. We would scan from genesis. The wallets being split are pre-fork BTC wallets, so the relevant
   history is ~964k blocks — full header chain, filter headers, and every matched block. A slow
   first run for a tool the user opens once.
3. Difficulty resets to minimum at the fork block, so early ECX reorg risk is elevated and a
   light client's header-chain assumptions are weakest exactly when we need them.

**Known endpoints (2026-08-20, all pre-launch and volatile — re-check every phase):**

| Endpoint | Status |
|---|---|
| `https://explorer.alpha.ecash.ninja` | **live**, Esplora-compatible API, tip 458330 (still syncing from genesis) |
| `https://esplora.alpha.ecash.ninja` | DNS resolves, 502 — not up yet |
| `ssl://drynet4.drivechain.dev:50002` | electrs, port open, drynet4-era |
| `seed.alpha.ecash.ninja:8533` | P2P seed, open |

Naming convention is `*.alpha.ecash.ninja`; expect `*.beta.` and a mainnet set per phase.

### The fork probe (Golden Rule 4)

Before an endpoint is usable, prove it is ECX and not Bitcoin. ECX and BTC share every block up
to `EcashHeight - 1` and **diverge from `EcashHeight` onward**, so:

1. Fetch the block hash at height `963648` from the candidate endpoint.
2. Compare against Bitcoin mainnet's hash at that height (bundled as a constant, refreshed per
   phase).
3. **Equal → it is a Bitcoin endpoint. Refuse, permanently, with a specific error.**
4. Missing/not-yet-reached → "endpoint has not synced past the fork"; usable for scanning, **not**
   for broadcast.

Cheap, decisive, and it catches the single worst configuration mistake this app can make.

**Dual view (post-v1, design for it now):** an optional read-only Bitcoin chain source lets us
show "unspent on ECX / unspent on BTC" side by side, which is how a user confirms the split
actually did what they think. It must be read-only by construction — the BTC source has no
broadcast method at the type level.

**Fees:** post-fork ECX has reset difficulty and an empty mempool; `estimatefee` will be absent
or noise. Use a static default with a hard floor (start at 1 sat/vB), let the user override, and
never call an estimator that could return a BTC-mempool number.

---

## 7. The split flow

What the user sees, end to end. Ordering is from `ecash-com/fast-facts` doc 03 and is not ours to
improvise (Golden Rule 6: ECX side first, always).

1. **Open the app** — current fork phase banner (§3) and chain-source status up front. If the
   indexer has not synced past `EcashHeight`, that is stated here and discovery is disabled
   (Golden Rule 9).
2. **Connect the device** — enumerate, unlock, read master fingerprint (§5.5). Air-gap users skip
   this and paste an xpub or descriptor instead.
3. **Find accounts with coins** — BIP44 discovery across the four standard script types (§5.6).
   Result is a table: script type, path, UTXO count, amount. "Scan deeper" and manual path entry
   are available but not the default. The device can be unplugged after this step.
4. **Select the account to split.**
5. **Enter the ECX destination** — see the footgun below. Device-derived is the default; a pasted
   address is the secondary path.
6. **Review** — the confirmation screen is the product (§10). Destination address *and*
   derivation path, amount, fee, `nLockTime` shown literally as `499999999`, input count, fork
   phase, and a plain statement that this spends on eCash and not on Bitcoin.
7. **Sign on the device.** The device will say *"Bitcoin"* — it has no way not to (Golden Rule 5).
8. **Re-verify then broadcast** — `verify_signed` on the returned bytes (Golden Rule 3), then
   broadcast only to an endpoint that passed the fork probe (Golden Rule 4).
9. **Wait for depth.** Post-fork difficulty is at minimum, so reorg risk is elevated — require
   meaningfully more confirmations than 6 and say why. **CONFIRM** the number against observed
   alpha block times before release.
10. **Hand off** — export the ECX destination descriptor for `ecash-electrum` / eCash.com Wallet,
    with the standing rule that every future ECX transaction keeps `nLockTime = 499999999`.

At no point before step 8 does the app offer any action that touches Bitcoin. After step 8 the
BTC side is **automatically safe**: those UTXOs are already spent on ECX, so a later BTC
transaction spending them has nothing to replay against. Explain that to the user rather than
making them take it on faith.

### The destination is the sharpest footgun in the app

**An ECX address *is* a Bitcoin address.** Same prefixes, same HRP, same checksum (§3) — nothing
in the string identifies the chain, and no validation we could write would tell them apart. So a
user can paste a Bitcoin exchange deposit address and neither we nor the device can object. Those
coins are simply gone: the exchange runs no ECX node, and the transaction is not even visible to
them.

Required mitigations, all of them:

- **Default to a pasted address** (REVISED 2026-08-21 — this section previously defaulted to a
  device-derived destination; that was wrong in practice). Two reasons. Most people splitting want
  the coins in a *different* wallet — a dedicated ECX wallet like `../ecash-electrum` — not a
  second account on the same seed, and forcing the device path makes the common case awkward.
  More importantly, until Ledger `register_wallet` lands (§12) a device-derived address **cannot
  be shown on the device screen**, so it is the *less* verifiable of the two: an address pasted
  out of the user's own ECX wallet is one they can check in that wallet, against a real receive
  screen. Defaulting to the option we cannot verify would have been false assurance.
- **A pasted address still requires a typed acknowledgement** naming the chain. Not a checkbox.
  This is now the primary path, so the §7.5 warning above carries more weight, not less.
- **A device-derived destination stays available** — a fresh account on the connected device,
  e.g. `m/84'/0'/1'`. Once `display_address` works it becomes genuinely safer than pasting and
  this default should be revisited. Until then, say plainly in the UI that the address was
  derived locally and has not been confirmed on the device.
- **Hard-warn on any destination with pre-fork history.** A reused address is the loudest
  available signal that the user actually means Bitcoin.
- **State plainly that no exchange accepts ECX deposits**, so an exchange address is always wrong.
- **The destination account must be ECX-only, forever, and must never have been used on BTC.**
  Mingling ECX and BTC UTXOs in one account is how people lose Bitcoin months later. This is a
  rule we state loudly; with a pasted address we cannot enforce it, which is exactly why
  device-derived is the default.

---

## 8. Invariants (`ecx-core`)

`finalize_ecx_psbt` asserts, and `verify_signed` re-asserts on the finalized transaction bytes:

1. `lock_time == 499_999_999`.
2. Every input has `sequence != 0xFFFFFFFF` (we set `0xFFFFFFFD`). Violating this makes the
   locktime ignored and **the transaction replays onto Bitcoin** — this is the single most
   expensive possible bug in the codebase.
3. Every input outpoint is in the intended, user-confirmed set.
4. Every output script is either the confirmed destination or change in the ECX-dedicated
   account. No unexpected outputs, ever.
5. Every non-taproot input carries `non_witness_utxo` (§5.4).
6. Fee is within a sane absolute cap; an absurd fee is a bug, not a user preference.

These are `Result`, not `assert!` — a failed invariant is a user-facing abort, not a panic, and
`ecx-core` has no `unwrap()` on any path a PSBT can reach. Property-test them.

---

## 9. Architecture

```
ecash-splitter/
├─ Cargo.toml               # workspace
├─ crates/
│  ├─ ecx-core/             # chain params, phases, magic locktime, EcxPsbt newtype,
│  │                        #   finalize_ecx_psbt, verify_signed, invariants (§8). No I/O.
│  ├─ ecx-chain/            # ChainSource trait: full_scan/sync/broadcast/raw tx/hash at height.
│  │                        #   esplora | electrum. Owns the fork probe (§6).
│  ├─ ecx-signer/           # Signer trait over async-hwi | trezor-client | air-gap (file, QR).
│  │                        #   Device enumeration, fingerprint check, blocking I/O off the UI.
│  ├─ ecx-wallet/           # bdk_wallet: watch-only descriptors from device xpubs, UTXO view,
│  │                        #   coin selection, PSBT construction. Watch-only by construction.
│  └─ ecx-split/            # the plan: enumerate → order → build → verify → broadcast → track.
└─ app/                     # desktop UI (framework open, §4). Screens + state only —
                           #   no I/O, no tx construction, no consensus logic.
```

**Dependency direction:** `app → ecx-split → {ecx-wallet, ecx-signer, ecx-chain} → ecx-core`.
Nothing depends upward. **`ecx-core` has no dependency on any UI framework, `tokio`, or any I/O crate** —
it is pure, sync, and exhaustively testable, which is the point. The UI never constructs a
transaction, never sets a locktime, and never calls a device directly.

---

## 10. UI rules

Framework-agnostic — these hold whichever way §4 lands.

- **The UI layer performs no I/O.** No network, no device, no filesystem beyond app config.
  Everything goes through the `ecx-*` crates behind channels. The UI never constructs a
  transaction, never sets a locktime, never calls a device directly (Golden Rule 8).
- **The confirmation screen is the product.** Everything in §7.6, laid out so the important
  facts cannot be missed by someone clicking quickly. Typed confirmation for large amounts.
- **Persistent ECX badge and fork-phase chip** at every money-touching surface, in ECX's own
  color, visually unmistakable from Bitcoin orange (mirrors `ecash-wallet-mobile` Golden Rule 6).
  The user must never have to remember which chain they are on.
- **Never state a balance from an unsynced chain source.** Show `syncing — 458,330 / 963,648`
  instead. "No coins found" is a claim, and below `EcashHeight` it is a false one (Golden Rule 9).
- **Never block the render loop on a device.** Every device and chain operation runs in the
  background and delivers results over a channel; every device operation has a visible
  "check your device" state and a working cancel. Trezor blocks and calls back mid-operation
  (§5.5) — it gets a dedicated thread.
- **There is no PIN screen and no passphrase screen in this app**, and adding one is a Golden
  Rule 1 violation. Every supported device takes both on-device; the UI's job is to say
  "confirm on your device" and wait. A device that demands host entry is an unsupported-device
  error (§5.5).
- **No secrets in logs, ever.** There are no private keys in this app by construction, but also:
  no PSBTs at info level, no addresses in crash reports, no telemetry at all.
- **Addresses and amounts must be legible, selectable, and copyable**, and must survive OS text
  scaling. Verify this explicitly — a native toolkit gives less of it for free than a WebView does.

**GPUI specifics.** "No WebView" is structural, and embedding one anywhere forfeits it (Golden
Rule 8). Follow the vendored `gpui-component` skill's style guide (§4) rather than hand-rolling
styles: use `cx.theme()` tokens (`.primary` / `.background` / `.foreground` / `.border`
/ `.muted`) so the app stays coherent as it grows, and build the ECX badge and phase chip as
themed components so they cannot drift out of sync with the rest of the palette.

---

## 11. Testing & release

- **Unit/property:** §8 invariants, exhaustively. Round-trip a built PSBT through a mock signer
  and confirm `verify_signed` catches every mutation — flipped sequence, swapped output, altered
  locktime, injected input. These tests are the reason the app can be trusted.
- **Integration:** regtest/signet ECX node with the `EcashHeight` patch; assert the built
  transaction is accepted as final by ECX `IsFinalTx` **and** rejected as non-final by Bitcoin
  Core. `../ecash-electrum` has a working end-to-end harness (`scratchpad/test_ecx_handoff.py`)
  — reuse its assertions.
- **Device matrix:** real hardware, every §5.4 CONFIRM item, recorded in `docs/device-matrix.md`
  with firmware versions. No release without it.
- **Build environment:** macOS CI must install the Metal Toolchain
  (`xcodebuild -downloadComponent MetalToolchain`, ~688 MB) or `gpui`'s build script fails before
  any Rust compiles. Pin it alongside the Rust toolchain — it is part of the reproducible build.
  **CONFIRM** GPUI's actual MSRV; ours is currently 1.85 from `async-hwi`, and the verified build
  used rustc 1.94.0.
- **Release:** macOS `.dmg`, Windows `.msi`, Linux AppImage — via `cargo-packager` 0.11.8,
  per §4. Zed's `script/bundle-{mac,linux,windows.ps1}` is the prior art if it falls short. **Reproducible builds plus independent attestation are the
  main defense** — fork-claim tools are a classic
  malware lure and users are right to be suspicious. Apple Developer ID + notarization; Windows
  Authenticode (hardware-token/HSM keys since the 2023 CA/B baseline change); detached GPG
  signatures with independent verifiers. Expect AV false positives regardless.
  Commit `Cargo.lock`; run `cargo-deny` in CI; keep the dependency tree small enough that a
  reviewer can actually read it. See `../ecash-electrum/CLAUDE.md` §"Build & release" — the
  signing burden is the real cost, and it is the same either way.

---

## 12. Out of scope for v1 (design for, don't build)

**Trezor Model One** — decided 2026-08-20, see §5.5; it is the only device that cannot take a
PIN or passphrase on-device, and supporting it would put both into this app's memory. Model One
holders are directed to `ecash-electrum`. Also out:
multisig and miniscript policies (Ledger `register_wallet` + HMAC persistence is the work);
the dual BTC/ECX read-only view; BIP300/301 sidechain deposits;
Lightning; ongoing send/receive after the split (that is `ecash-electrum`'s job); mobile.
