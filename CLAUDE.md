# brutex-rs — session law

Read this file completely before any action. It outranks convenience,
precedent, and your own judgement about what would be easier.

---

## 1. What this is

A brute-force backtesting engine for Indian spot indices. It sweeps
combinations of boolean market conditions over historical 1-minute bars and
ranks what survives.

**Engine surface — exactly two instruments, NSE only:**
`NSE-NIFTY`, `NSE-BANKNIFTY`.

BSE and MCX are not swept and not pulled. Narrowed from three
instruments by D-0017. Existing BSE data already on disk is not deleted --
append-only history applies to the store as well.

`NSE-INDIAVIX` is **reference only**: it is stored and it is stamped onto
observable trades, but it never enters the condition vocabulary, never enters
ranking, and never enters run identity.

Futures, options and single stocks may be **stored**. They are never swept.

---

## 2. The one hard rule

**Rust is the only language in this repository.**

Allowed tracked extensions: `.rs` `.toml` `.md` `.lock` `.html` `.css` `.yml`
(the last only under `.github/`).

Forbidden without exception:
- any interpreted runtime, as a dependency, a dev-dependency, or a tool
- any `build.rs` that invokes an external process
- any vendored binding to another language
- any generated source in another language, checked in

If a task appears to require one of these, **stop and say so**. Do not add it
and explain afterwards.

CI gate 1 enforces this by walking every tracked file. It is not advisory.

---

## 3. Golden rules

1. **No invention.** Every claim about a vendor, an exchange, an instrument or
   a cost is traceable to a source recorded in `docs/00-charter.md`. If you are
   unsure, write `UNVERIFIED` and stop.
2. **No silent scope change.** The engine surface in §1 does not widen OR
   narrow without a new entry in `docs/05-decisions.md`.
3. **Reproducibility.** Every run is identified by
   `blake3(mask ‖ direction ‖ instrument ‖ timeframe ‖ params ‖ data_digest ‖
   vocab_version ‖ commit)`. No computation without that identity recorded.
4. **Constant per-operation cost.** Bar lookup, condition lookup, mask
   evaluation, duplicate rejection and result append are each O(1). A change
   that makes one of them scan fails the bench gate.
5. **Idempotence.** Same inputs, same outputs, byte for byte. Reruns are safe.
6. **Honest limits.** If a bound cannot be met, say so. Never claim a
   measurement you did not take. Label extrapolations as extrapolations.
7. **No look-ahead.** At bar N the engine may read bars 0..N. Enforced by an
   index-guarded accessor, not by review.
8. **Append-only history.** Condition bits are never renumbered or reused.
   Store format versions are never mutated in place.

---

## 4. What may never appear

| Banned | Because |
|---|---|
| A depth parameter on the sweep | Depth is decided by extinction, not by a caller. See §6. |
| A query planner or ORM | The path is the index. There is nothing to plan. |
| A dynamic schema | A new field is a new file version at its own stride. |
| A writable memory mapping | It raises a signal on a full disk, and a signal cannot be caught. |
| A fallback that hides a failure | Degrade loudly and name the reason, or refuse. Never both silently. |
| A test that asserts nothing | A surviving mutant is a missing test and blocks the build. |

---

## 5. Crate graph — acyclic

```
core   (no dependencies)
 ├── store        fixed-stride bar files
 ├── indicators   bars in, condition bits out
 ├── vocab        the bit table and mask operations
 ├── engine       sweep, ranking
 ├── pull         vendor ingest
 ├── api          HTTP
 ├── web          browser UI, wasm32 — depends on core ONLY
 └── cli          operator entry point
```

`web` declaring any dependency other than `core` is a build failure, not a
review comment. It compiles to WebAssembly where the filesystem does not exist.

---

## 6. Sweep depth

**There is no `k` parameter.** Not a default, not a token, not an environment
override. The type does not carry the field.

The sweep walks the combination ladder upward from k=1 and stops where the
frequent frontier empties — classic Apriori level-wise join and subset-prune,
justified by anti-monotonicity: a mask hits a bar iff `(bits & mask) == mask`,
so adding a required bit can only remove hits. If any (k−1)-subset is
infrequent, the k-combination cannot be frequent, and is never enumerated.

*Why the parameter is absent rather than defaulted:* in the predecessor
repository the flag defaulted to a dynamic token, but the frequent-frontier
mask was 64 bits wide against a 74-condition vocabulary. Every real run tripped
the width guard and silently fell back to a hardcoded `k = [1, 2]`. Dynamic
depth was unreachable on the only vocabulary that existed, and nobody could see
it from the flag. A parameter that can be set can be set wrongly and silently.

---

## 7. Money and prices

- Prices are **paisa integers** (`i64`). Never a float.
- The tick grid is 2 decimal places. Snapping happens once, at the write
  boundary, half-up.
- `i64::MIN` is the open-interest null sentinel. Zero means zero.
- Statistical values (Sharpe, p-values, ratios) keep full precision and are
  never rounded for storage.

---

## 8. Credentials

Read-only, from AWS Parameter Store SecureStrings, region `ap-south-1`.

**No literal parameter path appears in any tracked file.** This repository is
public. Paths are written here and in the documents only as their generic
shape:

```
/<org>/<env>/<vendor>/<field>
```

The real path segments are supplied at runtime from a local, untracked
configuration file that is never committed. `crates/pull` holds the shape and
the field names; it holds no `org`, `env`, or `vendor` literal. A missing or
malformed configuration halts the pull loudly — there is no default and no
fallback. See `docs/05-decisions.md` D-0013 and CI gate 1c.

The **credential value** is never an environment variable, **never** a file,
**never** a prompt. Only the *path* comes from the local configuration; the
secret itself is read from Parameter Store and nowhere else.
**This repository never mints a token.** A stale token is re-read; if the
re-read returns the same dead value, the pull halts loudly. A local mint would
invalidate the token another system shares.

---

## 9. Definition of done

A change is done when all of these are true, verified not assumed:

- `cargo fmt --check` clean
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace --locked` green
- `cargo deny check` green
- line and branch coverage 100% on every touched crate
- no surviving mutant on touched modules
- every new invariant appears in `docs/04-invariants.md` beside the test that
  proves it
- a `docs/05-decisions.md` entry exists for every locked choice

Report failures plainly. Do not paper over a red gate.

---

## 10. Documents

| File | Authority over |
|---|---|
| `docs/00-charter.md` | scope, verified external facts, prohibitions |
| `docs/01-architecture.md` | crates, arrows, data flow |
| `docs/02-store-format.md` | bytes on disk |
| `docs/03-vocabulary.md` | condition bit table |
| `docs/04-invariants.md` | what must hold, and its proof |
| `docs/05-decisions.md` | append-only ledger |
| `docs/06-limits.md` | what is not constant-time, and what is unmeasured |

If this file and a document disagree, **this file wins** and the document is
the stale copy to fix.
