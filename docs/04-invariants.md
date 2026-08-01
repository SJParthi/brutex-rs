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
| S-11 | The checksum reproduces a hardcoded value at every length a wide kernel can break on — 0, 1, 8, 15, 16, 56, 60, 64, 4087, 4088, 4089 bytes, and the all-zero and all-ones block | `store::unit::the_crc_reproduces_a_hardcoded_value_at_every_length_that_can_break` | ✓ |
| S-12 | The shipped checksum kernel agrees with an independent bit-by-bit reference at every length across a stride boundary, on every target | `store::unit::the_fast_kernel_agrees_with_a_bit_by_bit_reference_on_every_length` | ✓ |
| S-13 | The header slot's covered domain is exactly bytes `0..56 ‖ 60..64`; filling the four-byte hole is a different number and is refused as such | `store::unit::the_covered_domain_is_the_slot_minus_its_checksum` | ✓ |
| S-14 | Splitting the checksum input at any point gives the answer for the whole | `store::unit::splitting_the_input_anywhere_gives_the_same_checksum` | ✓ |
| S-15 | The lookup table is the polynomial: lane 0 is one folded byte, and lane *k* is lane *k−1* advanced by one zero byte | `store::crc::the_table_is_the_polynomial_lane_by_lane` | ✓ |

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
| C-01 | Reading the header costs the same at 1×, 10× and 100× the region offered | `store::bench::header_read_is_flat` | ✓ |
| C-02 | Per-candidate evaluation cost is flat from 1× to 100× candidate count | `engine::bench::eval_ratio` | — |
| C-03 | Duplicate-rejection cost is flat from 1× to 100× seen-set size | `engine::bench::dedup_ratio` | — |
| C-04 | Result append cost is flat from 1× to 100× results held | `engine::bench::append_ratio` | — |
| C-07 | Sealing one block costs the same at 1×, 10× and 100× the file's record count | `store::bench::block_seal_is_flat` | ✓ |
| C-08 | The block checksum beats the bit-by-bit kernel it replaced by at least 3×, measured in the same process | `store::bench::checksum_beats_the_bit_loop` | ✓ |
| C-09 | Decoding one vendor row costs the same whether a field is 28 bytes or 4 MiB | `core::bench::decode_is_flat_in_field_width` | ✓ |
| C-10 | An over-wide vendor field is **refused**, not merely decoded quickly | `core::bench::an_over_wide_row_is_refused` | ✓ |

C-01 was previously stated as "bar read cost is flat from 1× to 100× file size"
and proven by `store::bench::read_ratio`, which did not exist — there is no bar
reader in `crates/store` yet. D-0034 restated it as the flatness that **is**
measurable today and named a bench that runs. The bar-read row returns when the
reader does.

**Ceiling.** A ratio is a failure above **1.4×** on dedicated hardware,
**3.0×** on shared CI. The gap is measurement noise on shared vCPU, not a
different standard — a ratio above 3.0× on shared hardware is a real
regression, and a number between 1.4 and 3.0 on shared hardware is re-run on
dedicated hardware before it is believed. The harnesses assert the CI number
and print the ratio, so a local run can be read against the tighter one.

**These rows are enforced by a gate that ran for the first time on
2026-08-01.** Before D-0034 gate 8 probed for a repository-root `benches/`
directory, found none, and exited zero — see `docs/06-limits.md` §7b and §7c.

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

## Instrument identity and the vendor merge

Added by D-0024. Every row here is proven by a test that runs today.

| # | Must hold | Proven by | |
|---|---|---|---|
| I-01 | A row on the equity segment that is not a share is declined, and the share of the same name is kept — whichever order the file lists them in | `core::vendor::the_cholafin_bond_is_declined_and_the_cholafin_share_is_kept` | ✓ |
| I-02 | Every measured debt and fund class of either vendor is declined by name, not by falling through a default | `core::vendor::every_measured_debt_and_fund_class_is_declined_by_name` | ✓ |
| I-03 | An index row is never gated on a listing class it does not have, from either vendor | `core::vendor::an_index_row_survives_the_gate_from_both_vendors` | ✓ |
| I-04 | The listing class is trimmed before it is read, so padding cannot decline every share in a file | `core::vendor::dhans_class_column_is_trimmed_before_it_is_read` | ✓ |
| I-05 | An SME listing is declined under its **own** reason and never lumped in with debt | `core::vendor::the_equity_board_is_kept_and_the_sme_board_is_declined_separately` | ✓ |
| I-06 | An ISIN is refused unless its ISO 6166 check digit verifies | `core::isin::the_one_real_row_with_a_bad_check_digit_is_refused` | ✓ |
| I-07 | The one real ISIN that fails its check digit never reaches the parse, because the equity gate declines it first | `core::vendor::the_sdl_with_the_bad_check_digit_never_reaches_the_isin_parse` | ✓ |
| I-08 | A kept equity carries a parseable ISIN or the row is an error; it is never a quiet `None` | `core::vendor::a_kept_equity_must_carry_a_parseable_isin` | ✓ |
| I-09 | An index carries no ISIN, and no sentinel is invented for one | `api::merge::an_index_carries_no_isin_and_still_merges_on_identity` | ✓ |
| I-10 | Two vendors giving one key two different ISINs is **one** key and one loud line naming both; neither is dropped and neither wins | `api::merge::a_cross_vendor_isin_conflict_is_reported_and_neither_side_is_dropped` | ✓ |
| I-11 | A series suffix is stripped only when a second vendor confirms the identity by ISIN | `api::merge::a_suffixed_symbol_merges_only_when_the_isin_confirms_it` | ✓ |
| I-12 | An unconfirmed suffix is left exactly as the vendor wrote it | `api::merge::an_unconfirmed_suffix_is_left_exactly_as_the_vendor_wrote_it` | ✓ |
| I-13 | A trailing dash that is not the row's own series is never stripped, so `BAJAJ-AUTO` survives | `core::vendor::a_dash_that_is_not_the_rows_own_series_is_never_stripped` | ✓ |
| I-14 | Every column a vendor's reader needs is required by name, and a missing one is refused and named | `api::master::every_column_a_vendor_needs_is_required_and_named_when_absent` | ✓ |
| I-15 | The binary's entry point is measured by running it, not exempted from the coverage gate | `api::binary::the_binary_reports_what_it_read_and_exits_zero` | ✓ |
| I-16 | A vendor field wider than `MAX_FIELD_BYTES` is refused **before** anything reads it, whichever of the ten fields it is | `core::vendor::an_over_wide_field_is_refused_whichever_field_it_is` | ✓ |
| I-17 | The width bound is the first byte that is too many and not one before, and it never substitutes for the parsers below it | `core::vendor::the_bound_is_the_first_byte_that_is_too_many_and_not_one_before` | ✓ |
| I-18 | The widest value measured in either real master still passes the width gate untouched | `core::vendor::a_row_of_ordinary_width_passes_the_gate_untouched` | ✓ |
| I-19 | The width gate runs before the test-marker scan and does not shadow it | `core::vendor::the_test_marker_scan_still_declines_a_real_test_listing` | ✓ |
| I-20 | A master larger than the reader holds is refused from its size, before it is read into memory | `api::master::a_master_larger_than_this_reader_holds_is_refused_before_it_is_read` | ✓ |
| I-21 | A row longer than the reader splits is named at its line number and never split | `api::master::a_row_longer_than_this_reader_splits_is_named_and_never_split` | ✓ |
| I-22 | An over-wide field makes the row an error that names the field; it is never a silent keep | `api::master::a_field_wider_than_core_will_read_is_an_error_and_not_a_silent_keep` | ✓ |

## The equity gate after D-0025, and the refusal after D-0026

Added by D-0025, D-0026 and D-0027. Every row here is proven by a test that
runs today.

| # | Must hold | Proven by | |
|---|---|---|---|
| I-16 | The three measured series tables are sorted, mutually disjoint and non-empty, so `binary_search` cannot return garbage and no code can get two verdicts | `core::vendor::the_measured_series_tables_are_sorted_disjoint_and_complete` | ✓ |
| I-17 | A cash-equity row whose series this engine has never seen gets its **own** reason, never the one a debenture gets | `core::vendor::an_unrecognised_series_is_its_own_loud_reason_never_a_bond` | ✓ |
| I-18 | The unrecognised **code itself** reaches the operator, not only a count of it | `api::master::an_unrecognised_series_is_recorded_under_the_code_itself` | ✓ |
| I-19 | A fund plan is declined from **both** vendors, and a genuine ETF on the equity board is still kept | `core::vendor::a_mutual_fund_plan_is_declined_from_both_vendors_on_one_series_alphabet` | ✓ |
| I-20 | The surveillance and partly-paid equity series are kept as equity, from both vendors, and never labelled debt | `core::vendor::the_surveillance_and_partly_paid_equity_series_are_kept_not_called_debt` | ✓ |
| I-21 | A declined row carries its ISIN as evidence, and an unparseable one is neither an error nor a silent substitute | `core::vendor::a_declined_row_carries_its_isin_as_evidence_for_the_cross_check` | ✓ |
| I-22 | One vendor keeping an ISIN another declined is a named disagreement, and the instrument is not dropped | `api::merge::one_vendor_keeping_what_another_declined_is_a_named_disagreement` | ✓ |
| I-23 | A decline about the venue is never mistaken for a disagreement about the paper | `api::merge::a_decline_about_the_venue_is_not_a_disagreement_about_the_paper` | ✓ |
| I-24 | A row with fewer fields than its columns need is unreadable and names the shortfall; it is never a routine decline | `api::master::a_row_with_too_few_fields_is_unreadable_and_names_the_shortfall` | ✓ |
| I-25 | Every distinct parse failure reaches the operator with its count and the first line that hit it | `api::master::unreadable_rows_are_grouped_by_reason_with_the_first_line_that_hit_it` | ✓ |
| I-26 | A searched page still says that a vendor was never read | `api::server::a_searched_page_still_says_a_vendor_was_never_read` | ✓ |
| I-27 | A vendor that was never read makes the status, the exit code and `/health` all say so | `api::binary::the_binary_exits_non_zero_when_a_vendor_was_never_read` · `api::server::health_answers_503_when_a_vendor_was_never_read` | ✓ |
| I-28 | An ISIN conflict refuses the universe rather than logging it and continuing | `api::server::an_isin_conflict_reaches_the_report_and_the_page` | ✓ |
| I-29 | An unrecognised listing class degrades the run, while a routine bond does not | `api::server::an_unrecognised_listing_class_names_the_code_and_degrades_the_run` | ✓ |
| I-30 | A universe member only one vendor named is counted separately from one two vendors confirmed, and named | `api::merge::the_census_separates_what_two_vendors_confirmed_from_what_one_asserted` | ✓ |

## The instrument universes

Added by D-0029. `crates/core/src/universe.rs` shipped with none of these rows;
CI gate 10 walks rows→tests and never tests→rows, so the build stayed green
while `CLAUDE.md` §9 was violated.

| # | Must hold | Proven by | |
|---|---|---|---|
| U-01 | Both constituent lists are sorted and unique, so `binary_search` is valid | `core::universe::both_lists_are_sorted_and_unique_so_binary_search_is_valid` | ✓ |
| U-02 | No two universes share a bit, so an append-only bitset stays safe (`CLAUDE.md` §3.8) | `core::universe::bits_are_distinct_powers_of_two` | ✓ |
| U-03 | The list lengths are the measured ones, and no exchange test instrument is ever a member | `core::universe::the_counts_are_the_measured_ones` | ✓ |
| U-04 | No SME ticker is in either universe — the claim `Skip::SmeBoard` declines 1,117 real shares on | `core::universe::no_measured_sme_ticker_belongs_to_either_universe` | ✓ |
| U-05 | An index is its own universe and a live derivative is in none | `core::universe::an_index_is_its_own_universe_and_a_live_derivative_is_in_none` | ✓ |

## Cross-cutting

| # | Must hold | Proven by | |
|---|---|---|---|
| X-01 | Run identity changes if any loaded bar differs by one field | `core::proptest::identity_sensitivity` | — |
| X-02 | Prices never touch a float on any path from wire to store to result | `core::lint::no_float_in_price` (a clippy deny plus a source check) | — |
| X-03 | Every tracked file has an allowed extension | CI gate 1 | ✓ |
| X-04 | No build script invokes an external process | CI gate 2 | ✓ |
| X-05 | `web` depends on `core` alone | CI gate 7 | ✓ |
| X-06 | **Line and region** coverage is 100% on every crate, with no omit list | CI coverage job (`--fail-under-lines 100 --fail-under-regions 100`) | ✓ |
| X-06b | ~~Branch coverage is 100% on every crate~~ | **NOT MEASURED.** `llvm-cov` instruments zero branches on the pinned stable toolchain and `--branch` cannot run there at all. Narrowed by D-0030; recorded in `docs/06-limits.md` §7. | — |
| X-07 | No mutant survives on a touched module | `cargo-mutants`, scheduled | — |
| X-08 | No tracked file contains a literal credential path | CI gate 1c | ✓ |
| X-09 | `core` declares no dependency at all | CI gate 9 | ✓ |
| X-10 | Every reachable row in this file names a test that exists | CI gate 10 | ✓ |
| X-12 | Each vendor writes only under its own path prefix; no vendor can overwrite another | `store::unit::vendor_prefix_isolated` | — |
| X-13 | A bar-for-bar mismatch between two vendors refuses the window and names the timestamp | `store::unit::vendor_disagreement_refuses` | — |
| X-11 | A price is constructible from a float only through the one checked conversion | `core::price::refuses_an_out_of_range_price_instead_of_saturating` (private field; no other path exists) | ✓ |

---

## Greeks — closed form, and the one solve that is not

`crates/greeks` is `f64` throughout and never sees a paisa. `CLAUDE.md` §7
reserves `i64` for prices and keeps statistical values at full precision; a
delta is the second kind. D-0037.

| # | Must hold | Proven by | |
|---|---|---|---|
| G-01 | Gamma and vega are **bit-identical** between a call and a put at the same strike — not close, identical, because both are computed above the branch | `greeks::bsm::gamma_and_vega_are_bit_identical_between_the_call_and_the_put` | ✓ |
| G-02 | `delta_call − delta_put == e^-qT` to a measured bound, and the bound is 2 ulps rather than a hoped-for zero | `greeks::bsm::the_delta_difference_is_the_carry_discount_to_a_measured_bound` | ✓ |
| G-03 | Put-call parity `C − P == S·e^-qT − K·e^-rT` holds across the grid to 2.1e-16 of spot | `greeks::bsm::put_call_parity_holds_across_the_grid` | ✓ |
| G-04 | Every one of the five greeks reproduces a central difference **wherever the stencil can resolve it**, and the number of comparisons actually made is asserted | `greeks::bsm::every_greek_reproduces_a_central_difference_wherever_one_can_be_taken` | ✓ |
| G-05 | The shipped normal CDF agrees with an independently implemented reference to 2.22e-16 absolute and 8.9e-9 relative — the check that caught a transposed digit in a Hart coefficient | `greeks::normal::hart_agrees_with_an_independent_reference` | ✓ |
| G-06 | The normal CDF matches published values at twelve points, on a **relative** tolerance so the tail is actually checked | `greeks::normal::the_cdf_matches_known_values` | ✓ |
| G-07 | Both Hart branches and both saturating tails are exercised, and the two branches meet at the split inside Hart's own tail accuracy | `greeks::normal::both_hart_branches_and_both_saturating_tails_are_exercised` | ✓ |
| G-08 | `volatility -> price -> IV` recovers the **volatility** to 1e-4 relative, and every point that does not is refused as one of exactly two named kinds whose counts account for the whole refusal count. Measured worst 2.33e-6; the superseded `price -> IV -> price` form is asserted too and is the weaker of the two — it measures `0e0` at the point where the volatility is 5.14% wrong. D-0037 | `greeks::solver::a_volatility_round_trips_through_the_solver_and_back` | ✓ |
| G-09 | One solve never costs more than `BRACKET_EVALUATIONS + NEWTON_STEPS + BISECTION_STEPS + FINAL_EVALUATION = 2 + 8 + 64 + 1 = 75` model evaluations — **every** evaluation, not only the ones inside the search — and both methods are actually exercised | `greeks::solver::the_iteration_count_never_exceeds_the_arithmetic_bound` | ✓ |
| G-10 | The same inputs give the same volatility **bit for bit**, and the same iteration count and method, **within one process and one target**. Bit-for-bit reproducibility does NOT hold across targets — a different libm moves 140 of 1,344 solved volatilities by up to 4.22e-14 relative — and `docs/06-limits.md` §18 carries the measurement (`CLAUDE.md` §3 rules 5 and 6) | `greeks::solver::the_solver_is_idempotent_to_the_bit` | ✓ |
| G-11 | A price does not determine a volatility when one unit in the last place *the price actually has* — set by the two legs it is a difference of, not by its own magnitude — moves the answer by more than `1e-3` of itself; that is **refused**, never returned | `greeks::solver::a_price_that_does_not_determine_a_volatility_is_refused_not_returned` | ✓ |
| G-12 | A quote at or below the discounted intrinsic value is refused on both sides, and a negative quote lands in the same refusal with `intrinsic` printed as zero | `greeks::solver::a_price_at_or_below_the_discounted_intrinsic_is_refused_on_both_sides` | ✓ |
| G-13 | A quote at or above the model's supremum is refused on both sides | `greeks::solver::a_price_at_or_above_the_model_supremum_is_refused_on_both_sides` | ✓ |
| G-14 | A quote inside the arbitrage bounds but outside the searched volatility band is refused, never clamped into it | `greeks::solver::a_price_outside_the_searched_band_is_refused_rather_than_clamped_into_it` | ✓ |
| G-15 | A NaN or an infinity in any input is refused **by the name of the field it arrived in** | `greeks::bsm::a_non_finite_input_is_refused_field_by_field` · `greeks::solver::a_non_finite_market_price_is_refused_before_anything_is_computed` | ✓ |
| G-16 | A non-positive spot, strike or volatility is refused by name, and `T <= 0` gets its own refusal rather than being called malformed | `greeks::bsm::a_non_positive_spot_strike_or_volatility_is_refused_by_name` · `greeks::bsm::an_expired_contract_is_its_own_refusal_and_not_a_malformed_input` | ✓ |
| G-17 | Every accepted range refuses the value just past it and names the field and the bound | `greeks::bsm::every_bound_refuses_the_value_just_past_it_and_names_it` | ✓ |
| G-18 | Inputs inside every bound that the model still cannot represent are refused, never returned as a `NaN` greek | `greeks::bsm::an_in_range_input_that_the_model_cannot_represent_is_refused_not_returned` | ✓ |
| G-19 | A strike between two rungs is refused, never rounded onto one; and a call and a put read the same rung from opposite sides | `greeks::moneyness::a_strike_between_two_rungs_is_refused_and_never_rounded_onto_one` · `greeks::moneyness::a_call_and_a_put_read_the_same_rung_from_opposite_sides` | ✓ |
| G-20 | A rung past `MAX_STEPS` is refused rather than truncated into range by the one `f64 -> i32` cast in the crate | `greeks::moneyness::a_rung_beyond_the_bound_is_refused_rather_than_truncated_into_range` | ✓ |
| G-29 | Every bound is INCLUSIVE: the value exactly on it is accepted, which is what stops the refusal above from being satisfied by refusing everything | `greeks::bsm::every_bound_accepts_the_value_exactly_on_it` | ✓ |
| G-30 | The Brenner-Subrahmanyam seed lands within 1% of the answer at the money, and the search then finishes in Newton in at most four steps | `greeks::solver::the_seed_lands_next_to_the_answer_at_the_money` | ✓ |
| G-31 | The reported iteration count is the work actually done, floor as well as ceiling: the three fixed evaluations plus at least one Newton step, and the bisection's 64 halvings on top of the Newton steps that preceded them | `greeks::solver::the_iteration_count_is_the_work_actually_done` | ✓ |
| G-32 | The reported cost is **every** model evaluation, checked against a count the solver did not compute — a thread-local counter inside `Checked::greeks`, the one function all of them pass through — on a Newton solve, on the worst solve and on a refusal | `greeks::solver::the_reported_cost_is_every_model_evaluation` | ✓ |
| G-33 | The scale the `Indeterminate` guard screens on is the sum of the two legs the price is a difference of, and it is measurably coarser than one ulp of the price itself — 100.8× on a one-day in-the-money NIFTY strike | `greeks::bsm::the_price_scale_is_the_two_legs_and_it_dwarfs_the_price_they_leave` | ✓ |

## Greeks against the vendors — the external anchor

Every row here is measured against one live Dhan option-chain response. D-0037.

| # | Must hold | Proven by | |
|---|---|---|---|
| G-21 | The shipped closed form reproduces all eight of Dhan's published numbers from a fitted state, so a sign error, a missing discount factor or a wrong unit anywhere in `greeks::bsm` fails here. **None of the eight is an independent prediction** — six are inputs to the fit and the two gammas are the transposition criterion restated, which G-35 pins. The claim that the gammas were a prediction is withdrawn by D-0037 | `greeks::vendor_anchor::our_greeks_reproduce_the_captured_dhan_chain` | ✓ |
| G-22 | Vega is published per one percentage point: the raw scaling implies an index level of 258, the per-percent scaling 25,851 | `greeks::vendor_anchor::vega_is_published_per_percentage_point_and_the_index_level_proves_it` | ✓ |
| G-23 | Theta is published per a **calendar** day and not a trading day: the two sides of one strike agree on `r` 23 times better under 365 than under 252. **365 itself is not selected** — 375 beats it, and G-34 locates what the criterion actually picks | `greeks::vendor_anchor::the_trading_day_divisor_is_excluded_and_the_calendar_one_is_not_selected` | ✓ |
| G-24 | The two published volatilities are transposed, by a scale-free identity that contains no spot, strike, maturity or rate | `greeks::vendor_anchor::the_two_published_volatilities_are_transposed_and_the_identity_says_so` | ✓ |
| G-25 | `delta_call − delta_put = 1.00603` is reproduced by `N(d1c) + N(−d1p)` at `q = 0`, and a single volatility would need `q = −42.54%`. This says **nothing** about the carry — it inverts the two deltas, so it cannot fail for any deltas, volatilities or carry — and D-0037 withdraws the claim that it did | `greeks::vendor_anchor::the_delta_difference_is_reproduced_by_the_two_volatilities_and_says_nothing_about_the_carry` | ✓ |
| G-26 | `T` is recovered in closed form from the deltas and the volatilities alone and lands inside its 95% rounding-box interval — **conditional on `q = 0` and on the transposition.** The interval is a rounding-box width, not an identification result | `greeks::vendor_anchor::the_maturity_is_recovered_in_closed_form_from_the_deltas_and_the_volatilities` | ✓ |
| G-27 | The fit does **not** close, and the residuals are pinned so no later change can claim it does — with the spot residual, which is invariant to the day divisor from 252 to 500, separated from the rate residual, which is an artifact of choosing 365 | `greeks::vendor_anchor::the_two_sides_disagree_on_the_rate_and_the_disagreement_is_reported_not_hidden` | ✓ |
| G-28 | `crates/greeks` declares no dependency and no dev-dependency, and names no type from this workspace, so it can be taken by git URL on its own | **CI gate 9b.** `greeks::standalone::the_whole_public_surface_is_reachable_with_nothing_else_in_scope` is the companion and proves only the narrower thing its name says: the listed public items are reachable with nothing else in scope. An integration test cannot see a leak it does not name, and a `pub use` adds no region for coverage to see. D-0037 | ✓ |
| G-34 | The criterion that excludes a 252-day divisor has its root at `D* = 370.0757`, where the two sides agree on `r` to 2.8e-15 points, and the spread is monotone in the divisor across 250–450 so no second root hides at 365 | `greeks::vendor_anchor::the_rate_criterion_has_its_root_at_370_and_365_is_a_convention_near_it` | ✓ |
| G-35 | The fitted gammas are `n(d1)^2/(100·vega·sigma)` and are therefore **invariant to `r`, `T`, `S` and `K`** — identical to fifteen digits with `r` forced from −50% to +200% and `T` scaled 0.25× to 10× — so reproducing them confirms nothing about any of those | `greeks::vendor_anchor::the_two_gammas_are_invariant_to_the_rate_and_the_maturity` | ✓ |
| G-36 | **No single contract reproduces the chain.** The two deltas force the model's vega ratio to 0.998641 against the vendor's 1.001360, a quantity containing no `S`, `K`, `T` or `r`; the best possible single contract is off by 884× the vendor's own display half-ulp | `greeks::vendor_anchor::no_single_contract_reproduces_the_chain_and_the_shortfall_is_a_number` | ✓ |
| G-37 | `q = 0` is **consistent** with the sample and not implied by it: `q = 1%` and `q = 2%` reproduce all eight published fields, gammas included, at maturities a third shorter | `greeks::vendor_anchor::the_carry_is_consistent_with_zero_and_the_sample_cannot_pin_it` | ✓ |

## How this file stays honest

1. A new guarantee anywhere in the codebase adds a row **here first**.
2. The row names a test. Not a plan for a test.
3. CI checks that each named test exists. A row pointing at nothing fails the
   build, which is what stops this file from decaying into a wish list.
4. Rows are never deleted to make a build pass. Either the invariant holds or
   the decision to abandon it is recorded in `docs/05-decisions.md`.
