# eCash Splitter

Claim **ECX** (the Layer Two Labs Bitcoin hard fork branded "eCash") from a Bitcoin hardware
wallet, without risking the BTC.

Not a wallet. It builds a replay-protected sweep, has a hardware device sign it, verifies the
bytes that come back, and broadcasts to ECX. Available as a desktop app and a CLI — both share
the same core, so they behave identically.

> **Mainnet.** Configured for the eCash fork at Bitcoin block **973,728**, network magic
> `eca5dcb7`, replay-protection locktime **499999998** — all read off `ecash-com/bitcoin` @
> `mainnet-bridge` on 2026-10-28. Endpoints are `esplora.ecash.ninja` and `explorer.ecash.ninja`
> from <https://drivechain.dev/config>.
>
> **The fork block had not been mined when this version shipped** (Bitcoin was at 973,577). Until
> it exists, there is no Bitcoin reference hash to compile in, so the fork probe cannot prove any
> endpoint is eCash and **broadcast is refused**. Discovery, building and signing all work. Once
> block 973,728 is a few blocks deep, either set `ECX_BITCOIN_FORK_HASH` to Bitcoin's hash at
> that height (from blockstream.info or mempool.space — never from an eCash endpoint) or install
> the follow-up release that compiles it in. See [Changing endpoints](#changing-endpoints).

## Install a release

Download from [Releases](https://github.com/ecash-com/ecash-splitter/releases). Two separate
downloads per platform — the GUI and the `ecx` CLI. They are not one binary: `ecx` has no GPUI
dependency, so it runs on headless machines where the GUI could not even start.

Check what you downloaded first:

```sh
sha256sum -c SHA256SUMS
```

> ### The binaries are unsigned
>
> There is no Apple Developer ID signature and no Windows Authenticode certificate, so both
> systems will warn you. That warning means *"nobody paid a certificate authority to vouch for
> this"* — it is not a claim that anything is wrong with the file. Verify the checksum, and if
> you want stronger assurance than our word, build from source: the whole point of a committed
> `Cargo.lock` and a pinned toolchain is that you can reproduce these binaries yourself.
>
> Be suspicious of tools like this in general. Fork-claim software is a classic way to steal
> coins, and you should not run one because a README sounded confident.

### macOS

The `.app` is ad-hoc signed — enough for Apple Silicon to run it at all, not enough for
Gatekeeper. What you do next depends on your macOS version, and the old advice no longer works:

**macOS 15 Sequoia and later** — Control-click → Open used to be the whole answer. It is now
only the first half: it still refuses, and the actual permission is granted in Settings.

1. **Control-click (right-click) the app → Open.** It refuses; dismiss the dialog.
2. **System Settings → Privacy & Security**, scroll down to **Security**.
3. A line names the app you just tried to open — click **Open Anyway**.
4. Authenticate, then confirm **Open Anyway**.

Step 1 is not optional. Settings only offers the button for an app that has already been blocked,
so going there first shows nothing. You do this once per app.

**macOS 14 Sonoma and earlier** — **Control-click → Open**, then **Open** in the dialog. That is
the whole thing; no trip to Settings.

If it still will not start, clear the download quarantine:

```sh
xattr -dr com.apple.quarantine "/Applications/eCash Splitter.app"
```

For the CLI, put `ecx` on your PATH and clear the same attribute:

```sh
xattr -d com.apple.quarantine ./ecx && chmod +x ./ecx && sudo mv ./ecx /usr/local/bin/
```

### Windows

SmartScreen shows **"Windows protected your PC"**. Click **More info** → **Run anyway**.

If the download is blocked outright, unblock it first: right-click the `.zip` → **Properties** →
tick **Unblock** → **OK**, then extract. Unblocking the archive before extracting saves doing it
per file.

### Linux

No signature machinery to fight. Make it executable and run it:

```sh
tar xzf ecash-splitter-*.tar.gz && chmod +x ecash-splitter && ./ecash-splitter
```

Icons for a `.desktop` entry are in `assets/linux/`.

## Build from source

### Requirements

- Rust 1.94 (pinned in `rust-toolchain.toml`)
- **macOS only:** the Metal Toolchain, which GPUI needs to compile shaders

```sh
xcodebuild -downloadComponent MetalToolchain   # ~688 MB, one time
```

### Build

```sh
cargo build
cargo test --workspace
```

### Run the desktop app

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

### Run the CLI

```sh
cargo run -p ecash-splitter-cli -- <command>
```

| Command | What it does |
|---|---|
| `devices` | List connected hardware wallets |
| `status` | Which chain this build targets, the tip, whether it is caught up, whether the endpoint is provably eCash, and whether its coins are durable |
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
# network  : mainnet
# tip      : 973577 (unix 1793207048)
# status   : caught up
# fork     : block 973728 (not yet reached)
# coins    : durable
# broadcast: REFUSED
#             No Bitcoin reference hash is configured for the fork at block 973728, ...
#
# — and once the fork block exists and the reference hash is set:
# fork     : block 973728 (activated)
# broadcast: allowed — endpoint proven to be eCash

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

## Changing endpoints

Hosts can move, so they are changeable three ways, in increasing order of permanence:

```sh
# 1. at runtime: the GUI has an endpoint field; the CLI takes --endpoint
cargo run -p ecash-splitter-cli -- status --endpoint https://explorer.ecash.ninja

# 2. by environment, no rebuild and no UI fiddling
ECX_ESPLORA_URL=https://esplora.ecash.ninja cargo run -p ecash-splitter
```

3. **In code** — `PRESETS` in `crates/ecx-chain/src/profile.rs` is the only place a hostname is
   written.

The fork probe also needs **Bitcoin's** block hash at the fork height, 973,728 — that is what
proves an endpoint is the fork and not the original chain. It is compiled in as
`BITCOIN_HASH_AT_FORK` (`crates/ecx-chain/src/lib.rs`), **currently `None` because the block has
not been mined**, and overridable the same way:

```sh
ECX_BITCOIN_FORK_HASH=<bitcoin's hash at block 973728> cargo run -p ecash-splitter
```

Take it from an independent Bitcoin source, **never from an eCash endpoint** — comparing a chain
against itself would clear anything. With no reference set, broadcasting is refused.

### The chain this build targets

Three values define it, and they must agree. Change one and not the others and the fork probe
compares the right block against the wrong chain:

| Value | Where |
|---|---|
| `NETWORK` — the chain's height, magic, magic locktime and whether its coins are durable | `crates/ecx-core/src/lib.rs` |
| `BITCOIN_HASH_AT_FORK` — **Bitcoin's** hash at that height | `crates/ecx-chain/src/lib.rs` |
| The endpoint | `PRESETS` in `crates/ecx-chain/src/profile.rs` |

Every field was read off the `mainnet-bridge` branch of `ecash-com/bitcoin` rather than guessed.
The replay marker is worth singling out: mainnet's `IsFinalTx` short-circuits on
`LOCKTIME_THRESHOLD - 2`, one below the value the pre-launch nets used, so the magic locktime is
**499999998**. `ECX_MAGIC_LOCKTIME` and `ECASH_HEIGHT` are both derived from `NETWORK`, so the
marker and the rest of the chain's identity cannot drift apart.

Durability is a property of **which chain the build targets**, never of the chain tip. Mainnet
coins are durable; the app would warn on a pre-launch net, and no tip height can change either
answer.

`ecx status` is the check: it names the network, reports whether the endpoint is provably eCash,
and says plainly when broadcast is refused and why.

`/api` is appended to an endpoint URL if omitted and trailing slashes are trimmed, so either form
works — **except on a host that serves Esplora at its root**, where appending `/api` is what
causes the 404. `esplora.ecash.ninja` is one of those, so hosts whose name begins `esplora.` are
left at their root while `explorer.*` hosts get `/api`. Pass a path explicitly to override the
guess. The explorer link is derived from the API base by removing `/api`; set `ECX_EXPLORER_URL`
separately when the two differ.

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
   `nLockTime = 499999998` and every input's sequence, and returns an `EcxPsbt` — not
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
ecx --version                    # ecx 0.3.0
```

the GUI footer (`v0.3.0 · mainnet · fork height 973,728`), and `CFBundleShortVersionString` in the macOS
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
