//! The disk round trip: `store::roundtrip::*`.
//!
//! # What was missing, and why a sample was not enough
//!
//! `docs/04-invariants.md` S-02 says `read(i)` returns the bytes `write(i, b)`
//! wrote **for every *i***, and until this file existed it named
//! `store::proptest::roundtrip`, which is in no file and could not be — there
//! is no `proptest` dependency in this workspace and adding one to satisfy a
//! row would be the tail wagging the dog. What *was* proven was the record
//! codec (S-18, `decode(image(r)) == r` over 4,276 records) and a **sampled**
//! seven indices of a 375-record file
//! (`store::write::a_record_reads_back_at_its_computed_offset`). Neither is the
//! row: one never touches a file, and the other walks 7 of 375 indices.
//!
//! So this file walks **every** committed index of a file that crosses three
//! checksum blocks, three appends and a close-and-reopen, and asserts two
//! different things at each one:
//!
//! * [`BarFile::read_record`] returns the record that was appended, and
//! * the 56 bytes at `Layout::offset_of(i)` in the file **on disk** are
//!   `Bar::image()` of that record — the bytes, which is what the row says.
//!
//! The second assertion is the one a codec test cannot make. A writer that
//! encoded correctly and landed the record at the wrong offset would satisfy
//! the first for a while: every read would go to the same wrong place.
//!
//! # The sentinel is walked, not assumed
//!
//! `CLAUDE.md` §7 makes `i64::MIN` the open-interest null and zero a real
//! measurement. Both appear in this file's records at known indices, so the
//! round trip carries the distinction through an `fsync` and a reopen rather
//! than only through the in-memory codec (S-08).
//!
//! # What this does not prove
//!
//! Nothing here crashes the process, pulls the power or fills the disk. The
//! file is closed and reopened, which re-reads the header region from the
//! bytes, but every byte still passed through one process's page cache.
//! Durability across a crash is S-04 and is not measured anywhere in this
//! repository.

// A test that asserts nothing is banned, and a test that cannot fail loudly is
// a test that asserts nothing. These allow the harness to panic on a broken
// invariant instead of threading `Result` through every assertion.
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
use store::file::{Appended, BarFile, StoreError};
use store::format::{Bar, OI_NULL, RECORD_LEN};
use store::layout::Layout;
use store::path::{FileKind, PathParts, StorePath, Timeframe, YearMonth};

// ===========================================================================
// Scratch
// ===========================================================================

/// Distinguishes two scratch roots taken in the same process.
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
            "brutex-store-roundtrip-{}-{tag}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("scratch root");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leaked scratch directory must never fail a test run.
        drop(fs::remove_dir_all(&self.root));
    }
}

// ===========================================================================
// Fixtures
// ===========================================================================

/// The open of the first one-minute bar of 2024-06-03, in microseconds.
const T0: i64 = 1_717_386_300_000_000;
/// One minute, in microseconds.
const MINUTE: i64 = 60_000_000;
/// The symbol id this file opens with.
const SYMBOL: u32 = 26_000;
/// How many records the walk covers.
///
/// A block holds 73 records, so this crosses two interior block boundaries and
/// ends inside a partial tail block — the three states a record can be in.
const RECORDS: i64 = 160;

/// The `index`-th record, with every field a different function of the index.
///
/// Every field differs from every other at every index, so a decoder that read
/// the right bytes into the wrong field would change the value rather than
/// reproduce it. `open_interest` walks the three cases `CLAUDE.md` §7
/// distinguishes: the null sentinel, a real zero, and an ordinary count.
fn record(index: i64) -> Bar {
    let open_interest = match index % 3 {
        0 => OI_NULL,
        1 => 0,
        _ => 900_000 + index,
    };
    Bar {
        ts_micros: T0 + index * MINUTE,
        open: 2_345_600 + index,
        high: 2_345_900 + index,
        low: 2_345_100 + index,
        close: 2_345_700 + index,
        volume: 1_000 + index * 7,
        open_interest,
    }
}

fn parts() -> PathParts<'static> {
    PathParts {
        vendor: Vendor::Groww,
        exchange: "NSE",
        segment: "INDEX",
        symbol: "NIFTY",
        timeframe: Timeframe::MINUTE_1,
        month: YearMonth::new(2024, 6).expect("2024-06"),
        file: FileKind::Bars,
    }
}

fn bars_path() -> StorePath<'static> {
    StorePath::new(parts()).expect("a legal path")
}

fn open(root: &Path) -> BarFile {
    BarFile::open_or_create(root, bars_path(), SYMBOL).expect("the month opens")
}

/// The whole file, as bytes.
fn image(root: &Path) -> Vec<u8> {
    fs::read(bars_path().to_path_buf(root)).expect("the bar file")
}

/// Asserts every committed index reads back its record, twice over.
///
/// Once through [`BarFile::read_record`], and once against the bytes the file
/// holds at the offset the geometry computes. `written` is the source of truth
/// the file is compared to, so a defect that corrupted both the write and the
/// read the same way still fails here.
fn walk_every_index(file: &BarFile, root: &Path, written: &[Bar]) {
    let bytes = image(root);
    let n = u64::try_from(written.len()).expect("a length that fits");
    assert_eq!(file.records(), n, "the counter is what was appended");

    for (ordinal, want) in written.iter().enumerate() {
        let index = u64::try_from(ordinal).expect("an index that fits");
        assert_eq!(
            file.read_record(index),
            Ok(*want),
            "record {index} did not read back"
        );

        let at = usize::try_from(Layout::V2.offset_of(index).expect("an offset"))
            .expect("an offset that fits");
        assert_eq!(
            &bytes[at..at + RECORD_LEN],
            &want.image()[..],
            "record {index} is not the bytes that were written, at its own offset"
        );
    }

    assert_eq!(
        file.read_record(n),
        Err(StoreError::NotCommitted {
            index: n,
            n_valid: n
        }),
        "one past the end is refused rather than answered with zeros"
    );
}

// ===========================================================================
// S-02
// ===========================================================================

#[test]
fn every_committed_index_returns_the_record_that_was_written() {
    let scratch = Scratch::new("every");
    let written: Vec<Bar> = (0..RECORDS).map(record).collect();

    let mut file = open(scratch.root());

    // Three appends rather than one, so the walk crosses two commits as well
    // as two block boundaries: an index written by the first append is read
    // after the third has moved the counter and rewritten the header slot.
    let mut done = 0usize;
    for count in [37usize, 60, 63] {
        let batch = &written[done..done + count];
        let first = u64::try_from(done).expect("an index that fits");
        let after = u64::try_from(done + count).expect("a count that fits");
        assert_eq!(
            file.append(batch),
            Ok(Appended::Committed {
                first_index: first,
                n_valid: after,
            }),
            "the batch starting at {first} did not commit"
        );
        done += count;
    }
    assert_eq!(
        done,
        usize::try_from(RECORDS).expect("a count that fits"),
        "the three batches are the whole walk"
    );

    walk_every_index(&file, scratch.root(), &written);

    // The same walk again after the handle is closed and the header is
    // re-read from the file's own bytes. A reader that only ever saw the
    // writer's in-memory header would pass the walk above and fail here.
    drop(file);
    let reopened = open(scratch.root());
    walk_every_index(&reopened, scratch.root(), &written);
}

#[test]
fn the_open_interest_sentinel_survives_the_disk_and_is_not_a_zero() {
    // S-08's round-trip half, taken through a file rather than through the
    // codec: `i64::MIN` means "the vendor sent none" and `0` means "zero",
    // and a file that confused them would report open interest on an index.
    let scratch = Scratch::new("sentinel");
    let written: Vec<Bar> = (0..6i64).map(record).collect();

    let mut file = open(scratch.root());
    assert_eq!(
        file.append(&written),
        Ok(Appended::Committed {
            first_index: 0,
            n_valid: 6
        })
    );
    drop(file);

    let reopened = open(scratch.root());
    assert_eq!(
        reopened.read_record(0).expect("record 0").open_interest,
        OI_NULL
    );
    assert_eq!(reopened.read_record(1).expect("record 1").open_interest, 0);
    assert_eq!(
        reopened.read_record(2).expect("record 2").open_interest,
        900_002
    );
    assert_ne!(
        reopened.read_record(0).expect("record 0").open_interest,
        reopened.read_record(1).expect("record 1").open_interest,
        "the null sentinel and a measured zero are two different answers"
    );
}
