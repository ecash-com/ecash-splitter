# eCash Splitter

Desktop tool for claiming **ECX** (the Layer Two Labs Bitcoin hard fork branded "eCash") from a
Bitcoin hardware wallet, without risking the BTC.

Not a wallet. It builds a replay-protected sweep, has a hardware device sign it, verifies the
signed bytes, broadcasts to ECX, and hands off to a real ECX wallet.

> **Pre-release.** The fork has not activated yet — alpha is block 963,648 (2026-08-23), and
> alpha/beta coins are destroyed and re-issued at full launch (973,728, 2026-10-31). Do not treat
> anything this produces before then as durable value.

[`docs/support-status.md`](docs/support-status.md) tracks what works today, what does not, and
what each missing piece needs.

Read [`CLAUDE.md`](CLAUDE.md) before changing anything. It is the project bible: the golden rules,
the verified ECX consensus facts, the hardware-wallet reality check, and the decisions already
settled.

## Build

```sh
cargo build
cargo test --workspace
cargo run -p ecash-splitter            # desktop app
cargo run -p ecash-splitter-cli -- --help   # `ecx` command line
```

**macOS also needs the Metal Toolchain** (GPUI compiles Metal shaders in a build script):

```sh
xcodebuild -downloadComponent MetalToolchain     # ~688 MB
xcodebuild -showComponent MetalToolchain         # expect: Status: installed
```

## Layout

| Crate | Role |
|---|---|
| `crates/ecx-core` | Consensus facts and the transaction invariants. Pure — no I/O, no async, no UI. The only place replay protection is stamped. |
| `crates/ecx-chain` | Chain sources, the fork probe, the sync gate. |
| `crates/ecx-signer` | Device abstraction over `async-hwi`, `trezor-client`, and air-gapped PSBT. |
| `crates/ecx-wallet` | Watch-only descriptors, account discovery, PSBT construction via BDK. |
| `crates/ecx-split` | The split plan: enumerate, order, build, verify, broadcast, track. |
| `app` | GPUI desktop shell. Performs no I/O. |
| `cli` | `ecx` — the same flow on the command line. Shares `ecx-split` with the desktop app. |

Dependency direction is `app → ecx-split → {ecx-wallet, ecx-signer, ecx-chain} → ecx-core`.
Nothing depends upward.

## The three rules everything else follows from

1. **No secret of any kind enters this app.** Not a seed, not an xprv, not a PIN, not a
   passphrase. Watch-only plus PSBT; every supported device unlocks on-device.
2. **One chokepoint stamps replay protection.** `ecx_core::finalize_ecx_psbt` sets
   `nLockTime = 499999999` and every input's sequence, and returns an `EcxPsbt` — which is not
   constructible any other way, so an unstamped PSBT cannot reach a signer.
3. **The device's output is untrusted.** `ecx_core::verify_signed` re-checks the finalized bytes
   against what the user confirmed, before anything is broadcast.

An input left at `nSequence = 0xFFFFFFFF` makes the locktime ignored and the transaction replays
onto Bitcoin. That is the most expensive possible bug here, and it is what the `ecx-core` test
suite exists to prevent.

## Licence

MIT.
