# 04 — Invariants

Every row names the test that proves it. **An invariant with no test named
beside it is deleted from this file, not shipped.** A consistency check in CI
fails if a row's test does not exist.

Status: `✓` proven · `○` test written, awaiting the crate · `—` not yet
reachable (the crate does not exist).

---

## Store

| # | Must hold | Proven by | |
|---|---|---|---|
| S-01 | `size_of::<Bar>() == 56` and `align_of::<Bar>() == 8` | `const _: () = assert!(…)` — a compile error, not a test | — |
| S-02 | `read(i)` returns the bytes `write(i, b)` wrote, for every *i* | `store::proptest::roundtrip` | — |
| S-03 | A reader never observes a record beyond `n_valid` | `store::loom::commit_counter_publishes_last` | — |
| S-04 | A crash between data write and counter publish loses the tail and corrupts nothing | `store::fault::kill_between_write_and_commit` | — |
| S-05 | A full disk during append returns `Err`, never a signal | `store::fault::enospc_returns_error` | — |
| S-06 | A flipped bit in any block is detected on the next read of that block | `store::fault::bitflip_detected` | — |
| S-07 | A file whose length does not divide by the stride truncates to the last whole record and logs | `store::fault::ragged_tail_truncates_loudly` | — |
| S-08 | `i64::MIN` in `open_interest` round-trips as null and is never confused with `0` | `store::proptest::oi_sentinel_distinct` | — |
| S-09 | Opening a file with an unknown `format_version` refuses; it never guesses | `store::unit::unknown_version_refuses` | — |
| S-10 | Two concurrent writers on one file are refused by the advisory lock | `store::integration::second_writer_refused` | — |

## Vocabulary and indicators

| # | Must hold | Proven by | |
|---|---|---|---|
| V-01 | Condition bit indices are stable across releases | `vocab::golden::bit_table_frozen` (byte-frozen fixture) | — |
| V-02 | At bar *i* the evaluator reads no bar `> i` | `indicators::barrier::no_lookahead` (index-guarded accessor) | — |
| V-03 | Bits `0..=(i)` are identical whether bars `i+1..` are absent, mutated, or extreme | `indicators::proptest::suffix_independence` | — |
| V-04 | Time-of-day and VWAP bits are cleared on a daily timeframe | `indicators::unit::daily_mask_clears` | — |
| V-05 | The fast evaluator agrees with a naive reference on random input | `indicators::proptest::differential_vs_naive` | — |
| V-06 | Bits are computed exactly once per slice | `indicators::bench::compute_call_count` (a counting spy) | — |

## Sweep

| # | Must hold | Proven by | |
|---|---|---|---|
| E-01 | `(bits & mask) == mask` is anti-monotone over mask supersets | `engine::proptest::antimonotone` | — |
| E-02 | The Apriori kept-set equals the brute-force kept-set, exactly | `engine::proptest::apriori_equals_bruteforce` (exhaustive at small *n*) | — |
| E-03 | Emission order matches the reference enumeration, so the ranked list is bit-identical, not merely set-equal | `engine::golden::emission_order` | — |
| E-04 | **No depth parameter exists on any public sweep entry point** | `engine::unit::no_depth_field` (reflects over the request type) | — |
| E-05 | The ladder terminates when a level produces no frequent candidate | `engine::unit::extinction_terminates` | — |
| E-06 | A duplicate candidate is rejected without a full scan | `engine::bench::dedup_ratio` | — |
| E-07 | A rerun with identical inputs produces byte-identical output | `engine::golden::rerun_byte_identical` | — |
| E-08 | Peak memory stays under the declared budget at the declared candidate count | `engine::bench::peak_rss` | — |

## Complexity — gate 8

| # | Must hold | Proven by | |
|---|---|---|---|
| C-01 | Bar read cost is flat from 1× to 100× file size | `store::bench::read_ratio` | — |
| C-02 | Per-candidate evaluation cost is flat from 1× to 100× candidate count | `engine::bench::eval_ratio` | — |
| C-03 | Duplicate-rejection cost is flat from 1× to 100× seen-set size | `engine::bench::dedup_ratio` | — |
| C-04 | Result append cost is flat from 1× to 100× results held | `engine::bench::append_ratio` | — |

**Ceiling.** A ratio is a failure above **1.4×** on dedicated hardware,
**3.0×** on shared CI. The gap is measurement noise on shared vCPU, not a
different standard — a ratio above 3.0× on shared hardware is a real
regression, and a number between 1.4 and 3.0 on shared hardware is re-run on
dedicated hardware before it is believed.

## Ingest

| # | Must hold | Proven by | |
|---|---|---|---|
| P-01 | A rate governor never issues above the configured ceiling, under any concurrency | `pull::loom::governor_ceiling` | — |
| P-02 | A bar outside the requested window is never stored | `pull::unit::window_boundary` | — |
| P-03 | A bar on a non-trading date is dropped and counted | `pull::unit::calendar_filter` | — |
| P-04 | Re-running an ingest stores nothing new and reports zero net-new | `pull::integration::idempotent_repull` | — |
| P-05 | A credential is read, never written; no token is ever minted | `pull::unit::readonly_credentials` (a write attempt must panic the test double) | — |
| P-06 | An auth failure halts the pull loudly rather than degrading | `pull::unit::auth_halt` | — |
| P-07 | A missing, unreadable, or incomplete credential configuration halts the pull and names the absent segment; it never defaults | `pull::unit::credential_config_absent_halts` | — |
| P-08 | The credential configuration supplies path segments only; a secret value found in it is refused | `pull::unit::credential_config_rejects_secret_value` | — |

## Cross-cutting

| # | Must hold | Proven by | |
|---|---|---|---|
| X-01 | Run identity changes if any loaded bar differs by one field | `core::proptest::identity_sensitivity` | — |
| X-02 | Prices never touch a float on any path from wire to store to result | `core::lint::no_float_in_price` (a clippy deny plus a source check) | — |
| X-03 | Every tracked file has an allowed extension | CI gate 1 | ✓ |
| X-04 | No build script invokes an external process | CI gate 2 | ✓ |
| X-05 | `web` depends on `core` alone | CI gate 7 | ✓ |
| X-06 | Line and branch coverage is 100% on every crate | CI coverage job | ✓ |
| X-07 | No mutant survives on a touched module | `cargo-mutants`, scheduled | — |
| X-08 | No tracked file contains a literal credential path | CI gate 1c | ✓ |
| X-09 | `core` declares no dependency at all | CI gate 9 | ✓ |
| X-10 | Every reachable row in this file names a test that exists | CI gate 10 | ✓ |
| X-11 | A price is constructible from a float only through the one checked conversion | `core::price::refuses_an_out_of_range_price_instead_of_saturating` (private field; no other path exists) | ✓ |

---

## How this file stays honest

1. A new guarantee anywhere in the codebase adds a row **here first**.
2. The row names a test. Not a plan for a test.
3. CI checks that each named test exists. A row pointing at nothing fails the
   build, which is what stops this file from decaying into a wish list.
4. Rows are never deleted to make a build pass. Either the invariant holds or
   the decision to abandon it is recorded in `docs/05-decisions.md`.
