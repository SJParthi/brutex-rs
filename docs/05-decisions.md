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

## D-0036 · 2026-08-02 · Greeks are their own crate, in `f64`, and the crate never sees a paisa

**Numbering.** This entry is D-0036 and not D-0035. D-0035 is claimed by
`crates/pull` on the unmerged branch `feat/pull`; this work was cut from `main`,
where the ledger ends at D-0034. Reusing 0035 would put two different decisions
under one identifier the moment both merge, and this file is append-only in
identifiers as well as in lines.

**Decision.** A new leaf crate, `crates/greeks`. Black-Scholes-Merton for
European options: `d1`, `d2`, price, delta, gamma, vega, theta, rho, implied
volatility, and a strike's position on the ladder in strike steps. It declares
**zero dependencies**, its public surface mentions **no type from this
workspace**, and every value crossing its boundary is an `f64`, a `u32`, an
`i32` or a plain enum it owns.

**Rejected — putting this in `crates/core`.** `core` is compiled to `wasm32`
through `crates/web` and is the one crate every other crate already depends on.
Greeks are needed by exactly one consumer here and by the `tickvault`
repository, which takes this crate by git URL. Folding it into `core` would
force a transcendental-function surface onto the browser build for no caller,
and would put the float exception below in the crate that must never have one.

**Rejected — an `f64` price type anywhere in the surface.** `CLAUDE.md` §7 is
not negotiated here: prices are `i64` paisa. This crate is `f64` **because it
never sees money**. There is no `i64` in its surface, no conversion from one,
and no snapping to a tick. The caller converts at its own boundary.
`greeks::standalone::the_whole_public_surface_is_reachable_with_nothing_else_in_scope`
is the structural proof: it names nothing but `greeks::`, and it stops
compiling the day a workspace type appears in a signature.

### The lint exception, and how narrow it is

`clippy::float_arithmetic` is `warn` at the workspace root and CI runs clippy
with `-D warnings`, so it is denied in practice. **The workspace level is not
touched.** Four module files carry `#![allow(clippy::float_arithmetic)]` as an
inner attribute with the reason attached — `normal.rs`, `bsm.rs`, `solver.rs`,
`moneyness.rs`. `error.rs` does not need it and does not have it, and every
other crate in the workspace is unaffected.

The justification is `CLAUDE.md` §7's own second sentence: *statistical values
keep full precision and are never rounded for storage*. A delta is a
derivative, a gamma is a second derivative, an implied volatility is a solved
parameter. None of them has a representation on a two-decimal tick grid, and
rounding one to paisa would destroy it. They are the statistical values §7
exempts, not the prices §7 constrains.

**One further exception, in one expression.**
`crates/greeks/src/moneyness.rs` carries
`#[allow(clippy::cast_possible_truncation)]` on the single `rounded as i32` that
turns a step count into an ordinal. The two checks that make the cast exact —
the value is a whole number, and its magnitude is at most `MAX_STEPS` — are the
two statements immediately above it, and `std` offers no checked `f64 -> i32`.
Every other cast in the workspace is still denied.

### What the vendors actually publish, measured rather than assumed

One live Dhan option-chain response, one strike, both sides. Four properties
of it had to be measured before it could anchor anything, and each is a test in
`crates/greeks/tests/vendor_anchor.rs`:

| | measured |
|---|---|
| **vega** | per **one percentage point**. The raw scaling implies an index level of **258.51**; the per-percent scaling implies **25,851.19** |
| **theta** | per **calendar day, over 365**. The two sides of one strike agree on `r` to **0.9999** points under 365 and **23.2598** under 252 — a factor of **23.3** |
| **the two IVs** | **transposed** relative to the delta/gamma/vega block. The scale-free identity `vega·gamma·sigma == n(d1)^2` gives **1.21979 / 0.82249** as published and **1.00012 / 1.00315** swapped, against gamma's own printed slack of 0.38% / 0.46% |
| **the carry** | **zero**. `delta_call − delta_put = 1.00603` is reproduced exactly by `N(d1c) + N(−d1p)` under the two volatilities. Under one volatility it would be `e^-qT` and would imply `q = −46%`, which is not a rate |
| **rho** | Dhan publishes **none**. Groww publishes all five. Rho is shipped here: it is one `N(d2)` on a quantity already computed |

Getting the day divisor wrong costs `365/252 = 1.448413` — a theta of `−15.15`
a day becomes `−10.46`, a **44.8%** error on one strike, and nothing in a
response says which convention produced it.

**The anchor is the two gammas.** Dhan publishes no spot, no strike and no
maturity, so they are solved out of the sample: the deltas and volatilities
give `T` in closed form, vega gives the spot, theta gives the rate. Four of the
eight published numbers are therefore *inputs* to the fit and are not evidence
for it. **Neither gamma is used anywhere in the fit.** Reproduced:

| | ours | Dhan | deviation |
|---|---|---|---|
| gamma call | 0.001319843888 | 0.00132 | **1.561e-7** |
| gamma put | 0.001086576818 | 0.00109 | **3.423e-6** |

Both inside the vendor's own display precision of `5e-6`. Fitted state:
`T = 0.0141324716` years (5.15835 calendar days), `S = 25851.1937 / 25781.0114`,
`K = 25858.2764 / 25791.7192`, `r = 9.4619% / 10.4618%`.

**And the fit does not close, which is recorded rather than smoothed.** The two
sides give spots 0.2722% apart and rates 0.9999 points apart, both outside the
rounding box of the printed fields.
`greeks::vendor_anchor::the_two_sides_disagree_on_the_rate_and_the_disagreement_is_reported_not_hidden`
pins those residuals so that no later change can quietly claim the sample is
cleaner than it is. **Nothing in this crate hardcodes a rate.**

### The normal distribution: Hart 5666, and a transposed digit

Three candidates, measured against a reference built from an all-positive-term
`erf` series below `|x| = 1.5` and a backward continued fraction above it:

| CDF | max abs | max rel | ns/call |
|---|---|---|---|
| reference | — | — | 423–497 |
| **Hart 5666 — shipped** | **2.22e-16** | **8.911872737504238e-9** at `x = −7.78` | **3.3** |
| A&S 7.1.26 | 6.97e-8 | 1.01 | — |

**Rejected — Abramowitz & Stegun 7.1.26**, which is the approximation everyone
reaches for. Its published `1.5e-7` bound on `erf` measures as `6.97e-8` on
`N(x)`: 7.2 absolute digits and **zero** relative digits in the tail. A CDF
with no relative accuracy in the tail cannot price a far strike at all.

**The differential test earned its cost on the first run.** Hart's `B6` was
entered as `1.755_667_161_832_64` for `1.755_667_163_182_64` — two digits
transposed. It left the tail *exactly* right, reproducing the calibrated
relative error at `x = −7.78` to all sixteen digits, and put **2.4e-13** of
absolute error in the body around `|x| = 1.73`. No known-value table at
five decimals would have seen it. A second implementation sharing no line of
code did. The test-side `sqrt(pi)` is now derived from `std::f64::consts::PI`
rather than transcribed, for the same reason.

### The solver: bracketed first, Newton as the optimisation

**`MAX_ITERATIONS = NEWTON_STEPS + BISECTION_STEPS = 8 + 64 = 72`, and 72 is
arithmetic, not observed.** The bisection performs *exactly* 64 halvings of
`[1e-6, 5]` with no early exit, no tolerance test and no stagnation detection:
`(5 − 1e-6)/2^64 ≈ 2.7e-19`, narrower than one unit in the last place of any
volatility in that band. Its cost does not depend on any input value, because
it does not look at the input to decide when to stop.

**Rejected — Newton as the primary method.** Measured over an 819-point grid:
Newton alone reached **427** iterations with a fixed tolerance and **444** with
stagnation detection, at `K/S = 1.10, T = 2h`, against a median of 7–8. Raising
its cap to 500 buys a median of 11 and a ceiling of 444. `CLAUDE.md` §3 rule 4
is a worst-case rule, and a method whose worst case is fifty times its median
cannot carry one.

**Stated because it argues against the shape shipped here:** at a cap of 8 the
Newton pre-pass did **not** beat the bracketed method alone on that grid — 63
against 55 worst, 19 against 17 median. It is kept because the common case
converges in three or four steps and never reaches the bisection, and because
its cost is capped at 8 whatever happens.

**Rejected — Brent over bisection.** Brent is faster (29 iterations against 55
on the constructed pathological case) and its worst case is not arithmetic in
the same one-line way; its three-way branch structure also has arms that a test
must contrive to reach, which is a coverage argument for a gate that measures
regions. A fixed halving count is the cheapest thing to be *certain* about.

**Rejected — returning an answer where the price does not determine one.**
Twenty-one grid points are unrecoverable at any cost; at `K/S = 0.75` Newton
"converges" to answers wrong by a relative 1.279, 4.942 and 28.9. Returning
them is exactly the fallback `CLAUDE.md` §4 bans. `GreeksError::Indeterminate`
refuses when one unit in the last place of the quote would move the answer by
more than `1e-3` of itself, and carries the volatility, the vega, the
uncertainty and the bound so the refusal is checkable.

That criterion is **necessary and not sufficient**, and this entry says so
rather than implying otherwise. `ulp(price)/vega` is optimistic: measured
against the smallest interval around the true volatility on which the computed
price is still strictly ordered, it understates the real floor by two orders of
magnitude at the money — `1.60e-16` estimated against `1.67e-14` measured at
seven days, `1.29e-16` against `2.73e-13` at one hour. Passing it does not
prove the answer is good to `1e-3`. Failing it proves the answer is worthless.

### No unreachable failure arm, anywhere

The same reasoning D-0035 applied to `checked_mul` behind a `?`. Three shapes
were removed from this crate after the coverage gate found them, and the
reasoning is recorded because all three are easy to write again:

- **A `matches!` on a bare variant pattern** compiles to a discriminant switch
  whose "no match" arm no run reaches. Replaced by `assert_eq!` against a fully
  constructed error value, which also checks the numbers the refusal carries.
- **A catch-all `Err(other) => panic!(..)` arm.** Replaced by counting the
  refusals that do occur and requiring the count to account for all of them.
- **An expression that appears only inside a failing assertion's message.**
  `value + mirrored` in `normal.rs` was a region no passing run executed.
  Hoisted above the assertion. This is the third route to the trap
  `docs/07-o1-architecture.md` records, after the `const fn` and the
  uncontrived collision.

There is deliberately **no `NoConvergence` variant**. Once the price is proved
to lie strictly between the model prices at the two ends of the band, the
bisection cannot fail, so a non-convergence arm would be an arm no input
reaches. Every way the search can fail is named at the point where it can
actually happen: outside the arbitrage bounds, outside the searched band, or
indeterminate.

### No bench, and why that is not a gap

Gate 8 counts benches across the workspace and `crates/core` and `crates/store`
carry them, so the gate still measures. This crate adds none. The closed form
has no loop and no input-dependent branch other than the one choosing a side,
and the solver's bound is an **iteration count** — a deterministic integer,
asserted as a number by
`greeks::solver::the_iteration_count_never_exceeds_the_arithmetic_bound`, which
is a stronger statement than a timing ratio and does not move with a scheduler.
**Whether `exp` and `ln` are constant-time in their arguments is not measured
here**; see `docs/06-limits.md` §18.

### UNVERIFIED, and not guessed

- **Whether Dhan's `r` is a market rate or a vendor constant.** Measured at
  9.46% / 10.46%, midpoint 9.96%; a hardcoded 10.0% is consistent with the
  sample and so is a rate near 7% under a strike-grid reading. This sample
  cannot separate them, and nothing here assumes one.
- **The day count that generates `T = 0.0141324716` years.** The *divisor*
  `365` is measured at 23.3:1. The day count is not, without the capture
  timestamp: 5.158 calendar days ending 15:30 IST lands at 21:06, outside
  session hours, while 3.561 trading days lands inside one.
- **Whether the vendor prices spot BSM or forward Black-76.** The closed form
  for `T` eliminates `ln(S/K) + rT` either way, so every conclusion above
  survives it, but a forward discount would shift `T` by about 4%.
- **The NIFTY and BANKNIFTY strike intervals.** No source in
  `docs/00-charter.md` states them. `Moneyness::from_ladder` therefore takes
  the interval as an argument and hardcodes nothing.

### One discrepancy an operator has to settle

`CLAUDE.md` §5 lists nine crates and `greeks` is not among them. It adds no
arrow to that graph — it depends on nothing and nothing in this workspace
depends on it yet — so it breaks no rule in §5. But §10 says that where
`CLAUDE.md` and a document disagree, `CLAUDE.md` wins and the document is the
stale copy. Here the document (`docs/01-architecture.md`, updated by this
change) is ahead of §5. **`CLAUDE.md` was not edited by this change**, and
bringing §5 into line is an operator decision, not one to take in passing.

### What mutation testing found, and the two it could not

`cargo mutants --package greeks`: **308 caught, 2 missed, 0 timed out, 12
unviable.** The first pass missed eleven, and every one was a genuine hole —
four bounds nothing stood on the inclusive edge of, two rho mutations that hid
behind the central-difference test's own noise floor, six arithmetic mutations
in the Brenner-Subrahmanyam seed that no assertion about an *answer* can see,
and an iteration counter whose ceiling was asserted and whose floor was not.
The repairs are `greeks::bsm::every_bound_accepts_the_value_exactly_on_it`,
`greeks::solver::the_seed_lands_next_to_the_answer_at_the_money`,
`greeks::solver::the_iteration_count_is_the_work_actually_done`, and a
resolvability gate that now takes the larger of the analytic and the numeric
value rather than trusting the one being tested.

**Two survive and this entry names them rather than claiming a clean sweep:**
`uncertainty > MAX_RELATIVE_UNCERTAINTY` against `>=`, whose boundary is an
exact bit pattern of a quotient no input can be chosen to produce; and
`price(middle) < price` against `<=` in the bisection, where both choices keep
the root bracketed and the answers can differ by at most `2.7e-19`. Neither is
a behaviour a caller could observe. `docs/06-limits.md` §18 carries the full
reading.

Two more were **removed rather than tested**. `normal.rs` compared `x > 0.0` in
two places where `x` can never be zero — once past the saturating tail, and
once where `upper` is exactly `0.5` at zero so both arms return the same
number. Both are now a question about the **sign**, which has no boundary for a
mutant to sit on. That is the general repair for this class: an unreachable
boundary is a design smell before it is a coverage problem.
