//! The refusals and the seams the happy path never reaches: `pull::pipeline::*`.
//!
//! # Why this file exists
//!
//! `pull::unit::*` proves the arithmetic and `pull::integration::*` drives a
//! folder of real bytes all the way to a bar file. Between them they walk the
//! path that works. What neither of them touches is the half of this crate that
//! only runs when something is wrong: every `Display` an operator reads when a
//! pull refuses, every bound that fires at the boundary, and the transport seam
//! itself — [`pull::fetch::FakeSource`], which exists so that no test needs a
//! socket and which, until this file, no test used.
//!
//! A refusal nobody has ever printed is a refusal whose message is a guess.
//!
//! # Every literal here is invented, and none is a path segment
//!
//! `CLAUDE.md` §8 and CI gates 1c and 1d. The instrument, exchange and segment
//! names below have been tracked since this repository's first commit; the
//! scratch directories are built by `format!`, so no bare lower-case word that
//! could *be* a parameter-path segment is introduced by this file.
//!
//! # Two refusals are deliberately absent, and are named rather than hidden
//!
//! * [`pull::archive::ArchiveError::Unreadable`] raised **by the directory
//!   iterator** — as opposed to by `read_dir` itself, which is proven below.
//!   Reaching it needs `readdir` to fail part-way through a walk it already
//!   started, which needs a filesystem that goes away underneath an open
//!   descriptor. No portable, deterministic way to arrange that exists here.
//! * The `else { break }` arms inside [`pull::fetch::RawWindow::decode`]. They
//!   are unreachable by construction — the length agreement immediately above
//!   them makes every index in range — and they are guards against a later
//!   edit, not dead code. Deleting them to raise a percentage is the trade this
//!   repository refuses.
//!
//! Both are recorded in the coverage report rather than papered over.

// The same exceptions every test module in this workspace takes: a test that
// cannot panic cannot fail, and the lints that forbid panicking exist to keep
// them out of the crate, not out of its tests.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use brutex_core::vendor::Vendor;
use std::collections::HashSet;
use std::fs;
use std::mem::discriminant;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use pull::archive::{self, ArchiveError, MAX_MEMBERS};
use pull::csv::{Columns, CsvError, decode};
use pull::fetch::{
    self, BarRequest, BarSource, FakeSource, FetchError, MAX_ROWS, ParallelArrays, RawRow,
    RawWindow,
};
use pull::fold::{Bucket, FoldError, fold};
use pull::ingest::{Failure, Ingested, Plan};
use pull::session::{Cadence, Day, DropCensus, DropReason, SessionError, Window};
use pull::vendor::{DateFormat, PriceScale, RangeEnd, TimestampEncoding};
use pull::work::Selection;
use store::format::Bar;
use store::path::{Timeframe, YearMonth};

// ===========================================================================
// Scratch
// ===========================================================================

/// Distinguishes two scratch trees taken in the same process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary directory that removes itself.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut root = std::env::temp_dir();
        root.push(format!("{}-{tag}-{serial}", std::process::id()));
        fs::create_dir_all(&root).expect("a scratch root");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leaked scratch directory must never fail a test run.
        drop(fs::remove_dir_all(&self.root));
    }
}

/// One `TrueData` index row: `date,time,price,volume,open_interest`.
///
/// 2022-10-03 at 09:15:01 IST, inside the regular session.
const ONE_ROW: &str = "20221003,09:15:01,38445.65,0,0\n";

/// The window the fixture rows fall inside.
fn window() -> Window {
    Window::new(
        Day::new(2022, 10, 3).expect("a real date"),
        Day::new(2022, 10, 4).expect("a real date"),
    )
    .expect("a forward window")
}

/// A request for that window, at one-minute cadence — the session filter applies.
fn request() -> BarRequest {
    BarRequest {
        window: window(),
        cadence: Cadence::Minute,
    }
}

// ===========================================================================
// crate::archive — the refusals, and the bounds at the boundary
// ===========================================================================

/// Every archive refusal prints, and no two print the same sentence.
///
/// A refusal an operator cannot tell apart from another refusal sends them to
/// the wrong fix, which is the whole reason each variant carries its own words.
#[test]
fn every_archive_refusal_prints_a_sentence_of_its_own() {
    let path = PathBuf::from("/NOWHERE/GFDL");
    let cases = [
        ArchiveError::NotADirectory { path: path.clone() },
        ArchiveError::Unreadable {
            path: path.clone(),
            detail: "THE OS SAID SO".to_owned(),
        },
        ArchiveError::MemberUnreadable {
            path: path.clone(),
            detail: "THE OS SAID SO".to_owned(),
        },
        ArchiveError::MemberNotText { path: path.clone() },
        ArchiveError::MemberMalformed {
            path: path.clone(),
            why: CsvError::TooManyRows { rows: 3, cap: 2 },
        },
        ArchiveError::TooManyMembers {
            members: 7,
            cap: MAX_MEMBERS,
        },
        ArchiveError::PathEscapes { path },
    ];

    let mut sentences = HashSet::with_capacity(cases.len());
    for case in &cases {
        let text = case.to_string();
        assert!(!text.is_empty(), "{case:?} prints nothing");
        assert!(
            sentences.insert(text.clone()),
            "two refusals read the same, so an operator cannot tell them \
             apart: {text}"
        );
        let as_error: &dyn core::error::Error = case;
        assert_eq!(as_error.to_string(), text, "the Error impl agrees");
    }
    assert_eq!(sentences.len(), cases.len());

    // Each refusal that has a subject names it.
    for case in &cases[..5] {
        assert!(
            case.to_string().contains("GFDL"),
            "a refusal about a path must name the path — {case:?}"
        );
    }
    let capped = cases[5].to_string();
    assert!(
        capped.contains('7') && capped.contains(&MAX_MEMBERS.to_string()),
        "the cap refusal names what was seen and what the bound is — {capped}"
    );
    assert!(
        cases[4].to_string().contains("the cap is 2"),
        "a malformed member carries the decoder's own words, which name the \
         line — {}",
        cases[4]
    );
}

/// A directory the operating system will not list is refused in its words.
///
/// UNIX-ONLY, and it assumes the test process is not root: root bypasses the
/// permission bits and would list the directory anyway. Both CI and the
/// operator's machine run this as an ordinary user.
#[cfg(unix)]
#[test]
fn a_directory_that_will_not_open_is_refused_with_the_operating_systems_words() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("UNLISTABLE");
    let dir = scratch.root.join("ARCHIVE");
    fs::create_dir_all(&dir).expect("a scratch archive");
    fs::write(dir.join("NIFTY.csv"), ONE_ROW).expect("one member");

    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).expect("close the directory");
    let refused = archive::read_dir(&dir, Columns::TrueDataIndex);
    // Reopen before asserting, so a failed assertion still leaves a tree the
    // scratch guard can remove.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("reopen the directory");

    let refused = refused.expect_err("a directory with no read bit cannot be listed");
    assert_eq!(
        discriminant(&refused),
        discriminant(&ArchiveError::Unreadable {
            path: dir.clone(),
            detail: String::new(),
        }),
        "listing failed, and that is what the refusal says — got {refused}"
    );
    let text = refused.to_string();
    assert!(
        text.contains(&dir.display().to_string()),
        "the refusal names the directory — {text}"
    );
    assert!(
        text.len() > dir.display().to_string().len() + "could not be listed: ".len(),
        "and carries the operating system's own detail rather than swallowing \
         it — {text}"
    );
}

/// A member the operating system will not read stops the walk and names it.
#[cfg(unix)]
#[test]
fn a_member_that_will_not_open_stops_the_walk_and_names_the_member() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("UNREADABLE");
    let dir = scratch.root.join("ARCHIVE");
    fs::create_dir_all(&dir).expect("a scratch archive");
    let member = dir.join("NIFTY.csv");
    fs::write(&member, ONE_ROW).expect("one member");

    fs::set_permissions(&member, fs::Permissions::from_mode(0o000)).expect("close the member");
    let refused = archive::read_dir(&dir, Columns::TrueDataIndex);
    fs::set_permissions(&member, fs::Permissions::from_mode(0o644)).expect("reopen the member");

    let refused = refused.expect_err("a member with no read bit cannot be read");
    assert_eq!(
        discriminant(&refused),
        discriminant(&ArchiveError::MemberUnreadable {
            path: member.clone(),
            detail: String::new(),
        }),
        "reading the member failed, not listing the directory — got {refused}"
    );
    assert!(
        refused.to_string().contains("NIFTY"),
        "and it names WHICH member — {refused}"
    );
}

/// A member path carrying a parent-directory component is refused before the
/// file is opened.
#[test]
fn a_member_path_that_escapes_the_walk_is_refused_before_it_is_opened() {
    let scratch = Scratch::new("ESCAPE");
    let dir = scratch.root.join("ARCHIVE");
    fs::create_dir_all(&dir).expect("a scratch archive");
    fs::write(dir.join("NIFTY.csv"), ONE_ROW).expect("one member");

    // The same directory, reached through its own parent. Every entry `read_dir`
    // yields therefore carries a `..` component.
    let sideways = dir.join("..").join("ARCHIVE");
    assert!(sideways.is_dir(), "the sideways path is the same directory");

    let refused =
        archive::read_dir(&sideways, Columns::TrueDataIndex).expect_err("the path escapes");
    assert_eq!(
        discriminant(&refused),
        discriminant(&ArchiveError::PathEscapes {
            path: PathBuf::new(),
        }),
        "refused as an escape, and refused BEFORE the file was opened — got \
         {refused}"
    );
    assert!(
        refused.to_string().contains("NIFTY"),
        "naming the offending member — {refused}"
    );

    // The straight path over the same bytes is accepted, so the refusal is
    // about the `..` and not about the fixture.
    let members = archive::read_dir(&dir, Columns::TrueDataIndex).expect("the direct walk");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].rows.len(), 1);
}

/// The walk stops at [`MAX_MEMBERS`] rather than following a folder somebody
/// pointed at their home directory.
#[test]
fn a_directory_past_the_member_cap_is_refused_at_the_cap() {
    let scratch = Scratch::new("CAP");
    let dir = scratch.root.join("ARCHIVE");
    fs::create_dir_all(&dir).expect("a scratch archive");

    // One more than the bound. Empty members: the cap is checked before a
    // member is read, so their contents are beside the point and their absence
    // keeps this test to a few seconds.
    for i in 0..=MAX_MEMBERS {
        fs::write(dir.join(format!("F{i}.csv")), "").expect("a member");
    }

    let refused = archive::read_dir(&dir, Columns::TrueDataIndex).expect_err("past the cap");
    assert_eq!(
        refused,
        ArchiveError::TooManyMembers {
            members: MAX_MEMBERS,
            cap: MAX_MEMBERS,
        },
        "the walk stopped AT the bound, and the refusal carries both numbers"
    );
}

// ===========================================================================
// crate::csv — the refusals, and the arms the shipped layouts do not take
// ===========================================================================

/// Every decoder refusal prints, names its line, and reads unlike the others.
#[test]
fn every_csv_refusal_prints_a_sentence_of_its_own() {
    let cases = [
        CsvError::FieldCount {
            line: 4,
            got: 9,
            want: 5,
        },
        CsvError::DateMalformed {
            line: 4,
            got: "NOT A DATE".to_owned(),
            format: DateFormat::CompactYmd,
        },
        CsvError::TimeMalformed {
            line: 4,
            got: "NOT A TIME".to_owned(),
        },
        CsvError::PriceMalformed {
            line: 4,
            got: "NOT A PRICE".to_owned(),
        },
        CsvError::TooManyRows {
            rows: MAX_ROWS,
            cap: MAX_ROWS,
        },
    ];

    let mut sentences = HashSet::with_capacity(cases.len());
    for case in &cases {
        let text = case.to_string();
        assert!(
            sentences.insert(text.clone()),
            "two refusals read alike: {text}"
        );
        let as_error: &dyn core::error::Error = case;
        assert_eq!(as_error.to_string(), text);
    }
    for case in &cases[..4] {
        assert!(
            case.to_string().contains("line 4"),
            "a per-line refusal names the line — {case:?}"
        );
    }
    assert!(
        cases[4].to_string().contains(&MAX_ROWS.to_string()),
        "and the bound names the bound — {}",
        cases[4]
    );
}

/// A decoder refusal becomes a transport failure without losing its words.
#[test]
fn a_decoder_refusal_carries_its_own_words_into_a_fetch_failure() {
    let why = CsvError::FieldCount {
        line: 2,
        got: 10,
        want: 5,
    };
    let lifted = FetchError::from(why.clone());
    assert_eq!(
        lifted,
        FetchError::TransportFailed {
            detail: why.to_string(),
        },
        "the decoder's sentence survives the conversion — a generic \"parse \
         failed\" would name a different fix from the one that is needed"
    );
    assert!(lifted.to_string().contains("line 2"));
}

/// The fractional half of the paisa grid: one place is tenths, three places is
/// a refusal, and a whole part that is not digits is a refusal too.
#[test]
fn a_price_is_put_on_the_paisa_grid_exactly_or_refused() {
    let decoded = decode(
        "20221003,09:15:01,38444.9,0,0\n20221003,09:15:02,38444.90,0,0\n\
         20221003,09:15:03,38444,0,0\n",
        Columns::TrueDataIndex,
    )
    .expect("three well-formed prices");
    assert_eq!(
        decoded[0].close, 3_844_490,
        "one decimal place is TENTHS — .9 is ninety paisa, not nine"
    );
    assert_eq!(decoded[1].close, 3_844_490, "and two places agree with it");
    assert_eq!(
        decoded[2].close, 3_844_400,
        "no decimal point is whole rupees"
    );

    // Every way a price is refused rather than rounded or guessed.
    for (body, why) in [
        ("20221003,09:15:01,38444.999,0,0\n", "a third decimal place"),
        ("20221003,09:15:01,.5,0,0\n", "no whole part at all"),
        (
            "20221003,09:15:01,1A.50,0,0\n",
            "a whole part that is not digits",
        ),
        (
            "20221003,09:15:01,38444.X0,0,0\n",
            "a fraction that is not digits",
        ),
    ] {
        let refused = decode(body, Columns::TrueDataIndex).expect_err(why);
        assert_eq!(
            discriminant(&refused),
            discriminant(&CsvError::PriceMalformed {
                line: 1,
                got: String::new(),
            }),
            "{why} is refused as a price, not silently rounded — got {refused}"
        );
    }

    // A negative price is a legal decimal and stays negative in paisa. The
    // engine never asks for one; the grid arithmetic does not care.
    let signed = decode("20221003,09:15:01,-1.25,0,0\n", Columns::TrueDataIndex)
        .expect("a signed decimal is still a decimal");
    assert_eq!(signed[0].close, -125);
}

/// A blank line is skipped and the row count is unaffected.
#[test]
fn a_blank_line_is_not_a_row() {
    let with_gaps = decode(
        "\n20221003,09:15:01,38445.65,0,0\n\n   \n20221003,09:15:02,38419.40,0,0\n\n",
        Columns::TrueDataIndex,
    )
    .expect("blank lines are not rows");
    assert_eq!(
        with_gaps.len(),
        2,
        "two rows, whatever the whitespace between"
    );

    // A carriage return is trimmed rather than parsed into the last field.
    let crlf = decode("20221003,09:15:01,38445.65,0,0\r\n", Columns::TrueDataIndex)
        .expect("CRLF is observed in vendor files");
    assert_eq!(crlf.len(), 1);
    assert_eq!(crlf[0].close, 3_844_565);
}

/// A file with more rows than the bound is refused at the bound.
///
/// Costly on purpose: [`MAX_ROWS`] is the boundary bound `docs/07` law 5 asks
/// for, and a bound nothing has ever crossed is a bound nobody has checked.
#[test]
fn a_file_past_the_row_cap_is_refused_at_the_cap() {
    // The shortest legal five-field row, repeated. One more than the bound.
    let row = "19700101,00:00:00,0,0,0\n";
    let mut body = String::with_capacity(row.len() * (MAX_ROWS + 1));
    for _ in 0..=MAX_ROWS {
        body.push_str(row);
    }

    let refused = decode(&body, Columns::TrueDataIndex).expect_err("past the cap");
    assert_eq!(
        refused,
        CsvError::TooManyRows {
            rows: MAX_ROWS,
            cap: MAX_ROWS,
        },
        "the decode stopped AT the bound, and says how many it had"
    );
}

// ===========================================================================
// crate::fetch — the refusals, the seam, and the encodings
// ===========================================================================

/// Every fetch refusal prints, and the length disagreement names all seven.
#[test]
fn every_fetch_refusal_prints_a_sentence_of_its_own() {
    let cases = [
        FetchError::LengthDisagreement {
            open: 3,
            high: 3,
            low: 3,
            close: 3,
            volume: 2,
            timestamp: 3,
            open_interest: 0,
        },
        FetchError::TooManyRows {
            rows: MAX_ROWS + 1,
            cap: MAX_ROWS,
        },
        FetchError::TimestampRefused {
            row: 4,
            raw: -1,
            why: SessionError::TimestampOutOfRange { secs: -1 },
        },
        FetchError::PriceRefused {
            row: 4,
            field: "OPEN",
            raw: i64::MAX,
        },
        FetchError::VendorRefused {
            status: 429,
            detail: "SLOW DOWN".to_owned(),
        },
        FetchError::TransportFailed {
            detail: "NO ROUTE".to_owned(),
        },
    ];

    let mut sentences = HashSet::with_capacity(cases.len());
    for case in &cases {
        let text = case.to_string();
        assert!(
            sentences.insert(text.clone()),
            "two refusals read alike: {text}"
        );
        let as_error: &dyn core::error::Error = case;
        assert_eq!(as_error.to_string(), text);
    }

    let disagreement = cases[0].to_string();
    for length in ["open 3", "high 3", "low 3", "close 3", "volume 2"] {
        assert!(
            disagreement.contains(length),
            "the refusal names EVERY length so an operator can see which field \
             the vendor truncated — {length} missing from {disagreement}"
        );
    }
    assert!(
        cases[4].to_string().contains(&429.to_string()),
        "a rate refusal is distinguishable from a broken vendor — {}",
        cases[4]
    );
    assert!(cases[2].to_string().contains("row 4"));
    assert!(cases[3].to_string().contains("OPEN"));
}

/// More rows than the bound refuses the window rather than truncating it.
#[test]
fn a_window_past_the_row_cap_is_refused_at_the_cap() {
    let over = MAX_ROWS + 1;
    let column = || vec![0_i64; over];
    let arrays = ParallelArrays {
        open: column(),
        high: column(),
        low: column(),
        close: column(),
        volume: column(),
        timestamp: column(),
        open_interest: Vec::new(),
    };
    assert_eq!(
        RawWindow::decode(&arrays).expect_err("past the cap"),
        FetchError::TooManyRows {
            rows: over,
            cap: MAX_ROWS,
        },
        "a vendor answering a one-day request with a decade is refused, not \
         trimmed"
    );
}

/// The seam every test in this crate is supposed to use, used.
#[test]
fn the_fake_source_answers_from_memory_and_can_be_told_to_refuse() {
    let request = request();

    let empty = FakeSource::default();
    assert_eq!(
        empty.window(&request).expect("the default answers"),
        RawWindow::default(),
        "the default is an EMPTY window rather than a refusal — a fake that \
         refused by default would make every test that forgot to configure it \
         pass for the wrong reason"
    );

    let row = RawRow {
        timestamp: 1_664_766_301,
        open: 100,
        high: 130,
        low: 90,
        close: 120,
        volume: 7,
        open_interest: Some(0),
    };
    let returning = FakeSource::returning(vec![row]);
    assert_eq!(
        returning.window(&request).expect("the fake answers"),
        RawWindow { rows: vec![row] },
        "and it answers with exactly the rows it was built from"
    );

    let refusing = FakeSource::refusing(FetchError::VendorRefused {
        status: 500,
        detail: "BROKEN".to_owned(),
    });
    assert_eq!(
        refusing
            .window(&request)
            .expect_err("this fake was built to refuse"),
        FetchError::VendorRefused {
            status: 500,
            detail: "BROKEN".to_owned(),
        }
    );
    assert!(
        !format!("{returning:?}").is_empty(),
        "a fake is inspectable"
    );
}

/// Fetch and land in one call, through the seam.
#[test]
fn fetching_and_landing_is_one_call_over_the_same_seam() {
    // 2022-10-03 09:15:00 IST is 03:45:00 UTC.
    let at = 1_664_768_700_i64;
    let row = RawRow {
        timestamp: at,
        open: 100,
        high: 130,
        low: 90,
        close: 120,
        volume: 7,
        open_interest: None,
    };
    let request = request();

    let landed = fetch::fetch_and_land(
        &FakeSource::returning(vec![row]),
        &request,
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Paisa,
    )
    .expect("one row, inside the window and the session");
    assert_eq!(landed.bars.len(), 1);
    assert_eq!(landed.bars[0].ts_micros, at * 1_000_000);
    assert_eq!(
        landed.bars[0].open_interest,
        i64::MIN,
        "an ABSENT open interest is the null sentinel; a vendor's literal zero \
         would have stayed a zero"
    );
    assert!(landed.census.is_empty(), "nothing was dropped");

    let refused = fetch::fetch_and_land(
        &FakeSource::refusing(FetchError::TransportFailed {
            detail: "NO ROUTE".to_owned(),
        }),
        &request,
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Paisa,
    )
    .expect_err("the source refused, so the call refuses");
    assert_eq!(
        refused,
        FetchError::TransportFailed {
            detail: "NO ROUTE".to_owned(),
        },
        "and it refuses in the transport's own words rather than its own"
    );
}

/// W1: the timestamp encoding is dispatched, never assumed — and all three
/// encodings land the same instant.
#[test]
fn every_timestamp_encoding_lands_the_same_instant() {
    // 2022-10-03 09:16:00 IST.
    let utc_secs = 1_664_768_760_i64;
    let ist_secs = utc_secs + 5 * 3_600 + 30 * 60;
    let request = request();

    let landed_at = |raw: i64, encoding: TimestampEncoding| {
        let row = RawRow {
            timestamp: raw,
            open: 1,
            high: 1,
            low: 1,
            close: 1,
            volume: 0,
            open_interest: None,
        };
        let landed = fetch::land(
            &RawWindow { rows: vec![row] },
            &request,
            encoding,
            PriceScale::Paisa,
        )
        .expect("one row inside the session");
        assert_eq!(landed.bars.len(), 1, "the row survived");
        landed.bars[0].ts_micros
    };

    let seconds = landed_at(utc_secs, TimestampEncoding::EpochSecondsUtc);
    let millis = landed_at(utc_secs * 1_000, TimestampEncoding::EpochMillisUtc);
    let ist = landed_at(ist_secs, TimestampEncoding::IstDateTimeText);
    assert_eq!(seconds, utc_secs * 1_000_000);
    assert_eq!(
        millis, seconds,
        "milliseconds are divided down, not read as seconds"
    );
    assert_eq!(
        ist, seconds,
        "a vendor's IST wall clock is converted rather than trusted — this is \
         W1, the only fault in this pipeline that STORED a wrong answer"
    );
}

/// A timestamp this build cannot hold refuses the whole window and names the
/// row it came from.
#[test]
fn a_timestamp_that_names_no_instant_refuses_the_whole_window() {
    let bad = RawRow {
        timestamp: i64::MIN,
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: 0,
        open_interest: None,
    };
    let good = RawRow {
        timestamp: 1_664_768_760,
        ..bad
    };
    let refused = fetch::land(
        &RawWindow {
            rows: vec![good, bad],
        },
        &request(),
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Paisa,
    )
    .expect_err("the second row names no instant");
    assert_eq!(
        discriminant(&refused),
        discriminant(&FetchError::TimestampRefused {
            row: 0,
            raw: 0,
            why: SessionError::TimestampOutOfRange { secs: 0 },
        }),
        "a timestamp this build cannot read is the vendor being wrong, not a \
         bar this engine declined — got {refused}"
    );
    assert!(
        refused.to_string().contains("row 1"),
        "and the refusal names WHICH row — {refused}"
    );
}

// ===========================================================================
// crate::fold — the refusal, and the open interest that survives a bucket
// ===========================================================================

/// Open interest is the LAST snapshot in the bucket that carried one, and an
/// absent one never overwrites a real one.
#[test]
fn a_fold_keeps_the_last_open_interest_that_was_actually_sent() {
    let at = |secs: i64| secs * 1_000_000;
    let snap = |ts: i64, price: i64, open_interest: i64| Bar {
        ts_micros: ts,
        open: price,
        high: price,
        low: price,
        close: price,
        volume: 1,
        open_interest,
    };

    let folded = fold(
        &[
            snap(at(0), 100, i64::MIN),
            snap(at(1), 130, 4_200),
            snap(at(2), 90, i64::MIN),
        ],
        Bucket::MINUTE,
    )
    .expect("three snapshots in one minute");

    assert_eq!(folded.len(), 1, "one minute in, one bar out");
    assert_eq!(folded[0].open, 100, "the FIRST in file order");
    assert_eq!(folded[0].high, 130);
    assert_eq!(folded[0].low, 90);
    assert_eq!(folded[0].close, 90, "the LAST in file order");
    assert_eq!(folded[0].volume, 3, "the volumes are summed");
    assert_eq!(
        folded[0].open_interest, 4_200,
        "the sentinel that arrived afterwards is ABSENT, not zero, so it never \
         overwrites a measurement"
    );

    // And a bucket whose every snapshot is absent stays absent.
    let never = fold(
        &[snap(at(0), 100, i64::MIN), snap(at(1), 110, i64::MIN)],
        Bucket::MINUTE,
    )
    .expect("two snapshots");
    assert_eq!(never[0].open_interest, i64::MIN);
}

/// A snapshot stamped before the one ahead of it is refused rather than sorted.
#[test]
fn an_out_of_order_snapshot_is_refused_and_the_refusal_prints() {
    let bar = |ts: i64| Bar {
        ts_micros: ts,
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: 1,
        open_interest: i64::MIN,
    };
    let refused = fold(&[bar(120_000_000), bar(60_000_000)], Bucket::MINUTE)
        .expect_err("the second snapshot precedes the first");
    assert_eq!(
        refused,
        FoldError::OutOfOrder {
            at: 1,
            previous: 120_000_000,
            found: 60_000_000,
        },
        "refused rather than sorted: rows sharing a second have no tiebreaker, \
         so a sort would invent an order and change which price became the open"
    );

    let text = refused.to_string();
    assert!(
        text.contains(&120_000_000.to_string())
            && text.contains(&60_000_000.to_string())
            && text.contains('1'),
        "the refusal names the position and both timestamps — {text}"
    );
    let as_error: &dyn core::error::Error = &refused;
    assert_eq!(as_error.to_string(), text);

    // A bucket wide enough to hold a whole session still refuses the same way,
    // so the rule is about order and not about the width.
    assert!(fold(&[bar(120_000_000), bar(60_000_000)], Bucket::DAY).is_err());
    assert_eq!(Bucket::DAY.secs(), 86_400);
    assert_eq!(Bucket::of_secs(0), None, "a zero-width bucket is refused");
}

// ===========================================================================
// crate::work and crate::ingest — the reporting halves
// ===========================================================================

/// A selection reports its instruments in the order it was given them.
#[test]
fn a_selection_reports_its_instruments_in_order() {
    let chosen = Selection::of(["NSE-NIFTY", "NSE-BANKNIFTY", "NSE-NIFTY", ""]);
    assert_eq!(
        chosen.names(),
        ["NSE-NIFTY".to_owned(), "NSE-BANKNIFTY".to_owned()],
        "given order, duplicates removed, and an empty name is not an \
         instrument"
    );
    assert_eq!(chosen.len(), chosen.names().len());
    assert!(!chosen.is_empty());
    assert!(Selection::of(Vec::<String>::new()).names().is_empty());

    let months = [YearMonth::new(2025, 1).expect("January 2025")];
    let cells = chosen.cells(&months, Timeframe::MINUTE_1);
    assert_eq!(
        cells
            .iter()
            .map(|c| c.instrument.as_str())
            .collect::<Vec<_>>(),
        chosen
            .names()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "and the cells are built from exactly those names"
    );
}

/// A run balances only when every row is accounted for and nothing failed.
#[test]
fn a_run_balances_only_when_every_row_is_accounted_for() {
    let mut census = DropCensus::default();
    census.count(DropReason::BeforeWindow);
    census.count(DropReason::AfterWindow);

    let balanced = Ingested {
        members: 1,
        rows_read: 5,
        bars_stored: 3,
        census,
        failures: Vec::new(),
    };
    assert!(
        balanced.balances(),
        "three stored plus two dropped is the five that were read"
    );

    let short = Ingested {
        bars_stored: 2,
        ..balanced.clone()
    };
    assert!(
        !short.balances(),
        "a row that vanished without landing anywhere is indistinguishable \
         from a row the vendor never sent"
    );

    let with_failure = Ingested {
        failures: vec![Failure {
            instrument: "NIFTY".to_owned(),
            why: "REFUSED".to_owned(),
        }],
        ..balanced.clone()
    };
    assert!(
        !with_failure.balances(),
        "and a run with a failed member does not balance even when the \
         arithmetic does"
    );
    assert_eq!(with_failure.failures[0].instrument, "NIFTY");
    assert!(
        !format!("{with_failure:?}").is_empty(),
        "a run is inspectable"
    );
    assert_eq!(Ingested::default().members, 0);
    assert!(
        Ingested::default().balances(),
        "a run that read nothing is trivially balanced"
    );
}

/// The wire value for a window's end honours the vendor's inclusivity, in one
/// place.
#[test]
fn the_wire_end_is_converted_once_and_refuses_past_the_calendar() {
    let last = Day::new(2022, 10, 4).expect("a real date");
    assert_eq!(
        fetch::wire_end(last, RangeEnd::Inclusive).expect("an inclusive vendor takes it unchanged"),
        last,
        "an inclusive vendor takes the operator's own last day"
    );
    assert_eq!(
        fetch::wire_end(last, RangeEnd::Exclusive).expect("the day after"),
        Day::new(2022, 10, 5).expect("the day after"),
        "an exclusive vendor takes the day AFTER — one conversion site, so the \
         off-by-one cannot come back"
    );

    let last_representable = Day::new(9999, 12, 31).expect("the last day");
    assert_eq!(
        fetch::wire_end(last_representable, RangeEnd::Exclusive)
            .expect_err("there is no day after it"),
        SessionError::NoNextDay,
        "refused by name rather than wrapping to the epoch"
    );
}

/// A member that is not text is refused as such, which names a different fix
/// from a parse failure.
#[test]
fn a_member_that_is_not_text_is_refused_as_a_ghost_the_filter_missed() {
    let scratch = Scratch::new("BINARY");
    let dir = scratch.root.join("ARCHIVE");
    fs::create_dir_all(&dir).expect("a scratch archive");
    // An AppleDouble stub begins with this magic and is not UTF-8. The ghost
    // filter catches the ones named `._x`; this one is named like data.
    fs::write(dir.join("NIFTY.csv"), [0x00, 0x05, 0x16, 0x07, 0xFF, 0xFE])
        .expect("a binary member");

    let refused =
        archive::read_dir(&dir, Columns::TrueDataIndex).expect_err("those bytes are not text");
    assert_eq!(
        refused,
        ArchiveError::MemberNotText {
            path: dir.join("NIFTY.csv"),
        },
        "refused as NOT TEXT rather than as a malformed row — the two name \
         different fixes"
    );
}

/// A price that leaves the paisa grid on conversion refuses the whole window,
/// and each of the four fields refuses in its own name.
#[test]
fn a_price_that_leaves_the_paisa_grid_refuses_the_window_and_names_its_field() {
    // Fits an i64, and does not fit one once multiplied by a hundred.
    let too_big = i64::MAX / 50;
    let base = RawRow {
        timestamp: 1_664_768_760,
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: 0,
        open_interest: None,
    };
    let rows = [
        RawRow {
            open: too_big,
            ..base
        },
        RawRow {
            high: too_big,
            ..base
        },
        RawRow {
            low: too_big,
            ..base
        },
        RawRow {
            close: too_big,
            ..base
        },
    ];

    let mut sentences = HashSet::with_capacity(rows.len());
    for row in rows {
        let refused = fetch::land(
            &RawWindow { rows: vec![row] },
            &request(),
            TimestampEncoding::EpochSecondsUtc,
            PriceScale::Rupees,
        )
        .expect_err("a rupee price this large has no paisa");
        assert_eq!(
            discriminant(&refused),
            discriminant(&FetchError::PriceRefused {
                row: 0,
                field: "",
                raw: 0,
            }),
            "refused on the grid, not rounded onto it — got {refused}"
        );
        let text = refused.to_string();
        assert!(
            sentences.insert(text.clone()),
            "each field must refuse in its OWN name, or an operator cannot see \
             which one the vendor sent wrong: {text}"
        );
        assert!(text.contains(&too_big.to_string()), "carrying the value");
    }
    assert_eq!(sentences.len(), 4, "four fields, four sentences");

    // The same value in paisa is not converted and therefore not refused.
    let landed = fetch::land(
        &RawWindow {
            rows: vec![rows[0]],
        },
        &request(),
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Paisa,
    )
    .expect("a vendor already quoting paisa is taken as is");
    assert_eq!(landed.bars[0].open, too_big);
}

// ===========================================================================
// crate::ingest — the member-level failures the run is supposed to survive
// ===========================================================================

/// The plan every ingest test below runs, with the one field it varies.
fn plan_over<'a>(request: &'a BarRequest, exchange: &'a str, scale: PriceScale) -> Plan<'a> {
    Plan {
        columns: Columns::TrueDataIndex,
        request,
        // `pull::csv::decode` has already converted the vendor's IST wall clock
        // to UTC epoch seconds, so this is what it hands on rather than a
        // second opinion about the encoding.
        encoding: TimestampEncoding::EpochSecondsUtc,
        // And it parses straight to paisa, so by default nothing is scaled.
        scale,
        timeframe: Timeframe::MINUTE_1,
        vendor: Vendor::Groww,
        exchange,
        segment: "INDEX",
    }
}

/// A folder holding one member per `(name, body)` pair.
fn folder_of(scratch: &Scratch, members: &[(&str, &str)]) -> PathBuf {
    let dir = scratch.root.join("ARCHIVE");
    fs::create_dir_all(&dir).expect("a scratch archive");
    for (name, body) in members {
        fs::write(dir.join(format!("{name}.csv")), body).expect("a member");
    }
    dir
}

/// A fresh store root.
fn store_of(scratch: &Scratch) -> PathBuf {
    let dir = scratch.root.join("STORE");
    fs::create_dir_all(&dir).expect("a scratch store");
    dir
}

/// A folder that is not there refuses the **whole run**, not one member.
#[test]
fn a_folder_that_is_not_there_refuses_the_whole_run() {
    let scratch = Scratch::new("ABSENT");
    let request = request();
    let refused = pull::ingest::from_dir(
        &scratch.root.join("NOT-THERE"),
        &store_of(&scratch),
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect_err("the folder does not exist");
    assert_eq!(
        discriminant(&refused),
        discriminant(&ArchiveError::NotADirectory {
            path: PathBuf::new(),
        }),
        "the folder is wrong and every member shares the folder, so this is \
         run-level and not counted as one member's failure — got {refused}"
    );
}

/// One malformed member is counted and named, and the run stores the others.
#[test]
fn a_member_that_cannot_be_stored_is_named_and_the_run_carries_on() {
    let scratch = Scratch::new("PARTIAL");
    // Twenty-five bytes: one past what `brutex_core::symbol::Symbol` holds, so
    // the instrument taken from the file name is refused.
    let too_long = "AAAAAAAAAAAAAAAAAAAAAAAAA";
    let dir = folder_of(&scratch, &[("NIFTY", ONE_ROW), (too_long, ONE_ROW)]);
    let request = request();
    let done = pull::ingest::from_dir(
        &dir,
        &store_of(&scratch),
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect("the folder and the column shape are both right");

    assert_eq!(done.members, 2, "both members were read");
    assert_eq!(done.rows_read, 2);
    assert_eq!(done.bars_stored, 1, "the good member still landed");
    assert_eq!(done.failures.len(), 1, "and exactly one failed");
    assert_eq!(
        done.failures[0].instrument, too_long,
        "the failure names WHICH member, so eleven thousand others are not \
         discarded to find it"
    );
    assert!(
        !done.failures[0].why.is_empty(),
        "and why, in the refusal's own words"
    );
    assert!(
        !done.balances(),
        "a run with a failed member does not balance, whatever the arithmetic \
         says"
    );
}

/// A member whose rows are all outside the window stores nothing and says so.
#[test]
fn a_member_whose_rows_all_fall_outside_the_window_stores_nothing() {
    let scratch = Scratch::new("OUTSIDE");
    // 2022-10-10 is a week past the window's last day.
    let outside = "20221010,09:15:01,38445.65,0,0\n20221010,09:15:02,38446.00,0,0\n";
    let dir = folder_of(&scratch, &[("NIFTY", outside)]);
    let request = request();
    let done = pull::ingest::from_dir(
        &dir,
        &store_of(&scratch),
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect("the folder is readable");

    assert_eq!(done.rows_read, 2);
    assert_eq!(done.bars_stored, 0, "nothing survived the window filter");
    assert!(
        done.failures.is_empty(),
        "and nothing FAILED — they were declined"
    );
    assert_eq!(
        done.census.of(DropReason::AfterWindow),
        2,
        "both rows are counted by the reason they were declined for"
    );
    assert!(
        done.balances(),
        "every row is accounted for: none stored, both counted"
    );
}

/// A member whose rows go backwards is refused by the fold rather than sorted.
#[test]
fn a_member_whose_rows_go_backwards_is_refused_by_the_fold() {
    let scratch = Scratch::new("BACKWARDS");
    // File order is the only order there is, and this file's order descends.
    let backwards = "20221003,10:15:02,38446.00,0,0\n20221003,09:15:01,38445.65,0,0\n";
    let dir = folder_of(&scratch, &[("NIFTY", backwards)]);
    let request = request();
    let done = pull::ingest::from_dir(
        &dir,
        &store_of(&scratch),
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect("the folder is readable");

    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    assert!(
        done.failures[0].why.contains("no tiebreaker"),
        "the fold's own refusal survives into the run's report — {}",
        done.failures[0].why
    );
}

/// A row this build cannot put on the paisa grid refuses its member and the
/// refusal keeps the converter's words.
#[test]
fn a_row_that_cannot_be_landed_refuses_only_its_own_member() {
    let scratch = Scratch::new("UNLANDABLE");
    // Decoded to paisa this fits an i64; read as RUPEES and multiplied by a
    // hundred it does not.
    let huge = "20221003,09:15:01,1000000000000000.00,0,0\n";
    let dir = folder_of(&scratch, &[("NIFTY", huge)]);
    let request = request();
    let done = pull::ingest::from_dir(
        &dir,
        &store_of(&scratch),
        plan_over(&request, "NSE", PriceScale::Rupees),
    )
    .expect("the folder is readable");

    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    assert!(
        done.failures[0].why.contains("paisa grid"),
        "the landing refusal's own sentence survives — {}",
        done.failures[0].why
    );
}

/// Bars that cross a month boundary are refused by name rather than filed into
/// whichever month came first.
#[test]
fn a_member_whose_bars_cross_a_month_boundary_is_refused_by_name() {
    let scratch = Scratch::new("SPAN");
    let across = "20221031,09:15:01,38445.65,0,0\n20221101,09:15:01,38446.00,0,0\n";
    let dir = folder_of(&scratch, &[("NIFTY", across)]);
    let request = BarRequest {
        window: Window::new(
            Day::new(2022, 10, 31).expect("a real date"),
            Day::new(2022, 11, 1).expect("a real date"),
        )
        .expect("a forward window"),
        cadence: Cadence::Minute,
    };
    let done = pull::ingest::from_dir(
        &dir,
        &store_of(&scratch),
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect("the folder is readable");

    assert_eq!(
        done.rows_read, 2,
        "both rows survived the window and the fold"
    );
    assert_eq!(done.bars_stored, 0, "and neither was filed");
    assert_eq!(done.failures.len(), 1);
    let why = &done.failures[0].why;
    let october = YearMonth::new(2022, 10).expect("October 2022").to_string();
    let november = YearMonth::new(2022, 11).expect("November 2022").to_string();
    assert!(
        why.contains(&october) && why.contains(&november),
        "the refusal names BOTH months, because splitting is a decision about \
         paths and it is the caller's to make — {why}"
    );
}

/// A path this build cannot form refuses the member and names it.
#[test]
fn a_path_segment_this_build_refuses_stops_the_member_not_the_run() {
    let scratch = Scratch::new("BADPATH");
    let dir = folder_of(&scratch, &[("NIFTY", ONE_ROW)]);
    let request = request();
    let done = pull::ingest::from_dir(
        &dir,
        &store_of(&scratch),
        // An empty exchange segment collapses two directory levels into one and
        // files the bars somewhere nobody looks. `StorePath` refuses it.
        plan_over(&request, "", PriceScale::Paisa),
    )
    .expect("the folder itself is fine");

    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    assert_eq!(done.failures[0].instrument, "NIFTY");
    assert!(
        done.failures[0].why.contains("NIFTY"),
        "the refusal names the member as well as the fault — {}",
        done.failures[0].why
    );
}

/// A store root that is not a directory refuses the member in the store's own
/// words.
#[test]
fn a_store_root_that_is_a_file_refuses_the_member_in_the_stores_words() {
    let scratch = Scratch::new("BADSTORE");
    let dir = folder_of(&scratch, &[("NIFTY", ONE_ROW)]);
    let root = scratch.root.join("STORE");
    fs::write(&root, "NOT A DIRECTORY").expect("a file where a store should be");

    let request = request();
    let done = pull::ingest::from_dir(&dir, &root, plan_over(&request, "NSE", PriceScale::Paisa))
        .expect("the vendor folder is still fine");

    assert_eq!(done.bars_stored, 0);
    assert_eq!(done.failures.len(), 1);
    assert!(
        !done.failures[0].why.is_empty(),
        "the store's refusal is carried, not swallowed"
    );
}

/// Bars that do not follow what the file already holds are refused by the
/// append, and the run reports it as one member's failure.
#[test]
fn bars_that_do_not_follow_the_file_are_refused_by_the_append() {
    let scratch = Scratch::new("REWIND");
    let store = store_of(&scratch);
    let request = request();

    let later = scratch.root.join("LATER");
    fs::create_dir_all(&later).expect("a folder");
    fs::write(later.join("NIFTY.csv"), "20221003,10:15:01,38445.65,0,0\n")
        .expect("the later member");
    let first = pull::ingest::from_dir(
        &later,
        &store,
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect("the folder is readable");
    assert_eq!(first.bars_stored, 1, "the later bar is on disk");
    assert!(first.balances());

    let earlier = scratch.root.join("EARLIER");
    fs::create_dir_all(&earlier).expect("a folder");
    fs::write(
        earlier.join("NIFTY.csv"),
        "20221003,09:15:01,38400.00,0,0\n",
    )
    .expect("the earlier member");
    let second = pull::ingest::from_dir(
        &earlier,
        &store,
        plan_over(&request, "NSE", PriceScale::Paisa),
    )
    .expect("the folder is readable");

    assert_eq!(
        second.bars_stored, 0,
        "a bar that precedes the file's tail is refused, because a format \
         addressed by base + header + index·stride cannot hold it"
    );
    assert_eq!(second.failures.len(), 1);
    assert!(
        !second.failures[0].why.is_empty(),
        "and the store's own refusal is what the run reports"
    );
}
