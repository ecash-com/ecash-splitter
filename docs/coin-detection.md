# Detecting splittable coins

Which coins actually need splitting, and which we can even see. Two separate problems:
**classification** (is this coin shared with Bitcoin?) and **coverage** (did we find it at all?).

The classification half follows `../ecash-wallet-mobile/docs/coin-splitting.md`, which worked it
out first — including the Esplora trap in §3. Read that document too.

---

## Classification

### Height alone is unsound, in both directions

eCash's replay marker is **permissive, not mandatory**. The fork only makes the magic locktime
*count as* final; nothing requires it. So eCash still accepts ordinary Bitcoin-valid transactions.
Spend an eCash coin from Sparrow, Electrum, or an exchange and that transaction is valid on
Bitcoin too — putting its outputs on **both chains, at post-fork heights**.

- **Under-reports, dangerously.** A post-fork coin from an unprotected spend is shared, and height
  calls it safe. The user believes they are separated; their next spend moves their BTC.
- **Over-reports.** A pre-fork coin already spent on Bitcoin cannot be replayed onto, so it needs
  no split — height insists it does.

The sound rule is inductive, not a height comparison:

> A UTXO is chain-specific **iff the transaction that created it could not appear on the other
> chain** — it carried the marker, or every one of its inputs was already chain-specific.

### Asking Bitcoin decides it in one step

`ecx check` asks a Bitcoin Esplora about each outpoint:

| Bitcoin's answer | Verdict |
|---|---|
| Does not know the transaction | eCash-only → **chain-specific** |
| Knows it, output **unspent** | live on both chains → **needs splitting** |
| Knows it, output **already spent** | a replay would double-spend → **separated** |
| Could not reach a backend | **unverified** |

**Four states, not two.** `unverified` is deliberately never folded into "settled":
`SplitCheck::worth_splitting` stays true while anything is unverified. "We could not check" and
"you are fine" must never be the same answer on a screen about money.

### The Esplora trap

`/tx/{txid}/outspend/{vout}` **does not 404 for a transaction that does not exist.** It answers
from the spend index and returns `200 {"spent": false}` for any txid at all. Verified 2026-08-24
against both hosts:

```
blockstream.info  /tx/{fake}            -> 404 Transaction not found
blockstream.info  /tx/{fake}/outspend/0 -> 200 {"spent":false}
mempool.space     /tx/{fake}            -> 404 Transaction not found
mempool.space     /tx/{fake}/outspend/0 -> 200 {"spent":false}
```

Reading that as "Bitcoin never saw it" classifies **every eCash-only coin as shared**, including
the output a split just created. Ask `/tx/{txid}` first for existence, then `/outspend` for
spentness — which is also cheaper, since an eCash-only coin resolves in one request.

`SplitVerdict::decide(exists, spent)` is pure and covers all four cases in tests, precisely
because the I/O is what kept the wrong logic untested.

### Endpoint

`--bitcoin <URL>` or `ECX_BITCOIN_ESPLORA_URL`, defaulting to `blockstream.info`. Only `/tx` and
`/outspend` are used, so a host that cannot serve `/scripthash` — mempool.space — is fine here,
unlike for wallet scanning.

The check is read-only HTTP: no key is touched, nothing is signed. It does reveal the account's
transaction ids to that operator, which is why it is a command rather than part of every scan.

---

## Coverage — what we might not see at all

Classification only rules on coins we found. These are the ways a coin stays invisible:

| Gap | State |
|---|---|
| **Accounts beyond the probe range** — default 3 indices per address type | `--accounts <N>` / GUI presets. Default 3 covers most users, not all |
| **Addresses beyond the gap limit** within a scanned account | `--gap <N>` / GUI presets |
| **Non-standard derivation paths** — anything not BIP44/49/84/86 | ❌ Not searched. A wallet using an unusual path is invisible |
| **A different seed** | Connect that device instead |

The first two are tunable. **An empty result means "nothing in what we searched", never "nothing
in your wallet"**, and both frontends say so when they find nothing, naming what was searched.

---

## What the splitter does with all this

`build_sweep` uses `drain_wallet`: **every** UTXO in the selected account, not a filtered subset.

That is deliberate. Sweeping everything is complete for the account it sweeps — afterwards every
coin in it is spent on eCash and the Bitcoin side is untouched — and it removes a whole class of
"we classified it wrong and left a shared coin behind" bug.

The cost is over-sweeping: coins already chain-specific or already separated get moved for a fee
that buys nothing. `ecx check` is what tells the user whether that is happening before they pay
for it. This matters most **after** a first split, when the destination account holds eCash-only
coins that a second run would sweep again pointlessly.

---

## Known gaps

1. **No caching.** Every `check` re-asks. The mobile app caches decided verdicts only — never
   `unverified`, since persisting that would make an outage look like a settled fact.
2. **No batching.** One or two requests per coin. `/address/{addr}/utxo` intersected against our
   set would be O(addresses) instead.
3. **The GUI has no check yet** — CLI only.
4. **Filtering the sweep by verdict is not implemented.** `check` informs; it does not change
   what `sign` sweeps.
