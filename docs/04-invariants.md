# 04 — Invariants

Every row names the test that proves it. **An invariant with no test named
beside it is deleted from this file, not shipped.** A consistency check in CI
fails if a row's test does not exist.

Status: `✓` proven · `◐` proven where it has been run, and where that is is
named · `○` test written, awaiting the crate · `—` not yet reachable (the crate
does not exist).

**X-07 and X-08 were narrowed by D-0036, not weakened.** X-07 sat at `—`, which
this legend defines as "the crate does not exist" — untrue once `crates/pull`
existed, and `CLAUDE.md` §9's mutation bullet had simply not been run. It is now
`◐` and names where it *has* been run: `crates/pull` only. `crates/core`,
`crates/store` and `crates/api` have never been measured, and no row here claims
otherwise. X-08 claimed CI gate 1c proved "no literal credential path"; the gate
matches only a slash-joined path with a well-known environment segment, which
was demonstrated by running its exact pattern over a file hardcoding two real
segments as bare constants. The row now says what the gate does, gate 1d covers
the crate where the gap matters, and `docs/06-limits.md` §18 records the rest.

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
| S-16 | The 56 bytes one known record encodes to are pinned as a literal array. A body replaced with zeros, a moved offset or a flipped byte order each fail | `store::unit::the_record_image_is_the_pinned_bytes_of_a_known_bar` | ✓ |
| S-17 | The image is little-endian and each of the seven fields owns its own eight bytes; all 56 (field, byte) placements are asserted against all 56 image bytes | `store::unit::the_image_is_little_endian_and_each_field_owns_its_own_offset` | ✓ |
| S-18 | `decode(image(r)) == r` exactly, over every 64-bit boundary in every field — `i64::MIN`, `i64::MAX`, zero, negatives — and the encoder is injective across the sampled 4,276 records | `store::unit::decoding_the_image_returns_the_record_byte_for_byte` | ✓ |
| S-19 | A buffer shorter than 56 bytes is refused by length and never completed with invented zeros | `store::unit::a_short_record_is_refused_and_never_completed_with_zeros` | ✓ |
| S-20 | `BLOCK_LEN == RECORD_STRIDE × RECORDS_PER_BLOCK == 56 × 73 == 4088`, and the last record of a block ends exactly on the block boundary — so no record straddles | `const _: () = assert!(…)` in `store::format` — a compile error — and `store::geometry::no_record_straddles_a_block` | ✓ |
| S-21 | Decoding a record reads exactly 56 bytes: a 1 MiB buffer of `0xFF` past the record gives the same answer as the bare 56 bytes. Behavioural, not a timing measurement | `store::unit::decoding_reads_exactly_fifty_six_bytes_however_long_the_buffer_is` | ✓ |

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
| C-11 | Reading the census beats re-deriving it from the entries by at least 100×, measured in one process | `pull::bench::census_beats_the_scan_it_replaces` | ✓ |
| C-12 | One entry lookup — hit or miss — costs the same at 1×, 10× and 100× the census, measured on the map a **loaded** manifest holds | `pull::bench::entry_lookup_is_flat` | ✓ |
| C-13 | Appending a month to a loaded census costs the same at 1×, 8× and 32× the census — measured at the counts `7·2^k` where a table reserved to exactly the census has **no free slot at all**, not only at round numbers | `pull::bench::append_after_load_is_flat` | ✓ |
| C-14 | Rendering one instruments page costs the same at 2,787 and at 50,000 instruments — for **every** one of the six sort columns, both directions, all four universe pills, the escape hatch and a clamped deep page | `api::bench::every_order_and_pill_is_flat`, `api::bench::the_hatch_and_the_last_page_are_flat` | ✓ |
| C-15 | One more instrument in the universe costs a request **at most 1 ns**, measured as the slope between two sizes that draw the same number of rows | `api::bench::gate`, the `C-15 … marginal` lines | ✓ |
| C-16 | The dashboard costs the same at 2 and at 50,000 instruments — it draws no rows, so its raw ratio is the whole statement | `api::bench::the_dashboard_is_flat` | ✓ |
| C-17 | The page still **draws** its rows: cost per rendered row does not grow with the universe, so flatness cannot be bought by rendering less | `api::bench::gate`, the `C-17 … per row` and `… rows` lines | ✓ |

C-14 through C-17 exist because `docs/07-o1-architecture.md` layer 12 was marked
BUILT with the words *"a fixed row cap with paging. NEVER O(universe)"* and only
the **rendered rows** were capped. An audit measured 3.569 ms at 2,787
instruments and 124.916 ms at 50,000 **rendering exactly 200 rows both times** —
62.5 ns per instrument per request, on a page whose output never changed shape.
The row cap is what hid it. After D-0042: 147,987 ns → 151,618 ns, ratio 1.02×,
marginal 0–259 ps per instrument.

C-15 is the row that matters most, because it is the audit's own number and it
needs no baseline: a slope measured between two sizes that draw the same 200
rows is universe size and nothing else. It went from 85,400 ps to 0–259 ps.

C-17 is the guard on the other three. A page that got flat by rendering nothing
would pass C-14, C-15 and C-16 and be worse than what it replaced, which is the
same class of mistake as capping the rows and calling the page constant.

**Search is deliberately not in this table.** It is measured by the same bench
and asserted by none of it, because a substring that matches most of the universe
must look at most of the universe. `docs/06-limits.md` §24 states the two cases
that stay linear and prints their cost.

C-13 is the row that had no bench at all until D-0040, and `CLAUDE.md` §3 rule 4
names **result append** among the operations that must be O(1). Measured at a
57,344-entry census: **5,100,585 ps per append before, 95,214 ps after**, and
the ratio across a 32× census went from 22.304× to 1.020×. The 1× baseline in
the "before" column was itself rehashing, so 53.6× is the honest figure for what
was removed. The round counts 1,000 / 10,000 / 50,000 passed the ceiling in both
columns — `HashMap::with_capacity` happened to round them up and leave spare
slots — which is why the harness measures both sets and why a bench that had
only visited round numbers would have reported the defect as absent.

C-12 was measured on a manifest built by `Manifest::genesis` + `record` until
D-0036 — a map that grew by rehashing — while three documents attributed the
flatness to a reservation taken on the load path that the harness never
called. The bench now builds its census through `Manifest::load`, so the map
measured is the map the claim is about, and
`pull::unit::the_loaded_index_is_reserved_from_the_census` (M-17) asserts where
the reservation comes from as a number.

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
| P-05 | A credential is read, never written; no token is ever minted | `pull::unit::readonly_credentials` (a write attempt must panic the test double) | ✓ |
| P-06 | An auth failure halts the pull loudly rather than degrading | `pull::unit::auth_halt` | ✓ |
| P-07 | A missing, unreadable, or incomplete credential configuration halts the pull and names the absent segment; it never defaults | `pull::unit::credential_config_absent_halts` · `pull::unit::a_missing_table_or_key_is_a_halt` | ✓ |
| P-08 | The credential configuration supplies path segments only; a secret value found in it is refused | `pull::unit::credential_config_rejects_secret_value` | ✓ |

Added by D-0035. `P-01` through `P-04` keep their `—`: there is no rate
governor, no window walk and no calendar filter yet, and the change that adds
them is the change that makes a live vendor call.

| # | Must hold | Proven by | |
|---|---|---|---|
| P-09 | A vendor that rejects a credential gets **one** re-read, and an unchanged value halts naming the vendor and the field; it is never re-minted | `pull::unit::a_dead_token_is_re_read_once_and_then_halts` | ✓ |
| P-10 | The parameter is always asked for decrypted, and the assembled path is what is asked for | `pull::unit::the_adapter_always_asks_for_a_decrypted_value` | ✓ |
| P-11 | A credential value never reaches a formatter: `Debug` is a redaction and there is no `Display` | `pull::unit::a_secret_never_prints_its_value` | ✓ |
| P-12 | A parameter path exists only for segments the configuration carries; a field nobody configured is refused, never assembled | `pull::unit::the_only_path_is_the_one_the_configuration_assembles` | ✓ |
| P-13 | The pasted-secret backstop fires at the first byte that is too many and not one before, and the length and byte-set bounds catch every credential shape measured | `pull::unit::the_secret_backstop_is_the_first_byte_that_is_too_many` | ✓ |
| P-14 | The declared path bound is reached exactly by a maximal configuration; it is tight, not merely sufficient | `pull::unit::a_maximal_path_is_exactly_the_declared_bound` | ✓ |
| P-15 | Every line the configuration reader does not understand is a halt naming the line; none is skipped | `pull::unit::every_line_this_reader_does_not_know_is_a_halt` · `pull::unit::a_vendor_tables_own_keys_are_checked_too` | ✓ |
| P-16 | The configured region is checked against the one `CLAUDE.md` §8 fixes, not merely read | `pull::unit::the_region_is_checked_rather_than_merely_read` | ✓ |
| P-17 | The configuration file is bounded **at the read** — at most `MAX_FILE_BYTES` + 1 bytes are ever pulled in, whatever `stat` claimed — and by line length before a line is parsed | `pull::unit::the_configuration_file_is_bounded_before_it_is_read` | ✓ |

Added by D-0036. P-17 previously read "by size before it is read", and the
check was `metadata().len()`, which is `0` for a FIFO, a character device and
any `/proc` entry — so the bound was not a bound and the read that followed was
unbounded. It is now taken on what was actually read.

| # | Must hold | Proven by | |
|---|---|---|---|
| P-18 | No formatter renders a path segment: `Debug` on the assembled path, on the whole configuration and on a vendor's table is a redaction, and `Display` on the path is the one audited exit | `pull::unit::no_formatter_renders_a_path_segment` | ✓ |
| P-19 | A path that is not a regular file is refused by name before it is opened, so a FIFO cannot hang the read and a device cannot make it unbounded | `pull::unit::a_path_that_is_not_a_regular_file_is_refused_by_name` | ✓ |

### The adaptive rate governor

Added by D-0037. Every row is proven by a test that runs today, and **not one of
those tests sleeps or reads a clock** — `pull::rate::Governor::admit` takes the
instant as an argument, so every boundary below is asserted at the exact
microsecond rather than near it.

`P-01` keeps its `—`, narrowed rather than satisfied. It claims the ceiling
holds *under any concurrency*; `Governor` takes `&mut self` and has no interior
mutability, so it cannot be shared across tasks without a lock that this crate
does not supply, and the loom test named beside it does not exist. What is
proven here is the single-caller arithmetic.

| # | Must hold | Proven by | |
|---|---|---|---|
| P-20 | The first request of a pull is admitted and costs exactly one permit in every bounded span; a span the vendor does not bound is `None`, never a large number | `pull::unit::the_first_request_of_a_pull_is_admitted` | ✓ |
| P-21 | A window admits exactly its allowance and refuses the next one, and a refusal charges nothing | `pull::unit::a_window_saturates_exactly_at_its_allowance_and_not_one_over` | ✓ |
| P-22 | A throttle halves the allowance of **every** span and drains every bucket, and leaves every published ceiling untouched | `pull::unit::a_refusal_halves_every_allowance_and_drains_every_bucket` | ✓ |
| P-23 | Sustained success raises the allowance by exactly one permit at a time and stops **at** the published ceiling, never past it | `pull::unit::sustained_success_walks_up_one_permit_at_a_time_and_stops_at_the_ceiling` | ✓ |
| P-24 | The allowance never reaches zero, however many refusals arrive; the floor of one is a rate, so a governor can always climb back out | `pull::unit::the_allowance_never_reaches_zero_however_many_refusals` | ✓ |
| P-25 | A drained window earns back at exactly the permitted rate — asserted at the microsecond on both sides — and idle time never accumulates past a full bucket | `pull::unit::a_drained_window_earns_back_at_the_permitted_rate_and_is_whole_after_one_span` | ✓ |
| P-26 | When spans disagree the one with the longest wait denies and is named, no span is charged, and a tie goes deterministically to the shorter span | `pull::unit::the_span_that_waits_longest_denies_and_nothing_is_charged` · `pull::unit::a_tie_between_two_spans_is_broken_by_the_shorter_one` | ✓ |
| P-27 | The wait a denial reports is the **smallest** that clears it: one microsecond earlier is still a refusal | `pull::unit::a_denial_names_the_exact_wait_that_clears_it` | ✓ |
| P-28 | A clock that goes backwards grants no capacity and cannot panic; recovery is measured from the highest instant ever seen, not from the bottom of the dip | `pull::unit::a_clock_that_goes_backwards_grants_nothing` | ✓ |
| P-29 | A clock reading at the far end of `u64` does not wrap and grants at most a full bucket, including at the widest ceiling × span product this build accepts | `pull::unit::a_clock_at_the_far_end_of_u64_does_not_wrap` | ✓ |
| P-30 | Budgets pool per `(vendor, request kind)`: one kind's pool exhausting leaves the other untouched, and a throttle on one moves only that one | `pull::unit::one_request_kinds_pool_is_exhausted_while_the_other_is_untouched` | ✓ |
| P-31 | The same script gives the same verdicts and the same final state, every time — and the script exercises both arms | `pull::unit::the_same_script_gives_the_same_verdicts_and_the_same_state` | ✓ |
| P-32 | Every declared rate bound is exact at the limit — `MAX_CEILING` accepted and the first value past it refused, naming the span — and a ceiling of zero is refused rather than read as "no window" | `pull::unit::every_declared_rate_bound_is_exact_at_the_limit` | ✓ |
| P-33 | The vendor figures in `pull::rate` are the ones `docs/00-charter.md` §4 records, frozen against hardcoded numbers rather than re-derived | `pull::unit::the_published_vendor_figures_are_the_ones_the_charter_records` | ✓ |
| P-34 | A governor's whole state is a fixed-size `Copy` struct: ten thousand admitted requests leave its `size_of` unchanged, and it owns no allocation | `pull::unit::a_governor_holds_no_allocation_and_no_history` | ✓ |
| P-35 | Against a vendor honouring less than it publishes, the allowance converges into `1..=honoured` and never passes the published ceiling; when the refusals stop it walks back up to that ceiling | `pull::unit::the_allowance_converges_onto_the_rate_the_vendor_actually_honours` | ✓ |

## The manifest — layer 13

Added by D-0035. `docs/07-o1-architecture.md` layer 13: counters, never scans.
Every row here is proven by a test that runs today.

| # | Must hold | Proven by | |
|---|---|---|---|
| M-01 | An entry survives its own 64-byte image, field for field, on every exchange and segment code | `pull::unit::an_entry_round_trips_through_its_image` | ✓ |
| M-02 | The checksum's domain is exactly bytes `0..60`; covering the checksum itself is a different number and is pinned against a hardcoded value | `pull::unit::the_covered_domain_is_the_image_minus_its_checksum` | ✓ |
| M-03 | A flipped bit in any of an entry's 512 bits is detected | `pull::unit::a_flipped_bit_in_any_entry_byte_is_detected` | ✓ |
| M-04 | A torn header commit never reports a count that was not committed — every one of the 65 prefixes gives the previous generation or the new one | `pull::unit::a_torn_header_commit_never_reports_an_uncommitted_count` | ✓ |
| M-05 | A header that became durable before the entries it counts falls back one generation rather than condemning the file — whether the entry region is short, **or its committed bytes are zeroed, garbage or half-written** | `pull::unit::a_header_published_before_its_entries_falls_back_a_generation` | ✓ |
| M-06 | A header counter that disagrees with the entries it counts is refused; the counter is checked against the thing it counts, once, on load | `pull::unit::a_counter_that_disagrees_with_its_entries_is_refused` | ✓ |
| M-07 | A key whose row count or last timestamp goes backwards is refused, on write and on load — and one that repeats its last timestamp exactly is accepted, on both | `pull::unit::a_row_count_that_went_backwards_is_refused` · `pull::unit::a_key_whose_history_goes_backwards_on_disk_is_refused` · `pull::unit::a_key_that_repeats_its_last_timestamp_is_accepted` | ✓ |
| M-08 | An entry's address is arithmetic, and the ordinal is bounded rather than the product checked | `pull::unit::the_offset_of_an_entry_is_arithmetic` | ✓ |
| M-09 | The exchange and segment codes on disk are frozen against hardcoded numbers, never re-derived from the enum | `pull::unit::the_exchange_and_segment_codes_are_frozen` | ✓ |
| M-10 | A manifest whose header names another vendor is refused by name; the file name and the header must agree | `pull::unit::a_manifest_for_another_vendor_is_refused` | ✓ |
| M-11 | The three counters are maintained on every write, and a refused record leaves them untouched | `pull::unit::the_counters_are_maintained_on_write` | ✓ |
| M-12 | Every counter refuses to wrap — generation, entry count, key count and row total, on write and on load | `pull::unit::the_header_counters_refuse_to_wrap` · `pull::unit::the_row_total_refuses_to_wrap` · `pull::unit::the_load_time_row_total_refuses_to_wrap` | ✓ |
| M-13 | A slot that is not this format's header is refused by name, and a commit found in the wrong slot is not a candidate | `pull::unit::a_slot_that_is_not_a_header_is_named` · `pull::unit::a_short_or_misplaced_header_region_is_refused` | ✓ |
| M-14 | A genesis census exists only for a file with nothing in it; a writer cannot start a new census over one that already holds months | `pull::unit::the_only_genesis_is_an_empty_file` | ✓ |
| M-15 | Every declared bound is exact — the value at the limit is accepted and the first one past it is refused, on `advance`, `validate`, `commit`, the line bound and the field bound | `pull::unit::every_declared_bound_is_exact_at_the_limit` | ✓ |
| M-16 | A generation recovered by stepping over a damaged one is never silent: the census names what it stepped over | `pull::unit::a_corrupt_header_slot_is_named_by_the_census_that_survives_it` | ✓ |
| M-17 | The loaded index is reserved from the committed entry count, not from the region's byte length, so the reservation is proportional to the census and never to the file | `pull::unit::the_loaded_index_is_reserved_from_the_census` | ✓ |
| M-18 | The reservation is the census doubled and never past `MAX_ENTRIES`; from half the ceiling upward it covers every append `advance` will ever accept, so no append can rehash at all | `pull::unit::the_reservation_is_capped_at_the_design_ceiling` | ✓ |
| M-19 | A loaded index carries free room for at least `n_valid` further appends, and none of them rebuilds the table — checked at a census of `7·2^8`, where a table reserved to exactly the census has zero free slots | `pull::unit::a_loaded_index_carries_headroom_for_the_appends_after_it` | ✓ |

M-18 and M-19 added by D-0040. **M-19 stops one short of an unconditional
claim, deliberately.** Its last assertion is that the append *after* the
headroom does grow the table, so the row cannot be read as "append never
rehashes": past `n_valid` new keys the cost is amortised O(1) with an
`O(n_keys)` worst case, and `docs/06-limits.md` §23 says what removing that
last arm would cost. The same test also refuted a claim written into
`Manifest::record` while D-0040 was being written — that an update to a key
already held can never grow the table. `HashMap::insert` asks for a slot before
it looks the key up, so on a full table it grows anyway; the test asserts the
update-inside-the-headroom case that does hold and the one past it that does
not.

M-14 through M-17 added by D-0036, each closing a defect that the tests above
could not see. M-05 and M-07 were restated there rather than replaced: both
claimed more than the code did.

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

## The ingest and store pages

Added by D-0038. Every row here is proven by a test that runs today.

| # | Must hold | Proven by | |
|---|---|---|---|
| A-01 | Starting a pull is a `POST`; a `GET` on either submit route answers 405 without reaching a parser | `api::server::the_server_answers_every_route_and_then_shuts_down_gracefully` | ✓ |
| A-02 | An expiry that has not passed is refused, and `today` itself counts as live | `api::ingest::an_expired_contract_is_accepted_and_a_live_one_can_never_be` · `api::server::the_fno_form_cannot_request_a_live_contract_over_http_either` | ✓ |
| A-03 | The expiry input's `max` is the day before today, so a live contract is not even offerable — and the parser refuses one anyway | `api::render::the_ingest_page_carries_two_forms_and_never_a_get_that_starts_a_pull` | ✓ |
| A-04 | A date field is `YYYY-MM-DD` or it is refused naming the field and the value; no ambiguous format is ever guessed at | `api::ingest::a_date_field_is_refused_by_name_whichever_way_it_is_wrong` | ✓ |
| A-05 | A window is inclusive at both ends, a backwards one is refused rather than swapped, and a one-day window is legal | `api::ingest::a_window_is_inclusive_both_ends_and_a_backwards_one_is_refused_not_swapped` | ✓ |
| A-06 | The window length bound is the first day that is too many and not one before | `api::ingest::the_window_is_bounded_at_the_boundary_and_the_bound_is_tight` | ✓ |
| A-07 | The vendor's non-inclusive `toDate` is shown on the receipt as the day after the operator's last day, and said to be so | `api::server::a_valid_window_is_echoed_with_the_wire_date_and_still_starts_nothing` | ✓ |
| A-08 | No capture counter renders `0` while nothing is running: an unmeasured value is `—`, under a block naming what is absent | `api::render::a_capture_that_is_not_running_shows_dashes_and_a_named_reason` | ✓ |
| A-09 | Every reason the session filter counts is named on the page by `DropReason::label`, measured or not, and its bar width is integer arithmetic | `api::render::a_running_capture_reports_real_counts_with_integer_bars` | ✓ |
| A-10 | An absent manifest renders as a named absence at a named path; it is never a 500 and never zeros | `api::census::an_absent_manifest_names_the_path_and_is_never_zeros` · `api::server::the_store_page_renders_an_absent_manifest_rather_than_failing` | ✓ |
| A-11 | A manifest whose counters are zero reads as zero and is not loud — an empty store is a real answer, not an absence | `api::census::a_manifest_whose_counters_are_zero_reads_as_zero_and_is_not_loud` | ✓ |
| A-12 | A manifest that will not load is loud and carries the manifest's own refusal, never a generic failure | `api::census::a_manifest_that_will_not_load_is_loud_and_names_the_refusal` | ✓ |
| A-13 | The coverage grid is addressed by ordinal arithmetic, so a page builds only the rows it shows | `api::census::the_grid_is_addressed_by_arithmetic_and_pages_without_building_the_rest` | ✓ |
| A-14 | A page past the end of the grid clamps to the last page rather than erroring or emptying | `api::server::the_store_page_reads_zero_as_zero_and_pages_past_the_end_by_clamping` · `api::server::the_store_grid_pages_when_it_is_larger_than_one_page` | ✓ |
| A-15 | A nav entry for a page that does not exist is shown disabled, never hidden and never linked | `api::server::the_server_answers_every_route_and_then_shuts_down_gracefully` | ✓ |

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
| X-07 | No mutant survives on a touched module | `cargo-mutants`, run per change. `crates/pull`, D-0036: **263 mutants, 227 caught, 36 unviable, 0 survivors** | ◐ |
| X-08 | No tracked file contains a **slash-joined** credential path whose environment segment is a well-known one | CI gate 1c | ✓ |
| X-08b | No literal under `crates/pull` that could be a path segment is undeclared | CI gate 1d | ✓ |
| X-09 | `core` declares no dependency at all | CI gate 9 | ✓ |
| X-10 | Every reachable row in this file names a test that exists | CI gate 10 | ✓ |
| X-12 | Each vendor writes only under its own path prefix; no vendor can overwrite another | `store::unit::vendor_prefix_isolated` | — |
| X-13 | A bar-for-bar mismatch between two vendors refuses the window and names the timestamp | `store::unit::vendor_disagreement_refuses` | — |
| X-11 | A price is constructible from a float only through the one checked conversion | `core::price::refuses_an_out_of_range_price_instead_of_saturating` (private field; no other path exists) | ✓ |

---

## How this file stays honest

1. A new guarantee anywhere in the codebase adds a row **here first**.
2. The row names a test. Not a plan for a test.
3. CI checks that each named test exists. A row pointing at nothing fails the
   build, which is what stops this file from decaying into a wish list.
4. Rows are never deleted to make a build pass. Either the invariant holds or
   the decision to abandon it is recorded in `docs/05-decisions.md`.

---

## Transaction costs

Added by D-0041, and appended at the end of the file rather than beside the
other sections because two other changes were editing this file at the same
time. Every row is proven by a test that runs today.

| # | Must hold | Proven by | |
|---|---|---|---|
| K-01 | A date in an unverified window produces a refusal and never a number, on both venues and on every representable day before the boundary | `costs::regime::the_exchange_charge_refuses_exactly_the_pre_boundary_days_and_no_others` · `costs::regime::the_exchange_charge_prices_the_boundary_and_refuses_the_day_before_it` | ✓ |
| K-02 | An unverified row carries no numeric field at all, so there is nothing to unwrap, default or configure — the escape hatch does not exist rather than being closed | `costs::regime::an_unverified_row_carries_no_number_for_anything_to_reach` | ✓ |
| K-03 | The refusal names the circulars that were identified, says none was retrieved, and carries the one action that would close the window | `costs::regime::the_refusal_names_the_circulars_that_were_identified_and_never_retrieved` · `costs::error::a_refusal_names_the_charge_the_venue_the_window_and_the_remedy` | ✓ |
| K-04 | A refusal window is derived from the table, never written down a second time, and a table with no refusal row reports none | `costs::regime::a_refusal_window_is_derived_from_the_table_and_never_written_twice` | ✓ |
| K-05 | A regime boundary is inclusive on its own day: the day before and the day of resolve to different rates, on every shipped boundary | `costs::regime::stt_on_options_premium_holds_on_both_sides_of_both_boundaries` · `costs::regime::stt_on_exercise_holds_on_both_sides_of_its_boundary` | ✓ |
| K-06 | The lookup agrees with a naive last-row-that-started scan on **every** representable day against **every** shipped table, and is idempotent | `costs::regime::the_lookup_agrees_with_a_naive_scan_on_every_representable_day` | ✓ |
| K-07 | Each rate covers exactly the days it should — asserted as a histogram over the whole 40,542-day domain, so an unshipped rate anywhere is a failed comparison rather than a value folded into a neighbouring bucket | `costs::regime::a_boundary_is_inclusive_on_its_own_day_across_the_whole_domain` | ✓ |
| K-08 | Every shipped table anchors at the first representable day, ascends strictly, has no hole, carries no negative rate and carries no blank citation — and the compiler, not a test, is what enforces it | `costs::regime::every_shipped_table_passes_the_validation_the_compiler_already_ran` · `costs::regime::the_validation_rejects_every_way_a_table_can_be_wrong` | ✓ |
| K-09 | Every flat rate is the figure its citation gives, and the `bps_x100` scale reads as rupees per crore — the reading the circulars themselves quote | `costs::rate::every_flat_rate_is_the_cited_figure` · `costs::rate::the_scale_reads_as_rupees_per_crore` | ✓ |
| K-10 | Stamp duty is charged on the buy leg and is zero on the sell leg, so a round trip pays it once | `costs::rate::stamp_duty_is_charged_on_the_buy_leg_and_never_on_the_sell_leg` | ✓ |
| K-11 | Brokerage is per executed **order** and flat for both brokers — a round trip is two orders, and lots do not enter it | `costs::rate::brokerage_is_flat_per_order_and_both_brokers_are_priced` | ✓ |
| K-12 | An underlying outside `CLAUDE.md` §1's engine surface is refused and never defaulted onto a venue, and the costable set is that surface read from `core` rather than a copy of it | `costs::venue::an_underlying_outside_the_engine_surface_is_refused_and_never_defaulted` · `costs::venue::the_costable_set_is_the_engine_surface_itself_and_cannot_drift_from_it` | ✓ |
| K-13 | A date outside the representable window is refused **by name**, distinct from a date that is not real; an impossible date is refused rather than normalised | `costs::day::refuses_a_year_outside_the_window_by_name` · `costs::day::refuses_an_impossible_date_rather_than_normalising_it` | ✓ |
| K-14 | The day ordinal is a bijection onto a contiguous integer range across all 111 years, so one date compare is one integer compare | `costs::day::every_representable_day_is_one_ordinal_after_the_one_before_it` · `costs::day::the_ordinals_are_the_hand_computed_ones` | ✓ |
| K-15 | This crate's year window is `core`'s, and a widening of `core` fails here rather than silently widening the rate tables | `costs::day::the_year_window_is_cores_and_a_drift_would_be_caught` | ✓ |
| K-16 | A refusal's `Display` propagates a writer error from **every** part it writes, rather than reporting success over a truncated refusal | `costs::error::display_propagates_a_writer_error_from_every_part_it_writes` | ✓ |

### Complexity rows for the same crate

These belong with C-01 through C-13 above and are recorded here for the same
concurrency reason. They are enforced by gate 8, which runs
`crates/costs/benches/ratio.rs`, and they are held to the same **3.0× shared-CI
ceiling**.

| # | Must hold | Proven by | |
|---|---|---|---|
| C-14 | A regime lookup costs the same whichever row it selects — the anchor row and the last row of the same table | `costs::bench::the_selected_row_does_not_change_the_cost` | ✓ |
| C-15 | A regime lookup costs the same on a two-row table as on a three-row one, so the trip count is not being paid for at runtime | `costs::bench::the_row_count_does_not_change_the_cost` | ✓ |
| C-16 | Refusing costs what pricing costs — a refusal is not a slow path callers learn to avoid asking for | `costs::bench::a_refusal_costs_what_a_rate_costs` | ✓ |
| C-17 | The pre-boundary window is **refused**, not merely refused quickly — speed is half the claim and a lookup that got fast by returning the current rate would pass every ratio above | `costs::bench::the_pre_boundary_window_still_refuses` | ✓ |

Measured on the operator's machine, 2026-08-07, over **three** runs: 1.7–4.3 ns
per lookup, worst ratio **1.552×** — and that worst figure came from the run
taken while other work was compiling on the same machine, where the baseline
itself moved from 2,127 ps to 3,833 ps. The two quiet runs both topped out at
**1.189×**. No figure from a CI runner is claimed, because none was taken.

## Transaction costs — the option arithmetic

Appended for the same concurrency reason as the section above: three agents held
this file open. K-17 through K-34 and C-18 through C-22 belong to
`crates/costs` stage 2 (`docs/05-decisions.md` D-0043). Every row names a test
that runs today.

| # | Must hold | Proven by | |
|---|---|---|---|
| K-17 | The at-the-money rung is exact integer half-up rounding onto the grid, with a tie going to the **higher** strike — and the source's own float pins land on the same answers | `costs::strike::a_spot_exactly_halfway_between_two_rungs_rounds_up` · `costs::strike::the_source_pins_land_on_the_source_answers` | ✓ |
| K-18 | The rung is the **nearest** one, checked against an independently written nearest-multiple search over every paisa of two whole steps, and it never moves backwards as the spot rises | `costs::strike::the_rung_is_the_nearest_one_for_every_spot_across_two_whole_steps` · `costs::strike::the_rung_never_moves_backwards_as_the_spot_rises` | ✓ |
| K-19 | The rounding is exact for a step that does not divide the spot evenly, including an **odd** step in paisa — the case the predecessor's float chain could only approximate | `costs::strike::a_step_that_does_not_divide_the_spot_evenly_is_still_exact` | ✓ |
| K-20 | The snap happens **once**: a rung re-rounded is itself, and a resolved strike is already on the grid | `costs::strike::the_snap_happens_once_and_a_second_pass_changes_nothing` | ✓ |
| K-21 | A spot below half a step has no rung and is **refused**, not struck at zero; a zero or negative spot is refused **by name**; a rung past `i64` is refused rather than wrapped | `costs::strike::a_spot_below_the_lowest_rung_is_refused_rather_than_struck_at_nothing` · `costs::strike::a_zero_or_negative_spot_is_refused_by_name` · `costs::strike::a_spot_above_the_highest_representable_rung_is_refused_rather_than_wrapped` | ✓ |
| K-22 | "Plus" is further **out** of the money in the trade direction: a call moves up and a put moves down, and the two sides are exact mirrors about the rung | `costs::strike::plus_is_further_out_of_the_money_in_the_trade_direction` · `costs::strike::the_two_sides_are_exact_mirrors_of_each_other_about_the_rung` | ✓ |
| K-23 | A moneyness that walks off the grid is refused by name — including `i32::MIN`, whose negation has no `i32` | `costs::strike::a_moneyness_that_walks_off_the_grid_is_refused_rather_than_wrapped` | ✓ |
| K-24 | The two swept underlyings carry **different** published steps (50 and 100 rupees), and the same strike therefore reads as a different moneyness on each | `costs::strike::the_two_swept_underlyings_carry_the_two_published_steps` · `costs::moneyness::the_two_grids_disagree_about_the_same_strike_and_that_is_the_point` | ✓ |
| K-25 | The offset rounds **half toward zero**, so `moneyness == 0` is exactly the at-the-money band — proven against an independent re-derivation of the source's own classify rule over every paisa of two whole steps, on both grids and both sides | `costs::moneyness::the_bucket_is_the_sign_of_the_moneyness_and_the_band_edge_agrees` · `costs::moneyness::a_tie_lands_at_the_money_and_one_paisa_past_it_does_not_on_either_side` | ✓ |
| K-26 | The offset matches an independently written nearest-multiple search across four whole steps, and the source's own decimal-oracle pins | `costs::moneyness::the_offset_matches_a_nearest_multiple_search_across_four_whole_steps` · `costs::moneyness::the_source_offset_pins_land_on_the_source_answers` | ✓ |
| K-27 | Resolving a strike from a moneyness and reading the moneyness back off it is the **identity**, on both sides and both grids | `costs::moneyness::the_moneyness_and_the_strike_are_exact_inverses_of_each_other` | ✓ |
| K-28 | Moneyness is defined at zero, at the documented chain edges (±10) and one past each — and one past is **outside the chain yet still resolvable**, because the chain is a query and never a refusal | `costs::moneyness::moneyness_at_zero_at_the_chain_edges_and_one_past_each` · `costs::moneyness::the_seven_locked_offset_rules_read_as_the_source_spells_them` | ✓ |
| K-29 | The lot size in force is the source's figure on **every** transition date and on the day before it, and the two underlyings differ on the same day | `costs::lot::every_transition_and_the_day_before_it_are_the_source_figures` · `costs::lot::the_two_underlyings_carry_different_lots_on_the_same_day` | ✓ |
| K-30 | The quantity is `lots × lot size` and nothing else; a lot count that is not a trade is refused by name; an overflow is refused rather than wrapped or saturated | `costs::lot::the_quantity_is_the_product_and_nothing_else` · `costs::lot::a_lot_count_that_is_not_a_trade_is_refused_by_name` · `costs::lot::a_quantity_past_i64_is_refused_rather_than_wrapped` | ✓ |
| K-31 | The next weekly expiry is the **first** day on or after the day asked with the regime's weekday — checked by brute-force day scan across the whole verified window on both underlyings | `costs::expiry::the_next_weekly_is_the_first_day_on_or_after_with_the_regimes_weekday` | ✓ |
| K-32 | The next monthly expiry is the last regime weekday of its own month or the next, on every day of the verified window — and the regime is **re-read** for a rolled month rather than carried across it | `costs::expiry::the_next_monthly_is_the_last_regime_weekday_of_its_own_or_the_next_month` · `costs::expiry::the_regime_is_re_read_for_the_rolled_month_and_not_carried_across` | ✓ |
| K-33 | A withdrawn weekly contract is a **cited value**, never a refusal, and the two never read the same | `costs::expiry::a_withdrawn_weekly_and_a_refusal_never_read_the_same` | ✓ |
| K-34 | Every day before the source's own recorded history **refuses**, on every stage-2 table, and the refusal is carried out of the date functions word for word rather than replaced by a vaguer one | `costs::lot::the_pre_history_window_refuses_on_every_day_of_it_and_never_after` · `costs::expiry::the_pre_history_window_refuses_on_both_tables_and_every_day_of_it` · `costs::strike::the_step_is_refused_before_the_window_the_source_verified` | ✓ |
| K-35 | The day ordinal round-trips exactly over all 40,542 representable days, the weekday advances one day at a time across all of them, and an ordinal off either end is refused by name rather than saturated | `costs::day::from_ordinal_is_the_exact_inverse_of_ordinal_over_the_whole_window` · `costs::day::the_weekday_walks_forward_one_day_at_a_time_across_the_whole_window` · `costs::day::an_ordinal_off_either_end_is_refused_by_name_and_never_saturated` | ✓ |
| K-36 | The per-underlying tables are keyed on `core`'s own `SWEPT` order, asserted at **compile time**, so a reorder or a widening is a build failure rather than a silently swapped lot size | `costs::venue::a_slot_is_cores_own_index_and_round_trips_to_cores_own_row` · `costs::strike::the_step_tables_are_keyed_on_cores_order_and_every_row_is_positive` · `costs::lot::every_shipped_lot_row_is_positive_and_keyed_on_cores_order` · `costs::expiry::every_shipped_expiry_row_is_a_trading_weekday_and_keyed_on_cores_order` | ✓ |
| K-37 | The generic dated lookup agrees with a naive last-row-that-started scan on **every** representable day, and every way a table can be shaped wrong is caught by the compiler | `costs::dated::the_lookup_agrees_with_a_naive_scan_on_every_representable_day` · `costs::dated::every_shape_violation_is_caught_and_a_sound_table_is_not` | ✓ |

### Complexity rows for the option arithmetic

Enforced by gate 8, `crates/costs/benches/ratio.rs`, against the same **3.0×
shared-CI ceiling**.

| # | Must hold | Proven by | |
|---|---|---|---|
| C-18 | The at-the-money rung costs the same wherever the spot is — a 50-rupee spot and a spot near `i64::MAX` | `costs::bench::the_spot_magnitude_does_not_change_the_rung_cost` | ✓ |
| C-19 | The resolved strike costs the same however deep the moneyness — `ATM`, the chain edge, and a million steps out — and reading a moneyness off a far strike costs what reading it off a near one does | `costs::bench::the_moneyness_depth_does_not_change_the_strike_cost` | ✓ |
| C-20 | The quantity costs the same for one lot and for a million, so the multiplication has not become an accumulation | `costs::bench::the_lot_count_does_not_change_the_quantity_cost` | ✓ |
| C-21 | The expiry costs the same wherever in the calendar it is asked: the in-month arm against the rollover arm, January against a December that rolls the year, and zero days ahead against six | `costs::bench::the_calendar_position_does_not_change_the_expiry_cost` | ✓ |
| C-22 | The stage-2 pre-history windows are **refused**, not merely refused quickly — a lot-size lookup that got fast by handing back the first recorded lot for a 2019 trade would pass every ratio above | `costs::bench::the_stage_two_pre_history_windows_still_refuse` | ✓ |

Measured on the operator's machine, 2026-08-07, over four runs. Per-call cost
0.62–15.7 ns. Every ratio held under 3.0×; the largest was **2.300×**, the
monthly rollover arm against the in-month arm, and that figure is the bound
working as stated rather than a scan appearing: the rollover arm does the month
resolution and the table lookup **twice**, which is exactly the "at most two"
the claim is. Every other stage-2 ratio, across all four runs, stayed inside
0.81×–1.14×. No figure from a CI runner is claimed, because none was taken.
