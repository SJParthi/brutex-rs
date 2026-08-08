//! A vendor's folder in, bars on disk out: `pull::integration::*`.
//!
//! # Every segment in this file is invented
//!
//! `CLAUDE.md` §8 and CI gates 1c and 1d: no literal parameter path appears in
//! any tracked file, and a test is a tracked file. The path segments below name
//! an exchange, a segment and an index that this repository has tracked since
//! its first commit; none of them is a credential path segment.
//!
//! # What this file is for
//!
//! `docs/04-invariants.md` P-02 and P-04 both talk about what is **stored**,
//! and both named tests that exist in no file. Every other test of this crate
//! stops one step short of a file: `pull::unit::*` proves the window
//! arithmetic, the decoder, the fold and the census, and none of them writes a
//! bar. Until `crates/pull/src/ingest.rs` existed there was nothing to write
//! one — that module is the join, and it is the only thing in this repository
//! that takes a vendor's rows all the way to `store::file::BarFile::append`.
//!
//! So these tests drive [`pull::ingest::from_dir`] against a real folder of
//! real CSV bytes and then **reopen the store and read every record back**.
//! Asserting on the returned counters alone would prove what the pipeline
//! *said* it did; the invariant is about what is on the disk.
//!
//! # The fixture, and why each row is there
//!
//! One `TrueData` index file — five fields, no header, `YYYYMMDD`, one snapshot
//! per row — over a two-day window, holding one row for each of the four ways a
//! row can be refused and four that survive:
//!
//! | Row | Verdict |
//! |---|---|
//! | 2022-10-02 10:00:00 | dropped, before the window |
//! | 2022-10-03 09:14:59 | dropped, before the session open |
//! | 2022-10-03 09:15:00 | kept |
//! | 2022-10-03 09:15:30 | kept — folds into the 09:15 bar |
//! | 2022-10-03 15:29:59 | kept — the last minute the exchange trades |
//! | 2022-10-03 15:30:00 | dropped, at the session close |
//! | 2022-10-04 09:20:00 | kept |
//! | 2022-10-05 10:00:00 | dropped, after the window |
//!
//! The two rows either side of a boundary are the point: 15:29:59 is inside and
//! 15:30:00 is outside, so an off-by-one in either direction fails rather than
//! passing on a fixture that never approaches the edge.
//!
//! # What these tests do not prove
//!
//! They drive one member, one month and one instrument. A member whose bars
//! cross a month boundary is refused by `ingest`, and that refusal is not
//! exercised here. Nothing here crashes mid-append or fills a disk.

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

use brutex_core::vendor::Vendor;
use store::file::BarFile;
use store::format::Bar;
use store::path::{FileKind, PathParts, StorePath, Timeframe, YearMonth};

use pull::csv::Columns;
use pull::fetch::BarRequest;
use pull::ingest::{self, Ingested, Plan};
use pull::session::{
    BARS_PER_REGULAR_SESSION, Cadence, Day, DropReason, IstMoment, SESSION_CLOSE_MINUTE,
    SESSION_OPEN_MINUTE, Window,
};
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
            "brutex-pull-ingest-{}-{tag}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("a scratch root");
        Self { root }
    }

    /// Where the vendor's files go.
    fn archive(&self) -> PathBuf {
        let dir = self.root.join("ARCHIVE");
        fs::create_dir_all(&dir).expect("a scratch archive");
        dir
    }

    /// Where the bars go.
    fn store(&self) -> PathBuf {
        let dir = self.root.join("STORE");
        fs::create_dir_all(&dir).expect("a scratch store");
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
/// Written as one literal rather than assembled, so the bytes this test drives
/// are the bytes a reader can check against the table in the file header.
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

/// How many rows [`BODY`] holds.
const ROWS: usize = 8;
/// How many of them survive the window, the session and the fold.
const BARS: usize = 3;

/// The first and last day the operator asked for.
fn window() -> Window {
    Window::new(
        Day::new(2022, 10, 3).expect("2022-10-03"),
        Day::new(2022, 10, 4).expect("2022-10-04"),
    )
    .expect("a forward window")
}

/// Writes the fixture member and returns the folder holding it.
///
/// The file name is the instrument: `ingest` takes the symbol from it, so
/// naming the file is naming the store path.
fn archive_with_the_fixture(scratch: &Scratch) -> PathBuf {
    let dir = scratch.archive();
    fs::write(dir.join("NIFTY.csv"), BODY).expect("the fixture member");
    dir
}

/// The store path the fixture's bars must land at.
fn bars_path() -> StorePath<'static> {
    StorePath::new(PathParts {
        vendor: Vendor::Groww,
        exchange: "NSE",
        segment: "INDEX",
        symbol: "NIFTY",
        timeframe: Timeframe::MINUTE_1,
        month: YearMonth::new(2022, 10).expect("October 2022"),
        file: FileKind::Bars,
    })
    .expect("a legal path")
}

/// Runs one ingest of the fixture into `store_root`.
fn run(archive: &Path, store_root: &Path, request: &BarRequest) -> Ingested {
    ingest::from_dir(
        archive,
        store_root,
        Plan {
            columns: Columns::TrueDataIndex,
            request,
            // `pull::csv::decode` converts the vendor's IST wall clock to UTC
            // epoch seconds where the format is known, so this is what it hands
            // on — not a second opinion about the encoding.
            encoding: TimestampEncoding::EpochSecondsUtc,
            // And it parses straight to paisa, so there is nothing to scale.
            scale: PriceScale::Paisa,
            timeframe: Timeframe::MINUTE_1,
            vendor: Vendor::Groww,
            exchange: "NSE",
            segment: "INDEX",
        },
    )
    .expect("the folder is readable and the column shape is right")
}

/// Every bar the store holds for the fixture's month, in index order.
///
/// Opens the month itself rather than trusting the counters the run returned:
/// the invariant is about the file.
fn stored_bars(store_root: &Path) -> Vec<Bar> {
    // The symbol id `ingest` stamps into the header is derived from the
    // instrument name, so the same name always opens the same file. A wrong id
    // is refused by `BarFile::open_or_create` rather than ignored, which makes
    // this call itself an assertion.
    let symbol_id = u32::try_from(brutex_core::universe::fnv1a("NIFTY") & 0xFFFF_FFFF)
        .expect("the low half of a 64-bit hash");
    let file =
        BarFile::open_or_create(store_root, bars_path(), symbol_id).expect("the month reopens");
    let mut out = Vec::new();
    for index in 0..file.records() {
        out.push(file.read_record(index).expect("a committed record"));
    }
    out
}

/// The whole bar file, as bytes.
fn file_image(store_root: &Path) -> Vec<u8> {
    fs::read(bars_path().to_path_buf(store_root)).expect("the bar file")
}

/// The IST moment a stored bar is stamped at.
fn moment_of(bar: &Bar) -> IstMoment {
    IstMoment::from_epoch_secs(bar.ts_micros.div_euclid(1_000_000)).expect("a readable timestamp")
}

// ===========================================================================
// P-02 — a bar outside the requested window is never stored
// ===========================================================================

#[test]
fn a_bar_outside_the_window_or_the_session_is_never_stored() {
    let scratch = Scratch::new("WINDOW");
    let archive = archive_with_the_fixture(&scratch);
    let store_root = scratch.store();
    let request = BarRequest {
        instrument_id: String::new(),
        window: window(),
        cadence: Cadence::Minute,
    };

    let done = run(&archive, &store_root, &request);
    assert_eq!(done.failures, Vec::new(), "no member failed");
    assert_eq!(done.members, 1);
    assert_eq!(done.rows_read, ROWS, "every row of the fixture was read");
    assert_eq!(done.bars_stored, BARS);

    // Each refusal is counted under its own reason. One total would not say
    // whether the vendor ignored the range or included the pre-open auction.
    assert_eq!(done.census.of(DropReason::BeforeWindow), 1);
    assert_eq!(done.census.of(DropReason::BeforeSessionOpen), 1);
    assert_eq!(done.census.of(DropReason::AtOrAfterSessionClose), 1);
    assert_eq!(done.census.of(DropReason::AfterWindow), 1);
    assert_eq!(done.census.total(), 4);

    // THE INVARIANT, READ OFF THE DISK. Not "the pipeline said it dropped
    // four" — every record the file actually holds is inside the operator's
    // window and inside the exchange's session.
    let bars = stored_bars(&store_root);
    assert_eq!(bars.len(), BARS, "the file holds what the run reported");
    let window = window();
    for bar in &bars {
        let at = moment_of(bar);
        assert!(
            window.contains(at.day()),
            "{} is outside the requested window",
            at.day()
        );
        assert!(
            at.minute_of_day() >= SESSION_OPEN_MINUTE,
            "a bar before the session open reached the disk"
        );
        assert!(
            at.minute_of_day() < SESSION_CLOSE_MINUTE,
            "a bar at or after the session close reached the disk"
        );
    }

    // And the three that survived are the three expected ones, at the minute
    // each bucket opens rather than at the snapshot that opened it.
    let days: Vec<Day> = bars.iter().map(|bar| moment_of(bar).day()).collect();
    assert_eq!(
        days,
        vec![
            Day::new(2022, 10, 3).expect("2022-10-03"),
            Day::new(2022, 10, 3).expect("2022-10-03"),
            Day::new(2022, 10, 4).expect("2022-10-04"),
        ]
    );
    let minutes: Vec<u32> = bars
        .iter()
        .map(|bar| moment_of(bar).minute_of_day())
        .collect();
    assert_eq!(
        minutes,
        vec![SESSION_OPEN_MINUTE, SESSION_CLOSE_MINUTE - 1, 9 * 60 + 20],
        "the first and last minutes the exchange trades are both inside"
    );
    assert_eq!(
        SESSION_CLOSE_MINUTE - SESSION_OPEN_MINUTE,
        BARS_PER_REGULAR_SESSION,
        "and those two minutes bound the 375-bar session the charter records"
    );

    // The fold is visible in the first bar: two snapshots inside 09:15 became
    // one bar whose open is the first and whose close is the last, in file
    // order. A window filter that ran after the fold would have folded a
    // dropped row into a kept bar and this would be 3_841_000.
    assert_eq!(bars[0].open, 3_844_565, "38445.65 in paisa, the first");
    assert_eq!(bars[0].close, 3_845_000, "38450.00 in paisa, the last");
    assert_eq!(bars[0].high, 3_845_000);
    assert_eq!(bars[0].low, 3_844_565);
    assert_eq!(bars[1].close, 3_850_000);
    assert_eq!(bars[2].close, 3_860_000);
}

#[test]
fn a_narrower_window_stores_strictly_fewer_bars_and_says_why() {
    // The same bytes, one day narrower: the rows that were inside are now
    // after the window and are counted as such. A filter keyed on anything but
    // the request would store the same three bars again.
    let scratch = Scratch::new("NARROW");
    let archive = archive_with_the_fixture(&scratch);
    let store_root = scratch.store();
    let request = BarRequest {
        instrument_id: String::new(),
        window: Window::new(
            Day::new(2022, 10, 3).expect("2022-10-03"),
            Day::new(2022, 10, 3).expect("2022-10-03"),
        )
        .expect("a one-day window"),
        cadence: Cadence::Minute,
    };

    let done = run(&archive, &store_root, &request);
    assert_eq!(done.failures, Vec::new(), "no member failed");
    assert_eq!(done.bars_stored, 2);
    assert_eq!(
        done.census.of(DropReason::AfterWindow),
        2,
        "2022-10-04 and 2022-10-05 are both after a one-day window"
    );

    let bars = stored_bars(&store_root);
    assert_eq!(bars.len(), 2);
    for bar in &bars {
        assert_eq!(
            moment_of(bar).day(),
            Day::new(2022, 10, 3).expect("2022-10-03"),
            "a day the operator did not ask for reached the disk"
        );
    }
}

// ===========================================================================
// P-04 — re-running an ingest stores nothing new
// ===========================================================================

#[test]
fn idempotent_repull_leaves_the_file_byte_identical() {
    let scratch = Scratch::new("IDEMPOTENT");
    let archive = archive_with_the_fixture(&scratch);
    let store_root = scratch.store();
    let request = BarRequest {
        instrument_id: String::new(),
        window: window(),
        cadence: Cadence::Minute,
    };

    let first = run(&archive, &store_root, &request);
    let before = file_image(&store_root);
    let bars_before = stored_bars(&store_root);

    let second = run(&archive, &store_root, &request);
    let after = file_image(&store_root);

    assert_eq!(
        before, after,
        "a re-run of the same window changed the bytes on disk"
    );
    assert_eq!(
        stored_bars(&store_root),
        bars_before,
        "and the records it holds are the same records"
    );
    assert_eq!(
        before.len(),
        after.len(),
        "no generation was spent and no record was appended"
    );

    // WHAT THE SECOND RUN REPORTS IS NOT ZERO, and this pins it rather than
    // hiding it. `Ingested::bars_stored` is the count of bars the member
    // OFFERED, taken from the batch handed to `append`, and `append` answers
    // `AlreadyPresent` without that count reaching the caller. So the disk half
    // of P-04 holds and the reporting half does not: an operator re-running a
    // pull is told three bars were stored when nothing was written.
    assert_eq!(first.bars_stored, BARS);
    assert_eq!(
        second.bars_stored, BARS,
        "the re-run reports the bars it offered, not the zero it wrote"
    );
    assert_eq!(second.failures, Vec::new(), "and it is not an error either");
}

#[test]
fn a_second_window_over_the_same_month_appends_rather_than_rewrites() {
    // Idempotence must not be bought by refusing every second run: a run that
    // brings bars the file does not have still appends them.
    let scratch = Scratch::new("APPEND");
    let archive = archive_with_the_fixture(&scratch);
    let store_root = scratch.store();

    let narrow = BarRequest {
        instrument_id: String::new(),
        window: Window::new(
            Day::new(2022, 10, 3).expect("2022-10-03"),
            Day::new(2022, 10, 3).expect("2022-10-03"),
        )
        .expect("a one-day window"),
        cadence: Cadence::Minute,
    };
    let first = run(&archive, &store_root, &narrow);
    assert_eq!(first.bars_stored, 2);
    assert_eq!(stored_bars(&store_root).len(), 2);

    let wider = BarRequest {
        instrument_id: String::new(),
        window: Window::new(
            Day::new(2022, 10, 4).expect("2022-10-04"),
            Day::new(2022, 10, 4).expect("2022-10-04"),
        )
        .expect("a one-day window"),
        cadence: Cadence::Minute,
    };
    let second = run(&archive, &store_root, &wider);
    assert_eq!(second.bars_stored, 1);

    let bars = stored_bars(&store_root);
    assert_eq!(bars.len(), 3, "the second day was appended to the first");
    assert_eq!(
        moment_of(&bars[2]).day(),
        Day::new(2022, 10, 4).expect("2022-10-04")
    );
}
