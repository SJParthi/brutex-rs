# 01 — Architecture

Ten crates. Every arrow points one way. The graph is acyclic and the linker
enforces it.

---

## 1. The graph

```
                          ┌────────┐
                          │  core  │   no dependencies at all
                          └───┬────┘
        ┌──────────┬──────────┼──────────┬──────────┬─────────┬─────────┐
        │          │          │          │          │         │         │
    ┌───▼───┐  ┌───▼────┐ ┌───▼───┐  ┌───▼───┐  ┌───▼──┐  ┌───▼──┐  ┌───▼───┐
    │ store │  │ vocab  │ │  web  │  │ pull  │  │ api  │  │ cli  │  │ costs │
    └───┬───┘  └───┬────┘ └───────┘  └───┬───┘  └──┬───┘  └──┬───┘  └───────┘
        │          │       wasm32        │         │         │
        │      ┌───▼──────────┐          │         │         │
        └─────►│ indicators   │          │         │         │
               └───┬──────────┘          │         │         │
                   │                     │         │         │
               ┌───▼──────┐              │         │         │
               │  engine  │◄─────────────┴─────────┴─────────┘
               └──────────┘
```

| Crate | Owns | May depend on | Exists |
|---|---|---|---|
| `core` | types, error enums, the condition bit table, pure rules, the calendar | nothing | ✓ |
| `greeks` | Black-Scholes-Merton in `f64`: the five greeks, implied volatility, the strike ladder | **nothing** | ✓ |
| `store` | the fixed-stride bar file: open, read, append, verify | `core` | ✓ |
| `vocab` | mask type, mask operations, the frequent-frontier structure | `core` | — |
| `indicators` | bars in, condition bits out | `core`, `store` | — |
| `engine` | Apriori generation, evaluation, trade walk, ranking | `core`, `store`, `vocab`, `indicators` | — |
| `pull` | vendor ingest, rate governor, credential read | `core`, `store` | ✓ |
| `api` | HTTP surface | `core`, `store`, `engine`, `pull` | ✓ |
| `costs` | Indian F&O transaction costs: the dated statutory rate regimes, the option arithmetic, the round-trip charge stack | **`core` only** | ✓ |
| `web` | browser UI, compiled to `wasm32-unknown-unknown` | **`core` only** | — |
| `cli` | operator entry point | everything | — |

**Six of the eleven exist today**: `core`, `greeks`, `store`, `pull`, `api`,
`costs`. The `Exists` column is read off `Cargo.toml`'s `members` list, which
names a directory only once that directory is there — see the comment in that
file. A row with `—` is a crate this document has planned and no code has yet.

**This table was merged from two branches that each rewrote it.** `feat/pull`
added the `Exists` column and the `costs` row; `feat/greeks` added the `greeks`
row against the older four-column shape. Neither was wrong and the union is the
answer — the same resolution the `D-0037` identifier collision and the
`06-limits.md` §18 collision needed in the same merge. Three collisions, one
cause: **a branch that reads only its own copy of a shared append-only
document.**

---

## 1a. `costs` — added by D-0041, drawn here by D-0045

`crates/costs` shipped across D-0041, D-0043 and D-0044, and **this diagram did
not have it for any of them.** Each of those three entries recorded the omission
as outstanding rather than fixing it, because the crate that wrote them did not
own this file. It is drawn now.

It sits beside `store` and `vocab` as a direct child of `core`, and **its only
arrow points at `core`** — one dependency, `brutex_core`, for three things it
refuses to define twice: `Exchange` (the circulars are exchange-scoped),
`Paisa` (`CLAUDE.md` §7 fixes money at integer paisa) and the calendar
validator (`day::TradeDay` validates through `core`'s, rather than writing a
second leap-year rule). The graph stays acyclic; nothing depends on `costs`.

**Nothing consumes it yet.** `grep 'costs' crates/*/Cargo.toml` returns only its
own manifest. The intended consumer is the engine's trade-costing path, and
`trip::price` is the single call it needs — so `engine` gains `costs` as a
dependency when `engine` exists.

**`CLAUDE.md` §5 draws this same graph and still does not list `costs`.** That
file is session law and is not edited from here. The correction it needs is
recorded in D-0045 and reported to the operator.

---

**`api → pull` was added by D-0038**, and the diagram above still draws them as
siblings. `/pull` and `/store` are the operator's window onto ingest, and every
rule they render already has exactly one definition in `pull`: the validated
calendar (`session::Day`), the inclusive window and the vendor's non-inclusive
`toDate` (`session::Window::wire_to`), the drop reasons and their tally
(`session::{DropReason, DropCensus}`), and the per-vendor counter file
(`manifest::Manifest`). Re-deriving any of them inside `api` would be a second
Gregorian rule and a second answer to what goes on the wire. The graph is still
acyclic — `pull` does not depend on `api` — and the build order is unchanged.

`greeks` is a **leaf with no arrow into it and none out of it**, which is why
it is not drawn in the diagram above. It is not part of the sweep. It is shared
with the `tickvault` repository, which takes it by git URL, so it declares zero
dependencies for the same reason `core` does — and unlike `core`, its public
surface mentions no type from this workspace at all, only `f64` and plain enums
it owns. **Both halves of that are enforced by CI gate 9b**, in the same shape
as gate 9 for `core` and gate 7 for `web`; before D-0046 they were advertised in
six places and enforced in none. It is also the one place in this repository
where a float is the correct type: `CLAUDE.md` §7 keeps statistical values at
full precision and reserves `i64` paisa for prices, and this crate never sees a
paisa. Its `rust-version` is written literally rather than inherited, because
the MSRV of a shared leaf crate is a property of the crate and not of the
workspace hosting it. See `docs/05-decisions.md` D-0046.

**`CLAUDE.md` §5 does not list it.** That is a real discrepancy and §10 makes
`CLAUDE.md` the winner, so this row is the document running ahead of session
law rather than the other way round. `greeks` adds no arrow to §5's graph, so
it violates nothing in it; bringing §5 into line is an operator decision, and
it is **still open** — see D-0046, "One discrepancy an operator has to settle",
which carries the one-line repair.

---

## 2. Why `web` depends on `core` alone

It compiles to WebAssembly. There is no filesystem there. If `web` could
declare `store` as a dependency, someone would eventually call it, and the
failure would surface as a runtime panic in a browser rather than as a
compile error on a laptop.

The constraint is not a convention. `crates/web/Cargo.toml` lists one
dependency and CI gate 7 fails on any other.

The payoff: every display rule — how a price renders, how a percentage is
computed, what counts as a valid mask — lives in `core` and is compiled twice,
once native and once to WASM. One implementation, two targets, no drift
between what the server believes and what the browser shows.

---

## 3. Data flow, end to end

```
vendor
  │  pull: one request per window, rate-governed, resumable
  ▼
raw candles ──► validate ──► paisa integers ──► store::append
                                                    │  pwrite, then
                                                    │  publish n_valid
                                                    ▼
                                            bars/<exch>/<seg>/<sym>/<tf>/<yyyy-mm>.bin
                                                    │
                                            store::open (read-only mmap)
                                                    │
                                            indicators::compute  ── once per slice
                                                    ▼
                                            bar_bits: Vec<u128>   one word per bar
                                                    │
                                            engine::sweep
                                              k=1 frontier
                                              join + prune
                                              evaluate
                                              stop at extinction
                                                    ▼
                                            ranked survivors ──► results file
                                                    │
                                        api ────────┴──────── web (wasm)
```

Three properties matter more than the boxes:

1. **The disk is touched once per slice per launch.** After the initial open,
   every bar read is pointer arithmetic against a mapping that is already
   resident.
2. **Condition bits are computed once and shared read-only** across every
   thread and every candidate. In the predecessor system this recomputation
   was the dominant cost — roughly eleven thousand times the per-mask cost —
   and it was re-paid per worker per tuple.
3. **There is no boundary to cross.** Nothing is marshalled between runtimes,
   so no per-trade materialisation cost exists to be optimised later.

---

## 4. Path is the index

```
bars/groww/NSE/INDEX/NIFTY/1min/2024-03.bin
     │     │   │     │     │    └── the month file
     │     │   │     │     └─────── timeframe
     │     │   │     └───────────── symbol
     │     │   └─────────────────── segment
     │     └─────────────────────── exchange
     └───────────────────────────── vendor (D-0019)
```

Locating a slice is a string join and an open. There is no catalogue to
consult, no index to rebuild, no registry that can disagree with the
filesystem. Adding a symbol requires no registration: the first write creates
the directory.

Directory listings are never globbed on a read path. A read computes the exact
path it wants; if the file is absent, that is a specific, named absence rather
than a scan that returned nothing.

---

## 5. Concurrency

| Layer | Model | Shared mutable state |
|---|---|---|
| `pull` | async, one task per window, a governor between tasks and the vendor | none — each task owns its window |
| `indicators` | single pass, sequential by construction (state carries forward) | none |
| `engine` | data parallel over candidates via `rayon` | none — bar bits are read-only, each shard owns its own output |
| `store` append | one writer, positional writes, commit counter published last | the counter, published with a release store |
| `api` | async, request-scoped | none |

The rule that makes this hold: **a sweep never mutates anything a reader can
observe mid-flight.** Results accumulate per shard and merge once at the end.

---

## 6. Where the constant-time claims live

| Operation | Cost | Mechanism |
|---|---|---|
| Locate a slice | O(1) | path join |
| Read bar *i* | O(1) | `base + 32768 + i·56` — see `docs/02-store-format.md` §1 |
| Read condition bits for bar *i* | O(1) | index into a `Vec<u128>` |
| Test one candidate against one bar | O(1) | `(bits & mask) == mask` |
| Reject a duplicate candidate | O(1) | filter probe, then a sharded exact confirm |
| Append one result | O(1) amortised | bounded selector, fixed capacity |

Total sweep work is **not** constant — it scales with bars × candidates. Only
the per-operation cost is constant, and that is the only thing a design can
promise. `docs/06-limits.md` says the same thing at more length.
