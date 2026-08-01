# brutex

A brute-force backtesting engine for Indian spot indices. It sweeps combinations
of boolean market conditions over historical 1-minute bars and ranks what
survives.

**One language. No exceptions.** CI walks every tracked file and fails the build
on any extension outside `.rs .toml .md .lock .html .css .yml`. There is no
interpreted runtime, no build script that spawns a process, and no vendored
binding to another language — including in the web UI, which is server-rendered
HTML with zero JavaScript.

---

## What it is for

Given six years of NSE minute bars, try every surviving combination of 74 market
conditions and report which combinations made money. The search stops where the
frequent frontier empties — **there is no depth parameter**, not a default, not
an environment override. The type does not carry the field.

That absence is deliberate. In the predecessor repository the depth flag
defaulted to a dynamic token, but the frontier mask was 64 bits wide against a
74-condition vocabulary. Every real run tripped the width guard and silently
fell back to a hardcoded `k = [1, 2]`. Dynamic depth was unreachable on the only
vocabulary that existed, and nobody could see it from the flag. A parameter that
can be set can be set wrongly and silently.

## Scope

| | |
|---|---|
| Swept | `NSE-NIFTY`, `NSE-BANKNIFTY` — exactly two |
| Tracked equities | NIFTY Total Market constituents |
| Indices | the NSE index series |
| Stored, never swept | futures, options, single stocks |
| Never stored | currently-listed derivatives — history means *expired* contracts |
| Purpose | backtesting only. No live trading, no order routing |

## The rules that shape the code

**Constant per-operation cost.** Bar lookup, condition lookup, mask evaluation,
duplicate rejection and result append are each O(1). A change that makes one of
them scan fails the bench gate. The sweep itself is combinatorial — each *step*
is constant, the number of steps is not, and the documents say so.

**Prices are paisa integers.** `i64`, never a float. The one permitted
floating-point conversion lives behind a private field and refuses NaN, infinity
and out-of-range rather than saturating.

**No look-ahead.** At bar *N* the engine may read bars 0..*N*, enforced by an
index-guarded accessor rather than by review.

**Append-only history.** Condition bits are never renumbered or reused. Store
format versions are never mutated in place.

**Degrade loudly, or refuse.** Never both silently. A vendor disagreement stops
the run and names what disagreed; an unreadable row is counted and shown, not
dropped.

## Identity

Two vendors describe the same market. They disagree, and the disagreements are
not obvious:

- One vendor's `INSTRUMENT=EQUITY` covers 4,416 debentures and 81 treasury
  bills. Read naively, `CHOLAFIN` resolves to a 7.5% non-convertible debenture
  rather than the share — the bond row precedes the share row in the file, so
  insert-if-absent wins silently, and takes the wrong tick size with it.
- The other leaks an internal symbol into the public column on exactly 209 rows.
- Exchange token numbers are reissued when a security changes series, so a store
  keyed on them breaks on *rebuild* rather than on first build.

So identity is `(exchange, ISIN)`, ISIN is validated including its check digit,
and the ticker is a display label. The ISIN sits **beside** the key rather than
inside it: as a key field, a vendor disagreement would produce two keys and the
map would split in silence.

## Layout

```
core   (no dependencies at all)
 ├── store        fixed-stride bar files
 ├── indicators   bars in, condition bits out
 ├── vocab        the bit table and mask operations
 ├── engine       sweep, ranking
 ├── pull         vendor ingest
 ├── api          HTTP
 ├── web          browser UI, wasm32 — depends on core ONLY
 └── cli          operator entry point
```

`core` declaring a dependency, or `web` declaring anything but `core`, is a
build failure rather than a review comment.

## Running it

```
cargo run -p api --release
```

Serves `http://127.0.0.1:8080` — dashboard, instrument browser with universe
filters, sortable columns and paging. Vendor masters are read from
`$HOME/.brutex/masters` unless `BRUTEX_MASTERS` says otherwise.

## What must be true before a change lands

`cargo fmt --check` · `cargo clippy --workspace --all-targets -- -D warnings` ·
`cargo test --workspace --locked` · `cargo test --doc --workspace` ·
`cargo deny check` · **100% line and region coverage on every touched crate** ·
no surviving mutant on touched modules · every new invariant beside the test
that proves it · a decision-ledger entry for every locked choice.

## Documents

| File | Authority over |
|---|---|
| `CLAUDE.md` | the rules above. If a document disagrees with it, it wins |
| `docs/00-charter.md` | scope, verified external facts, prohibitions |
| `docs/01-architecture.md` | crates, arrows, data flow |
| `docs/02-store-format.md` | bytes on disk |
| `docs/03-vocabulary.md` | condition bit table |
| `docs/04-invariants.md` | what must hold, and its proof |
| `docs/05-decisions.md` | append-only ledger |
| `docs/06-limits.md` | what is **not** constant-time, and what is unmeasured |

`docs/06-limits.md` is the one to read if you want to know what this does not
do. It is kept honest on purpose: a measurement nobody took is recorded as
unmeasured rather than assumed.

## Credentials

Read-only, from AWS Parameter Store SecureStrings. **No literal parameter path
appears in any tracked file** — this repository is public, and CI fails the
build if one does. Paths are assembled at runtime from a local, untracked
configuration holding path segments only, never a secret value. The credential
value is never an environment variable, never a file, never a prompt. This
repository never mints a token.
