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
| 3 | Maps | Pre-sized with headroom | `HashMap::with_capacity(reservation_for(n))`, factor 2. **Lookup** is O(1) worst case. **Append** is O(1) worst case for the first `n_valid` calls after a load, and **amortised** O(1) after that — see below | ◐ |
| 4 | Membership | No search of any kind | Open-addressed table built at compile time. **Never `binary_search`** | ✓ |
| 5 | Address | Arithmetic, never lookup | `base + header + i·stride`. The path is the index | ✓ |
| 6 | Hot data | 16 bytes per bar, not 56 | Bit plane separate from bar plane. 8 × `u128` per 128-byte cache line, zero straddle | ✓ |
| 7 | Residency | Load once, never re-read | Pin the bit plane. 7.75 GB of 48 GB — it never exceeds RAM at this scale | ○ |
| 8 | Evaluation | One instruction | `(bits & mask) == mask` on a `u128`, no branch | ✓ |
| 9 | Allocation | **Zero in the hot loop** | Preallocated frontier. No `format!`, no `String`, no `push` | ○ |
| 10 | Parallelism | Work-stealing, never static | 10 P-cores. A static split stalls waiting on the 4 efficiency cores | ○ |
| 11 | Blocks | Whole records only | `BLOCK_LEN` is a whole multiple of the stride, so straddling is **unrepresentable** rather than handled | ✓ |
| 12 | Rendering | Bounded page **answered from a precomputed order** | A fixed row cap is not enough — see below. `crates/api/benches/ratio.rs`, C-14 … C-17. **Substring search is the named exception and stays O(universe)** | ◐ |
| 13 | Counting | Counters, never scans | A manifest per vendor. One read, not a walk of every file | ◐ |

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
| Page render, 2,787 → 50,000 instruments | **0.929× – 1.088×**, every sort column × pill, plus the hatch and a clamped deep page | Layer 12. `cargo bench -p api`, exit 0, 2026-08-07 — re-measured for D-0045. Absolute ~137–166 µs at *both* sizes, release profile. Marginal cost of one more instrument: **0 – 259 ps** per request (C-15), against 85,400 ps before D-0042 |
| Dashboard, 2 → 50,000 instruments | **1.052×** | Layer 12. Was 80.640× under a docstring that already said "nothing here scans" (C-16) |
| Instruments **search**, 2,787 → 50,000 | **6.53 ms** at n = 50,000 for a 2-byte needle | **NOT flat and not asserted.** Printed by the same bench, never gated. `docs/06-limits.md` §24 |
| Checksum per block | **~7,050 ns** table, from 15,622 ns bitwise | The hardware instruction reaches 381.9 ns; see `docs/06-limits.md` |
| GPU vs all 14 cores | **66 ms vs 142 ms — 2.1×** | Compute-bound: 1,219 GB/s effective, 5.3× measured DRAM bandwidth |
| Bar read past RAM | ~100 ns → **61,566 ns** | 616×. Physics. Layer 7 exists because of it |
| Manifest census vs deriving it | **193,449×** and **402,568×** on two runs | Layer 13. The counter against decoding 10,000 entries, same process. The counter side has been seen at 378–789 ps across runs while the scan holds at ~152 µs; **the cause of that spread is not established** — see below. D-0035, D-0036 |
| Manifest entry lookup, 1→100× census | **0.994–1.049×** | Layer 3 in the layer-13 file: reserved from a known bound, so no rehash — measured on the map a **loaded** manifest holds, which is the only one that reservation applies to (D-0036) |

**Not O(1), and never claimed to be:** the sweep. Apriori over the vocabulary is
combinatorial — each *step* is 0.2 ns, the number of steps is not constant.
`CLAUDE.md` §3 rule 4 says "constant **per-operation** cost" for that reason.

**The C-11 spread was explained here, and the explanation was wrong.** This
table said "the spread is the counter side at the clock's floor". The bench now
measures the clock and prints it: the smallest non-zero interval `Instant`
reports on this host is **41 ns**, and the counter side is averaged over 100,000
repetitions, so one tick is **410 fs** of reported cost — the 789 ps observation
is 1,924 ticks. The counter side is three orders of magnitude above the clock
floor, so the floor is not what moves it. The observation stays; the cause is now recorded as unestablished.
`CLAUDE.md` §3 rule 6 — never claim a measurement you did not take, and a causal
claim asserted as measured fact is that. D-0036.

**Layer 12 was `✓` with the words "a fixed row cap with paging. Never
O(universe)", and the words were false while the row cap was real.** Only the
*rendered rows* were capped. Every request still re-folded the whole instrument
map for the pill counts, re-filtered it into a fresh `Vec`, re-sorted it and
reversed it — to draw a fixed 200 rows. An audit measured **3.569 ms at 2,787
instruments and 124.916 ms at 50,000, rendering exactly 200 rows both times**:
62.5 ns per instrument per request, on a page whose output never changed shape.
**The row cap is what hid it**, and this table's tick is what carried it. D-0042
moved every ordering and every filter into one build at load time; the layer is
now backed by a bench that asserts a ratio rather than by a sentence.

It is `◐` and not `✓` for two reasons, both named rather than rounded off:

- **Substring search is O(universe) and no index fixes it.** A needle of one or
  two bytes has no trigram to narrow with, and a needle whose rarest trigram
  most of the universe carries narrows to nearly the universe. Measured and
  printed by the same bench, asserted by none of it: **6.53 ms at n = 50,000**.
  `docs/06-limits.md` §24 states why a 1-gram/2-gram index would cost memory and
  buy nothing.
- **The catalog build itself has no ceiling.** The bench prints it (41 ms for
  all three universes together) and asserts nothing about it, so a build that
  turned quadratic would fail no gate. That is a real hole in this layer.

**Layer 3 was `✓` with "zero rehash, so O(1) worst case rather than average",
and D-0040 found the append could still rehash.** The reservation was
`with_capacity(n_valid)` — exactly the census, no free slot — so the *first*
append after a load rebuilt the table at every census of the form `7·2^k`. It
measured **5,100,585 ps per append at a 57,344-entry census**, and it passed
every round-number bench because `with_capacity` happened to round 1,000 /
10,000 / 50,000 up and leave spare slots. The reservation is now the census
doubled, capped at `MAX_ENTRIES`. What that buys is stated exactly: O(1) worst
case for the first `n_valid` appends after a load, **amortised** O(1) after
that. `pull::unit::a_loaded_index_carries_headroom_for_the_appends_after_it`
(M-19) asserts the growth past the headroom, deliberately, so the row cannot be
read as an unconditional claim. `docs/06-limits.md` §23.

**Layer 13 is `◐`, not `✓`.** The manifest exists, its three totals are
maintained on write and checked against the entries on load, and both bounds
above are measured. What is *not* built is a **filtered** census — "how many
expired option series" still walks the manifest's own entries, which is one
sequential file read instead of ~248,000 directory operations but is not a
counter read. The counters are also never checked against `bars/` itself, so
they can drift from the files they describe in both directions and nothing here
detects it. And the directory walk the file replaces has never been measured
here at all; every statement about that saving is an **EXTRAPOLATION**. See
`docs/06-limits.md` §17.

---

## How a layer is proven

A layer is not built because the code looks right. It is built when a test
asserts the bound as a **number**:

- Layer 4's probe length is asserted at `<= 8` and printed. The first attempt
  measured **14** — worse than the `binary_search` it replaced, still O(1) by
  definition — and the test refused it until the table was widened.
- Layer 8's flatness is asserted against the 1.4× ceiling in
  `docs/04-invariants.md`, measured across every mask width.
- Layer 3's guarantee is the *absence* of a rehash **over a stated number of
  appends**, so the bound is the reservation itself, taken from a count known
  before the loop starts — and the number of appends it covers is part of the
  claim, not a detail. Stated without that clause (as it was until D-0045) the
  sentence is broader than the code.
- Layer 12's flatness needs a **second** measurement to mean anything: cost per
  *rendered row* (C-17). A page that got flat by rendering less would pass every
  ratio and be worse than what it replaced. That is the same class of mistake as
  capping the rows and calling the page constant, which is exactly what this
  layer shipped with.

Two traps this has already hit, recorded so they are not hit again:

**`const fn` is invisible to coverage.** A table built at compile time is
executed by the compiler, so runtime instrumentation sees none of it — the same
hole as an unreachable branch, arriving by a different route. It needs a test
that calls the builder in a non-`const` context.

**A collision branch needs a real collision.** Filling a small table does not
necessarily collide; the probe branch then stays unentered while the test
passes. The colliding pair is computed against the actual hash, not hoped for.
