# 01 — Architecture

Nine crates. Every arrow points one way. The graph is acyclic and the linker
enforces it.

---

## 1. The graph

```
                          ┌────────┐
                          │  core  │   no dependencies at all
                          └───┬────┘
        ┌──────────┬──────────┼──────────┬──────────┬─────────┐
        │          │          │          │          │         │
    ┌───▼───┐  ┌───▼────┐ ┌───▼───┐  ┌───▼───┐  ┌───▼──┐  ┌───▼──┐
    │ store │  │ vocab  │ │  web  │  │ pull  │  │ api  │  │ cli  │
    └───┬───┘  └───┬────┘ └───────┘  └───┬───┘  └──┬───┘  └──┬───┘
        │          │       wasm32        │         │         │
        │      ┌───▼──────────┐          │         │         │
        └─────►│ indicators   │          │         │         │
               └───┬──────────┘          │         │         │
                   │                     │         │         │
               ┌───▼──────┐              │         │         │
               │  engine  │◄─────────────┴─────────┴─────────┘
               └──────────┘
```

| Crate | Owns | May depend on |
|---|---|---|
| `core` | types, error enums, the condition bit table, pure rules, the calendar | nothing |
| `store` | the fixed-stride bar file: open, read, append, verify | `core` |
| `vocab` | mask type, mask operations, the frequent-frontier structure | `core` |
| `indicators` | bars in, condition bits out | `core`, `store` |
| `engine` | Apriori generation, evaluation, trade walk, ranking | `core`, `store`, `vocab`, `indicators` |
| `pull` | vendor ingest, rate governor, credential read | `core`, `store` |
| `api` | HTTP surface | `core`, `store`, `engine` |
| `web` | browser UI, compiled to `wasm32-unknown-unknown` | **`core` only** |
| `cli` | operator entry point | everything |
| `greeks` | Black-Scholes-Merton in `f64`: the five greeks, implied volatility, the strike ladder | **nothing** |

`greeks` is a **leaf with no arrow into it and none out of it**, which is why
it is not drawn in the diagram above. It is not part of the sweep. It is shared
with the `tickvault` repository, which takes it by git URL, so it declares zero
dependencies for the same reason `core` does — and unlike `core`, its public
surface mentions no type from this workspace at all, only `f64` and plain enums
it owns. It is also the one place in this repository where a float is the
correct type: `CLAUDE.md` §7 keeps statistical values at full precision and
reserves `i64` paisa for prices, and this crate never sees a paisa. See
`docs/05-decisions.md` D-0036.

**`CLAUDE.md` §5 does not list it.** That is a real discrepancy and §10 makes
`CLAUDE.md` the winner, so this row is the document running ahead of session
law rather than the other way round. `greeks` adds no arrow to §5's graph, so
it violates nothing in it; bringing §5 into line is an operator decision.

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
