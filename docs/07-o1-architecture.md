# 07 — The O(1) architecture

Thirteen layers and five laws. Each layer is a rule about what the code **may
do**, not a hope about how fast it will run. Break any one and every layer above
it stops being constant time.

Status: `✓` built · `◐` partly built · `○` not built.

---

## The five laws

Every layer below is one of these applied somewhere specific.

**1 · Fixed width everywhere.** Variable-length input means variable-time
hashing. A 2-letter ticker and a 24-letter one must cost the same.

**2 · Pre-size every map.** Growth is the *only* source of O(n) in a hash table.
Reserve the bound up front and the word "amortised" leaves the guarantee.

**3 · Never scan to answer a question.** Maintain a counter instead. "How many
do I have?" must be a read, not a walk.

**4 · Arithmetic beats lookup.** If the address can be computed, never search
for it.

**5 · Bound every input at the boundary.** Unbounded input is unbounded time.
Every O(1) claim dies at the first unbounded input, and unbounded input always
arrives from outside.

---

## The thirteen layers

| # | Layer | The rule | How | |
|---|---|---|---|---|
| 1 | Identity | Fixed width, never variable | `Symbol` 24 B, `Isin` 12 B, `InstrumentKey` is `Copy` with a structural hash | ✓ |
| 2 | Hashing | No cryptographic hash on a trusted path | FNV-1a, not SipHash. `core` may declare no dependency (gate 9), so the hash is four `const` lines rather than a crate | ◐ |
| 3 | Maps | **Pre-sized, never grow** | `HashMap::with_capacity(bound)` — zero rehash, so O(1) **worst case** rather than average | ✓ |
| 4 | Membership | No search of any kind | Open-addressed table built at compile time. **Never `binary_search`** | ✓ |
| 5 | Address | Arithmetic, never lookup | `base + header + i·stride`. The path is the index | ✓ |
| 6 | Hot data | 16 bytes per bar, not 56 | Bit plane separate from bar plane. 8 × `u128` per 128-byte cache line, zero straddle | ✓ |
| 7 | Residency | Load once, never re-read | Pin the bit plane. 7.75 GB of 48 GB — it never exceeds RAM at this scale | ○ |
| 8 | Evaluation | One instruction | `(bits & mask) == mask` on a `u128`, no branch | ✓ |
| 9 | Allocation | **Zero in the hot loop** | Preallocated frontier. No `format!`, no `String`, no `push` | ○ |
| 10 | Parallelism | Work-stealing, never static | 10 P-cores. A static split stalls waiting on the 4 efficiency cores | ○ |
| 11 | Blocks | Whole records only | `BLOCK_LEN` is a whole multiple of the stride, so straddling is **unrepresentable** rather than handled | ✓ |
| 12 | Rendering | Bounded page, never the set | A fixed row cap with paging. Never O(universe) | ✓ |
| 13 | Counting | Counters, never scans | A manifest per vendor. One read, not a walk of every file | ○ |

---

## Always · never

| ✓ Always | ✗ Never |
|---|---|
| Fixed-size types | Growing a map in a hot path |
| `with_capacity` from a known bound | `binary_search` behind an O(1) label |
| Compute the offset | Cryptographic hashing on a trusted path |
| Counters maintained on write | Allocating inside a loop |
| Refuse over-long input, loudly | Scanning to count |
| Work-stealing across cores | Accepting unbounded input |

---

## What is measured, and what is not

Measured on an Apple M4 Pro, 48 GB, macOS 26.5.2, rustc 1.97.1.

| Operation | Measured | Note |
|---|---|---|
| One mask evaluation | **0.2007–0.2101 ns**, flat 1→74 bits (**1.047×**) | The hardware floor. Identical at 0%, 28% and 100% hit rate, so no data-dependent branch |
| Universe membership | worst probe **6** (750 members) / **7** (213 members) | Replaced ~10 comparisons that grew with the list |
| Page render | **0.35–0.6 ms** | Was 150 ms when it re-parsed the masters per request |
| Checksum per block | **~7,050 ns** table, from 15,622 ns bitwise | The hardware instruction reaches 381.9 ns; see `docs/06-limits.md` |
| GPU vs all 14 cores | **66 ms vs 142 ms — 2.1×** | Compute-bound: 1,219 GB/s effective, 5.3× measured DRAM bandwidth |
| Bar read past RAM | ~100 ns → **61,566 ns** | 616×. Physics. Layer 7 exists because of it |

**Not O(1), and never claimed to be:** the sweep. Apriori over the vocabulary is
combinatorial — each *step* is 0.2 ns, the number of steps is not constant.
`CLAUDE.md` §3 rule 4 says "constant **per-operation** cost" for that reason.

---

## How a layer is proven

A layer is not built because the code looks right. It is built when a test
asserts the bound as a **number**:

- Layer 4's probe length is asserted at `<= 8` and printed. The first attempt
  measured **14** — worse than the `binary_search` it replaced, still O(1) by
  definition — and the test refused it until the table was widened.
- Layer 8's flatness is asserted against the 1.4× ceiling in
  `docs/04-invariants.md`, measured across every mask width.
- Layer 3's guarantee is the *absence* of a rehash, so the bound is the
  reservation itself, taken from a count known before the loop starts.

Two traps this has already hit, recorded so they are not hit again:

**`const fn` is invisible to coverage.** A table built at compile time is
executed by the compiler, so runtime instrumentation sees none of it — the same
hole as an unreachable branch, arriving by a different route. It needs a test
that calls the builder in a non-`const` context.

**A collision branch needs a real collision.** Filling a small table does not
necessarily collide; the probe branch then stays unentered while the test
passes. The colliding pair is computed against the actual hash, not hoped for.
