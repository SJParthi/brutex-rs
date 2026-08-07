# 04 — Invariants

Every row names the test that proves it. **An invariant with no test named
beside it is deleted from this file, not shipped.** A consistency check in CI
fails if a row's test does not exist.

Status: `✓` proven · `◐` proven where it has been run, and where that is is
named · `○` test written, awaiting the crate · `—` not yet reachable (the crate
does not exist) · `✗` **the named test does not exist in any file, and the crate
it names is tracked today** — the row is a gap, not a proof.

**`✗` was added by D-0045 and it is the point of that entry.** Seven rows wore
`—` while naming a test that exists in zero files, in three crates that are
tracked and compiled today. `—` means "the crate does not exist", so those rows
read as *not yet reachable* when the truth was *nobody wrote it*. A row pointing
at a phantom test is worse than an empty row, because it looks proven. Every
such row now carries `✗`, keeps the test name it would need, and says in its own
cell that the name is a plan rather than a proof. The names are deliberately
**left in the backticks** so CI gate 10 goes on reporting them by name every
run; removing the token would make the gate green by blinding it.

**Six of the ten rows gate 10 reported now name a test that runs. Four still do
not, and the gate is still red on exactly those four.** What changed and what
did not is in *The ten phantom rows, one at a time* near the end of this file.
The four that remain — P-03, X-01, X-02, X-13 — kept their phantom names for the
reason the paragraph above gives, and a red gate whose four lines a reader can
name is worth more than a green one bought by deleting them.

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
| S-02 | `read(i)` returns the bytes `write(i, b)` wrote, for every *i* | `store::roundtrip::every_committed_index_returns_the_record_that_was_written` · `store::roundtrip::the_open_interest_sentinel_survives_the_disk_and_is_not_a_zero` — **every** index of a 160-record file, across three appends, two block boundaries and a close-and-reopen, asserted twice at each index: the record `read_record` returns, and the 56 raw bytes the file holds at `Layout::offset_of(i)`. The second is the half a codec test cannot make. This row named a `proptest` module for which this workspace has no dependency; S-18's `decode(image(r)) == r` over 4,276 records is still the codec's proof and was never the file's | ✓ |
| S-03 | A reader never observes a record beyond `n_valid` | `store::fault::commit_counter_publishes_last` — the module was written `store::loom::`, and there is **no `loom` module and no `loom` dependency**; the test is a plain `#[test]` walking all 65 commit prefixes | ✓ |
| S-04 | A crash between data write and counter publish loses the tail and corrupts nothing | `store::fault::kill_between_write_and_commit` | — |
| S-05 | A full disk during append returns `Err`, never a signal | **PROVEN AT THE CLASSIFIER, NOT AT THE KERNEL.** `store::file::a_full_disk_mid_write_is_returned_and_never_signalled` · `store::file::every_classified_kind_gets_its_own_name` — a scripted host returns `ErrorKind::StorageFull` part way through a write and the loop hands back `StoreError::DiskFull` as a **value**, which is the whole argument for banning a writable mapping. That loop is the only write path `append` has. **UNVERIFIED: that a real full disk produces that kind on this store's write.** No test in this repository fills a filesystem, and `crates/store/tests/write.rs` names this and the read-only mount as the two conditions it will not fake. The row named a `fault` module test that exists in no file | ◐ |
| S-06 | A flipped bit in any block is detected on the next read of that block | `store::fault::bitflip_detected` | — |
| S-07 | A file whose length does not divide by the stride truncates to the last whole record and logs | `store::fault::ragged_tail_truncates_loudly` | — |
| S-08 | `i64::MIN` in `open_interest` is never confused with `0`, and round-trips as null through the record image | `store::unit::oi_sentinel_distinct` — the module was written `store::proptest::`, and **no `proptest` dependency exists**; it is a plain `#[test]`, and it proves the *distinctness* half only. The *round-trip* half is `store::unit::decoding_the_image_returns_the_record_byte_for_byte` (S-18), which walks `i64::MIN` in every field | ✓ |
| S-09 | Opening a file with an unknown `format_version` refuses; it never guesses | `store::unit::unknown_version_refuses` | — |
| S-10 | Two concurrent writers on one file are refused by the advisory lock | `store::write::a_second_writer_is_refused_while_the_month_is_held` — the test **exists and passes**, and this row named an `integration` module that does not exist while claiming no lock was exercised anywhere. A second `BarFile` on a held month is `StoreError::Locked` naming the lock file, the month opens again once the first is dropped, and the lock file is not deleted. Two open descriptions in **one process**; a second *process* is exercised nowhere here, and `crates/store/src/file.rs` lists what an advisory lock does not protect against | ✓ |
| S-11 | The checksum reproduces a hardcoded value at every length a wide kernel can break on — 0, 1, 8, 15, 16, 56, 60, 64, 4087, 4088, 4089 bytes, and the all-zero and all-ones block | `store::unit::the_crc_reproduces_a_hardcoded_value_at_every_length_that_can_break` | ✓ |
| S-12 | The shipped checksum kernel agrees with an independent bit-by-bit reference at every length across a stride boundary, on every target | `store::unit::the_fast_kernel_agrees_with_a_bit_by_bit_reference_on_every_length` | ✓ |
| S-13 | The header slot's covered domain is exactly bytes `0..56 ‖ 60..64`; filling the four-byte hole is a different number and is refused as such | `store::unit::the_covered_domain_is_the_slot_minus_its_checksum` | ✓ |
| S-14 | Splitting the checksum input at any point gives the answer for the whole | `store::unit::splitting_the_input_anywhere_gives_the_same_checksum` | ✓ |
| S-15 | The lookup table is the polynomial: lane 0 is one folded byte, and lane *k* is lane *k−1* advanced by one zero byte | `store::crc::the_table_is_the_polynomial_lane_by_lane` | ✓ |
| S-16 | The 56 bytes one known record encodes to are pinned as a literal array. A body replaced with zeros, a moved offset or a flipped byte order each fail | `store::unit::the_record_image_is_the_pinned_bytes_of_a_known_bar` | ✓ |
| S-17 | The image is little-endian and each of the seven fields owns its own eight bytes; all 56 (field, byte) placements are asserted against all 56 image bytes | `store::unit::the_image_is_little_endian_and_each_field_owns_its_own_offset` | ✓ |
| S-18 | `decode(image(r)) == r` exactly, over every 64-bit boundary in every field — `i64::MIN`, `i64::MAX`, zero, negatives — and the encoder is injective across the sampled 4,276 records | `store::unit::decoding_the_image_returns_the_record_byte_for_byte` | ✓ |
| S-19 | A buffer shorter than 56 bytes is refused by length and never completed with invented zeros | `store::unit::a_short_record_is_refused_and_never_completed_with_zeros` | ✓ |
| S-20 | `BLOCK_LEN == RECORD_STRIDE × RECORDS_PER_BLOCK == 56 × 73 == 4088`, and the last record of a block ends exactly on the block boundary — so no record straddles | `store::geometry::no_record_straddles_a_block` (5,000 indices), plus **three** compile-time pins in `store::format` that can actually fail: `BLOCK_LEN == 4088`, `RECORDS_PER_BLOCK == 73`, `BLOCK_LEN == 56 * 73`. **The closed-form assertion the source comment points at cannot fail and proves nothing** — see below | ✓ |
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

**Re-measured independently for D-0045**, by running `cargo bench -p api` on the
same machine on 2026-08-07 — exit **0**, "all ratios within the ceiling". Across
all six sort columns × four pills, plus the escape hatch and the clamped deep
page: **C-14** ratio 2,787 → 50,000 spans **0.929× – 1.088×**; **C-15** marginal
**0 – 259 ps** per instrument per request against the asserted 1,000 ps ceiling;
**C-16** dashboard **1.052×** at 2 → 50,000; **C-17** cost per rendered row
**0.202× – 0.404×**, with the row counts printed and checked (200 drawn at
n = 50,000). The absolute page figure is ~137–166 µs at both sizes. The audit's
"before" pair (3.569 ms and 124.916 ms) was taken on a **debug** server and this
is a release bench, so only the *shape* is comparable between them — the ratio
is, and the ratio is the claim.

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
and proven by `store::bench::read_ratio`, which did not exist — there was no bar
reader in `crates/store` then. D-0034 restated it as the flatness that **is**
measurable today and named a bench that runs.

**The reader has since shipped and this paragraph's last sentence has not.**
`BarFile::read_record` is one multiply, one add and one 56-byte positional read,
and S-02 walks it at every index of a 160-record file. C-01 still names the
header read rather than the bar read, and deliberately: **no bench in this
repository times a syscall.** `crates/store/benches/ratio.rs` measures the
arithmetic and the checksum, which is what `crates/store/src/file.rs` says in as
many words beside `read_record` — the operation is constant and the device
latency underneath it is UNVERIFIED. A bar-read ratio row that timed a `pread`
would be measuring the operator's disk, so the row returns when there is a bench
that separates the two, not merely when the reader exists.

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
| P-01 | A rate governor never issues above the configured ceiling, under any concurrency | `pull::concurrency::the_ceiling_holds_however_many_threads_share_the_governor` · `pull::concurrency::a_throttle_recorded_by_one_thread_binds_every_other` — 512 requests from eight real threads at one fixed instant are issued exactly the allowance between them, three times over, and a throttle one caller records binds the rest. `Governor::admit` takes `&mut self`, the type holds no interior mutability and the crate is `#![forbid(unsafe_code)]`, so exclusive access is the **only** sharing safe Rust admits and no two `admit` calls can interleave — which is why the `loom` module this row named is not merely absent but would have nothing to enumerate. **What is not bounded: two separate `Governor` values.** The type is `Copy`; nothing here or in the crate holds the sum of two of them to one ceiling | ◐ |
| P-02 | A bar outside the requested window is never stored | `pull::integration::a_bar_outside_the_window_or_the_session_is_never_stored` · `pull::integration::a_narrower_window_stores_strictly_fewer_bars_and_says_why` — **"never stored" is now checkable**, because `pull::ingest::from_dir` takes a vendor's folder all the way to an append. Both tests reopen the month afterwards and read **every** committed record back, asserting each one is inside the operator's window and inside the exchange's session, from a fixture carrying a row on each side of every boundary — 15:29:59 in, 15:30:00 out. The narrower window stores strictly fewer bars off the same bytes, so the filter is keyed on the request rather than on the file. The window *arithmetic* remains `pull::unit::a_window_is_inclusive_at_both_ends_and_refuses_to_run_backwards`, `pull::unit::every_second_of_a_day_falls_on_exactly_one_side_of_the_session` and `pull::unit::an_inclusive_window_survives_the_vendors_exclusive_to_date`. One member, one month, one instrument | ✓ |
| P-03 | A bar on a non-trading date is dropped and counted | **NOT PROVEN, and the code says so first.** `pull::unit::calendar_filter` exists in no file, and `crates/pull/src/session.rs` states plainly that there is **no trading calendar and no holiday list** here — so there is nothing yet to prove. A weekend rule without a holiday list would be wrong, which is why `pull::unit::a_saturday_is_a_full_session_because_there_is_no_weekend_rule` asserts the *absence* as the current behaviour. **The name stays in the backticks and CI gate 10 goes on reporting it by name every run.** That is the intent, not an oversight: this row is one of the four the gate is deliberately red on | ✗ |
| P-04 | Re-running an ingest stores nothing new and reports zero net-new | **THE DISK HALF HOLDS. THE REPORTING HALF DOES NOT, AND THE TEST PINS THAT RATHER THAN HIDING IT.** `pull::integration::idempotent_repull_leaves_the_file_byte_identical` · `pull::integration::a_second_window_over_the_same_month_appends_rather_than_rewrites` — a second run over the same folder and window leaves the bar file **byte for byte** what it was, and a run that brings bars the file does not hold still appends them, so idempotence is not bought by refusing every second run. What is **false today** is "reports zero net-new": `Ingested::bars_stored` counts the bars a member *offered*, `BarFile::append`'s already-present answer never reaches it, and the re-run therefore reports 3 stored where it wrote 0. The test asserts that 3. Fixing it is a `crates/pull/src/ingest.rs` change this row does not own | ◐ |
| P-05 | A credential is read, never written; no token is ever minted | `pull::unit::readonly_credentials` (a write attempt must panic the test double) | ✓ |
| P-06 | An auth failure halts the pull loudly rather than degrading | `pull::unit::auth_halt` | ✓ |
| P-07 | A missing, unreadable, or incomplete credential configuration halts the pull and names the absent segment; it never defaults | `pull::unit::credential_config_absent_halts` · `pull::unit::a_missing_table_or_key_is_a_halt` | ✓ |
| P-08 | The credential configuration supplies path segments only; a secret value found in it is refused | `pull::unit::credential_config_rejects_secret_value` | ✓ |

Added by D-0035. **That paragraph read "`P-01` through `P-04` keep their `—`:
there is no rate governor, no window walk and no calendar filter yet." Two
thirds of it went stale and nothing moved the rows.** D-0037 shipped
`pull::rate::Governor`; `pull::session` shipped `Window`, `Day`, `DropReason`
and `DropCensus`. So a rate governor and a window walk both exist today, and all
four rows still named tests that exist in no file while wearing a glyph that
means *the crate does not exist*. `crates/pull` is tracked and compiled.
Corrected to `✗` by D-0045, each row saying which half is proven and which half
is not. Only the calendar filter is still genuinely absent from the code, and
`crates/pull/src/session.rs` is where that is decided and said.

**Three of those four rows have since been given tests, and the fourth has not.**
`crates/pull/src/ingest.rs` shipped the join — a vendor's folder to
`BarFile::append` — which is what made "stored" a checkable word for the first
time, so P-02 and P-04 are now driven end to end by
`crates/pull/tests/integration.rs`, and P-01 by
`crates/pull/tests/concurrency.rs`. **P-03 is unchanged and stays `✗`**: there
is still no trading calendar, and `crates/pull/src/session.rs` and
`crates/pull/src/lib.rs` both still say P-03 keeps its `—` where the table has
said `✗` since D-0045. Those two headers are stale on the glyph and right on the
substance; they are not this change's files to edit.

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

**That paragraph read "`P-01` keeps its `—`, narrowed rather than satisfied",
and the glyph was wrong twice over** — D-0045 moved it to `✗`, and it is now
`◐`. The reasoning it gave is still the right reasoning and is now the
*argument* rather than the excuse: `Governor` takes `&mut self` and has no
interior mutability, so it cannot be shared across tasks without a lock this
crate does not supply — and that is exactly why an interleaving checker has
nothing to enumerate here. `crates/pull/tests/concurrency.rs` supplies the lock
in the test, races eight threads through it, and asserts the pool is held to the
allowance one caller would have had. The single-caller arithmetic below is
unchanged and is still where every boundary is asserted at the microsecond.

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
**The seven rows below were appended as I-16 … I-22, and those seven ids were
already taken** by the equity-gate section that follows. **Renumbered to I-31 …
I-37 by D-0045**, taking the next free ids after I-30. No file outside this one
cited either block — checked before the renumber — so nothing else moves.

| # | Must hold | Proven by | |
|---|---|---|---|
| I-31 | A vendor field wider than `MAX_FIELD_BYTES` is refused **before** anything reads it, whichever of the ten fields it is | `core::vendor::an_over_wide_field_is_refused_whichever_field_it_is` | ✓ |
| I-32 | The width bound is the first byte that is too many and not one before, and it never substitutes for the parsers below it | `core::vendor::the_bound_is_the_first_byte_that_is_too_many_and_not_one_before` | ✓ |
| I-33 | The widest value measured in either real master still passes the width gate untouched | `core::vendor::a_row_of_ordinary_width_passes_the_gate_untouched` | ✓ |
| I-34 | The width gate runs before the test-marker scan and does not shadow it | `core::vendor::the_test_marker_scan_still_declines_a_real_test_listing` | ✓ |
| I-35 | A master larger than the reader holds is refused from its size, before it is read into memory | `api::master::a_master_larger_than_this_reader_holds_is_refused_before_it_is_read` | ✓ |
| I-36 | A row longer than the reader splits is named at its line number and never split | `api::master::a_row_longer_than_this_reader_splits_is_named_and_never_split` | ✓ |
| I-37 | An over-wide field makes the row an error that names the field; it is never a silent keep | `api::master::a_field_wider_than_core_will_read_is_an_error_and_not_a_silent_keep` | ✓ |

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
| A-16 | The coverage grid's axis is the store's own vocabulary, so a manifest holding only F&O futures still reaches the grid — rows held and cells held cannot disagree about an empty store | `api::census::a_store_of_nothing_but_futures_still_reaches_the_grid` | ✓ |
| A-17 | A census that is absent or unreadable contributes no series to the axis and invents none; two vendors holding one series contribute one row | `api::census::an_unloadable_census_adds_nothing_to_the_axis` | ✓ |
| A-18 | The two swept series are always on the axis, held or not, so a fresh install names what it is missing | `api::census::a_store_of_nothing_but_futures_still_reaches_the_grid` · `api::server::a_site_with_no_universe_still_shows_the_two_instruments_that_matter` | ✓ |
| A-19 | A series renders as the store names it, and `Series::of` and `Series::at` are inverses, so the axis and the probe are the same values | `api::census::a_series_reads_back_as_the_store_names_it` | ✓ |
| A-20 | No page this server emits contains a script, and the date picker — the widget most tempted to need one — contains none either | `api::render::the_page_contains_no_script_at_all` · `api::calendar::nothing_the_picker_emits_is_a_script` | ✓ |
| A-21 | At most one date panel per form can be open, so two panels cannot overlap at any viewport | `api::calendar::the_latch_is_a_radio_so_two_panels_cannot_be_open_at_once` | ✓ |
| A-22 | Exactly one pane of the picker is shown at a time, chosen by which radios are checked and by no extra control | `api::calendar::three_panes_are_emitted_and_the_year_pane_is_the_one_with_no_prerequisite` | ✓ |
| A-23 | A month arrow steps exactly one month and never crosses a year boundary; the two that would are inert spans, not labels | `api::calendar::the_arrows_step_one_month_and_do_not_cross_a_year_boundary` | ✓ |
| A-24 | No control in the picker is `required`, because an unfocusable `required` control blocks submission in silence | `api::calendar::no_control_in_the_picker_is_required` · `api::server::the_pickers_do_not_block_submission_and_do_not_close_on_their_own_chrome` | ✓ |
| A-25 | Nothing later than a field's ceiling can be clicked — not a day, and not a month in the ceiling's own year | `api::calendar::the_computed_rules_cover_alignment_the_month_end_and_the_ceiling` | ✓ |
| A-26 | The picker never offers a year no `Day` can hold | `api::calendar::the_offered_span_ends_at_the_cap_and_is_twelve_years_long` | ✓ |

## The vendor socket

Added by D-0049 and D-0050. `crates/pull/src/http.rs` is the only code in this
repository that reaches a broker, so every row here is about a bar that must not
be invented and a credential that must not travel.

| # | Must hold | Proven by | |
|---|---|---|---|
| H-01 | Every one of a bar's seven fields is read from the **one** object the descriptor's `envelope` names, so a bar can never be assembled from two different JSON objects | `pull::http::one_bar_can_never_be_assembled_from_two_different_objects` | ✓ |
| H-02 | With no envelope declared, the top level is the only place looked; a wrapped body is refused rather than rummaged through | `pull::http::no_envelope_means_the_top_level_and_nowhere_else` | ✓ |
| H-03 | A declared envelope the answer does not carry is refused naming the key expected **and** the keys present, so a wrong descriptor is a one-row diff | `pull::http::a_missing_envelope_names_the_key_expected_and_the_keys_present` | ✓ |
| H-04 | Arrays under a declared envelope are found, and only there | `pull::http::arrays_under_the_declared_envelope_are_found` | ✓ |
| H-05 | A redirect is never followed, so the credential never reaches a host the descriptor did not name — proven over two real sockets, and confirmed to fail without the policy | `pull::http::a_redirect_is_refused_and_the_token_never_reaches_its_target` | ✓ |
| H-06 | A redirect is reported as `VendorRefused` carrying its status, the `Location`, and the reason — never silently, and never carrying the credential | `pull::http::a_redirect_is_refused_and_the_token_never_reaches_its_target` | ✓ |
| H-07 | An ordinary refusal still carries the vendor's own words and its own status, so a 429 stays distinguishable from a 500 | `pull::http::a_non_redirect_refusal_carries_the_body_and_its_status` | ✓ |
| H-08 | The credential never appears in a `Debug` rendering, and the blocking seam refuses by name rather than silently blocking | `pull::http::the_sync_seam_refuses_and_the_token_is_never_printed` | ✓ |

## Cross-cutting

| # | Must hold | Proven by | |
|---|---|---|---|
| X-01 | Run identity changes if any loaded bar differs by one field | **NOT PROVEN, AND THERE IS NOTHING TO PROVE IT AGAINST.** `core::proptest::identity_sensitivity` exists in no file; no `proptest` dependency exists. There is no run-identity function either: `blake3` sits in the workspace dependency table and **no member takes it**, and no crate holds a function that hashes `CLAUDE.md` §3 rule 3's eight inputs. So this row has neither the test nor the code, and the name stays in the backticks — gate 10 is deliberately red on it | ✗ |
| X-02 | Prices never touch a float on any path from wire to store to result | **PARTLY PROVEN, and the row overstated both halves.** `core::lint::no_float_in_price` exists in no file, so the *source check* does not exist. The *lint* is `float_arithmetic`, and in `Cargo.toml` it is `"warn"`, **not** `"deny"` — it fails a build only because CI passes `-D warnings`, and it fires on float *arithmetic*, never on a float *type* held in a price. What is genuinely proven is X-11: `Price` has a private field, so the one checked conversion is the only constructor. **No test was written for this row and none is claimed.** A source scan is CI gate 11's mechanism, not a test's, and a Rust test that grepped the tree would assert a spelling rather than the property; the name stays in the backticks and gate 10 is deliberately red on it | ✗ |
| X-03 | Every tracked file has an allowed extension | CI gate 1 | ✓ |
| X-04 | No build script invokes an external process | CI gate 2 | ✓ |
| X-05 | `web` depends on `core` alone | CI gate 7 | ✓ |
| X-06 | **Line and region** coverage is 100% on every crate, with no omit list | **THE GATE EXITS 1 ON THIS TREE.** Measured by running the CI command itself — `cargo llvm-cov --workspace --locked --fail-under-lines 100 --fail-under-regions 100 --summary-only`, cargo-llvm-cov 0.8.4, 2026-08-07 at commit `79c5e80`: **exit 1**, TOTAL **96.75% regions** (851 of 26,169 missed), **96.24% lines** (592 of 15,734), 93.88% functions (94 of 1,537). Eight files are short, and the table below names every one | ✗ |
| X-06b | ~~Branch coverage is 100% on every crate~~ | **NOT MEASURED.** `llvm-cov` instruments zero branches on the pinned stable toolchain and `--branch` cannot run there at all. Narrowed by D-0030; recorded in `docs/06-limits.md` §7. | — |
| X-07 | No mutant survives on a touched module | `cargo-mutants`, run per change. `crates/pull`, D-0036: **263 mutants, 227 caught, 36 unviable, 0 survivors**. `crates/costs`, D-0044: **163 mutants over `trip.rs`, `money.rs`, `fill.rs`, `scope.rs`, `error.rs` — 106 caught, 57 unviable, 0 survivors**, and one mutant that *did* survive a first run is written up in `docs/06-limits.md` §27. **Never measured at all: `crates/core`, `crates/store`, `crates/api`**, and the nine `crates/costs` files outside that list. `crates/store` planted five mutants by hand, which §22 records is not a survey. There is no `cargo-mutants` step in CI, so nothing enforces this row | ◐ |
| X-08 | No tracked file contains a **slash-joined** credential path whose environment segment is a well-known one | CI gate 1c | ✓ |
| X-08b | No literal under `crates/pull` that could be a path segment is undeclared | CI gate 1d | ✓ |
| X-09 | `core` declares no dependency at all | CI gate 9 | ✓ |
| X-10 | Every reachable row in this file names a test that exists | **THE GATE EXITS 1 ON THIS TREE, AND THIS ROW SAID `✓` WHILE IT DID.** Measured by running gate 10's own script at this commit: 293 rows read, 294 named tests checked against a tracked crate, 17 skipped for a crate that is not a workspace member, **4 missing** — P-03, X-01, X-02, X-13. It read **10 missing** before this pass. The four are `✗` in their own rows, each saying what is absent, and CI gate 10 goes on naming them every run. This row is `✗` and not `◐` on the same argument X-06 makes: a tick beside a red gate is the defect, whatever the reason for the red | ✗ |
| X-12 | Each vendor writes only under its own path prefix; no vendor can overwrite another | `store::unit::vendor_prefix_isolated` — the test **exists and passes**, and this row wore `—` anyway. It proves the claim *lexically*: the first segment is a `Vendor` rather than a string, and no segment can hold a separator. It does not touch a filesystem | ✓ |
| X-13 | A bar-for-bar mismatch between two vendors refuses the window and names the timestamp | **NOT PROVEN, AND THE REASON HAS CHANGED.** `store::unit::vendor_disagreement_refuses` exists in no file. `crates/store` **does** have a bar reader now — `BarFile::read_record`, walked at every index by S-02 — so the missing piece is no longer the reader. What is missing is the comparison: nothing in this repository opens two vendors' months and matches them bar for bar, so there is no code for a test to drive. The name stays in the backticks and gate 10 is deliberately red on it | ✗ |
| X-11 | A price is constructible from a float only through the one checked conversion | `core::price::refuses_an_out_of_range_price_instead_of_saturating` (private field; no other path exists) | ✓ |

### X-06, measured rather than asserted — D-0045

The row above carried a tick. The gate it names exits 1, and had been exiting 1
for some time. These are the eight files short of 100%, from the run described
in the row:

| File | Regions | Missed | | Lines | Missed | |
|---|---|---|---|---|---|---|
| `pull/src/vendor.rs` | 445 | 445 | **0.00%** | 366 | 366 | **0.00%** |
| `pull/src/ingest.rs` | 168 | 168 | **0.00%** | 100 | 100 | **0.00%** |
| `pull/src/archive.rs` | 142 | 50 | 64.79% | 93 | 34 | 63.44% |
| `pull/src/fetch.rs` | 210 | 60 | 71.43% | 148 | 47 | 68.24% |
| `pull/src/csv.rs` | 373 | 99 | 73.46% | 160 | 30 | 81.25% |
| `pull/src/fold.rs` | 94 | 11 | 88.30% | 77 | 14 | 81.82% |
| `store/src/file.rs` | 813 | 17 | 97.91% | 510 | 0 | 100.00% |
| `api/src/ingest.rs` | 730 | 1 | 99.86% | 546 | 1 | 99.82% |

Six of the eight are in `crates/pull`, and **two of them are at 0.00%** — 613
regions and 466 lines between them, in tracked files with no `#[test]` in them
at all. `archive.rs`, `fetch.rs`, `csv.rs` and `fold.rs` are the same pattern
less severely. That is modules committed ahead of their tests, and it is the
same shape as the defect D-0029 recorded for `core/src/universe.rs`: CI gate 10
walks rows→tests and never tests→rows, so a module that claims nothing is
invisible to it.

**This figure is a snapshot and it moved twice while D-0045 was being written.**
The first run of the session measured 97.41% regions / 96.93% lines over six
short files; commits `1f98bb5`, `eb95996` and `79c5e80` then landed and it fell
to the figure above over eight. An audit before that measured 95.31% lines /
95.91% regions. **The gate has been red across all three**, and the direction is
not monotone. That volatility is the argument for recording a command, a commit
and a date here rather than a tick: the tick was wrong at every one of those
points and looked equally right at each.

### S-20 names three assertions that cannot fail — D-0045

`crates/store/src/format.rs` defines `BLOCK_LEN` as
`RECORD_STRIDE * RECORDS_PER_BLOCK`. Three of the `const` assertions beside it
are therefore **tautologies** — they restate that definition and hold for every
possible value of the two factors:

- `assert!(BLOCK_LEN.is_multiple_of(RECORD_STRIDE))`
- `assert!(BLOCK_LEN / RECORD_STRIDE == RECORDS_PER_BLOCK)`
- `assert!((RECORDS_PER_BLOCK - 1) * RECORD_STRIDE + RECORD_STRIDE == BLOCK_LEN)`
  — which reduces to `n·s == n·s`

**Measured, not reasoned:** a scratch crate carrying the identical definitions
and exactly these three assertions compiles at **exit 0** with
`RECORDS_PER_BLOCK = 72`, and again at **exit 0** with `RECORDS_PER_BLOCK = 1`.
The mutant D-0039 reports killing — `BLOCK_LEN` 4088 → 4096 — is also
unwritable, because `BLOCK_LEN` is not a literal to mutate.

The third of these is the one the source comment introduces with *"a compile
error rather than a walk"*, and `CLAUDE.md` §4 bans a test that asserts nothing.
**The no-straddle property is still genuinely secured** — by `BLOCK_LEN == 4088`,
`RECORDS_PER_BLOCK == 73` and `BLOCK_LEN == 56 * 73`, which are falsifiable and
which D-0039 also added, plus the runtime walk. So this is a wrong *citation*,
not a missing guarantee, and S-20 now names the assertions that carry it. The
source comment is in `crates/store/src/format.rs` and is reported rather than
edited — that file is not this change's to touch.

### The nine rows that name a tool this repository does not have — D-0045

`loom` and `proptest` appear in **no `Cargo.toml` in this workspace**. Verified
by `grep -rn 'loom\|proptest' --include=Cargo.toml .`, which returns nothing.
Nine rows name a module of one of those two names:

*Written as a list, not a table: a `| id |` row here would be parsed by CI gate
10 as a real invariant row and counted twice.*

- **S-02** named `proptest::roundtrip` — ✗ no such function anywhere.
- **S-03** named `loom::commit_counter_publishes_last` — ✓ it **exists**, as
  `store::fault::…`, an ordinary `#[test]`. Path corrected above.
- **S-08** named `proptest::oi_sentinel_distinct` — ✓ it **exists**, as
  `store::unit::…`, an ordinary `#[test]`. Path corrected above.
- **V-03** named `indicators::proptest::suffix_independence` — `—`,
  `crates/indicators` does not exist.
- **V-05** named `indicators::proptest::differential_vs_naive` — `—`, same.
- **E-01** named `engine::proptest::antimonotone` — `—`, `crates/engine` does
  not exist.
- **E-02** named `engine::proptest::apriori_equals_bruteforce` — `—`, same.
- **P-01** named `pull::loom::governor_ceiling` — ✗ no such function, and
  `crates/pull` is tracked and compiled today.
- **X-01** named `core::proptest::identity_sensitivity` — ✗ no such function,
  and `crates/core` is tracked and compiled today.

The four `—` entries are legitimately unreachable: their crates do not exist, which
is what `—` means. But **the module name is a promise about a dependency**, and
adding either tool is a workspace-manifest change nobody has made or decided on.
Those four rows are naming an implementation that would have to be chosen first.
No row here now claims a property-based or a concurrency proof that has ever run.

**Two of those nine have since moved, and the tool count did not change.**
`loom` and `proptest` are still in **no** `Cargo.toml` in this workspace, and
neither was added to satisfy a row — re-checked with the same command. S-02 is
now proven by `store::roundtrip::…`, ordinary `#[test]`s that walk *every* index
rather than sampling random ones, and P-01 by `pull::concurrency::…`, ordinary
`#[test]`s with real threads. X-01 is unchanged and still `✗`. A property-based
proof and an interleaving proof remain things this repository has never run, and
no row claims either.

### Why gate 10 could not see any of this — D-0045

Gate 10's own comment says it: *"The module segment. `store::unit::x` and
`store::fault::x` are the same question to this gate."* It matches on
`(crate, fn)`. So S-03 and S-08 passed it for their whole lives while pointing
at modules that do not exist, and the ten genuinely-absent tests were caught
only when the gate stopped honouring the `—` glyph as a skip. A row can still
name the wrong module and stay green. That gap is now recorded rather than
rediscovered.

### The ten phantom rows, one at a time

*Written as bullets, not a table, for the reason the section above gives: a
`| id |` line here would be read by CI gate 10 as a real invariant row.*

**No decision id is claimed for this pass.** `docs/05-decisions.md` ends at
D-0045 and is not this change's file; the ledger entry it owes is named at the
foot of this section.

Two of the ten were **already proven and pointing at the wrong name** — the
worst of the three cases, because such a row reads as a gap when the property
holds, and the next person writes the test twice:

- **S-10** named `store::integration::second_writer_refused` and said "no
  advisory lock is exercised anywhere in this repository".
  `store::write::a_second_writer_is_refused_while_the_month_is_held` had been
  exercising one, and passing. Row corrected; nothing was written.
- **S-05** named `store::fault::enospc_returns_error`.
  `store::file::a_full_disk_mid_write_is_returned_and_never_signalled` proves
  the classifier and the write loop against a scripted host. Row corrected to
  `◐`, because the kernel half is not proven and cannot be here.

Four had **no test and all four turned out to be writable** — three test files,
eight tests, and not one new dependency. Two of them were writable only because
code landed after the row was last read: `crates/store/src/file.rs` gave the
crate a bar file at all, and `crates/pull/src/ingest.rs` gave this repository
its first path from a vendor's folder to a stored bar:

- **S-02** — `crates/store/tests/roundtrip.rs`. Every index of a 160-record
  file, the decoded record and the raw bytes at its computed offset, across
  three appends and a reopen.
- **P-01** — `crates/pull/tests/concurrency.rs`. Eight threads, one fixed
  instant, exactly the allowance issued between them. `◐`: two separate
  governors are two budgets and nothing bounds their sum.
- **P-02**, **P-04** — `crates/pull/tests/integration.rs`. A vendor's folder
  through `pull::ingest::from_dir` to a file, then the file reopened and every
  record read back. P-04 is `◐` and the reason is a finding rather than a
  caveat: the re-run **writes** nothing and **reports** three.

Four could not be proven and were **not** faked. Each keeps its name, its `✗`
and its own account of what is missing:

- **P-03** — there is no trading calendar and `docs/00-charter.md` records no
  holiday list. A weekend rule would be wrong, which is a stronger reason than
  "not yet".
- **X-01** — no run-identity function exists in any crate; `blake3` is in the
  workspace table and no member takes it.
- **X-02** — the source check named is CI gate 11's job, not a test's. Writing
  a Rust test that greps the tree would assert a spelling.
- **X-13** — the bar reader arrived; the two-vendor comparison did not.

**CI gate 10 therefore still exits 1, on four lines, by choice.** The one lever
that would silence them is the `allow_pending` allowlist in
`.github/workflows/ci.yml`, which is deliberately empty and is not this change's
file. Using it would be a decision to stop reporting four known gaps, and that
is a `docs/05-decisions.md` entry somebody has to sign — which is the ledger
entry this pass owes, alongside the three new test files.

---

## Greeks — closed form, and the one solve that is not

`crates/greeks` is `f64` throughout and never sees a paisa. `CLAUDE.md` §7
reserves `i64` for prices and keeps statistical values at full precision; a
delta is the second kind. D-0046.

| # | Must hold | Proven by | |
|---|---|---|---|
| G-01 | Gamma and vega are **bit-identical** between a call and a put at the same strike — not close, identical, because both are computed above the branch | `greeks::bsm::gamma_and_vega_are_bit_identical_between_the_call_and_the_put` | ✓ |
| G-02 | `delta_call − delta_put == e^-qT` to a measured bound, and the bound is 2 ulps rather than a hoped-for zero | `greeks::bsm::the_delta_difference_is_the_carry_discount_to_a_measured_bound` | ✓ |
| G-03 | Put-call parity `C − P == S·e^-qT − K·e^-rT` holds across the grid to 2.1e-16 of spot | `greeks::bsm::put_call_parity_holds_across_the_grid` | ✓ |
| G-04 | Every one of the five greeks reproduces a central difference **wherever the stencil can resolve it**, and the number of comparisons actually made is asserted | `greeks::bsm::every_greek_reproduces_a_central_difference_wherever_one_can_be_taken` | ✓ |
| G-05 | The shipped normal CDF agrees with an independently implemented reference to 2.22e-16 absolute and 8.9e-9 relative — the check that caught a transposed digit in a Hart coefficient | `greeks::normal::hart_agrees_with_an_independent_reference` | ✓ |
| G-06 | The normal CDF matches published values at twelve points, on a **relative** tolerance so the tail is actually checked | `greeks::normal::the_cdf_matches_known_values` | ✓ |
| G-07 | Both Hart branches and both saturating tails are exercised, and the two branches meet at the split inside Hart's own tail accuracy | `greeks::normal::both_hart_branches_and_both_saturating_tails_are_exercised` | ✓ |
| G-08 | `volatility -> price -> IV` recovers the **volatility** to 1e-4 relative, and every point that does not is refused as one of exactly two named kinds whose counts account for the whole refusal count. Measured worst 2.33e-6; the superseded `price -> IV -> price` form is asserted too and is the weaker of the two — it measures `0e0` at the point where the volatility is 5.14% wrong. D-0046 | `greeks::solver::a_volatility_round_trips_through_the_solver_and_back` | ✓ |
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

Every row here is measured against one live Dhan option-chain response. D-0046.

| # | Must hold | Proven by | |
|---|---|---|---|
| G-21 | The shipped closed form reproduces all eight of Dhan's published numbers from a fitted state, so a sign error, a missing discount factor or a wrong unit anywhere in `greeks::bsm` fails here. **None of the eight is an independent prediction** — six are inputs to the fit and the two gammas are the transposition criterion restated, which G-35 pins. The claim that the gammas were a prediction is withdrawn by D-0046 | `greeks::vendor_anchor::our_greeks_reproduce_the_captured_dhan_chain` | ✓ |
| G-22 | Vega is published per one percentage point: the raw scaling implies an index level of 258, the per-percent scaling 25,851 | `greeks::vendor_anchor::vega_is_published_per_percentage_point_and_the_index_level_proves_it` | ✓ |
| G-23 | Theta is published per a **calendar** day and not a trading day: the two sides of one strike agree on `r` 23 times better under 365 than under 252. **365 itself is not selected** — 375 beats it, and G-34 locates what the criterion actually picks | `greeks::vendor_anchor::the_trading_day_divisor_is_excluded_and_the_calendar_one_is_not_selected` | ✓ |
| G-24 | The two published volatilities are transposed, by a scale-free identity that contains no spot, strike, maturity or rate | `greeks::vendor_anchor::the_two_published_volatilities_are_transposed_and_the_identity_says_so` | ✓ |
| G-25 | `delta_call − delta_put = 1.00603` is reproduced by `N(d1c) + N(−d1p)` at `q = 0`, and a single volatility would need `q = −42.54%`. This says **nothing** about the carry — it inverts the two deltas, so it cannot fail for any deltas, volatilities or carry — and D-0046 withdraws the claim that it did | `greeks::vendor_anchor::the_delta_difference_is_reproduced_by_the_two_volatilities_and_says_nothing_about_the_carry` | ✓ |
| G-26 | `T` is recovered in closed form from the deltas and the volatilities alone and lands inside its 95% rounding-box interval — **conditional on `q = 0` and on the transposition.** The interval is a rounding-box width, not an identification result | `greeks::vendor_anchor::the_maturity_is_recovered_in_closed_form_from_the_deltas_and_the_volatilities` | ✓ |
| G-27 | The fit does **not** close, and the residuals are pinned so no later change can claim it does — with the spot residual, which is invariant to the day divisor from 252 to 500, separated from the rate residual, which is an artifact of choosing 365 | `greeks::vendor_anchor::the_two_sides_disagree_on_the_rate_and_the_disagreement_is_reported_not_hidden` | ✓ |
| G-28 | `crates/greeks` declares no dependency and no dev-dependency, and names no type from this workspace, so it can be taken by git URL on its own | **CI gate 9b.** `greeks::standalone::the_whole_public_surface_is_reachable_with_nothing_else_in_scope` is the companion and proves only the narrower thing its name says: the listed public items are reachable with nothing else in scope. An integration test cannot see a leak it does not name, and a `pub use` adds no region for coverage to see. D-0046 | ✓ |
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

These were appended as **C-14 through C-17** and those four ids were already
taken, by the `crates/api` rendering rows above. **Renumbered to `C-K-01`
through `C-K-04` by D-0045** — the ids `crates/costs/benches/ratio.rs` has been
printing all along. Enforced by gate 8, which runs that bench, and held to the
same **3.0× shared-CI ceiling**.

| # | Must hold | Proven by | |
|---|---|---|---|
| C-K-01 | A regime lookup costs the same whichever row it selects — the anchor row and the last row of the same table | `costs::bench::the_selected_row_does_not_change_the_cost` | ✓ |
| C-K-02 | A regime lookup costs the same on a two-row table as on a three-row one, so the trip count is not being paid for at runtime | `costs::bench::the_row_count_does_not_change_the_cost` | ✓ |
| C-K-03 | Refusing costs what pricing costs — a refusal is not a slow path callers learn to avoid asking for | `costs::bench::a_refusal_costs_what_a_rate_costs` | ✓ |
| C-K-04 | The pre-boundary window is **refused**, not merely refused quickly — speed is half the claim and a lookup that got fast by returning the current rate would pass every ratio above | `costs::bench::the_pre_boundary_window_still_refuses` | ✓ |

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
shared-CI ceiling**. Renumbered from **C-18 … C-22** by D-0045, onto the ids the
bench prints.

| # | Must hold | Proven by | |
|---|---|---|---|
| C-K-05 | The at-the-money rung costs the same wherever the spot is — a 50-rupee spot and a spot near `i64::MAX` | `costs::bench::the_spot_magnitude_does_not_change_the_rung_cost` | ✓ |
| C-K-06 | The resolved strike costs the same however deep the moneyness — `ATM`, the chain edge, and a million steps out — and reading a moneyness off a far strike costs what reading it off a near one does | `costs::bench::the_moneyness_depth_does_not_change_the_strike_cost` | ✓ |
| C-K-07 | The quantity costs the same for one lot and for a million, so the multiplication has not become an accumulation | `costs::bench::the_lot_count_does_not_change_the_quantity_cost` | ✓ |
| C-K-08 | The expiry costs the same wherever in the calendar it is asked: the in-month arm against the rollover arm, January against a December that rolls the year, and zero days ahead against six | `costs::bench::the_calendar_position_does_not_change_the_expiry_cost` | ✓ |
| C-K-09 | The stage-2 pre-history windows are **refused**, not merely refused quickly — a lot-size lookup that got fast by handing back the first recorded lot for a 2019 trade would pass every ratio above | `costs::bench::the_stage_two_pre_history_windows_still_refuse` | ✓ |

Measured on the operator's machine, 2026-08-07, over four runs. Per-call cost
0.62–15.7 ns. Every ratio held under 3.0×; the largest was **2.300×**, the
monthly rollover arm against the in-month arm, and that figure is the bound
working as stated rather than a scan appearing: the rollover arm does the month
resolution and the table lookup **twice**, which is exactly the "at most two"
the claim is. Every other stage-2 ratio, across all four runs, stayed inside
0.81×–1.14×. No figure from a CI runner is claimed, because none was taken.

## Transaction costs — the round trip

`crates/costs` stage 3. Every row is proven by a test in `crates/costs`; the
worked-example rows are the predecessor repository's own enforced oracles from
`COSTS_VERIFIED` §5, reproduced to the paisa. See `docs/05-decisions.md` D-0044
and `docs/06-limits.md` §27.

| # | Must hold | Proven by | |
|---|---|---|---|
| K-38 | The four worked examples price to the **paisa** — one NIFTY lot, five NIFTY lots, one BANKNIFTY lot and the BSE rate set — with every rate and every lot size read out of this crate's own dated tables rather than restated in the test | `costs::trip::the_first_worked_example_prices_one_nifty_lot_to_the_paisa` · `costs::trip::the_second_worked_example_amortises_the_flat_brokerage_over_five_lots` · `costs::trip::the_third_worked_example_prices_one_banknifty_lot` · `costs::trip::the_fourth_worked_example_prices_the_bse_rate_set_through_the_pure_core` | ✓ |
| K-39 | The transaction tax reads the **sell** premium and nothing else: not the buy leg, not both legs, and never `strike × quantity` — with each wrong answer priced out beside the right one | `costs::trip::the_transaction_tax_reads_the_sell_premium_and_nothing_else` | ✓ |
| K-40 | Stamp duty is charged on the **buy** leg exactly once, and the sell-side rate is zero so the sell leg cannot be charged even deliberately | `costs::trip::the_stamp_duty_is_charged_on_the_buy_leg_and_exactly_once` | ✓ |
| K-41 | GST is 18% on the services base — brokerage, exchange charge, SEBI fee and IPFT, each already rounded, with the tax and the stamp duty excluded — rounded **once**; rounding two 9% halves separately overcharges by exactly ₹1, computed rather than asserted | `costs::trip::the_gst_is_rounded_once_and_rounding_each_half_overcharges_by_a_rupee` · `costs::trip::the_gst_base_is_the_sum_of_all_four_service_components_with_a_plus_sign` | ✓ |
| K-42 | Brokerage is per executed **order**, flat: a thousand lots pay what one lot pays, on both brokers, while every other charge scales | `costs::trip::the_brokerage_is_flat_per_order_and_a_thousand_lots_pay_what_one_pays` | ✓ |
| K-43 | A round trip whose entry day lands in an unverified window **refuses entirely** — no charge is priced at zero and no current rate is applied backwards — and the refusal carries the window, the citation gap and the remedy out whole | `costs::trip::a_round_trip_in_the_unverified_window_refuses_the_whole_trip_and_names_it` · `costs::trip::the_dated_pair_carries_whichever_of_the_two_lookups_refused` | ✓ |
| K-44 | Every regime is keyed on the **entry** day: a round trip that straddles a tax boundary is priced at the regime it opened under, and the lot size is the entry day's too | `costs::trip::the_regime_is_the_entry_days_and_the_exit_day_never_moves_it` · `costs::trip::the_lot_size_is_the_entry_days_and_a_pre_history_entry_refuses` | ✓ |
| K-45 | Each leg fills on the adverse extreme of its **own** bar plus one further tick, the two directions read different anchors off the same two bars, the sell floor binds at one tick and shortens the realized slippage with it, and the buy leg needs no floor because no legal bar can reach one | `costs::fill::a_long_fills_the_entry_high_and_the_exit_low_each_one_tick_adverse` · `costs::fill::a_short_fills_the_exit_high_and_the_entry_low_and_is_adverse_on_both_legs` · `costs::fill::the_sell_floor_binds_at_one_tick_and_the_realized_slippage_follows_it` · `costs::fill::the_buy_leg_needs_no_floor_because_no_legal_bar_can_reach_it` | ✓ |
| K-46 | The worst-case fill never flatters an open-anchored one, at either bracket end of any bar, on either direction | `costs::fill::the_worst_case_fill_never_flatters_an_open_anchored_one` | ✓ |
| K-47 | The two rounding laws are different functions: a levy ceils to the paisa per leg and is summed after, a statutory levy floors to the paisa and then ceils to the whole rupee — and each is the **least** integer at or above its quotient, checked densely and at every remainder that can flip it | `costs::money::the_statutory_raw_stage_floors_where_the_levy_stage_ceils` · `costs::money::the_ceiling_is_the_least_integer_at_or_above_the_quotient` · `costs::money::the_rupee_ceiling_is_the_least_whole_rupee_at_or_above_the_amount` | ✓ |
| K-48 | An index spot or index future is priced **signal-only** — every charge zero, net equal to gross, the fills unchanged — and it consults no rate, so it prices inside a window where every rate refuses | `costs::scope::only_the_option_segment_bears_the_charge_stack` · `costs::trip::a_signal_only_segment_pays_nothing_and_its_net_is_its_gross` · `costs::trip::a_signal_only_segment_prices_inside_a_window_where_every_rate_refuses` | ✓ |
| K-49 | An expiry outcome is **refused**, never priced with premium arithmetic; an exit before its entry, a lot count on a segment with no options lot table, a zero or negative quantity, an inverted bar and a sub-tick bar high are each refused **by name** | `costs::trip::an_expiry_outcome_is_refused_rather_than_priced_as_a_normal_close` · `costs::trip::an_exit_day_before_its_entry_day_is_refused` · `costs::trip::a_lot_count_is_refused_for_a_segment_the_options_lot_table_does_not_cover` · `costs::trip::a_round_trip_of_no_contracts_is_refused_by_name_on_every_path` · `costs::fill::a_bar_whose_low_is_above_its_high_is_refused_rather_than_swapped` · `costs::fill::a_bar_whose_high_is_below_one_tick_is_refused_by_name` | ✓ |
| K-50 | Every arithmetic site that can leave `i64` is refused **by name** and never wrapped or saturated — both notionals, the slippage line, the net, each per-leg levy, each two-leg sum, the GST base, the total, and each statutory stage | `costs::trip::every_position_overflow_site_is_refused_by_name_and_never_wrapped` · `costs::trip::every_levy_overflow_site_is_refused_by_name_and_never_wrapped` · `costs::trip::every_flat_levy_overflow_site_is_refused_by_name_and_never_wrapped` · `costs::trip::the_sell_leg_of_a_per_leg_levy_is_guarded_independently_of_the_buy_leg` · `costs::trip::the_statutory_rupee_ceiling_is_reachable_from_the_stack_and_refuses` · `costs::trip::a_signal_only_round_trip_guards_its_arithmetic_too` · `costs::fill::a_fill_or_a_slippage_past_i64_is_refused_by_name_and_never_wrapped` · `costs::money::a_result_past_i64_is_refused_by_name_and_never_saturated` | ✓ |
| K-51 | A breakdown's own two laws hold on every swept combination — the total is the sum of its seven itemised charges and the net is the gross less the total — and a breakdown that broke either is **refused rather than reported** | `costs::trip::the_internal_laws_hold_across_a_deterministic_sweep_of_the_envelope` · `costs::trip::a_breakdown_that_broke_its_own_law_is_refused_rather_than_reported` · `costs::trip::the_breakdown_itemises_seven_charges_that_sum_to_its_total` | ✓ |
| K-52 | A losing round trip still pays every charge, and a round trip whose charges exceed its gross reports a negative net — both computed, not asserted | `costs::trip::a_losing_round_trip_reports_a_negative_net_and_still_pays_every_charge` · `costs::trip::a_round_trip_whose_charges_exceed_its_gross_reports_a_negative_net` | ✓ |
| K-53 | The entry point is the pure core plus the resolution and nothing else, on every swept combination of underlying, date, direction and segment | `costs::trip::the_entry_point_and_the_pure_core_agree_on_every_swept_combination` | ✓ |

### Complexity rows for the round trip

Enforced by gate 8, `crates/costs/benches/ratio.rs`, against the same **3.0×
shared-CI ceiling**.

Renumbered from **C-23 … C-25** by D-0045, onto the ids the bench prints.

| # | Must hold | Proven by | |
|---|---|---|---|
| C-K-10 | The charge stack costs the same however big the trade is: one lot against a million, a five-paisa premium against a lakh-rupee one, and a winning trip against a losing one | `costs::bench::the_trade_size_does_not_change_the_charge_stack_cost` | ✓ |
| C-K-11 | The whole entry point costs the same whatever it is asked: the trade's size, its date and its underlying do not move it, and a signal-only trip never costs **more** than a cost-bearing one | `costs::bench::the_entry_point_costs_the_same_whatever_it_is_asked` | ✓ |
| C-K-12 | The unverified window **refuses the whole round trip** — not merely refuses it quickly — while the day after prices and a signal-only trip in the same window prices at zero | `costs::bench::the_stage_three_refusals_are_still_refusals` | ✓ |

Measured on the operator's machine, 2026-08-07, over four runs. Per-call cost
32.9–37.6 ns for `charge_stack` and 37.3–41.2 ns for `price`. Every ratio held:
C-23 spanned 0.895×–1.030× and C-24's three size/date/underlying ratios spanned
0.954×–1.039×. The two figures far **below** 1.0× are the two arms that do less
work by design and are reported rather than hidden: a signal-only trip resolves
no rate (0.128×–0.137×) and a refused trip stops at the first refusal
(0.164×–0.179×). No figure from a CI runner is claimed, because none was taken.
