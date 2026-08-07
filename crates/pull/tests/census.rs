//! The counter and the bars describe the same slice, or neither does:
//! `pull::census::*`.
//!
//! # Why this file exists
//!
//! An ingest worked — 194 GDFL contracts, 354,675 rows, 62,978 bars, 194 bar
//! files on disk — and `/store` showed an em-dash in every cell, because the
//! page reads the manifest and the ingest never wrote one. A walk-free page is
//! blind to bars nobody counted, which is the price of the counter and the
//! reason it has to be maintained on the write path rather than reconstructed.
//!
//! `crates/pull/tests/integration.rs` proves what reaches the **bar files**.
//! This file proves what reaches the **census**, and that the two agree — the
//! entry's row count and both timestamps are checked against the bar file's own
//! committed header, not against the number the run reported.
//!
//! # Every segment in this file is invented
//!
//! `CLAUDE.md` §8 and CI gates 1c and 1d: no literal parameter path appears in
//! any tracked file, and a test is a tracked file. `NSE`, `INDEX`, `FUT` and
//! `NIFTY` name an exchange, an exchange segment and an index that this
//! repository has tracked since its first commit; none of them is a credential
//! path segment.
//!
//! # The four states a run can leave the census in
//!
//! | State | Proved by |
//! |---|---|
//! | published, and equal to the files | `the_census_counts_exactly_what_the_store_holds` |
//! | untouched, because nothing changed | `a_re_run_leaves_the_census_byte_for_byte` |
//! | never opened, so nothing was written | `a_census_that_will_not_open_stops_the_run_before_it_writes` |
//! | bars on disk that it does not count | `a_month_the_census_refuses_is_named_not_swallowed`, `a_census_that_cannot_be_installed_names_what_is_left_uncounted` |
//!
//! The last row is the one that matters most and it is the one with two tests:
//! a member whose bars landed and whose counter did not is worse than one that
//! failed outright, because the bars are real, the census denies them, and the
//! next run refetches a month the store already holds.

// A test that asserts nothing is banned, and a test that cannot fail loudly is
// a test that asserts nothing.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use brutex_core::instrument::{Exchange, Segment};
use brutex_core::symbol::Symbol;
use brutex_core::vendor::Vendor;
use store::file::BarFile;
use store::path::{FileKind, PathParts, STORE_ROOT, StorePath, Timeframe, YearMonth};

use pull::csv::Columns;
use pull::fetch::BarRequest;
use pull::ingest::{self, Ingested, Plan};
use pull::manifest::{
    ENTRY_STRIDE, Entry, EntryKey, HEADER_LEN, MAX_ENTRIES, Manifest, manifest_path,
};
use pull::session::{Cadence, Day, Window};
use pull::vendor::{PriceScale, TimestampEncoding};

// ===========================================================================
// Scratch
// ===========================================================================

/// Distinguishes two scratch trees taken in the same process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary directory tree that removes itself.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut root = std::env::temp_dir();
        root.push(format!(
            "brutex-pull-census-{}-{tag}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("a scratch root");
        Self { root }
    }

    /// Where the bars and the census go.
    fn store(&self) -> PathBuf {
        let dir = self.root.join("STORE");
        fs::create_dir_all(&dir).expect("a scratch store");
        dir
    }

    /// A folder holding one member per `(name, body)` pair.
    fn archive(&self, members: &[(&str, &str)]) -> PathBuf {
        let dir = self.root.join("ARCHIVE");
        fs::create_dir_all(&dir).expect("a scratch archive");
        for (name, body) in members {
            fs::write(dir.join(format!("{name}.csv")), body).expect("a member");
        }
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leaked scratch directory must never fail a test run.
        drop(fs::remove_dir_all(&self.root));
    }
}

// ===========================================================================
// The fixture
// ===========================================================================

/// One `TrueData` index member: `date,time,price,volume,open_interest`.
///
/// The same eight rows `integration.rs` drives, and they are the same eight for
/// a reason: four are declined, three become bars, and **one folds into a bar
/// that is already open**. That last row is the whole of `rows_folded`.
const BODY: &str = "\
20221002,10:00:00,38400.00,0,0
20221003,09:14:59,38410.00,0,0
20221003,09:15:00,38445.65,0,0
20221003,09:15:30,38450.00,0,0
20221003,15:29:59,38500.00,0,0
20221003,15:30:00,38510.00,0,0
20221004,09:20:00,38600.00,0,0
20221005,10:00:00,38700.00,0,0
";

/// Rows in [`BODY`].
const ROWS: usize = 8;
/// How many become bars.
const BARS: usize = 3;
/// How many fold into a bar that is already open — 09:15:30 into 09:15:00.
const FOLDED: usize = 1;
/// How many are declined by the window or the session.
const DROPPED: usize = 4;

/// The vendor whose census these tests write.
const VENDOR: Vendor = Vendor::Groww;

/// The month [`BODY`] lands in.
fn month() -> YearMonth {
    YearMonth::new(2022, 10).expect("October 2022")
}

/// The two days the operator asks for.
fn window() -> Window {
    Window::new(
        Day::new(2022, 10, 3).expect("2022-10-03"),
        Day::new(2022, 10, 4).expect("2022-10-04"),
    )
    .expect("a forward window")
}

/// One request over [`window`].
fn request() -> BarRequest {
    BarRequest {
        window: window(),
        cadence: Cadence::Minute,
    }
}

/// The plan every run below uses, with the one field a caller varies.
fn plan_over<'a>(request: &'a BarRequest, segment: &'a str) -> Plan<'a> {
    Plan {
        columns: Columns::TrueDataIndex,
        request,
        encoding: TimestampEncoding::EpochSecondsUtc,
        scale: PriceScale::Paisa,
        timeframe: Timeframe::MINUTE_1,
        vendor: VENDOR,
        exchange: "NSE",
        segment,
    }
}

/// Runs one ingest.
fn run(archive: &Path, store_root: &Path, request: &BarRequest) -> Ingested {
    ingest::from_dir(archive, store_root, plan_over(request, "INDEX"))
        .expect("the folder is readable and the column shape is right")
}

/// The census key one instrument's month is filed under.
fn key(instrument: &str) -> EntryKey {
    EntryKey {
        exchange: Exchange::Nse,
        segment: Segment::Index,
        symbol: Symbol::new(instrument).expect("a legal symbol"),
        timeframe: Timeframe::MINUTE_1,
        month: month(),
    }
}

/// The census on disk, read the way `api::census` reads it.
fn census_of(store_root: &Path) -> Manifest {
    let bytes = fs::read(manifest_path(store_root, VENDOR)).expect("a census on disk");
    Manifest::open_image(VENDOR, &bytes).expect("and this build reads it")
}

/// The bar file one instrument's month is in.
fn bar_file(store_root: &Path, instrument: &str) -> BarFile {
    let path = StorePath::new(PathParts {
        vendor: VENDOR,
        exchange: "NSE",
        segment: "INDEX",
        symbol: instrument,
        timeframe: Timeframe::MINUTE_1,
        month: month(),
        file: FileKind::Bars,
    })
    .expect("a legal path");
    let symbol_id = u32::try_from(brutex_core::universe::fnv1a(instrument) & 0xFFFF_FFFF)
        .expect("the low half of a 64-bit hash");
    BarFile::open_or_create(store_root, path, symbol_id).expect("the month reopens")
}

// ===========================================================================
// The census counts what the store holds
// ===========================================================================

/// Every counter row is checked against the bar file it claims to describe.
#[test]
fn the_census_counts_exactly_what_the_store_holds() {
    let scratch = Scratch::new("COUNTS");
    let archive = scratch.archive(&[("NIFTY", BODY), ("BANKNIFTY", BODY)]);
    let store = scratch.store();

    let done = run(&archive, &store, &request());
    assert_eq!(done.failures, Vec::new(), "no member failed");
    assert_eq!(done.members, 2);
    assert_eq!(done.bars_stored, 2 * BARS);
    assert_eq!(
        done.counted, 2,
        "both slices are in the census, and a member that stored bars is \
         either counted or named as a failure"
    );

    let path = manifest_path(&store, VENDOR);
    assert!(
        path.exists(),
        "the run published a census at {}",
        path.display()
    );
    assert_eq!(
        fs::metadata(&path).expect("the census").len(),
        HEADER_LEN + 2 * ENTRY_STRIDE,
        "one header region and exactly two 64-byte entries"
    );

    let census = census_of(&store);
    assert_eq!(census.entries(), 2, "one entry per member that stored bars");
    assert_eq!(census.keys(), 2, "two distinct months of two instruments");
    assert_eq!(
        census.total_rows(),
        done.bars_stored as u64,
        "the counter's row total is the bars the run put on disk"
    );
    assert_eq!(census.degraded_reason(), None, "and it loads clean");
    assert_eq!(census.header().vendor, VENDOR);

    // THE INVARIANT, READ OFF THE DISK. Not "the census says three" — the bar
    // file's own committed header says three, and the census agrees with it.
    for instrument in ["NIFTY", "BANKNIFTY"] {
        let entry = census
            .entry(&key(instrument))
            .unwrap_or_else(|| panic!("{instrument} is not in the census"));
        let file = bar_file(&store, instrument);
        let header = file.header();
        assert_eq!(
            entry.rows, header.n_valid,
            "{instrument}: the census counts {} rows and the file holds {}",
            entry.rows, header.n_valid
        );
        assert_eq!(entry.rows, BARS as u64);
        assert_eq!(
            entry.first_ts_micros,
            file.read_record(0).expect("record 0").ts_micros,
            "{instrument}: the entry's first timestamp is record 0's"
        );
        assert_eq!(
            entry.last_ts_micros,
            file.read_record(header.n_valid - 1)
                .expect("the last record")
                .ts_micros,
            "{instrument}: the entry's last timestamp is the last record's"
        );
        assert_eq!(entry.key.month, month());
        assert_eq!(entry.key.timeframe, Timeframe::MINUTE_1);
    }
}

/// A folded row is consumed, not lost, and the books balance because of it.
#[test]
fn a_folded_row_is_counted_as_consumed_and_the_books_balance() {
    let scratch = Scratch::new("FOLDED");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();

    let done = run(&archive, &store, &request());
    assert_eq!(done.rows_read, ROWS);
    assert_eq!(done.bars_stored, BARS);
    assert_eq!(
        done.rows_folded, FOLDED,
        "09:15:30 merged into the bar 09:15:00 had already opened — its \
         volume, high, low and close are IN that bar, so it is neither a bar \
         nor a drop"
    );
    assert_eq!(done.census.total() as usize, DROPPED);
    assert_eq!(
        ROWS,
        BARS + FOLDED + DROPPED,
        "the four categories are the whole of what was read"
    );
    assert!(
        done.balances(),
        "eight read = three stored + one folded + four dropped; before the \
         fold had a counter this run answered NO with nothing wrong"
    );

    // The folded row is not a claim about arithmetic: it is in the bar.
    let file = bar_file(&store, "NIFTY");
    let first = file.read_record(0).expect("record 0");
    assert_eq!(first.open, 3_844_565, "38445.65, the first snapshot");
    assert_eq!(first.close, 3_845_000, "38450.00, the folded one");
}

// ===========================================================================
// Idempotence — CLAUDE.md §3 rule 5, and the counter is part of it
// ===========================================================================

/// A second identical run leaves the census file byte for byte as it was.
#[test]
fn a_re_run_leaves_the_census_byte_for_byte() {
    let scratch = Scratch::new("RERUN");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);

    let first = run(&archive, &store, &request());
    let before = fs::read(&path).expect("the census");

    let second = run(&archive, &store, &request());
    let after = fs::read(&path).expect("the census");

    assert_eq!(
        before, after,
        "a re-run appended a second entry saying what the first entry already \
         said, and two identical runs left two different files"
    );
    assert_eq!(first, second, "and the two runs report the same thing");
    assert_eq!(second.failures, Vec::new());
    assert_eq!(
        second.counted, 1,
        "the slice is still counted on the second run — it was already \
         recorded, which is not the same as not being counted"
    );

    let census = census_of(&store);
    assert_eq!(census.entries(), 1, "one entry, not two");
    assert_eq!(census.header().generation, 1, "and one commit, not two");
    assert_eq!(census.total_rows(), BARS as u64);
}

/// A run that stores nothing writes no census at all.
#[test]
fn a_run_that_stores_nothing_publishes_nothing() {
    let scratch = Scratch::new("NOTHING");
    // Every row is a week past the window.
    let outside = "20221010,09:15:01,38445.65,0,0\n20221010,09:15:02,38446.00,0,0\n";
    let archive = scratch.archive(&[("NIFTY", outside)]);
    let store = scratch.store();

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.counted, 0, "there is no slice to count");
    assert_eq!(
        done.failures,
        Vec::new(),
        "and nothing failed — it declined"
    );
    assert!(
        !manifest_path(&store, VENDOR).exists(),
        "an empty census is not the same fact as no census, and writing one \
         would turn 'nothing has been ingested' into 'the counter says zero'"
    );
}

/// A second window over the same month records the whole file, not the batch.
#[test]
fn a_second_window_records_the_whole_month_not_the_suffix() {
    let scratch = Scratch::new("APPEND");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();

    let narrow = BarRequest {
        window: Window::new(
            Day::new(2022, 10, 3).expect("2022-10-03"),
            Day::new(2022, 10, 3).expect("2022-10-03"),
        )
        .expect("a one-day window"),
        cadence: Cadence::Minute,
    };
    let first = run(&archive, &store, &narrow);
    assert_eq!(first.bars_stored, 2);
    assert_eq!(
        census_of(&store)
            .entry(&key("NIFTY"))
            .expect("the month is in the census")
            .rows,
        2
    );

    let wider = BarRequest {
        window: Window::new(
            Day::new(2022, 10, 4).expect("2022-10-04"),
            Day::new(2022, 10, 4).expect("2022-10-04"),
        )
        .expect("a one-day window"),
        cadence: Cadence::Minute,
    };
    let second = run(&archive, &store, &wider);
    assert_eq!(second.bars_stored, 1, "one bar was appended");

    let census = census_of(&store);
    let entry = census.entry(&key("NIFTY")).expect("still recorded");
    let file = bar_file(&store, "NIFTY");
    assert_eq!(
        entry.rows, 3,
        "the entry counts the FILE, not the one-bar batch that was offered"
    );
    assert_eq!(entry.rows, file.header().n_valid);
    assert_eq!(
        entry.first_ts_micros,
        file.read_record(0).expect("record 0").ts_micros
    );
    assert_eq!(
        entry.last_ts_micros,
        file.read_record(2).expect("record 2").ts_micros
    );
    assert_eq!(census.entries(), 2, "two commits, because the month grew");
    assert_eq!(census.keys(), 1, "of one key");
    assert_eq!(census.total_rows(), 3);
}

// ===========================================================================
// A slice the census cannot key is a slice that is not stored
// ===========================================================================

/// A segment the census cannot name stores no bars either.
#[test]
fn a_segment_the_census_cannot_key_stores_no_bars() {
    let scratch = Scratch::new("SEGMENT");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();

    // `FUT` passes `StorePath`, which accepts any upper-case segment, and is
    // not one of INDEX, CASH or FNO. Bars under it would be bars the census
    // could never name and `/store` could never report.
    let request = request();
    let done = ingest::from_dir(&archive, &store, plan_over(&request, "FUT"))
        .expect("the folder itself is fine");

    assert_eq!(done.bars_stored, 0, "and therefore nothing was stored");
    assert_eq!(done.counted, 0);
    assert_eq!(done.failures.len(), 1);
    assert_eq!(done.failures[0].instrument, "NIFTY", "the member is named");
    assert!(
        done.failures[0].why.contains("FUT"),
        "and so is the segment — {}",
        done.failures[0].why
    );
    assert!(
        !store.join(STORE_ROOT).exists(),
        "not one byte of bars reached the disk under a segment nothing counts"
    );
    assert!(!manifest_path(&store, VENDOR).exists());
}

// ===========================================================================
// A census that will not open stops the run before it writes
// ===========================================================================

/// A census file this build refuses stops the run before a bar is written.
#[test]
fn a_census_that_will_not_open_stops_the_run_before_it_writes() {
    let scratch = Scratch::new("GARBAGE");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);
    fs::create_dir_all(path.parent().expect("a parent")).expect("the census directory");
    // Long enough to be a header region, and not a census.
    fs::write(&path, vec![0xAB; 40_000]).expect("garbage at the census path");

    let done = run(&archive, &store, &request());
    assert_eq!(done.members, 1);
    assert_eq!(
        done.rows_read, ROWS,
        "the folder was read, so the report says how much was in it rather \
         than blaming the vendor for our refusal"
    );
    assert_eq!(done.bars_stored, 0, "and not one bar was written");
    assert_eq!(done.failures.len(), 1);
    assert_eq!(
        done.failures[0].instrument,
        path.display().to_string(),
        "the failure names the census, because that is what has to be looked at"
    );
    assert!(
        !done.failures[0].why.is_empty(),
        "in the manifest's own words"
    );
    assert!(!done.balances(), "and a run that refused does not balance");
    assert!(
        !store.join(STORE_ROOT).exists(),
        "a bar written now would be a bar the census denies, which is worse \
         than a run that did nothing"
    );
}

/// A census path that cannot be read at all is not a census that is absent.
#[test]
fn a_census_that_cannot_be_read_is_not_treated_as_a_first_ingest() {
    let scratch = Scratch::new("UNREADABLE");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);
    // A directory where the census file goes: it has metadata, and reading it
    // is refused by the host.
    fs::create_dir_all(&path).expect("a directory at the census path");

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    assert!(
        done.failures[0].why.contains("could not be read"),
        "the host's refusal is carried — {}",
        done.failures[0].why
    );
}

/// A census path that cannot even be measured stops the run and says so.
#[test]
fn a_census_that_cannot_be_measured_stops_the_run() {
    let scratch = Scratch::new("TOOLONG");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    // One path component past every filesystem's name limit. Neither
    // "no file" nor "no directory": the host cannot answer the question.
    let store = scratch.root.join("A".repeat(500));

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    assert!(
        done.failures[0].why.contains("could not be measured"),
        "and it is refused as unmeasurable rather than assumed absent — {}",
        done.failures[0].why
    );
    assert!(
        done.failures[0]
            .why
            .contains("will not start a second census"),
        "which is the D-0036 defect this refusal exists to prevent — {}",
        done.failures[0].why
    );
}

/// A census larger than this build could have written is refused by name.
#[test]
fn a_census_larger_than_this_build_can_write_is_refused_by_name() {
    let scratch = Scratch::new("HUGE");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);
    fs::create_dir_all(path.parent().expect("a parent")).expect("the census directory");
    // Sparse: the bound is checked before the read, so nothing this size is
    // ever allocated — which is the whole point of checking it first.
    let ceiling = HEADER_LEN + MAX_ENTRIES * ENTRY_STRIDE;
    fs::File::create(&path)
        .expect("a file")
        .set_len(ceiling + 1)
        .expect("one byte past what this build can write");

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    let why = &done.failures[0].why;
    assert!(
        why.contains(&(ceiling + 1).to_string()) && why.contains(&ceiling.to_string()),
        "the refusal names what was found and what the ceiling is, so an \
         operator sees which to change — {why}"
    );
}

// ===========================================================================
// Bars on disk that the census does not count — named, never swallowed
// ===========================================================================

/// A month the census refuses leaves the bars named, not silently uncounted.
#[test]
fn a_month_the_census_refuses_is_named_not_swallowed() {
    let scratch = Scratch::new("BACKWARDS");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);

    // A census that already claims far more rows for this month than the run
    // will store. `Manifest::record` refuses a row count that goes backwards,
    // which is the check that keeps the counter monotonic per key.
    let mut seeded = Manifest::open(VENDOR, &[], &[]).expect("a genesis census");
    seeded
        .record(Entry {
            key: key("NIFTY"),
            rows: 9_999,
            first_ts_micros: 1_664_000_000_000_000,
            last_ts_micros: 1_664_900_000_000_000,
        })
        .expect("a first entry");
    fs::create_dir_all(path.parent().expect("a parent")).expect("the census directory");
    let before = seeded.image();
    fs::write(&path, &before).expect("the seeded census");

    let done = run(&archive, &store, &request());
    assert_eq!(
        done.bars_stored, BARS,
        "the bars did land — this is the case that matters"
    );
    assert_eq!(done.counted, 0, "and not one of them is counted");
    assert_eq!(done.failures.len(), 1);
    assert_eq!(done.failures[0].instrument, "NIFTY");
    let why = &done.failures[0].why;
    assert!(
        why.contains("does not count"),
        "the run says plainly that bars are on disk uncounted — {why}"
    );
    assert!(
        why.contains("NIFTY"),
        "and names the member, so it is actionable — {why}"
    );
    assert!(!done.balances());
    assert_eq!(
        bar_file(&store, "NIFTY").header().n_valid,
        BARS as u64,
        "the bars are really there, which is why this is worse than a refusal"
    );
    assert_eq!(
        fs::read(&path).expect("the census"),
        before,
        "and the census was not rewritten: nothing was recorded, so there was \
         nothing to publish"
    );
}

/// A census that cannot be installed names how many slices are left uncounted.
#[test]
fn a_census_that_cannot_be_installed_names_what_is_left_uncounted() {
    let scratch = Scratch::new("NOINSTALL");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    // A file where the census directory goes. Reading through it is
    // `NotADirectory`, which is one of the two shapes of "there is nothing
    // there" — so the run proceeds on a genesis census and fails at the
    // install, which is exactly the sequence this test is about.
    fs::write(store.join("manifest"), "NOT A DIRECTORY").expect("a file in the way");

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, BARS, "the bars landed");
    assert_eq!(done.counted, 1, "and the census counted them, in memory");
    assert_eq!(done.failures.len(), 1);
    let why = &done.failures[0].why;
    assert!(
        why.contains("1 slice(s) are on disk"),
        "the failure says how many slices the unpublished census held — {why}"
    );
    assert!(
        why.contains("could not be published"),
        "and that publishing is what failed — {why}"
    );
    assert!(!done.balances());
}

/// A temporary that cannot be created is a failed install, not a torn one.
#[test]
fn a_census_whose_temporary_cannot_be_written_publishes_nothing() {
    let scratch = Scratch::new("NOTEMP");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);
    fs::create_dir_all(path.parent().expect("a parent")).expect("the census directory");
    // A directory occupying the temporary's name. The image is written to a
    // temporary and renamed precisely so a half-written census never appears
    // at the live path; this proves the live path stays untouched when the
    // temporary itself is refused.
    fs::create_dir_all(path.with_extension("man.writing")).expect("a directory in the way");

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, BARS);
    assert_eq!(done.failures.len(), 1);
    assert!(
        done.failures[0].why.contains("could not be published"),
        "{}",
        done.failures[0].why
    );
    assert!(
        !path.exists(),
        "nothing was installed at the live path, whole or partial"
    );
}

// ===========================================================================
// A degraded census is repaired loudly, and what it lost is stated
// ===========================================================================

/// A census that loaded degraded is named, and the run installs the repair.
#[test]
fn a_degraded_census_is_named_and_the_run_installs_the_repair() {
    let scratch = Scratch::new("DEGRADED");
    let archive = scratch.archive(&[("NIFTY", BODY)]);
    let store = scratch.store();
    let path = manifest_path(&store, VENDOR);

    // Two generations, both valid on disk: generation 1 in slot 1, generation
    // 2 in slot 0. `Manifest::image` writes only the slot its own generation
    // belongs in, so the older slot is copied across from the older image.
    let mut older = Manifest::open(VENDOR, &[], &[]).expect("a genesis census");
    older
        .record(Entry {
            key: key("AAA"),
            rows: 10,
            first_ts_micros: 1_664_000_000_000_000,
            last_ts_micros: 1_664_000_600_000_000,
        })
        .expect("the first month");
    let mut newer = older.clone();
    newer
        .record(Entry {
            key: key("BBB"),
            rows: 20,
            first_ts_micros: 1_664_100_000_000_000,
            last_ts_micros: 1_664_100_600_000_000,
        })
        .expect("the second month");
    assert_eq!(older.header().generation, 1, "slot 1");
    assert_eq!(newer.header().generation, 2, "slot 0");

    let mut damaged = newer.image();
    let older_image = older.image();
    // Slot 1 is the second 16,384-byte slot of the header region.
    damaged[16_384..16_448].copy_from_slice(&older_image[16_384..16_448]);
    // And the newest slot is torn: one byte of generation 2's header.
    damaged[40] ^= 0xFF;

    fs::create_dir_all(path.parent().expect("a parent")).expect("the census directory");
    fs::write(&path, &damaged).expect("a damaged census");

    let done = run(&archive, &store, &request());
    assert_eq!(done.bars_stored, BARS, "the ingest still ran");
    assert_eq!(done.counted, 1);
    assert_eq!(
        done.failures.len(),
        1,
        "and it is loud: a census that fell back is not a census that loaded"
    );
    let why = &done.failures[0].why;
    assert!(why.contains("DEGRADED"), "{why}");
    assert!(
        why.contains("bars/"),
        "and it says where the months it lost can be found, because they \
         cannot be got back from the census — {why}"
    );
    assert!(!done.balances());

    // The repair is installed: what loads now is clean, holds the recovered
    // generation's month and the one this run recorded — and does NOT hold
    // the month the torn generation had committed. That is the honest half.
    let census = census_of(&store);
    assert_eq!(census.degraded_reason(), None, "the damage is gone");
    assert_eq!(census.keys(), 2);
    assert!(census.entry(&key("AAA")).is_some(), "the recovered month");
    assert!(census.entry(&key("NIFTY")).is_some(), "and this run's");
    assert_eq!(
        census.entry(&key("BBB")),
        None,
        "the month the torn generation had committed is gone, exactly as the \
         failure said it would be"
    );
    assert_eq!(census.total_rows(), 10 + BARS as u64);
}

// ===========================================================================
// The lost update
// ===========================================================================

/// **A RUN THAT CANNOT PUBLISH ITS COUNT WRITES NO BARS AT ALL.**
///
/// # The incident
///
/// Two POSTs fired at the same instant over two folders sharing no files: both
/// receipts said STORED, both said "every row accounted for", forty bar files
/// landed, and the census held twenty entries — run B only. 6,433 bars, 48.3%
/// of everything written and `fsync`ed, invisible to the counter.
///
/// A lock was added and it covered the wrong half. The census cycle is
/// read-whole-file, mutate in memory, write-whole-file; locking only the
/// *install* serialises the two renames and leaves the two reads racing. A
/// reads 20, B reads the same 20, A installs 20+A, B installs 20+B on top.
/// Neither ever finds the lock contended, because neither holds it while the
/// other is reading — so the receipts stay perfect and the entries still go.
///
/// # Why this test holds the lock instead of racing two threads
///
/// The obvious test — spawn two ingests behind a barrier and assert nothing is
/// lost — was written first and **thrown away, because it passed against the
/// broken placement three runs out of three.** Two real ingests do not reliably
/// interleave: one finishes its install before the other reaches its read, and
/// then there is no lost update to find. A test that cannot fail against the
/// defect it names is a test that asserts nothing, which `CLAUDE.md` §4 bans.
///
/// So this holds the lock outright — exactly what a concurrent run holds — and
/// asserts the consequence that separates the two placements with no timing in
/// it at all:
///
/// * **Lock inside `install`** (the old shape): the read needs no lock, so the
///   run reads the census, writes every bar file, `fsync`s them, and only then
///   discovers at install time that it cannot publish. Bars on disk, count not
///   updated — the precise state the incident produced.
/// * **Lock across the whole cycle** (the fix): the run refuses before it reads
///   anything, so not one byte of bar data is written.
///
/// `bars_stored == 0` and an absent `bars/` tree are therefore true under the
/// fix and false under the defect, deterministically, on every run.
#[test]
fn a_run_that_cannot_take_the_census_lock_writes_no_bars_at_all() {
    let scratch = Scratch::new("lock-held");
    let store = scratch.store();

    // Hold the census lock the way a concurrent run holds it.
    let census_path = manifest_path(&store, VENDOR);
    fs::create_dir_all(census_path.parent().expect("the census has a parent"))
        .expect("the manifest directory");
    let lock_path = census_path.with_extension("man.lock");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .expect("the lock file");
    held.try_lock().expect("this test is the first holder");

    let archive = scratch.archive(&[("AAAA", BODY), ("BBBB", BODY), ("CCCC", BODY)]);
    let done = ingest::from_dir(&archive, &store, plan_over(&request(), "INDEX"))
        .expect("the folder itself is readable — the census is what is contended");

    // It refused, and it said which path is contended.
    let census_name = census_path.display().to_string();
    let refusal = done
        .failures
        .iter()
        .find(|failure| failure.instrument == census_name)
        .unwrap_or_else(|| {
            panic!(
                "a run that cannot take the census lock must refuse and name it. \
                 failures: {:?}",
                done.failures
            )
        });
    assert!(
        refusal.why.contains("another pull holds the census lock"),
        "the refusal must say what is wrong in words an operator can act on: {}",
        refusal.why
    );

    // And it refused BEFORE writing. This is the half the old placement failed.
    assert_eq!(
        done.bars_stored, 0,
        "the run could never have published its count, so it must not have \
         written bars nothing would count. {} bars reached the disk uncounted, \
         which is the incident this lock exists to prevent.",
        done.bars_stored
    );
    assert!(
        !store.join("bars").exists(),
        "a run that refused at the census still created the bars tree at {}",
        store.join("bars").display()
    );
    assert!(
        done.counted == 0 && done.rows_read > 0,
        "the folder WAS read — {} rows — and nothing was counted, which is what \
         an honest refusal looks like",
        done.rows_read
    );
}
