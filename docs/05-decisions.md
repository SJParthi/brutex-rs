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

## D-0016 · 2026-08-01 · Track the latest stable toolchain, not a frozen one

**Decision.** `rust-toolchain.toml` moves from **1.85.0 to 1.97.1** — current
stable — and CI tooling installs its latest release rather than a pinned old
one. The toolchain is still checked in and still identical on every machine and
in CI; what changes is that it is kept current instead of left to age.

**Rejected — keeping 1.85.0.** It was pinned in February 2025 and had become
the binding constraint on everything else. Concretely, gate 3 could not run at
all:

1. `cargo-deny ^0.16` failed to *parse* the RUSTSEC advisory database:
   `RUSTSEC-2026-0109.md` uses TOML front-matter it rejects.
2. `cargo-deny 0.18.9` fixes that, but requires rustc **1.88.0**.
3. `cargo-deny 0.18.3` installs on 1.85.0 — and still cannot parse the
   database.

There was no version of the tool that both installed on the pinned toolchain
and did its job. The gate was not misconfigured; it was **impossible**. A
security gate that cannot run is worse than an absent one, because red starts
to mean "the tool is broken again" rather than "something is wrong".

**Rejected — floating `channel = "stable"`.** Reproducibility requires that
two machines resolve the same compiler. A named version keeps that guarantee;
only the number moves, and moving it stays a decision recorded here.

**Verified before landing, not assumed.** On 1.97.1: `cargo fmt --check`
clean, `cargo clippy --workspace --all-targets -- -D warnings` clean, 14 tests
passing. `edition = "2024"` and `resolver = "3"` are unaffected;
`rust-version` moves to 1.97 to match.

**The general rule this sets.** Pinning is for *reproducibility*, not for
*avoidance*. A pin that is never advanced silently becomes a ceiling on tools,
lints and advisories — and the failure surfaces far from its cause, as it did
here: three separate commits chased a symptom in `deny.toml` and in the tool
version before the toolchain itself turned out to be the constraint.

---

## D-0017 · 2026-08-01 · NSE only; the swept surface narrows from three to two

**Decision.** The engine sweeps **`NSE-NIFTY` and `NSE-BANKNIFTY`**. BSE and
MCX are not pulled. `BSE-SENSEX` is no longer swept.

**Rejected — keeping `BSE-SENSEX`.** It is the shortest series by a wide
margin: the lake holds SENSEX from **2022-09-01** against 2020-01 for the two
NSE indices. Any three-instrument result is therefore silently capped at the
shortest history, and a window chosen to include SENSEX throws away 2 years 8
months of NIFTY and BANKNIFTY data without saying so. Dropping it removes the
cap rather than working around it.

**Rejected — deleting BSE data already on disk.** Append-only history applies
to the store, not only to condition bits. Existing SENSEX bars stay; the
engine simply will not sweep them, and `is_sweepable` says so in one place.

**Why this is a narrowing and still needs an entry.** Golden rule 2 previously
said the surface "does not widen" without a ledger entry. That was a gap: a
*narrowing* silently changes every historical comparison just as much, because
a result set produced over three instruments is not comparable with one
produced over two. The rule now reads "widen OR narrow", and this entry is the
first use of it.

---

## D-0018 · 2026-08-01 · Store every NSE instrument; sweep two until one earns the widening

**Decision.** `pull` fetches and stores **every NSE instrument** — 12,460 cash
and 78,163 F&O rows in the vendor's own instrument master, 90,623 in total.
The **sweep** stays at the two indices until a two-instrument sweep produces a
profitable result.

**Rejected — sweeping all NSE equities now.** Total sweep work is linear in
symbols: 2 → ~750 is **375× the compute**, paid before anything has been shown
to work. `docs/06-limits.md` §4 still records that a full production sweep has
never been run. The same discipline as D-0015: build the capability, defer the
spend.

**Rejected — storing only the two indices.** Storage is cheap and re-pulling
is not. With every NSE instrument on disk, widening the sweep later is one
ledger entry and **zero re-pulling**. Pulling narrowly now would make the
widening a multi-hour vendor-bound operation instead of a config change.

**What widening will require, and is not solved today.** Indices never split;
equities do. An unadjusted 1:5 split is a **fake 80% overnight crash**, and
every condition fires on it — `gap_down_day`, `bar_bearish`,
`close_below_ema200`, the whole bar-shape family. There is no corporate-action
handling anywhere in this design, and the lake survey never looked for one
because indices do not need it.

The offsetting gain, recorded so it is not forgotten: equities carry **real
traded volume**, so VWAP bits 52–53 stop abstaining. On the index surface they
are permanently dead by construction.

**Behaviour when a corporate action is met: refuse the window, loudly.** A
suspected split or bonus — an unexplained overnight gap beyond a threshold —
causes the sweep to **refuse that window and name the date**, rather than
producing a result built on a fake crash. This follows
`docs/00-charter.md` prohibition 6: degrade loudly and name the reason, or
refuse; never silently.

**Rejected — back-adjusting prices from a corporate-action feed.** It is what
charting vendors do and it is the eventual right answer, but none of the four
vendors has been *verified* to supply split and bonus records. Building on an
unverified source would put a fabricated price into the store, which is worse
than refusing. Upgrading later invalidates no stored bar, because raw prices
are what is stored.

**Rejected — sweeping raw prices and ignoring the problem.** Cheapest, and it
contradicts prohibition 6 outright. A spurious signal that looks real is the
failure mode this repository is organised against.

---

## D-0019 · 2026-08-01 · The vendor is the first path segment

**Decision.** Every vendor gets its own complete series. The store path gains
a vendor segment at the front:

```
bars/<vendor>/<exchange>/<segment>/<symbol>/<timeframe>/<yyyy-mm>.bin
bars/groww/NSE/INDEX/NIFTY/1min/2024-06.bin
bars/dhan/NSE/INDEX/NIFTY/1min/2024-06.bin
```

A vendor can be added, re-pulled, or deleted by touching one directory and
nothing else. No merge step, no precedence rule, no migration of anyone
else's data.

**Rejected — one canonical series with a provenance overlay.** Half the
storage, and it still records where each bar came from. But it needs a
precedence rule decided *before* any evidence exists about which vendor is
more accurate, and once merged a vendor cannot be cleanly removed — its bars
are already interleaved with everyone else's.

**Rejected — one series, first writer wins, no provenance.** Cheapest, and it
makes a vendor disagreement **invisible**. Two vendors handing over different
prices for the same minute is exactly the kind of silent failure charter
prohibition 6 exists to forbid.

**Why the cost is acceptable.** A doubled series doubles storage: NIFTY at
1-minute over 6.5 years is 34 MB, so two vendors is 68 MB. That is not a
number worth trading a correctness property for.

**Why O(1) is unaffected.** The path is still the index — one more string
segment in a join that was already a join. Locating a slice remains a path
construction and an open; no catalogue, no scan, no lookup. Adding a fifth
vendor creates a directory and changes no code.

---

## D-0020 · 2026-08-01 · A vendor disagreement refuses the window and names it

**Decision.** Both vendors' bars stay on disk. A cross-verification pass diffs
the two series bar for bar; any mismatch beyond the tick grid is reported
**loudly, with the exact timestamp and both values**, and the sweep **refuses
that window** rather than silently choosing a side.

**Rejected — primary vendor wins, log the difference.** Never blocks a run,
which is precisely the problem: it lets a sweep produce a ranked result over
data that another vendor disputes, and the dispute survives only as a log line
nobody reads. A result that looks clean but rests on contested prices is worse
than no result.

**Rejected — deciding the rule later.** Tempting, because there is no
measurement yet of how often the two vendors actually disagree. But the rule
has to exist before the first sweep runs, and "we will decide when it happens"
resolves in practice to whatever the code happens to do.

**Why refusal rather than repair.** There is no principled way to pick a
winner without external evidence. Both vendors claim to relay the same
exchange feed, so a disagreement means at least one is wrong and nothing on
this machine can say which. Refusing names the problem; picking hides it.

**Consistent with what is already decided.** This is the same shape as
D-0018's corporate-action rule — refuse the affected window and name the date
— and the same shape as `Paisa::from_rupees_half_up` refusing a non-finite
price rather than substituting one. A refusal is a fact; a substitution is a
fabrication.

---

## D-0021 · 2026-08-01 · The lake is the source of F&O history; neither broker is

**Decision.** Expired option and future history comes from the **existing lake
on local disk**. Vendor APIs backfill only what the lake is missing. No
historical F&O data is purchased.

**Measured, not assumed** — a full survey of `~/.brutex/lake/bars/NSE/FNO`:

| | |
|---|---|
| Contract directories | **116,086** — 115,927 options, 159 futures |
| NIFTY option contracts | **60,996**, expiring **2020-01-02 → 2026-08-25** |
| BANKNIFTY options | 54,931 |
| Underlyings | NIFTY and BANKNIFTY only — no single stocks |
| On disk | **18 GB**, 196,954 parquet files, **zero empty directories** |
| **Absent from BOTH live vendor masters** | **115,272 — 99.3%** |

Both brokers purge on expiry: the earliest index-option expiry in either live
master is **2026-08-04**, three days after this measurement. Relying on the
masters alone would lose 99.3% of the history that is already owned.

**Contract identity is preserved independently of the directory names.**
`~/.brutex/lake/registry.duckdb` holds a `contracts` table of **169,530 rows**
(`exch, underlying, expiry_date, contract_symbol, strike, side,
candle_status`), spanning NIFTY 2020-01-02 → 2026-12-29. **No on-disk
directory is unknown to the registry.**

**Option bars already carry computed greeks** — `iv, delta, gamma, theta,
vega, rho, spot_at_bar, t_years_used, rate_used, greeks_provenance_id` — 17
columns in total. The tick vendor quoted for this data **excludes greeks
explicitly**.

**Rejected — buying historical F&O.** Two quotes exist, ₹1,15,050 and
₹3,04,787, for a superset of data already on disk, minus the greeks.

**Rejected — refetching from the brokers.** 116,086 contracts against a rate
limit, to reproduce what is already local.

**The one real gap.** The registry knows **~15,900 contracts, almost entirely
expiry-year 2024**, that have no bars on disk. That — and only that — is what
a vendor backfill is for.

---

## D-0022 · 2026-08-01 · The converter is a numeric boundary, not a copy

**Decision.** The lake→store converter **converts**; it does not transfer
bytes. Two transformations are mandatory and each is a correctness boundary:

| Lake | Store |
|---|---|
| prices as `double` | **`i64` paisa** — `CLAUDE.md` §7 |
| `timestamp` INT64 µs **UTC** | same, but IST is UTC + 19,800 s at every display |

**Why this is recorded rather than assumed.** The lake stores option OHLC as
IEEE doubles. `CLAUDE.md` §7 says prices are paisa integers and never a float.
A converter written as a copy would carry doubles into a store whose entire
addressing and comparison model assumes integers. The spot survey measured the
maximum deviation of `x·100` from an integer at **9.3 × 10⁻¹⁰ paisa**, so the
conversion is exact — but it must actually happen.

**Rejected — widening the store record to hold a float.** D-0002 and D-0011
fix the 56-byte stride permanently.

---

## D-0023 · 2026-08-01 · Verified vendor capability for expired contracts

**Decision.** Recorded as verified external facts, with the route that
verified each, because two earlier claims in this session were wrong and were
corrected only by going to the live source.

| Fact | Route | Lane |
|---|---|---|
| Groww `/v1/historical/{expiries,contracts,candles}` exist; FNO **"available from 2020"**; `year` accepts **"2020 - current year"**; `groww_symbol` is constructible as Exchange·Symbol·`DDMmmYY`·Strike·`CE/PE/FUT`; 1-minute window **30 days** | `curl` on `groww.in`, server-rendered, three URLs with three distinct hashes and 24 content markers | verified |
| The Groww endpoint is live | `curl` → **`401 "Missing token in request."`** — not a 404 | verified |
| Dhan `/v2/charts/rollingoption`: **45 days** per call, intervals `1/5/15/30/60`, strike enum `ATM, ATM+10, ATM-10`, **last 5 years rolling**, index **and** stock options, returns IV/OI/spot | **live browser** on `docs.dhanhq.co` | verified |
| **Dhan cannot reach 2020-01.** Five years *rolling* puts the floor at ~2021-08 and moving | live browser | verified |
| Dhan has **no expired-futures endpoint** | live browser | verified |
| Rate limits, either vendor · Dhan Data-API pricing · any behaviour with a real token | — | **UNVERIFIED** |

**The method matters, and is part of the decision.** Dhan's documentation is a
JavaScript application: three different URLs return one byte-identical 33,884
byte shell to `curl`, containing **zero** content markers. Reading it without a
browser produced a mangled capture from which a **non-existent `ATM±3` rule**
was reconstructed and reported as fact. Groww's documentation is
server-rendered and `curl` receives it intact — but `groww.in` is blocked in
both browsers by policy, so `curl` is the only route available there.

**No vendor claim enters this repository without naming the route that
produced it.**

---

## D-0024 · 2026-08-01 · The equity gate is the series column; the ISIN is a cross-check, not a key

**Decision, part one — what counts as an equity is read from the vendor's own
class column, per vendor, and everything else on the equity segment is
declined with a named reason.**

`INSTRUMENT = EQUITY` does not mean "a share". It means "trades on the equity
segment". Measured on the real masters this session:

| Vendor | Column read | Kept | Declined |
|---|---|---|---|
| Groww | `series` | `EQ` 2,407 · `BE` 289 | ~100 debt/fund series codes, plus `SM` 401 and `ST` 157 as SME |
| Dhan | `INSTRUMENT_TYPE` (**trimmed** — the values are space padded) | `ES` 2,974 · `ETF` 388 | `DBT` 4,416 · `DEB` 1,429 · `Other` 141 · `TB` 81 · `GB` 78 · `CB` 72 · `MF` 66 · `InvITU` 21 · `REIT` 6 · `PTC` 1 · `PS` 1 |

Of Dhan's 9,674 NSE equity-segment rows, **6,312 are not equity at all**.

**Why this is a correctness fix and not tidying.** Those rows do not merely
inflate a count — they **capture the ticker**. Dhan line 167146 is
`NSE,E,19257,INE121A08PJ0,EQUITY,,CHOLAFIN,…,DEB,D1,…,5.0`, a 7.5% NCD, and it
appears **before** line 171414, `…,INE121A01024,EQUITY,,CHOLAFIN,…,ES,EQ,…,10.0`,
the share. Insert-if-absent therefore resolved `CHOLAFIN` to the bond and took
its tick size, silently and dependent on nothing but file order. `MOTHERSON`
(`INE775A08105`, an NCD) and `ELECTCAST` (`INE086A13016`, a warrant) went the
same way. All three are NIFTY Total Market members; two are F&O underlyings.
After the gate, duplicate tickers in Dhan's equity segment fall from **4 to 0**,
and all three resolve to the share's own ISIN.

**The gate is applied only to a row that already decoded as cash equity.** An
index row has no class — Groww leaves `series` empty on all 24 of its NSE index
rows and writes the *ticker* in the `isin` column; Dhan writes `NA` in both.
Gating any earlier declines `NIFTY` and `BANKNIFTY`, which is the entire
engine surface.

**Rejected — one flat table of accepted codes across both vendors.** The two
alphabets are disjoint, and a flat table invites one vendor's code to be
silently accepted for the other. Same reasoning as `Vendor::segment_of`.

**Rejected — erroring on an unrecognised class.** Unlike a segment or an
instrument type, this alphabet is open-ended: NSE mints a new debt series
whenever it needs one and a hundred already exist. An unknown code is declined
and **counted under its reason**; what never happens is an unknown code being
accepted.

**SME is a separate reason, and the vendors are asymmetric about it.** An SME
listing IS a share, so filing it with the debentures would hide a real choice
behind a wrong label. It is declined because the equity universe is F&O
underlyings plus NIFTY Total Market, and neither contains an SME listing.
Groww marks the board in `series` (`SM`, `ST`); **Dhan's `INSTRUMENT_TYPE` does
not distinguish it** — an SME share is `ES` there — so Dhan keeps SME rows that
Groww declines. That asymmetry is recorded in `docs/06-limits.md` §10 rather
than papered over by reading a second Dhan column on a guess.

---

**Decision, part two — the ISIN sits BESIDE the instrument key and is never a
field of it.**

`InstrumentKey` derives `Hash` and `Eq` over every field and is used directly
as a `HashMap` key; that collision **is** the deduplication (D-0015). If the
ISIN were a field, two vendors disagreeing about one instrument's ISIN would
produce **two different keys**, the merge map would hold it twice, and the
disagreement would never be seen by anyone. Adding the field that was supposed
to make identity more precise would silently split it.

Beside the key, the same disagreement is one loud line naming the key and both
ISINs — the shape D-0020 already requires of every vendor disagreement.
Measured: across the 4,080 ISINs the two masters share there are **zero**
conflicts today, and all 750 NIFTY Total Market members resolve in both vendors
with agreeing ISINs. The check costs nothing until the day it does not.

**Rejected — keying on the ISIN instead of the symbol.** Indices have no ISIN
at all, so the two instruments the engine sweeps could not be keyed. Inventing
a sentinel ISIN for `NIFTY` was rejected for the reason every sentinel is
rejected here: it is a fabrication that reads like a fact.

**Rejected — the exchange token as the cross-check.** It is not time-stable.
NSE issues a token per (symbol, series), so an `EQ`→`BE` migration mints a new
one — `SICALLOG` went 19434 → 19440 between the two snapshots. Exactly the 22
of 4,080 rows whose series differ also differ in token, while the ISIN held
fixed across all 4,080.

**The check digit is verified, not assumed.** Exactly one row in either master
fails it — `IN1520250085`, a state development loan — and the equity gate
declines it before an ISIN is ever parsed. The parse therefore happens **after**
the gate, and that order is itself tested.

---

**Decision, part three — Groww's series suffix is stripped only when two
independent things agree.**

Groww leaks `internal_trading_symbol` into `trading_symbol` on exactly **209**
of the 4,080 shared ISINs, and the rule is exact with zero residual:
`groww.trading_symbol == dhan.UNDERLYING_SYMBOL + "-" + series`. It reaches
tradeable equity and ETFs — `BLUECHIP-BE`, `CBAZAAR-ST`, `HDFCLIQUID-EQ`,
`LOWVOL-EQ` — so "only debt is suffixed" is **false**.

The strip requires the trailing segment to be the row's **own series** (decided
in `core`, where the series is) **and** some vendor to have asserted the
stripped identity under the **same ISIN** (decided in `api`, where both vendors
are). Where both do not hold, the symbol stands exactly as the vendor wrote it.

**Rejected — stripping a trailing `-XX` wherever it appears.** `BAJAJ-AUTO`
and `NAM-INDIA` are real tickers. `crates/core/src/symbol.rs` already argues
that blind normalisation manufactures the collision it exists to prevent, and
an unmerged row is a visible duplicate while a wrongly merged one is two
instruments silently becoming one.

---

**Consequence, measured before and after on the real masters:**

| | kept | declined | unreadable |
|---|---|---|---|
| groww, before | 4,104 | 129,274 | 0 |
| groww, after | **2,720** | 130,658 (+558 SME, +826 not equity) | 0 |
| dhan, before | 9,667 | 190,689 | 104 |
| dhan, after | **3,377** | 196,979 (+6,290 not equity) | 104 |

Merged: **3,400** instruments, **0** ISIN conflicts. The unreadable counts are
unchanged, which is the check that requiring an ISIN on a kept equity added no
new failures.

> **Superseded in part by D-0025.** The column D-0024 chose for Dhan —
> `INSTRUMENT_TYPE` — was measurably wrong on 57 rows, and the per-vendor
> alphabet it argued for stopped being two alphabets once both vendors were
> pointed at the NSE series. The *shape* of D-0024 stands: gate the equity
> segment, apply it only to a row already decoded as cash equity, keep SME
> under its own reason, and hold the ISIN beside the key. Only the column and
> the catch-all changed. Nothing in this entry is edited; D-0025 records what
> replaced it and why.

---

## D-0025 · 2026-08-01 · The equity gate reads the NSE board series, from both vendors, against a measured table

**Decision — the gate reads one exchange-issued fact, `series` at Groww and
`SERIES` at Dhan, and every code it does not recognise is its own loud
outcome.**

D-0024 read Groww's NSE `series` and Dhan's own `INSTRUMENT_TYPE`. An
adversarial review found both arms wrong at an edge, and both wrongnesses had
one cause: `INSTRUMENT_TYPE` is minted by a broker, and the code was trusting
one vendor column per vendor with nothing to check it against.

**What Dhan's paper class got wrong.** Measured by joining the two masters on
ISIN:

| Dhan `INSTRUMENT_TYPE` | Dhan's own `SERIES` | Rows | What they really are |
|---|---|---:|---|
| `ETF` | `MF` | 54 | Franklin `FISTIP*`/`FICRF*`, PGIM `PGIMCSA*`, Bandhan `BPF0*` — open-ended fund plans, not ETFs. Groww carries 29 of the ISINs under `series=MF` and declines them. |
| `Other` | `EQ` | 2 | `IVZINNIFTY` (Invesco India Nifty ETF) and `NARMADA` (Narmada Agrobase Ltd) — real listings Groww keeps. |
| `MF` | `EQ` | 1 | `INFRABEES` (Nippon India ETF Infra BeES) — a genuine ETF. |

Every one of those 57 rows is a case where the two columns on the **same Dhan
row** disagree, and the series is right on all 57. Under D-0024 the 54 fund
plans entered the equity universe as `Kind::Equity` — the exact category
`Skip::NotEquityListing`'s own text says it exists to remove.

**What the Groww arm got wrong.** It accepted `EQ` and `BE` only, so 30 genuine
equity listings were declined and *counted under the reason "not an equity
listing"*, which was false about them:

| Series | Rows (g/d) | What it is |
|---|---:|---|
| `BZ` | 25 / 38 | trade-for-trade under surveillance — `HDIL`, `RAJESHEXPO`, `IL&FSENGG`, `ANSALAPI`, `FEL`, `ARSHIYA` |
| `IT` | 2 / 2 | trade-for-trade, illiquid |
| `SZ` | 1 / 2 | trade-for-trade, surveillance (second list) |
| `E1` | 2 / 3 | partly-paid equity |

Three independent confirmations that these are equity: the NSDL security-type
digits of the ISIN are `01` on 27 of the 30 and `IN9…` (partly paid) on the
other 3, against `08` for the `CHOLAFIN` NCD and `13` for the `ELECTCAST`
warrant the gate still declines; Dhan classes all 30 as `ES`; and they are
ordinary listed companies.

**The measurement that made one table legitimate.** `docs/06-limits.md` §10
said the Dhan/Groww asymmetry could only be closed by "measuring Dhan's
`SERIES` alphabet first and recording the result — not adding the column
because the numbers would look tidier". That measurement was taken:

| Dhan `INSTRUMENT_TYPE` | The `SERIES` values on those rows |
|---|---|
| `ES` (2,974) | `EQ` 2,079 · `BE` 291 · `SM` 414 · `ST` 145 · `BZ` 38 · `E1` 3 · `IT` 2 · `SZ` 2 — **nothing else** |
| `ETF` (388) | `EQ` 334 · `MF` 54 |
| every debt class | only debt series (`SG`, `GS`, `N0`…`NZ`, `Y*`, `Z*`, `D1`, `W1`, `TB`, …) |

Dhan's `SERIES` is the same NSE alphabet as Groww's `series`, and no
equity-segment row in either file leaves it empty. On the 4,080 ISINs the two
masters share, the two series columns disagree on **22** rows — all snapshot
skew inside one board, `EQ`↔`BE` or `SM`↔`ST`, e.g. `SICALLOG` — and the
**board verdict differs on 0**. One exchange fact, carried by both vendors,
agreeing everywhere.

**Rejected — keeping the per-vendor table.** D-0024 argued a flat table
"invites one vendor's code to be silently accepted for the other". True of two
broker alphabets; false of one exchange alphabet carried twice. Two copies of
one fact are free to drift, and the drift is exactly what produced the 57
errors above.

**Rejected — reading both columns and refusing where they disagree.** It would
decline `INFRABEES`, `IVZINNIFTY` and `NARMADA`, which are real, and it would
emit 57 conflict lines every run for a column already known to be the wrong
one. A check against a source measured to be wrong is noise, not evidence.

**Rejected — an `INF` issuer prefix as the fund test.** `HDFCLIQUID`
(`INF179KC1JG3`) is a genuine ETF with an `INF` prefix. The rule would have
been wrong in both directions.

**Decision — an unrecognised series is `Skip::UnrecognisedListingClass`, not
`Skip::NotEquityListing`.**

Both arms of D-0024's gate ended in `_ => NotEquity`, so a code the engine had
never seen was reported in the identical words a routine debenture gets.
Demonstrated on the real Dhan master by rewriting `EQ` to `EQX`: **2,438 shares
vanished**, every F&O underlying among them, the report still printed `ok`, and
the exit code was still 0. That is the failure `Vendor::segment_of` is
documented as making unrepeatable, reproduced in a different column.

It stays a *decline* and not an error — NSE mints a debt series whenever it
needs one, and turning a routine bond listing into a failed ingest would be the
opposite mistake. But it is its own decline: its own reason string, its own
counter, **the offending code itself** carried to the operator
(`api::master::Loaded::unrecognised`), and a non-zero exit (D-0026). The
120-code `NON_EQUITY_SERIES` table is what makes "unrecognised" meaningful; it
is the measured union of both masters, so a new NSE debt series is a one-line
append.

**Rejected — erroring on an unknown code**, as `segment_of` and `type_of` do.
Those alphabets are closed and tiny; this one is open-ended with 120 members
already. An error per bond row would make the unreadable count meaningless.

**Consequence, measured on the real masters:**

| | kept | declined | unreadable |
|---|---|---|---|
| groww, D-0024 | 2,720 | 130,658 | 0 |
| groww, D-0025 | **2,750** | 130,628 (SME 558 · not equity 796) | 0 |
| dhan, D-0024 | 3,377 | 196,979 | 104 |
| dhan, D-0025 | **2,767** | 197,589 (SME 559 · not equity 6,341) | 104 |

Merged **2,787** instruments · **0** ISIN conflicts · **0** eligibility
conflicts. Dhan falls by 610 (54 fund plans + 559 SME, less the 3 real
listings its paper class had wrongly declined); Groww rises by 30 (`BZ`, `IT`,
`SZ`, `E1`). The SME decline is now symmetric — 558 at Groww and 559 at Dhan,
the same paper by ISIN — which is what §10 recorded as unclosed.

All **750** NIFTY Total Market members and **208** of the 213 F&O underlyings
resolve as a kept equity in **both** vendors; the other five are indices. See
D-0027 for the two that only one vendor names.

---

## D-0026 · 2026-08-01 · A disagreement or a missing vendor refuses the universe, and the exit code says so

**Decision — `report` prints `DEGRADED`, `run` exits 3, and `/health` answers
503 whenever a vendor was never read, two vendors disagreed, or a listing class
was not recognised.**

The code and this ledger both claimed a conflict was a refusal — `merge.rs`
said a non-empty conflicts vector "is a refusal to believe the merge, not a
warning to be scrolled past"; `isin.rs` called it "a single loud refusal …
which is what D-0020 already requires"; D-0024 called it "the shape D-0020
already requires of every vendor disagreement". Nothing refused. `report`
emitted an unconditional `ok`, `run` returned 0 for every completed report, and
`/health` answered 200 with that body. A monitor checking the exit code, the
HTTP status or the first line saw green while one of the two masters had never
been opened.

D-0020 is titled "A vendor disagreement refuses the window and names it" and
requires naming **and** refusing. The old behaviour named and continued, which
is the "primary vendor wins, log the difference" shape D-0020 exists to reject.
Report-and-continue may well be right for a master merge — but then it is a new
locked choice needing its own honest text, not an assertion of conformity with
a decision that says the opposite. This is that text.

**What is refused, and what is not.** The disputed instruments stay in the map
and on the page. Dropping them would hide the disagreement, which is the
failure `CLAUDE.md` §4 forbids. What is refused is the *universe as a whole*:
`merge::Merged::verdict` returns `Disputed`, `server::Read::is_clean` is false,
and no caller can turn that into a success.

**Exit code 3, not 1.** `FAILED` means "I tried and could not". Here the work
completed and the output is real; it is the answer that must not be trusted.
Distinct codes let a monitor tell a crashed process from a half-read universe.

**Rejected — refusing to print anything.** The tallies are exactly what an
operator needs in order to find out *why* the read is degraded. Refusing to
emit them would trade one silent failure for another.

**Rejected — degrading on the unchecked index identity of D-0027.** It is a
permanent structural fact, not a change, so every run would be `DEGRADED` and
the signal would mean nothing. It is named loudly on every run instead.

---

## D-0027 · 2026-08-01 · Index identity is unchecked, and the report says which members rest on one vendor

**Decision — no alias table is invented for the indices the two vendors spell
differently; the report names every universe member only one vendor asserted.**

An index carries no ISIN, so the cross-check D-0024 added cannot reach it.
Measured: of 35 merged NSE index keys, **only four are spelled identically by
both masters** — `NIFTY`, `BANKNIFTY`, `FINNIFTY`, `NIFTYIT`. Two of the
mismatches are F&O underlyings:

| Groww | Dhan |
|---|---|
| `NIFTYJR` | `NIFTYNXT50` |
| `NIFTYMIDSELECT` | `MIDCPNIFTY` |
| `MIDCAP50` | `NIFTYMCAP50` |

The merged universe therefore holds each of these **twice**, as two
single-vendor instruments. 211 of the 213 F&O underlyings are confirmed by both
vendors; 2 are not.

**Rejected — an alias table mapping `NIFTYJR` to `NIFTYNXT50`.** The evidence
is suggestive — both vendors' *derivative* rows use the Dhan spelling — but it
is an inference, and `crates/core/src/symbol.rs` argues at length that blind
normalisation manufactures the collision it exists to prevent. Merging two
instruments on a guess is precisely the failure the ISIN cross-check exists to
catch; doing it where no cross-check is possible would be worse, not better.

**Rejected — leaving it implied.** The previous behaviour reported `0 isin
conflicts` and said nothing, so a clean-looking report was the only evidence
either way. The report now prints, on every run, how many members of each
universe resolved and how many **two vendors confirmed**, followed by an
`UNCHECKED IDENTITY` line naming each single-vendor member. `docs/06-limits.md`
§12 records the limit.

Neither swept instrument is affected: `NSE-NIFTY` and `NSE-BANKNIFTY` are named
identically by both vendors and carry two vendor tags.

---

## D-0028 · 2026-08-01 · `.claude/` is ignored, not tracked

**Decision — `/.claude/` and `/mutants.out*` are added to `.gitignore`.**

`.claude/launch.json` was in the working tree and matched no ignore rule, so
`git check-ignore` exited 1 and the next `git add -A` would have tracked it.
`.json` is not in `CLAUDE.md` §2's allowed extension list at all, and CI gate
1b confines `.json` and `.yml` to `.github/` and `crates/web/` — so the commit
would have failed the build at gate 1b, naming the file. Its sibling
`settings.local.json` was safe only by accident, through a rule in the
operator's personal `~/.config/git/ignore` that no clone of this repository
carries.

**Rejected — deleting the directory.** It is working local tooling, and the
rule is about what this repository *tracks*, not about what sits beside it.
Ignoring states that intent; deleting would invite it back untracked and
unignored.

`mutants.out/` is `cargo-mutants` build output and writes `.json` for the same
reason; both are ignored in the same change.

---

## D-0029 · 2026-08-01 · The instrument universes are transcribed Rust data, and the merge consults them

**Decision — `crates/core/src/universe.rs` holds the NIFTY Total Market and
F&O underlying lists as sorted `[&str]` literals, membership is a bitset looked
up *from* the key, and `api::merge` stamps every merged instrument with it.**

This module shipped in the D-0024 change with no entry here, no invariant rows,
no charter source for its 963 transcribed instrument names, a `See
docs/06-limits.md` pointer to a section that did not exist, and **no caller
anywhere in the workspace**. `CLAUDE.md` §9 requires a ledger entry for every
locked choice and an invariants row beside the test that proves it; golden rule
1 requires every claim about an instrument to be traceable to a source recorded
in `docs/00-charter.md`. None of that was done, and CI stayed green because
gate 10 walks invariant-rows→tests and never tests→rows. This entry, the rows
`U-01`…`U-04` in `docs/04-invariants.md`, §4a of the charter and §11 of the
limits are the correction.

**Why Rust literals.** `CLAUDE.md` §2 permits no `.csv`, so a constituent list
cannot be tracked as a data file. The data is fine; the format is not.

**Why membership is not a field of `InstrumentKey`.** The key derives `Hash`
and `Eq` over every field and *is* the deduplication (D-0015). The same
contract carrying different memberships would become two keys and dedup would
break in silence — the identical argument D-0024 makes about the ISIN.

**Why it must have a caller.** `Skip::SmeBoard` declines 1,117 real shares on
the ground that "neither list contains an SME listing". That claim was a
sentence in a comment about a module nothing consulted. It is now (a) checked
by `core::universe::no_measured_sme_ticker_belongs_to_either_universe` against
14 SME tickers taken verbatim from the masters, and (b) load-bearing: the merge
stamps `Entry::universe`, the page renders a Universe column, and the report
prints a census on every run. Measured on the real masters: **0 of the 559
Dhan SME tickers** are in either list.

**Rejected — a perfect hash.** Lookup is `binary_search`, O(log n) — at most
ten comparisons on 750 entries. That is a departure from golden rule 4 and it
is written down rather than glossed (`docs/06-limits.md` §11). Membership is
asked once per instrument at merge time and never once per bar, so it is not on
the constant-cost path the rule protects; a perfect hash would buy nothing at
this size and would add machinery to maintain.

**UNVERIFIED and carried as such:** neither list has been checked against an
NSE constituent circular. Both are snapshots of a rebalanced index.

---

## D-0030 · 2026-08-01 · Branch coverage is not measured, and X-06 no longer claims it is

**Decision — `docs/04-invariants.md` X-06 is narrowed to the line and region
coverage the CI job actually enforces, and the branch half is recorded as
unmeasured in `docs/06-limits.md` §7.**

X-06 read "Line and branch coverage is 100% on every crate", proven by "CI
coverage job", status ✓. The coverage job passes `--fail-under-lines 100
--fail-under-regions 100` and nothing else. `cargo llvm-cov` reports
`Branches 0 0 -` for every file — zero branches instrumented — so the branch
half was a green tick over a measurement that had never run.

It is not merely unreported, it is **unrunnable as configured**: branch
coverage needs `-Z coverage-options=branch`, which is nightly-only, and
`rust-toolchain.toml` pins stable 1.97.1. `cargo llvm-cov --branch` fails
outright with `error: 1 nightly option were parsed`.

**Rejected — moving to nightly to satisfy the row.** The pin is itself a
decision, and trading a reproducible toolchain for a coverage column is the
wrong trade.

**Rejected — leaving the row as it was.** A gate that claims a measurement
nobody took is the same shape as the defects this change exists to fix, and
`docs/06-limits.md` §7 is the table that exists to say what a green build does
not prove.

Region coverage is the closest thing the stable toolchain measures: it counts
every distinct execution region, which subsumes most of what branch coverage
would catch on this code. It is enforced at 100% and it is what the row now
claims. When branch coverage stabilises, the row widens again and this entry
records why it was narrow.

---

## How to add an entry

Next free ID, today's date, the decision in one sentence, the alternative
rejected, and the reason. If you cannot name what was rejected, the decision
is not yet made — it is a default, and defaults do not belong in this file.

---

## D-0031 · 2026-08-01 · The on-disk format is version 2, and version 1 is refused rather than read

**Decision — the store format is `BRUTEXB2`, `FORMAT_VERSION = 2`.
`Layout::KNOWN` contains version 2 alone, so a version 1 file is refused by
name. Version 1's geometry is left exactly as D-0002 and D-0005 describe it and
is not redefined.**

The hardening in this change altered the on-disk geometry in three ways:

| | v1 (D-0002, D-0005) | v2 |
|---|---|---|
| Header | 64 bytes, one copy | 32,768 = 2 slots × 16,384 stride |
| Block | 4,096 bytes | **4,088 = 56 × 73** |
| Commit | 3 fields rewritten in place | double-buffered slot, highest valid generation wins |

The block size is the load-bearing one. 4,096 is not a multiple of 56, so
4096/56 = 73.14 and a record could span two blocks — 23 of the first 2,000 did.
A record that straddles cannot be verified against one checksum, and verifying
it against only the block it starts in checks part of its bytes and calls that
a pass. 56 × 73 = 4,088 makes straddling **unrepresentable** rather than
handled, which is why the geometry moved rather than the verifier gaining a
second block lookup.

**The first attempt mutated version 1 in place** — it changed `HEADER_LEN` and
`BLOCK_LEN`, moved every field offset and inserted `generation`, while leaving
`MAGIC = b"BRUTEXB1"` and `FORMAT_VERSION = 1`. That is exactly what §3 rule 8
and §4 forbid, and it defeated the version dispatch built in the same change:
the one geometry change that had actually happened was invisible to it, because
both geometries answered version 1. An old file was then detected only by
accident — the checksum had moved, so the header failed to decode and the
operator was told "the header is unreadable", a loud refusal naming the wrong
reason. Had the checksum domains happened to agree, the file would have been
read at a 64-byte offset shift and returned plausible integers.

**Version 1 is not in `KNOWN`.** No v1 file exists anywhere: the version shipped
in PR #10 as format and offset arithmetic with no writer, so nothing has ever
been written in it. A v1 file therefore cannot be encountered, and supporting a
geometry no file uses would be untestable code guarding an impossible case.
Should one ever appear it is refused with its version number named — never
guessed at, never read at the current stride.

**Cost of getting this wrong later.** One line today: a new magic, a new version
constant, a second row in `KNOWN`. Once bars are on disk it is a migration of
every file, and the failure mode in the meantime is silently plausible numbers.

Supersedes the geometry halves of D-0002 (64-byte header) and D-0005 (4 KiB
block) for version 2 onward. Both remain the correct description of version 1.

---

## D-0032 · 2026-08-01 · The checksum is a compile-time table, not a bit loop and not an intrinsic

**Decision — `store::crc` computes CRC-32C with a slice-by-8 lookup table built
by a `const fn` at compile time. One body, on every target. No `unsafe`, no
`core::arch` intrinsic, no `#[cfg(target_arch)]` selection, no dependency.
`crc32c` takes `&[u8]`; the header's discontiguous domain is served by a second
entry point, `crc32c_split`.**

The kernel was bit-by-bit: eight shift-and-mask iterations per byte, no table.
Measured on an Apple M4 Pro, `rustc` 1.97.1, release profile, minimum of 201
trials × 200 reps with `black_box` on both sides, over one full 4,088-byte
block:

| kernel | ns / block | ns / byte | ns / record (73 per block) |
|---|---|---|---|
| bit-by-bit — what shipped | 13,952.5 | 3.413 | 191.1 |
| 256-entry table | 6,868.3 | 1.680 | 94.1 |
| **slice-by-8 — this decision** | **1,487.5** | **0.364** | **20.4** |
| slice-by-16 | 1,632.7 | 0.399 | 22.4 |

`block::seal`, which is what a verifier actually calls, was measured before and
after through the same harness — `crates/store/benches/ratio.rs`, run against a
worktree pinned at the pre-fix commit and against this one, three runs each,
minimum taken:

| | before (min of 3) | after (min of 3) | factor |
|---|---|---|---|
| `crc32c`, one block | 13,512.1 ns | 1,491.3 ns | 9.06× |
| `block::seal`, one block | 15,291.0 ns | 1,485.0 ns | **10.30×** |
| `Header::read_region`, 1× region | 194.6 ns | 17.1 ns | 11.39× |

The gap between the two rows is the second scan. `byte_count` folded one
`saturating_add` per byte to compute a length `bytes.len()` gives in O(1), so
every seal walked the block twice: seal minus checksum was 1,779 ns before and
is inside the noise band after. It is retired in the same change, because a CRC
fix alone would have left it costing more than the checksum beside it.

`read_region` was never touched; it got 11× faster because decoding a header
slot is a 60-byte checksum, and it decodes several.

**Why the signature had to move.** `crc32c` took `IntoIterator<Item = u8>`,
justified by the header slot needing to skip the four bytes holding its own
checksum without a copy. That signature *structurally* forbids reading eight
bytes at a time, so a four-byte hole in a 64-byte header was costing every
4,088-byte data block an order of magnitude, forever. The hole is now served by
`crc32c_split(head, tail)`, which threads one register through two runs.

**Why not the hardware instruction.** `is_aarch64_feature_detected!("crc")` is
true on the operator's machine and the ARMv8 CRC32C instruction is roughly 41×
the bit loop by an earlier probe's measurement — a number this decision does not
claim, because this session did not take it. Two things rule it out anyway.
Every crate here carries `#![forbid(unsafe_code)]`, which an `#[allow]` cannot
re-open, so an intrinsic needs a whole new crate to hold one `unsafe`
expression. And CI runs on `ubuntu-24.04`, `x86_64`, on every job: an
`aarch64`-gated body would never be compiled, linted, tested or covered there,
while `--fail-under-regions 100` would still report 100% because the body does
not exist in that build. That is a gate reporting success without taking the
measurement — §3 rule 6. One body on both hosts means what CI measures is what
the operator runs.

**Why not `crc32c = "0.6"`**, which is already in the workspace dependency
table. It would work and it carries its `unsafe` internally. It was rejected for
the reason `crates/store/Cargo.toml` already records — a new package in
`Cargo.lock` while a concurrent workflow edits a second crate's manifest, against
a CI gate that builds `--locked --offline` — and because the table kernel gets
within about 4× of a hardware path for zero dependencies and zero exceptions.

**Why not slice-by-16.** Measured beside slice-by-8 above. It is slower at both
lengths this store actually checksums and wants twice the table; it wins only
past 64 KiB.

**The covered domain of a header slot is frozen at bytes `0..56 ‖ 60..64`.**
Zero-filling the hole to get one contiguous 64-byte buffer is the obvious
simplification and it computes a different number: on a sample slot, 0x3AB11682
against 0x1AF1D503. Every slot already on disk would fail its checksum, and a
failed slot checksum condemns the whole file. Because it is a change of *domain*
rather than of algorithm, no round-trip test and no published check value would
have noticed.

**What was missing that let this ship.** The only checksum VALUE pinned anywhere
was `b"123456789"` — nine bytes. Everything else was a round trip, which passes
under any deterministic function. A wide kernel is correct on every input
shorter than its stride, so the old suite could not have detected a wrong fast
path on a single real store input. `store::unit::the_crc_reproduces_a_hardcoded_value_at_every_length_that_can_break`
pins fifteen values across the boundaries a wide kernel breaks on, and
`store::unit::the_fast_kernel_agrees_with_a_bit_by_bit_reference_on_every_length`
runs the retired kernel beside the new one. §2's extension allowlist forbids a
tracked binary fixture, so a Rust constant is the only place a golden value can
live.

**What is not claimed.** A CRC is O(n) in the bytes it covers and this does not
change that. See `docs/06-limits.md` §14.

---

## D-0033 · 2026-08-01 · A vendor master field is bounded before it is read, not a hundred lines after

**Decision — `MasterRow` fields are bounded at `core::vendor::MAX_FIELD_BYTES`
= 64, checked as the first statement of `decode_master_row` and refused with
`InstrumentError::FieldTooWide { field, len }`. The reader bounds what feeds it
too: `api::master::MAX_MASTER_BYTES` = 256 MiB before the file is read at all,
and `api::master::MAX_ROW_BYTES` = 4,096 before a row is split.**

`crates/core/src/symbol.rs` opens by arguing this exact failure must not happen:
*"A vendor that one day emits a 4 KiB identifier would silently make every dedup
probe 200× more expensive, and no test would notice because nothing would be
wrong, only slow."* The `TEST_MARKERS` scan reintroduced it one layer **above**
the 24-byte guard written to prevent it —
`TEST_MARKERS.iter().any(|m| row.underlying.contains(m) || row.trading_symbol.contains(m))`
searched two raw vendor `&str` with no bound anywhere in front of it.

Measured, with `underlying` pinned to `RELIANCE` so the verdict is identical at
every length and only the scanned-but-unused `trading_symbol` grows:

| `trading_symbol` | cost | verdict |
|---|---|---|
| 8 B | 56.4 ns | `Ok(Keep(RELIANCE))` |
| 1 KiB | 102.0 ns | `Ok(Keep(RELIANCE))` |
| 16 KiB | 997.8 ns | `Ok(Keep(RELIANCE))` |
| 4 MiB | 245,839.2 ns | `Ok(Keep(RELIANCE))` |

Re-measured through `crates/core/benches/ratio.rs`, run three times against a
worktree pinned at the pre-fix commit and three times against this one, minimum
taken:

| `trading_symbol` | before | after | |
|---|---|---|---|
| 28 B — the widest real value | 46.9 ns, kept | 51.1 ns, kept | **9% slower**, and that is the price |
| 6.4 KB — 100× the bound | 356.6 ns, kept | 7.6 ns, refused | 46.9× |
| 4 MiB | 229,517.3 ns, **kept** | 7.7 ns, **refused** | 29,858× |

The first row is the honest cost of the fix: ten `str::len` reads on every row,
about 4 ns. It buys the other two.

Linear over four orders of magnitude — and **worse than the cost**: the 4 MiB
row was accepted and stored. `Symbol::new` only ever saw whichever field became
the identity, and `trading_symbol` becomes the identity only when `underlying`
is empty. Even the rows the guard did refuse, it refused *after* paying the
scan: a declined 4 MiB option contract returned `Err` after 274,766 ns, so an
attacker paid nothing to burn a quarter of a millisecond per row. §4 wants a
loud refusal; an unbounded refusal is not one.

**Why 64.** Measured across both real masters on 2026-08-01 — 33,990,514 B of
`dhan_scrip.csv`, 19,224,497 B of `groww_instruments.csv` — the widest value in
any column this decoder reads is **28 bytes**: `MCX_MCXBULLDEX28AUG2632100CE`
in Groww's `isin`, `NIFTYNXT50-Aug2026-101500-CE` in Dhan's `SYMBOL_NAME`. 64 is
2.28× that. The widest value in *any* column of either file, including ones
never read, is 80 bytes — a fund name — so a vendor moving a prose column into a
column we read is refused rather than scanned, which is the case worth catching.

**Why not reuse `SYMBOL_CAPACITY` (24).** That bound is what an *identity* may
be. This is what this engine is willing to *look at*. A 40-byte strike or
expiry string is not a symbol and never will be, and refusing it would refuse
rows that decode correctly today.

**Why a new error variant rather than `Malformed`.** `Malformed` is a claim
about content — the vendor sent nonsense. An over-wide field may be perfectly
well-formed; the refusal is about how much of it this engine will read. The
operator needs to tell those apart, so the variant carries the field name and
the actual length and the message prints both.

**Why the reader is bounded as well.** `load` read the whole file with
`read_to_string` and split every row with `split(',').collect()` before any
field was examined, so the decoder's own gate could not run until an unbounded
allocation had already happened. A file this process cannot hold is not an error
`read_to_string` can report — it is an OOM kill — so the size is checked from
`metadata` first. Real files are 34 MB and 19 MB; the bound is 256 MiB. Real
rows are at most 486 B; the bound is 4,096 B. Both refusals name the number, so
an operator who legitimately outgrows one is told what to raise.

---

## D-0034 · 2026-08-01 · Gate 8 measures, or it fails; and every gate's tool version is pinned

**Decision — the gate 8 workflow step looks for benches at
`crates/*/benches/*.rs` and treats an empty set as a FAILURE, never a skip.
Two harnesses exist, both `harness = false` and `test = false`, both
dependency-free. `cargo-deny` and `cargo-llvm-cov` are installed at exact
versions.**

`CLAUDE.md` §3 rule 4 names this gate as the enforcement mechanism for constant
per-operation cost: *"A change that makes one of them scan fails the bench
gate."* That sentence was false for the whole life of the repository. The step
began `[ -d benches ] || { echo "skip — no benches yet"; exit 0; }`, which tests
for a directory at the repository **root**; Cargo benches live at
`crates/<name>/benches`, so the probe could never have found one even after they
were written. `ci-ok` went green on it every time. Both defects D-0032 and
D-0033 repair merged through it. `docs/06-limits.md` §7b had already recorded
that this made C-01..C-04 unenforceable "in perpetuity"; this is the repair.

**A skip that reports success is the fallback §4 bans.** An empty bench set now
fails and says what to do about it.

**The gate was checked against the tree it failed to catch.** Both harnesses
were run, unchanged except for the pre-fix API spelling, against a worktree
pinned at the commit before this change. They **fail** there: C-08 reports the
checksum at 0.995× the bit loop (because it *was* the bit loop), C-09 reports
4,898× at a 4 MiB field, and C-10 reports `refused=false`. A gate that has never
been observed failing is a gate nobody has tested.

**Why no benchmarking framework.** A harness would add a dependency tree to take
a handful of `Instant` readings. `harness = false` makes each bench an ordinary
binary that prints every number it took and exits non-zero on a breach; nothing
is reported as passing that was not measured. `test = false` keeps `cargo test`
and the coverage gate out of a timing loop.

**What they assert.** `docs/04-invariants.md` C-01 and C-07..C-10. The ceiling
is that file's shared-CI number, 3.0×, in integer thousandths —
`clippy::float_arithmetic` is a workspace lint and a ratio is the one place it
would be tempting. The statistic is the **minimum** over 60 trials: a shared
runner's scheduler can only ever make a sample slower. Measured during this
session against a foreign process at 686% CPU, the minimum moved 0.2% across
three runs while the median moved 78%.

**C-08 is a ratio, not a nanosecond ceiling.** An absolute threshold would have
to be guessed for hardware this repository has never measured on — CI is
`x86_64`, the operator's machine is `aarch64` — and a guessed threshold is
either red for no reason or green for every regression. The bench carries the
retired bit-by-bit kernel and requires the shipped one to beat it by at least
3× in the same process, under the same load. Measured here: 8.9×.

**Tool versions.** Gate 3 said in prose that `cargo-deny` was "pinned EXACTLY at
0.18.3" while the command was `cargo install cargo-deny --locked` — and
`--locked` pins the tool's own dependency graph, never the tool version. The
coverage gate had the same shape. `--fail-under-regions 100` is the threshold
most sensitive to how a given `cargo-llvm-cov`/LLVM pair enumerates regions: a
version that splits one region into two turns the gate red with no source
change, and one that stops instrumenting a construct turns it green while
uncovered code exists. Neither is distinguishable from a real regression in the
log. Both are now exact — `cargo-deny 0.19.0`, `cargo-llvm-cov 0.8.4`, the
versions the operator's machine measures with. The advisory database is still
fetched fresh, which is the part that is supposed to move.

**What this gate still does not cover.** C-02, C-03 and C-04 name `engine::`
tests and `crates/engine` does not exist; their status stays `—`. Gate 8 now
enforces over what exists rather than over nothing, which is a different claim
from enforcing over everything. See `docs/06-limits.md` §7c.

---

## D-0035 · 2026-08-01 · `crates/pull` is three things and no vendor call: the path configuration, the read-only secret port, and the manifest

**Decision — `crates/pull` ships with (1) a strict reader for the untracked
credential *path* configuration, (2) a one-method secret source with an SSM
adapter over a port and no AWS SDK dependency, and (3) a fixed-stride per-vendor
manifest. It ships with no vendor HTTP call and no `/pull` page.**

### Why the page is not in this change

A control panel for a downloader that cannot download is a dashboard reporting
on nothing, which is the shape this repository refuses everywhere else — the
same argument that made gate 8's skip arm unacceptable in D-0034. The page ships
with the fetch logic, in one change, so that the first thing it renders is a
real result.

### Part 1 — the configuration reads path segments, and halts on everything else

`CLAUDE.md` §8 and D-0013: no literal parameter path appears in any tracked
file, because this repository is public and a path names an account, an
environment and a vendor relationship. `crates/pull/src/config.rs` therefore
holds four *names of segments* and the renderer that joins them. Every segment
in its code, its tests and its doc examples is invented — `orgone`, `testenv`,
`vendorone`, `fieldone`. CI gate 1c was run against this tree and is green.

**Checked, not assumed.** The `org` and `env` values in the operator's own
`~/.brutex/credentials.toml` were searched for as whole words across every
tracked file: `org` appears nowhere, and `env` appears once — inside gate 1c's
own `envs=` alphabet in `.github/workflows/ci.yml`, which is the gate's search
pattern rather than a path, and which predates this change.

**One honest caveat.** The *vendor table name* is `Vendor::as_str()` — a public
broker name that has been tracked in `crates/core` since its first commit and is
in the README, the charter, and every store path. The vendor's **parameter path
segment** is the `vendor` key inside that table, and `crates/pull` never writes
one down: every such value in this repository is invented. If an operator sets
the key to the same text as the table name, the two coincide in *their* file —
which is their choice and which nothing here can see. The key exists rather than
the segment being derived from `Vendor::as_str()` for exactly that reason:
deriving it would make the path segment a tracked literal by construction, for
every deployment, permanently.

**Every refusal is a halt, and none of them defaults.** A missing file, a
missing key, a missing vendor table, an empty segment, a separator inside a
segment, a wrong region, or a line the grammar does not cover: each names the
line and stops. P-07.

**Nothing from the file is echoed back.** Errors carry the line number and, for
keys this reader defines, a `&'static str` key. They never quote the file's
text. An operator who pastes a token where a field name belongs would otherwise
have it copied into a log line by the very check that caught the mistake. The
two exceptions are one offending byte and a length.

**Rejected — a TOML crate.** The grammar needed is four keys, one table shape,
quoted strings and a one-line array. A general parser accepts a great deal more
and would then have to be told which of it to refuse, which is the same work in
a less obvious place, plus a dependency and a `serde` derive to read eleven
lines. The reader here refuses **every** line it does not recognise, which is
the property that matters: a typo'd key is a halt, not a silent absence that
becomes a default one layer up.

**Rejected — reading `HOME` inside the crate.** `config::default_config_path`
takes the home directory. Two reasons, and the second is the load-bearing one:
process-global state makes two configurations in two tests impossible, and the
absent-`HOME` arm is a branch no test on any real machine could enter —
`std::env::set_var` is `unsafe` under edition 2024 and every crate here carries
`#![forbid(unsafe_code)]`, so it could not even be forced. An unreachable arm
behind a gate reporting 100% is the false green `docs/07-o1-architecture.md`
records twice.

**The pasted-secret check is a backstop and is labelled one.** The checks that
do the work are the length bound (32 bytes) and the byte set (`[a-z0-9_-]`);
every credential shape this repository has had in front of it — an access key, a
JWT, a base64 blob — is refused by one of those first, and
`pull::unit::credential_config_rejects_secret_value` drives all four. The
backstop catches the residue: 24 or more undelimited alphanumerics with a digit
in them. It is a heuristic over a shape and **not** a proof about entropy;
`docs/06-limits.md` §16 says what it does not catch.

### Part 2 — one read method, proved against a double that panics on a write

`SecretSource` has exactly one method and it reads. `ParameterStore` — the port
an SDK plugs into — has exactly one method and it reads. There is no `write`,
no `put`, no `rotate` and no `mint` anywhere in the crate's surface, so a token
cannot be minted by code that does not exist, and adding the method would be a
diff a reviewer sees rather than a call buried in an SDK builder. That is P-05's
structural half.

The behavioural half is `pull::unit::readonly_credentials`. It drives a whole
credential read through a double whose `put_parameter` **panics**, asserts one
read and zero writes, and then calls that write directly under
`catch_unwind` and requires it to fail. A double that would not have failed
proves nothing about the code that did not call it.

**A dead token is re-read once and then the pull stops.** `CLAUDE.md` §8 in one
function: `reread_after_rejection` takes the value the vendor just rejected,
reads the parameter again, returns the fresh value if it differs, and halts with
`DeadToken` if it does not. No third read, no backoff, no mint — a local mint
would invalidate the token another system shares. P-09.

**Rejected — taking `aws-config` and `aws-sdk-ssm` in this change.** They sit in
the workspace dependency table, unused. Taking them here adds roughly a hundred
transitive crates to `Cargo.lock` in support of no call at all, and `cargo deny
check` would then be verifying a licence and advisory surface nothing exercises.
The port is one method with the two arguments the real API takes; the adapter
over it is fully exercised against a double, and the change that first makes a
live call writes one `impl ParameterStore`, takes the dependency, and states the
`cargo deny check` result at that point. **No AWS SDK dependency was added and
no live call was made in this change.**

**`with_decryption` is always `true`.** The real API defaults it to false, which
returns the ciphertext of a SecureString — a perfectly ordinary string that
every vendor rejects as a bad credential. That failure looks exactly like an
expired token and is not one, so the argument has one value in this repository
and `pull::unit::the_adapter_always_asks_for_a_decrypted_value` pins it.

### Part 3 — the manifest is a counter file, and the counter is checked

`docs/07-o1-architecture.md` layer 13. One file per vendor,
`manifest/<vendor>.man`, recording per `(instrument, timeframe, month)` the row
count and the first and last timestamp, with three totals in the header:
entries written, distinct keys, rows held. All three are field reads.

**Geometry.** A 32,768-byte header region of two 64-byte slots at 16,384-byte
spacing, then 64-byte entries. `HEADER_LEN + i·64` is an add and a multiply.
Both the slot and the entry carry a CRC-32C over their first 60 bytes.

**Reused from `crates/store`, as code:** `crc::crc32c` — a second checksum
implementation is the one duplication that fails silently, because two kernels
that disagree produce two files each of which verifies only against itself;
`path::Timeframe` and `path::YearMonth` — the manifest key must be the tuple
that names a bar file; and `format::SLOT_LEN`, `SLOT_STRIDE` and
`MAX_SLOT_COUNT` — the 16,384-byte spacing is D-0031's measured failure-unit
argument, unchanged.

**Reused as reasoning, not code:** the commit counter as the authority (D-0004),
one write of one self-checked unit, and alternating slots.

**Deliberately not reused: `store::header::Header`.** Its fields are a bar
file's. Widening it to serve both would make one `format_version` describe two
geometries, which is what `store::layout`'s dispatch exists to make impossible.
Its discontiguous checksum domain is not reused either: that shape exists
because a bar slot has a reserved field *after* its checksum, and this one does
not.

**A torn write is detectable rather than believed**, by four mechanisms that
each cover what the others cannot: `n_valid` makes a torn tail unobservable, an
entry's own CRC catches damage below it, a slot's own CRC plus the other slot
catches a torn header, and `validate` against the region catches a header that
became durable before its entries — falling back a generation rather than
condemning the file.

**And the counter is checked against the thing it counts.** `Manifest::load`
recomputes the distinct-key count and the row total from the entries and refuses
a header that disagrees. A counter that is never checked is a number, not a
measurement — and this module exists so that number can be trusted without a
walk. The check runs once, on load, where a walk is already being paid. M-06.

**Rejected — checking the multiplication instead of bounding the ordinal.**
`MAX_ENTRIES` is 2,097,152 and `MAX_ENTRIES · 64 + HEADER_LEN` is 134 MB, so
past the bound the arithmetic cannot overflow and no offset needs a failure arm.
The earlier shape had `checked_mul` and `checked_add` behind a `?` in
`Manifest::record` whose error arms no input could reach — a branch nobody has
checked, sitting behind a gate that would still report 100%. The same reasoning
moved the vendor lookup in `CredentialConfig::path_for` from `?` to `and_then`.

**What layer 13 does not buy, said plainly.** A *filtered* census — "how many
expired option series" — is still a walk of the manifest's entries. It is a
sequential read of one file instead of ~248,000 directory operations, which is
the difference the layer is about, but it is not O(1) and is not claimed to be.
A per-segment counter would make it one and would be a new field at a new format
version, never a dynamic schema. `docs/06-limits.md` §17.

### What was measured

`crates/pull/benches/ratio.rs`, on an Apple M4 Pro, release profile, minimum of
60 trials:

| | measured |
|---|---|
| C-11 census counter vs decoding 10,000 entries | 789 ps vs 152,631,250 ps — **193,449×** (378 ps / 402,568× on an earlier run) |
| C-12 entry lookup, 10× census | 1.013× |
| C-12 entry lookup, 100× census | 1.027× |
| C-12 absent lookup, 10× / 100× census | 0.994× / 1.049× |

The C-11 spread across runs is the counter side, not the scan side: a field read
sits at the clock's resolution floor, so its measured picoseconds move by a
factor of two between runs while the scan holds at ~152 µs. **The floor asserted
is 100×**, three orders of magnitude below the worse of the two observations, so
the gate is a regression detector rather than a reading of one afternoon's
scheduler.

The directory walk the manifest actually replaces is **NOT** measured: it
depends on a filesystem, a page cache and a device the harness does not control,
and timing it would report the state of one machine's cache rather than a
property of the code. Any statement about that saving is an **EXTRAPOLATION**
from the entry scan above.

---

## D-0036 · 2026-08-01 · The pull crate's bounds, its redactions and its fall-backs are made real, and three claims that were not measured are withdrawn

**Decision — eighteen defects found by adversarial review of `crates/pull` are
fixed at their cause rather than at the symptom. Where a claim could not be
made true, the claim is withdrawn and the residual is recorded. No test that
exposed a defect was deleted, no range was narrowed, and no gate was weakened.**

This entry supersedes three statements in D-0035. That entry is not edited —
the ledger is append-only — so what was wrong is named here, sentence by
sentence.

### 1 — The configuration file's size bound was not a bound

`CredentialConfig::load` took its bound from `std::fs::metadata(path).len()` and
then called `read_to_string`. For every non-regular file that number is `0`:
`stat /dev/zero` on this host reports `size=0 type=Character Device`, and a
FIFO and a `/proc` entry report the same. So the check passed trivially and the
read that followed was unbounded on an endless stream. The review that found
this drove it: `load` on `/dev/zero` reached about 3 GB of resident memory in a
second and was still growing when it was killed, and `load` on a FIFO blocked in
`open` and never returned — those two numbers are that review's, not a
re-measurement here. They are precisely the two outcomes the comment above
`MAX_FILE_BYTES` said the check existed to prevent — "an allocator failure or an
OOM kill, and neither reaches an operator as *the credential configuration is
too big*". It was also a TOCTOU: `metadata` and `read_to_string` are two
syscalls, and a regular file that grew in between was read in full.

**Now:** `metadata` is still called first, and its *only* job is to refuse
anything that is not a regular file — `ConfigError::NotARegularFile`, naming the
path (P-19). A FIFO must be refused before `open`, because `open` on a FIFO with
no writer blocks forever and a hang is the one failure a test suite cannot
report. The read that follows is bounded by `Read::take(MAX_FILE_BYTES + 1)` and
the refusal is on what was actually read (P-17), which closes the device case
and the growth race in one change.

**Rejected — reporting the file's true length in `TooLarge`.** The read stops
one byte past the bound, so the true length is not known here. The variant
carries `at_least` and says so, rather than a second `stat` whose answer would
be as stale as the first one was.

### 2 — The assembled path was printable, from the type built to hold it

`secret.rs` states the threat model in its own words: a halt "never carries the
assembled path, which names an account, and an error is the thing most likely to
be logged, pasted into an issue and pushed to a public repository". `Secret` was
given a hand-written redacting `Debug` for exactly that reason. `CredentialPath`
— which *is* the assembled path — got a derive, and so did `CredentialConfig`
and the vendor table. `format!("{path:?}")` against the operator's real file
rendered all four live segments in the clear, and `pull::unit::the_only_path_
is_the_one_the_configuration_assembles` **asserted** that it did.

**Now:** all three carry hand-written `Debug` implementations rendering the
shape and the segment lengths (P-18), `Display` on the path is the single
audited exit in the role `Secret::expose` plays for a value, and the test that
certified the leak now asserts its absence.

### 3 — Gate 1c proves less than X-08 claimed

`crates/pull/src/lib.rs` called gate 1c "the check that this held". It is not:
its pattern needs a slash-joined path whose environment segment is one of ten
words. A file hardcoding a real `org` and a real `env` as two bare constants was
run past that exact regex and did not match.

**Now:** X-08's row is narrowed to what the gate does, `lib.rs` and the
`config.rs` header say the same, and **gate 1d** is added: every double-quoted
literal under `crates/pull` that could *be* a path segment must appear in an
allowlist in the workflow. That turns "every segment here is invented" into a
check and makes adding one a deliberate act. `docs/06-limits.md` §18 records
what neither gate can do.

### 4 — D-0035's "one honest caveat" is not hypothetical here

D-0035 wrote that if an operator sets the vendor key to the same text as the
table name "the two coincide in *their* file — which is their choice and which
nothing here can see", and its "Checked, not assumed" paragraph searched only
`org` and `env`. Extending that search to all four roles, against the operator's
own untracked file, without printing any value:

| role | tracked files containing it |
|---|---|
| `org` | 0 |
| `env` | 1 — inside gate 1c's own `envs=` alphabet in `ci.yml` |
| region | 5 — `CLAUDE.md` publishes it in the law |
| `vendor` segment (each of two) | **11 and 13**, including `crates/pull` |
| field names | 2 documents each, which §8 expressly permits |

So of the four segments, the vendor and the field are published and the region
is published by law. The separation the `vendor` key exists to make possible is
available in this deployment and is not taken. That is an operator-side choice —
a parameter path whose vendor segment is not the broker's public name — and it
is now said plainly in `config.rs` and in `docs/06-limits.md` §18 instead of
being implied to be unknowable.

### 5 — A fall-back that made a broken file look like a working one

`ManifestHeader::read_region` computed a header-slot refusal, stored it in a
local, and dropped it on the success path. Flipping one bit in the newest slot
of a two-month manifest returned `Ok` with the census silently reduced from 2
entries / 300 rows to 1 / 100, and no API a caller could reach said so. That is
the fallback `CLAUDE.md` §4 bans, on the one file whose entire purpose is a
number that can be trusted without a walk.

**Now:** `read_region` returns a `HeaderRead` — the newest generation, the one
below it, and what was stepped over — and `Manifest::degraded_reason()` carries
that to the caller (M-16). Recording is still permitted on a degraded census,
because recovering from a torn commit *is* writing the next generation and
refusing would leave the commonest crash with no way forward.

### 6 — M-05 was false as stated

M-05 claims a header that became durable before its entries "falls back one
generation rather than condemning the file". The fall-back lived entirely in
`validate(capacity)`, where `capacity` is the entry region's **byte length**. So
it engaged only when the region was physically short. When the counted entry's
64 bytes were present and never written back — a crash between a data write and
its flush, a lost block, a preallocated extent — the entry's CRC failed and
`load` condemned the whole file, while the previous generation sat intact in the
other slot and loaded perfectly when handed to `load` on its own. The test named
as M-05's proof passed an **empty** entry region, so it could only ever exercise
the length arm.

**Now:** `Manifest::load` walks *down* the generations. If the published one
does not describe these bytes, the one below it is tried — it counts a prefix of
the same entries, and `read_region` has already validated it. Measured against
a zeroed tail entry, a garbage one and a half-written one, all three now recover
to the previous generation and name what they stepped over. When no generation
survives it is a refusal, and the refusal reported is the newest generation's,
because that is the state a writer published.

### 7 — A genesis census over a file that already had one

`Manifest::genesis` was public. A writer that called it on a vendor which
already had a manifest produced a census that was silently, verifiably wrong and
still loaded clean: the stale slot 0 wins on generation, the fresh generation-1
commit lands in slot 1, entry 0 is overwritten, and because real index months
share a row count the recomputed key count and row total still agree with the
stale header.

**Now:** `genesis` is private and `Manifest::open(vendor, header, entries)` is
the door. It hands out a genesis manifest for a file with **nothing** in it and
calls `load` otherwise, so the wrong state is unrepresentable rather than merely
discouraged (M-14).

### 8 — The index was reserved from the file, not from the census

`let reserve = MAX_ENTRIES_LEN.min(entries.len() / IMAGE_LEN);` sized the map
from the untrusted region length while the comment above it argued for the tight
bound. A header committing exactly one entry inside a region at the design
ceiling allocated **574,619,656 bytes** to hold a one-key index — measured with
a counting allocator.

**Now:** the reservation is `header.n_valid`, which `validate` has already
proved is within both `MAX_ENTRIES` and the region's capacity, so the no-rehash
property is unchanged and the allocation is proportional to the census (M-17).

### 9 — C-12 measured a map the claim was not about

`census()` built every manifest it timed with `genesis` + `record`, and that map
reserves nothing and grows by rehashing. `Manifest::load` — the only place a
bound is reserved — was never called in the bench. Meanwhile `docs/06-limits.md`
§17, `docs/07-o1-architecture.md` line 79 and the bench's own doc comment all
attributed the flatness to that reservation. Two mutants of the reservation
expression survived the whole suite for the same reason.

**Now:** `census()` round-trips through `Manifest::load`, so the map measured is
the map the claim is about, and M-17 asserts the reservation as a number.

### 10 — The C-11 spread's stated cause is refuted by measuring the clock

D-0035, `docs/06-limits.md` §17 and `docs/07-o1-architecture.md` all said "a
field read sits at the clock's resolution floor, so its measured picoseconds
move by a factor of two between runs". The harness now measures the smallest
non-zero interval `Instant` reports and prints it beside the ratio. It is tens
of nanoseconds; the counter side is averaged over 100,000 repetitions, so one
tick is a fraction of a picosecond of reported cost against observations of
hundreds of picoseconds. The counter side is roughly three orders of magnitude
above the clock floor, so the floor is not what moves it.

**Now:** the observation is recorded (378–789 ps across runs, scan holding at
~152 µs) and the cause is recorded as **not established**. The 100× floor never
rested on the explanation. `CLAUDE.md` §3 rule 6.

### 11 — `SecretError::NotDecrypted` advertised a check nobody wrote

Nothing in `crates/pull/src` constructed it. `SsmSecretSource::read` passes
`with_decryption: true` and wraps whatever came back with no ciphertext check at
all, so the protection its doc comment described did not exist and the failure
it named would still occur — as a `DeadToken` halt naming the wrong cause.

**Rejected — writing the check.** Recognising a KMS blob means knowing a
vendor's ciphertext prefix and length, which is an external fact with no source
recorded in `docs/00-charter.md`; `CLAUDE.md` §3 rule 1 forbids inventing one.
The variant is removed and `docs/06-limits.md` §19 records what that leaves
unprotected, so no doc comment describes a check that is not there.

### 12 — The counters can drift from the bar files, and nothing said so

`Manifest::load` checks the header against this file's own entries and against
nothing else. An entry claiming 999,999,999 rows for a month with no bar file
anywhere loads clean and `total_rows()` returns it as fact; a crash between a
bar write and the manifest append leaves the census permanently short; a deleted
bar file leaves it permanently long. That is inherent — confirming it is the
~248,000-operation directory walk the layer exists to avoid — but it was
nowhere written down. `docs/06-limits.md` §17, the `manifest` module doc and
`docs/07-o1-architecture.md` now name it, and say that a reconciliation pass is
not built.

### 13 — `cargo-mutants` was not run, and §9 requires it

D-0035 stated plainly that it had not been run and left X-07 at `—`, which that
file's legend defines as "not yet reachable (the crate does not exist)". The
crate exists. It was run here, and the survivors it found are fixed at their
cause:

- **Seven exact-limit boundaries** — `MAX_ENTRIES` in `advance`, `validate` and
  `commit`, `MAX_FILE_BYTES`, `MAX_LINE_BYTES`, `MAX_FIELDS`, and the load-time
  equal-timestamp check — had no test supplying a value sitting *on* the limit,
  so every one of those comparisons could be flipped between `>` and `>=` with
  the suite green. M-15 and the P-17 test now pin each one at the limit and one
  past it, which is the standard `the_secret_backstop_is_the_first_byte_that_
  is_too_many` already set for one bound and nowhere else.
- **The map reservation** — killed by rewriting it to come from the counter
  (item 8), which removes the arithmetic operator that was being mutated, and by
  M-17 asserting the result.
- **`is_specific -> false`** — killed by M-16, which requires the stepped-over
  refusal to be reported by name.
- **`Secret::is_empty -> false`** — an *equivalent* mutant: the method can only
  ever return `false`, because `Secret::new` refuses an empty value, so no test
  could distinguish the mutant from the original. It existed only because clippy
  demands an `is_empty` beside a method named `len`. `len` is renamed
  `byte_len`, and `is_empty` is deleted. A function whose only possible answer
  is one value is not proven by a test that calls it.

X-07's status moves off `—`.

### What was measured, and on what

An Apple M4 Pro, macOS 26.5.2, rustc 1.97.1, release profile for the bench.

| | measured |
|---|---|
| smallest non-zero `Instant` interval on this host | **41 ns**, so 410 fs of reported cost at 100,000 reps |
| C-11 counter, this run | **789 ps** — **1,924 clock ticks**, not one |
| C-11 census counter vs decoding 10,000 entries | 789 ps vs 180,058,300 ps — **228,210×** |
| C-12 entry lookup, 10× / 100× census | 0.929× / 1.025×, on the **loaded** map |
| C-12 absent lookup, 10× / 100× census | 1.008× / 0.909× |
| `cargo mutants --package pull` | 263 mutants, **227 caught, 36 unviable, 0 survivors**, 3m |
| coverage, `crates/pull` | config 603 regions / 384 lines, manifest 874 / 598, secret 103 / 83 — 100.00% on both measures |

The clock figure is the one that settles item 10: the counter observation is
three orders of magnitude above the clock's floor, so the floor is not what
moves it between runs.

Every gate in `CLAUDE.md` §9 was re-run against the finished tree. The `x86_64`
numbers are still unrecorded (`docs/06-limits.md` §17), and the directory walk
the manifest replaces is still an **EXTRAPOLATION**.
