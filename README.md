# eCash Splitter

Claim **ECX** (the Layer Two Labs Bitcoin hard fork branded "eCash") from a Bitcoin hardware
wallet, without risking the BTC.

Not a wallet. It builds a replay-protected sweep, has a hardware device sign it, verifies the
bytes that come back, and broadcasts to ECX. Available as a desktop app and a CLI — both share
the same core, so they behave identically.

> **Pre-release.** eCash activates at block 963,648. Signing works today; **broadcasting does
> not**, because until the chains diverge no endpoint can be proven to be ECX. Alpha and beta
> coins are also destroyed and re-issued at full launch (973,728, 2026-10-31) — nothing produced
> before then is durable value.

## Requirements

- Rust 1.94 (pinned in `rust-toolchain.toml`)
- **macOS only:** the Metal Toolchain, which GPUI needs to compile shaders

```sh
xcodebuild -downloadComponent MetalToolchain   # ~688 MB, one time
```

## Build

```sh
cargo build
cargo test --workspace
```

## Run the desktop app

```sh
cargo run -p ecash-splitter
```

Connect device → search for accounts → pick one → enter a destination → build → review → sign.

## Run the CLI

```sh
cargo run -p ecash-splitter-cli -- <command>
```

| Command | What it does |
|---|---|
| `devices` | List connected hardware wallets |
| `status` | Chain tip, whether it is caught up, and how far off the fork is |
| `discover` | Find accounts with coins on the connected device |
| `build --account <PATH> --to <ADDRESS>` | Build the sweep PSBT and print it |
| `sign --to <ADDRESS>` | Build, show the PSBT, confirm, sign on the device, verify. Does **not** broadcast |

Useful options: `--endpoint <URL>`, `--accounts <N>` (how many account indices to search per
address type), `--gap <N>`, `--feerate <SAT_PER_VB>`, `--psbt-out <FILE>`.

```sh
# find your accounts
cargo run -p ecash-splitter-cli -- discover

# sweep one of them; prompts for the account if --account is omitted
cargo run -p ecash-splitter-cli -- sign --to bc1q...
```

## Supported devices

Ledger (verified on hardware), Coldcard, Specter, Jade, and Trezor Model T / Safe 3 / Safe 5.
Trezor Model One is deliberately unsupported — it cannot take a PIN or passphrase on-device.

[`docs/support-status.md`](docs/support-status.md) has the full picture: what works, what does
not, and what each missing piece needs.

## The three rules everything else follows from

1. **No secret of any kind enters this app.** Not a seed, not an xprv, not a PIN, not a
   passphrase. Watch-only plus PSBT; every supported device unlocks on-device.
2. **One chokepoint stamps replay protection.** `ecx_core::finalize_ecx_psbt` sets
   `nLockTime = 499999999` and every input's sequence, and returns an `EcxPsbt` — not
   constructible any other way, so an unstamped PSBT cannot reach a signer.
3. **The device's output is untrusted.** `ecx_core::verify_signed` re-checks the returned bytes
   against what the user confirmed, before anything is broadcast.

An input left at `nSequence = 0xFFFFFFFF` makes the locktime ignored and the transaction replays
onto Bitcoin. That is the most expensive possible bug here, and it is what the `ecx-core` test
suite exists to prevent.

## Layout

| Crate | Role |
|---|---|
| `crates/ecx-core` | Consensus facts and transaction invariants. Pure — no I/O, no async, no UI |
| `crates/ecx-chain` | Chain sources, the fork probe, the sync gate |
| `crates/ecx-signer` | Hardware wallets, over `async-hwi` and `trezor-client` |
| `crates/ecx-wallet` | Watch-only descriptors, account discovery, PSBT construction via BDK |
| `crates/ecx-split` | The flow itself — what both frontends drive |
| `app` | GPUI desktop shell |
| `cli` | `ecx` command line |

Dependencies point one way: `app`/`cli` → `ecx-split` → `{ecx-wallet, ecx-signer, ecx-chain}` →
`ecx-core`.

Read [`CLAUDE.md`](CLAUDE.md) before changing anything — the golden rules, the verified ECX
consensus facts, and the decisions already settled.

## Licence

MIT.
