# 05 — Decision ledger

Append only. Entries are never edited — a later entry supersedes an earlier
one and says so. Every locked choice gets a row before the code that depends
on it.

Format: **ID · date · decision · what was rejected · why.**

---

## D-0001 · Rust is the only language

**Decision.** One language, enforced by a CI extension allowlist plus a build
script check, not by convention.

**Rejected.** A mixed system with a native hot layer. That is what the
predecessor repository was, and the boundary between the two runtimes cost
roughly 2.6 µs per emitted trade in marshalling and object construction — a
cost that only exists because the boundary exists.

**Why.** A rule that lives in a document is negotiated in every pull request.
A rule that fails CI is not. Gate 1 has no sympathetic exception: the file
ends in `.rs` or the build is red.

---

## D-0002 · Fixed-stride store, addressed by arithmetic

**Decision.** 64-byte header, 56-byte records, `ptr = base + 64 + i·56`.
Read-only mapping, positional writes.

**Rejected.** A columnar format. It is excellent for analytical scans and
wrong for this shape of work: the sweep reads a contiguous slice once and then
does billions of arithmetic operations against it. Decoding a batch to reach
one bar is cost with no return.

**Why.** Three machine operations to locate any bar, and no library between
the request and the bytes.

---

## D-0003 · The read mapping is read-only; writes use positional syscalls

**Decision.** Never map a store file writable.

**Rejected.** A writable mapping, which is the obvious way to append.

**Why.** A writable mapping that exhausts disk space raises **SIGBUS** — a
signal, asynchronous, catchable by no construct in any language. The
predecessor system halted cleanly on a full disk. Adopting a writable mapping
would have been a strict regression while looking like an optimisation. This
was found by an adversarial failure audit, not by reasoning, which is why the
audit is part of the process rather than a one-off.

---

## D-0004 · A commit counter published last

**Decision.** The header holds `n_valid`. Append writes data, fsyncs,
then publishes the counter with a release store.

**Rejected.** Treating file length as the record count.

**Why.** File length grows the instant the first byte lands. A reader between
that instant and the last byte of the record sees a half-written record made
of plausible integers. The counter makes the torn state unobservable rather
than unlikely.

---

## D-0005 · One checksum per 4 KiB block

**Decision.** A sidecar CRC file, one entry per block, verified on read.

**Rejected.** No checksum. Also rejected: a whole-file checksum.

**Why.** A flipped bit in a raw `i64` price yields a different, plausible
price. There is no parse to fail and no structure to violate — the corruption
is silent and permanent. A whole-file checksum would detect it but only at
O(file) cost, which is not affordable on a read path.

---

## D-0006 · There is no sweep depth parameter

**Decision.** The sweep request type carries no `k` field. Depth ends where
the frequent frontier empties.

**Rejected.** A depth parameter with a dynamic default.

**Why.** That is exactly what the predecessor had, and it failed silently: the
frontier mask was 64 bits against a 74-condition vocabulary, so every real run
tripped a width guard and fell back to a hardcoded `k = [1, 2]` with pruning
disabled. Dynamic depth was unreachable on the only vocabulary that existed,
and nothing in the flag revealed it. A parameter that can be set can be set
wrongly, silently, by a fallback nobody reads. Removing the field removes the
class.

---

## D-0007 · Condition bit indices are frozen

**Decision.** Append only. A retired condition keeps its bit forever as a
tombstone that evaluates false.

**Rejected.** Renumbering to keep the table tidy.

**Why.** A stored result is a set of bit positions. Renumbering makes every
historical result silently mean something else, with byte-identical storage
and no test capable of noticing.

---

## D-0008 · `u128` mask with 54 free bits

**Decision.** The mask is `u128`. 74 live, 54 free.

**Rejected.** `u64`, which fits today's count only if the vocabulary never
grows — and it already grew past 64 once, which is how D-0006's failure
happened.

**Why.** Headroom that costs 8 bytes per mask and removes an entire failure
class is not a trade-off worth debating.

---

## D-0009 · The browser crate depends on `core` alone

**Decision.** `crates/web` lists exactly one dependency and compiles to
`wasm32-unknown-unknown`. CI gate 7 enforces it.

**Rejected.** Letting the browser crate reuse `store` types directly.

**Why.** There is no filesystem in WebAssembly. Without the constraint the
failure surfaces as a runtime panic in a browser instead of a compile error on
a laptop. The payoff is that every display rule is compiled twice from one
source, so the server and the browser cannot disagree.

---

## D-0010 · Prices are paisa integers

**Decision.** `i64` paisa everywhere. Floats appear in no price, cost, or P&L.

**Rejected.** Decimal-typed prices, and float prices.

**Why.** Integers are exact, comparable, hashable, and free. The tick grid is
two decimal places, so paisa loses nothing. Snapping happens once, at the
ingest boundary, half-up.

---

## D-0011 · Enriched fields live in a sibling file

**Decision.** Overlay values get their own file, own version, own stride,
addressed by the same index.

**Rejected.** Widening the base record.

**Why.** The base stride must be constant forever, or the addressing rule
acquires a version check and stops being three machine operations.

---

## D-0012 · Credentials are read-only and never minted here

**Decision.** Parameter Store SecureStrings, region `ap-south-1`, read-only. A
stale token is re-read. If the re-read returns the same dead value, the pull
halts loudly.

**Rejected.** Minting a fresh token locally on an auth failure.

**Why.** The token is shared with another system. A local mint invalidates
theirs. Halting is correct; helpfulness here is a fault.

---

## D-0013 · 2026-08-01 · No literal credential path in a tracked file

**Decision.** This repository is public. No tracked file contains a real AWS
Parameter Store path. `CLAUDE.md` §8 and `docs/00-charter.md` §4 carry only the
generic shape `/<org>/<env>/<vendor>/<field>`.

`crates/pull` compiles in exactly two things: the path *template* and the
*field* names (`api-key`, `totp-secret`, `access-token`, `client-id`). It
compiles in no `org`, no `env`, and no `vendor` segment. Those three are read
at process start from an untracked local configuration file:

```
$HOME/.brutex/credentials.toml
```

That file holds **path segments only — never a secret value.** The secret is
still read from Parameter Store and from nowhere else, which leaves D-0012
untouched. A missing file, an unreadable file, a malformed file, or a missing
segment **halts the pull loudly and names which segment was absent.** There is
no default, no environment-variable fallback, and no prompt — a fallback here
would silently point a pull at the wrong account, which is D-0006's failure
class wearing different clothes.

CI gate 1c greps every tracked file for a concrete `/<segment>/<env>/…` shape
and fails the build on a hit, so the redaction cannot decay back.

**Rejected — literal `const` strings in `crates/pull`.** The task that raised
this decision described the real paths as "constants in `crates/pull`, read
from a local untracked config file, never committed". Those two clauses
contradict each other: `crates/pull` is tracked, so a constant in it is
published to a public repository, which is precisely what the redaction exists
to stop. The contradiction was raised with the operator before this entry was
written and resolved in favour of runtime resolution with zero literals.

**Rejected — an environment variable for the path.** It is invisible in a
process listing's absence, it is inherited by children, and CLAUDE.md §8
already rules the mechanism out for the value. Applying a weaker rule to the
path than to the secret is an inconsistency someone would eventually exploit.

**Rejected — committing the config with the segments redacted at review time.**
Redaction by review is the thing gate 1c replaces. A rule that lives in a
document is negotiated in every pull request; a rule that fails CI is not.

**Why.** A Parameter Store path names an account, an environment, and a vendor
relationship. On a public repository that is reconnaissance handed over for
free, and it cannot be retracted once cloned — the path stays in the git
history of every fork. The cost of the runtime read is one file open at
process start, which is not on any hot path measured by gate 8.

---

## D-0014 · 2026-08-01 · The calendar is transcribed data carrying an evidence lane per date

**Decision.** `crates/core` holds the NSE/BSE calendar as Rust literals with
**zero dependencies and no runtime fetch**. Every date carries the lane it was
verified at — `Verified`, `Secondary`, or `Unverified` — and the lane is a
value in the type, not a comment. A sweep window that crosses an `Unverified`
date reports the fact rather than absorbing it.

This supersedes the `docs/00-charter.md` §3 row that listed the special
weekend sessions as 2021-01-30 and 2021-02-01. Both are wrong. 2021-01-30 has
zero bars in the lake and appears in no predecessor source; 2021-02-01 was an
ordinary Monday with a full 375-bar session, so it is a normal trading day and
not a special one. The correct and complete set is **2020-02-01, 2025-02-01,
2026-02-01** — the only three years since 2020 in which 1 February fell on a
weekend. All three show exactly 375 bars.

**Rejected — fetching the calendar from the exchange at runtime.** It makes
`core` depend on the network, which would break its zero-dependency rule and
make a sweep non-reproducible: the same run on two days could load two
calendars and produce two different `data_digest` values from identical bars.
The predecessor already proved the fetch unreliable — nineteen consecutive
attempts to read the 2026 circular returned HTTP 403.

**Rejected — inheriting the predecessor's holiday sets as verified.** They are
demonstrably incomplete. Four dates have zero bars across all three engine
instruments and appear in no holiday set, and a fifth is mis-dated by one day.
Copying them over with the `verified` label would promote an evidence lane
during a copy, which `docs/00-charter.md` §4 forbids in as many words.

**Rejected — silently adding the four disputed dates as holidays.** The lake
is strong evidence and it is not a circular. Golden Rule 1 says an unverified
fact is written `UNVERIFIED` and stopped at, not quietly promoted because it
looks right. They are carried in `docs/06-limits.md` §9 with their evidence.

**Why a lane per date rather than one lane for the file.** The 2020–2025
dates are individually checkable against bars; the whole of 2026 is
secondary-sourced and one of its dates is already disputed by data. A single
file-level lane would have to take the worst case and would mark 2024 as
unreliable as 2026, which is false and would make the field useless. The lane
is the finest granularity that is honest.

---

## D-0015 · 2026-08-01 · Minute bars only until a minute-level result earns the upgrade

**Decision.** The engine ingests **1-minute bars from the two brokers only**.
Second-level data, tick data, bid/ask, and the two paid CSV vendors are
**deferred until a minute-level sweep has produced a profitable result**. The
seams that make them cheap to add are built now; the data is not bought now.

**Rejected — buying the second-level data first.** Two live quotes exist:
₹1,15,050 for 6.5 years of NIFTY F&O at 1-second, and ₹3,04,787 for 7 years of
NSE F&O. Neither is recoverable if the minute-level hypothesis does not hold,
and nothing measured so far says it does — `docs/06-limits.md` §4 still records
that **a full production sweep has never been run**. Paying before that
sentence is replaced by a measurement is buying on an extrapolation.

**Rejected — deferring the seams as well.** The retrofit cost is what makes a
deferral expensive, so the seams are load-bearing today:

| Seam | Built now | Cost to switch on later |
|---|---|---|
| `timeframe_secs` is a `u32` of **seconds** | already the store header field; `1` is a legal value | none — no format change, no migration |
| Path is the index | `bars/<exch>/<seg>/<sym>/<tf>/<yyyy-mm>.bin`; `<tf>` becomes `1sec` | none — the first write creates the directory |
| Bid/ask/greeks live in the `.ovl` sibling | D-0011 already forbids widening the base record | none — own version, own stride, same index *i* |
| Per-vendor decode behind one seam | the two brokers already disagree (rows vs parallel columns), so the seam is forced to exist | one implementation per vendor |

**What this defers, and why that is a relief.** Fixed-stride addressing is O(1)
only on a **dense** grid. At 1-second there are 22,500 slots per session, so a
dense grid costs ~1.26 MB per instrument per day — roughly **1.5 TB for NIFTY
options alone** over 6.5 years, against ~100 GB sparse. Sparse storage turns
the timestamp→bar lookup into a search and breaks the repository's central
guarantee. That choice is genuinely hard, it is unavoidable at second-level,
and it does not arise at all at minute-level. Deferring the data defers the
dilemma honestly rather than pre-committing to an answer.

**Vendor facts captured now so they are not re-derived later** (evidence lane
attached; none of this is in the engine surface):

| Fact | Value | Lane |
|---|---|---|
| GDFL CSV columns | `Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest`, header present, date `DD/MM/YYYY` | verified from a sample file |
| TrueData CSV columns | `YYYYMMDD, HH:MM:SS, LTP, Volume, OpenInterest, Bid, BidQty, Ask, AskQty` — **no header row** | verified from the vendor's own email |
| Column order differs | GDFL puts bid/ask **before** volume/OI; TrueData **after**. A positional reader silently swaps Open Interest with a bid price. | verified |
| TrueData row identity | **none** — the instrument is the *filename* only | verified from a sample |
| GDFL websocket identity | `OPTSTK_SBIN_28OCT2025_CE_860`, `OPTIDX_NIFTYNXT50_28OCT2025_CE_61900` | verified from vendor documentation |
| GDFL CSV identity | `NIFTY03JUL2522800CE.NFO` | verified from a sample |
| Naming conventions in play | **five, all mutually incompatible** — Groww symbol, Dhan numeric id, GDFL CSV, GDFL websocket, TrueData filename | verified |
| GDFL epoch fields | `LastTradeTime` / `ServerTime` are epoch **seconds** | documented |
| Excluded from the GDFL quote | Option Chain, Option Greeks | quoted |
| TrueData licence | forbids forwarding, resale, and commercial use | quoted |

**Why five naming conventions is the real reason the mapping layer exists.**
One canonical `InstrumentKey` that every vendor resolves *to* is not
architectural taste; with five incompatible spellings of the same contract, a
shared identity is the only thing that makes deduplication meaningful across
sources. That layer is built at minute-level, where there are only two
spellings to reconcile, and it is the thing that makes vendors three and four
cheap.

---

## How to add an entry

Next free ID, today's date, the decision in one sentence, the alternative
rejected, and the reason. If you cannot name what was rejected, the decision
is not yet made — it is a default, and defaults do not belong in this file.
