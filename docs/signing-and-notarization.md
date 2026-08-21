# Signing, notarization, and distribution

What it takes to ship a build a stranger will run. Researched 2026-08-21; the Windows rules in
particular changed recently, so re-check before acting.

Nothing here is done yet. `CLAUDE.md` §11 calls this "the real cost, not the code", and that is
still the right framing — a fork-claim tool is a classic malware lure, so users are right to be
suspicious of an unsigned binary, and an unsigned binary is also actively hostile to run
(Gatekeeper and SmartScreen both fight the user rather than explaining themselves).

---

## The reference implementation

**[Liana](https://github.com/wizardsardine/liana)** is the closest prior art available: a Rust
desktop Bitcoin wallet using **the same `async-hwi`** we do, shipping notarized macOS builds.
Their `contrib/release/macos/README.md` documents the whole process, including the parts Apple's
own docs gloss over. Read it before starting.

Two things their setup settles for us:

1. **They ship no entitlements file at all** — hardened runtime and nothing else.
2. **They sign and notarize from Linux**, with no Mac in the loop.

---

## macOS

### Does USB HID need an entitlement?

**Evidently not.** Liana talks to Ledgers over hidapi under hardened runtime with no entitlements
plist, and ships notarized.

This is worth stating because searching the problem finds a lot of noise about `node-hid` losing
device paths under hardened runtime. That is an **Electron** problem: native Node modules trip
library validation, and the usual fix is
`com.apple.security.cs.disable-library-validation`. We are a single statically-linked Rust
binary with no loadable native modules, so it does not apply.

Note also that `com.apple.security.device.usb` is an **App Sandbox** entitlement. Hardened
runtime is not the sandbox. We need hardened runtime (notarization requires it); we do not need
the sandbox, and should not opt into it.

> **Verify rather than assume.** The first notarized build must be tested on a clean machine with
> a real Ledger attached. If HID enumeration fails there, the fix is an entitlements plist with
> `com.apple.security.device.usb`, which then forces the sandbox and a much larger conversation.

### What is required

| | |
|---|---|
| Apple Developer Program | **Already have one** — the same account signs eCash.com Wallet (`../ecash-wallet-mobile`, bundle `com.ecash.mobile.wallet`). No enrolment wait, no extra $99 |
| Certificate | **Developer ID Application** (for distribution outside the App Store), profile type **G2 Sub-CA** |
| CSR | Apple asks for one generated on a Mac. It can be generated with OpenSSL instead — RSA 2048, no choice in size or type |
| Runtime | Hardened runtime, `--code-signature-flags runtime` |
| Notarization | Submit to Apple's notary service, then **staple** the ticket |
| API key | From **App Store Connect**, *not* from the developer account's "Keys" page. This trips people up |

### What carries over from eCash.com Wallet, and what does not

The **account and team carry over**; the certificate does not.

- The mobile app is distributed through the **App Store**, which uses an *Apple Distribution*
  certificate. A desktop app shipped outside the store needs a **Developer ID Application**
  certificate — same team, different certificate type, and it has to be created.
- `../ecash-wallet-mobile/Darwin/fastlane/apikey.json` is an **App Store Connect API key**, which
  is exactly the kind notarization wants. It may be reusable directly; check its scope, since a
  key limited to app distribution may not cover the notary service.
- Pick a bundle identifier in the same namespace — `com.ecash.splitter` alongside
  `com.ecash.mobile.wallet`.

That reduces the macOS work to: create the Developer ID cert, confirm the API key's scope, and
wire `rcodesign` into CI.

### Tooling

**[`rcodesign`](https://crates.io/crates/apple-codesign)** (0.29.0) — pure Rust, signs and
notarizes **from Linux**. This matters more than convenience: it means signing can happen in the
same reproducible CI environment as the build, rather than on someone's laptop.

```sh
rcodesign sign --code-signature-flags runtime \
    --pem-source signing.key --der-source developer_id.cer \
    eCashSplitter.app

rcodesign notary-submit --max-wait-seconds 600 \
    --api-key-path ./appstore_api_key.json --staple eCashSplitter.app
```

Notarization can take an hour or more. `notary-log` checks a pending submission.

The Apple-native path is `codesign` + `xcrun notarytool` + `xcrun stapler`, and requires a Mac.
`altool` is dead — Apple stopped accepting it in November 2023.

### Also needed

- **Metal Toolchain in CI** (`xcodebuild -downloadComponent MetalToolchain`, ~688 MB). GPUI
  compiles shaders in a build script and fails without it, before any Rust compiles.

---

## Windows

Since **June 2023** every code-signing certificate, OV and EV alike, requires **HSM-backed key
storage**. Software `.pfx` files are no longer issued by any CA. That change is what makes this
awkward: there is no longer a "just sign it" option.

### Two routes

**Azure Trusted Signing** (renamed **Azure Artifact Signing**) — Microsoft's managed service.

- **$9.99/month** Basic (5,000 signatures), $99.99/month Premium (100,000)
- No token to buy, no HSM to manage, nothing to plug into CI
- Identity verified once; it then issues **short-lived certificates**, renewed daily and valid
  ~72 hours
- **Restricted to verified US, Canadian, EU and UK businesses and self-employed individuals** —
  this is the eligibility question to answer first
- Signs Windows Authenticode only

**Traditional OV/EV certificate** — $400–900/year, plus HSM logistics.

- EV earns SmartScreen reputation **immediately**; OV has to accumulate it, meaning early users
  see warnings
- Keys live on a hardware token or a cloud HSM such as Azure Key Vault. A physical token in CI
  is genuinely painful
- Azure Key Vault code-signing certificates drop to **1-year validity from February 2026**

### Recommendation

**Azure Trusted Signing, if eligible.** An order of magnitude cheaper, no token to wire into CI,
and it sidesteps the HSM problem entirely. Confirm eligibility before budgeting for anything else.

### Expect antivirus false positives regardless

Signing does not prevent them. A freshly-signed binary with no reputation, that talks to USB
devices and constructs Bitcoin transactions, is going to be flagged. Budget time for submitting
false-positive reports.

---

## Linux

No signing authority to satisfy. AppImage or `.deb`, with **detached GPG signatures**.

Also required, and easy to forget: **udev rules per vendor** for HID and serial access. Without
them the app cannot see a device at all, which presents as "no devices found" and sends users
hunting for the wrong problem.

---

## Reproducible builds matter more than the signature

`CLAUDE.md` §11: for a fork-claim tool, **reproducible builds plus independent attestation are
the main defence**. A signature proves who built it. It does not prove *what* they built.

Upstream Electrum's model is worth copying: one signer, several independent verifiers who rebuild
from source, compare hashes, and publish detached signatures to a signatures repository. That is
what lets a suspicious user check the binary rather than trust us.

Concretely: `Cargo.lock` is committed, `rust-toolchain.toml` pins 1.94.0, and the Metal Toolchain
version needs pinning too.

---

## Order of work

1. **Answer the Azure Trusted Signing eligibility question.** It determines the Windows budget,
   it is a business question rather than a technical one, and it is now the only item with a
   real lead time.
2. **Create a Developer ID Application certificate** on the existing team, and check whether the
   mobile project's App Store Connect API key covers the notary service.
3. **Get an unsigned build working on Linux and Windows at all.** Neither has ever been built.
   Signing an app that does not run is premature.
4. **macOS signing via `rcodesign` in CI**, then verify HID on a clean machine with a real device.
5. **Windows signing.**
6. **Reproducible build documentation** and a signatures repository.

macOS is now the cheap half: the account exists, so it is a certificate and some CI wiring.
