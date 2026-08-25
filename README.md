# eCash Splitter

Claim **ECX** (the Layer Two Labs Bitcoin hard fork branded "eCash") from a Bitcoin hardware
wallet, without risking the BTC.

Not a wallet. It builds a replay-protected sweep, has a hardware device sign it, verifies the
bytes that come back, and broadcasts to ECX. Available as a desktop app and a CLI — both share
the same core, so they behave identically.

> **Alphanet.** Configured for the alpha fork at block 963,648, which activated on 2026-08-23.
> The full flow works, broadcast included, and has been run end to end against a Ledger with the
> result confirmed on-chain. `ecx status` confirms the endpoint is provably eCash.
> **Alpha and beta coins are destroyed and re-issued at full launch** (973,728, 2026-10-31), so
> nothing produced now is durable value.
>
> Each phase forks at its own height against its own Bitcoin block. Moving to beta or to the real
> launch means updating `ECASH_HEIGHT`, `BITCOIN_HASH_AT_FORK` and the endpoint — see
> [Changing endpoints between fork phases](#changing-endpoints-between-fork-phases).

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

For the real thing — app icon, proper name in the Dock — build a macOS `.app` bundle:

```sh
./contrib/bundle-macos.sh            # or: ./contrib/bundle-macos.sh release
open "target/debug/eCash Splitter.app"
```

`cargo run` launches a bare binary, which has nowhere to carry an icon or a name; macOS reads
both from a bundle's `Info.plist`. This is also the bundle the signing work operates on.

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
| `sign --to <ADDRESS>` | Build, show the PSBT, confirm, sign on the device, verify. Add `--broadcast` to publish |
| `broadcast --tx <HEX>` | Publish a transaction signed earlier |
| `track --txid <TXID>` | How deeply buried a broadcast transaction is |

Useful options: `--endpoint <URL>`, `--accounts <N>` (how many account indices to search per
address type), `--gap <N>`, `--feerate <SAT_PER_VB>`, `--psbt-out <FILE>`, `--broadcast`.

```sh
# is the endpoint reachable, caught up, and provably eCash?
cargo run -p ecash-splitter-cli -- status

# find your accounts
cargo run -p ecash-splitter-cli -- discover

# ask Bitcoin which coins are genuinely shared and worth splitting
cargo run -p ecash-splitter-cli -- check

# sign only — prints the PSBT and the signed hex, publishes nothing
cargo run -p ecash-splitter-cli -- sign --to bc1q...

# sign and publish; prompts for the account if --account is omitted
cargo run -p ecash-splitter-cli -- sign --to bc1q... --broadcast

# then watch it confirm
cargo run -p ecash-splitter-cli -- track --txid <TXID>
```

Without `--broadcast`, `sign` stops after verifying and prints the signed hex — publish it later
with `broadcast --tx <HEX>`. With it, the fork probe has to clear first or nothing is sent.

## Changing endpoints between fork phases

These hosts move every phase — drynet → alpha → beta → mainnet — so they are changeable three
ways, in increasing order of permanence:

```sh
# 1. at runtime: the GUI has an endpoint field; the CLI takes --endpoint
cargo run -p ecash-splitter-cli -- status --endpoint https://explorer.beta.ecash.ninja

# 2. by environment, no rebuild and no UI fiddling
ECX_ESPLORA_URL=https://explorer.beta.ecash.ninja/api cargo run -p ecash-splitter
```

3. **In code** — `PRESETS` in `crates/ecx-chain/src/profile.rs` is the only place a hostname is
   written.

The fork probe also needs **Bitcoin's** block hash at the fork height — that is what proves an
endpoint is the fork and not the original chain. It is compiled in as `BITCOIN_HASH_AT_FORK`
(`crates/ecx-chain/src/lib.rs`) and overridable the same way:

```sh
ECX_BITCOIN_FORK_HASH=<bitcoin's hash at the fork height> cargo run -p ecash-splitter
```

Take it from an independent Bitcoin source, **never from an eCash endpoint** — comparing a chain
against itself would clear anything. With no reference set, broadcasting is refused.

### Moving to the next phase

Three values change together, and they must agree:

| Value | Where |
|---|---|
| `ECASH_HEIGHT` | `crates/ecx-core/src/lib.rs` |
| `BITCOIN_HASH_AT_FORK` — Bitcoin's hash at that height | `crates/ecx-chain/src/lib.rs` |
| The endpoint | `PRESETS` in `crates/ecx-chain/src/profile.rs` |

`ecx status` is the check: it reports whether the endpoint is provably eCash.

`/api` is appended if omitted and trailing slashes are trimmed, so either form works. The
explorer link is derived from the API base by removing `/api`; override it separately with
`ECX_EXPLORER_URL` if they ever differ.

Changing a URL never loosens safety: every endpoint is still gated by the fork probe before
anything is broadcast.

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

## Versioning

One version, in `[workspace.package]` at the repo root. Every crate inherits it with
`version.workspace = true`, and it reaches the user in three places:

```sh
ecx --version                    # ecx 0.1.0
```

the GUI footer (`v0.1.0 · fork height 963,648`), and `CFBundleShortVersionString` in the macOS
`.app`, which `contrib/bundle-macos.sh` reads from the same root. Bump it once; tag the release
to match.

## Icons

`app_icon.png` is the 1024×1024 source. [`assets/`](assets/) holds the generated `.icns`, `.ico`,
and Linux PNG set, plus `generate.py` to rebuild them. macOS gets a rounded, padded variant
because it does not auto-mask app icons — see [`assets/README.md`](assets/README.md).

`./contrib/bundle-macos.sh` builds a macOS `.app` that uses them. Windows and Linux packaging is
still unstarted — see [`docs/signing-and-notarization.md`](docs/signing-and-notarization.md).

## Licence

MIT.
