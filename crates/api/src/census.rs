//! What the store holds, read from the counter file and **never** from a
//! directory.
//!
//! # The one rule this module exists to obey
//!
//! `docs/07-o1-architecture.md` law 3: *never scan to answer a question.*
//! "How many months of NIFTY do I hold" against a bar tree of ~248,000 files is
//! ~248,000 `stat` calls, it gets slower with every ingest, and putting it
//! behind an HTTP request means a page whose cost grows with the thing it
//! describes. [`pull::manifest::Manifest`] is the counter maintained on write;
//! this module reads it and probes it, and there is no `read_dir` anywhere
//! under `crates/api`.
//!
//! # Read once, rendered many times
//!
//! Loading a manifest decodes and checksum-verifies every committed entry, so
//! it is O(entries) — once. `crates/api/src/server.rs` reads the masters at
//! startup for exactly this reason and measured **150 ms per request** for
//! getting it wrong; the manifests are read at startup beside them, and every
//! `/store` request is field reads and hash probes off that.
//!
//! # An absent manifest is not an error
//!
//! Before the first ingest there is no file, and that is the ordinary state of
//! a fresh install. It renders as a named absence — the path that was looked
//! for — rather than as a failure, and rather than as zeros, because "nothing
//! has been ingested" and "the counter says zero" are different claims.
//! A file that exists and will not load is a third thing again, and is loud.

use std::path::{Path, PathBuf};

use brutex_core::instrument::{Exchange, InstrumentKey, Kind, Segment};
use brutex_core::symbol::Symbol;
use brutex_core::vendor::Vendor;
use pull::manifest::{ENTRY_STRIDE, EntryKey, HEADER_LEN, MAX_ENTRIES, Manifest, manifest_path};
use pull::session::Day;
use store::path::{Timeframe, YearMonth};

/// [`pull::manifest::HEADER_LEN`] in the width a slice is split at.
///
/// Written as a literal in both widths rather than converted, for the reason
/// `pull::manifest` gives about its own pair: there is no cast to deny and no
/// fallible conversion whose failure arm no test could ever reach. The
/// assertion below keeps the two honest.
const HEADER_LEN_USIZE: usize = 32_768;
const _: () = assert!(HEADER_LEN == 32_768 && HEADER_LEN_USIZE == 32_768);

/// The largest manifest this reader will hold in memory.
///
/// # Why a bound exists at all
///
/// [`read_vendor`] used to call `std::fs::read` with nothing in front of it, so
/// the size of the allocation was the size of whatever was at that path. Every
/// refusal `pull::manifest` owns — a counter past [`MAX_ENTRIES`], a region too
/// small for its counter, an entry that fails its checksum — is decided *after*
/// the bytes are already in memory, which is far too late to be a bound. That
/// is the same defect `crates/api/src/master.rs` records as D-0033 and fixes
/// with `MAX_MASTER_BYTES`, one directory away, and the manifest reader did not
/// have it.
///
/// # Why this number and not a round one
///
/// It is not chosen, it is **derived**: `HEADER_LEN + MAX_ENTRIES ×
/// ENTRY_STRIDE` is the largest file the writer can produce, because
/// `ManifestHeader::advance` refuses a counter past [`MAX_ENTRIES`] and every
/// entry occupies exactly [`ENTRY_STRIDE`] bytes. 32,768 + 2,097,152 × 64 =
/// 134,250,496 bytes. A file larger than that is not a manifest this build
/// could have written, so refusing it loses nothing and the refusal names both
/// numbers so an operator sees what to raise.
pub const MAX_MANIFEST_BYTES: u64 = HEADER_LEN + MAX_ENTRIES * ENTRY_STRIDE;

/// The derivation above, checked at compile time rather than in a comment.
const _: () = assert!(MAX_MANIFEST_BYTES == 134_250_496);

/// How many months back the coverage grid reaches.
///
/// Three years. The grid is `instruments × months`, and both factors are
/// bounded by construction, so the row count is known before a single probe is
/// issued and the page can be paged by arithmetic rather than by collecting.
pub const GRID_MONTHS: usize = 36;

/// One vendor's census, as this process found it.
#[derive(Debug, Clone)]
pub enum Census {
    /// No file at that path. The ordinary state before the first ingest.
    Absent,
    /// A file that exists and could not be turned into a census.
    ///
    /// An I/O failure, or a manifest this build refuses — a header that does
    /// not decode, a counter the entry region cannot support, an entry that
    /// fails its own checksum.
    Unreadable {
        /// What refused it, in the refusal's own words.
        reason: String,
    },
    /// A census that loaded.
    Held {
        /// The counters, and the index a probe goes through.
        manifest: Box<Manifest>,
    },
}

impl Census {
    /// Which of the three states this is, as one stable word.
    ///
    /// A total match rather than a `matches!` at each call site: three states
    /// named once is three states that cannot be confused, and a fourth would
    /// fail to compile here rather than fall through a wildcard somewhere else.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match *self {
            Self::Absent => "absent",
            Self::Unreadable { .. } => "unreadable",
            Self::Held { .. } => "held",
        }
    }
}

/// One vendor's manifest, and where it was looked for.
#[derive(Debug, Clone)]
pub struct VendorCensus {
    /// Whose census this is.
    pub vendor: Vendor,
    /// The path that was looked at, named so an absence is actionable.
    pub path: PathBuf,
    /// What was found there.
    pub state: Census,
}

impl VendorCensus {
    /// The three counters, if this census loaded: months, rows, entries.
    ///
    /// One field read each — `docs/04-invariants.md` M-series, and the whole
    /// point of layer 13.
    #[must_use]
    pub fn counters(&self) -> Option<(u64, u64, u64)> {
        match self.state {
            Census::Held { ref manifest } => {
                Some((manifest.keys(), manifest.total_rows(), manifest.entries()))
            }
            Census::Absent | Census::Unreadable { .. } => None,
        }
    }

    /// Which commit this census is, if it loaded.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        match self.state {
            Census::Held { ref manifest } => Some(manifest.header().generation),
            Census::Absent | Census::Unreadable { .. } => None,
        }
    }

    /// What loading had to step over, if anything.
    ///
    /// `Some` means the file is damaged and this census is what could still be
    /// recovered from it — the counters below are then the *recovered*
    /// generation's, and a month a later generation had committed is not in
    /// them. [`pull::manifest::Manifest::degraded_reason`] says so at the
    /// source; this carries it to the page.
    #[must_use]
    pub fn degraded(&self) -> Option<String> {
        match self.state {
            Census::Held { ref manifest } => manifest.degraded_reason().map(|why| why.to_string()),
            Census::Absent | Census::Unreadable { .. } => None,
        }
    }

    /// Whether an operator has to be told about this census.
    #[must_use]
    pub fn is_loud(&self) -> bool {
        !matches!(self.state, Census::Held { .. }) || self.degraded().is_some()
    }

    /// The line an operator reads first.
    ///
    /// `CLAUDE.md` §4: degrade loudly and name the reason. An absent file names
    /// the path, an unreadable one names the refusal, and a recovered one says
    /// what it stepped over.
    #[must_use]
    pub fn note(&self) -> String {
        match self.state {
            Census::Absent => format!(
                "{}: UNAVAILABLE — no manifest at {}. Nothing has been ingested \
                 for this vendor, or the store root is not the one you think.",
                self.vendor.as_str(),
                self.path.display()
            ),
            Census::Unreadable { ref reason } => format!(
                "{} UNREADABLE · {} — at {}",
                self.vendor.as_str(),
                reason,
                self.path.display()
            ),
            Census::Held { ref manifest } => manifest.degraded_reason().map_or_else(
                || {
                    format!(
                        "{}: {} month(s), {} row(s), generation {}",
                        self.vendor.as_str(),
                        manifest.keys(),
                        manifest.total_rows(),
                        manifest.header().generation
                    )
                },
                |why| {
                    format!(
                        "{} DEGRADED CENSUS · recovered generation {} after stepping \
                         over: {why}",
                        self.vendor.as_str(),
                        manifest.header().generation
                    )
                },
            ),
        }
    }

    /// How many rows this vendor holds for one month of one instrument.
    ///
    /// One hash probe. `None` is "this census does not hold that month", which
    /// includes the case where there is no census at all.
    #[must_use]
    pub fn rows_for(&self, key: &EntryKey) -> Option<u64> {
        match self.state {
            Census::Held { ref manifest } => manifest.entry(key).map(|e| e.rows),
            Census::Absent | Census::Unreadable { .. } => None,
        }
    }
}

/// One vendor's manifest, read off disk.
///
/// Never fails: every outcome is a state the page renders, because a `/store`
/// page that 500s when nothing has been ingested yet is a page that is broken
/// on a fresh install.
#[must_use]
pub fn read_vendor(root: &Path, vendor: Vendor) -> VendorCensus {
    let path = manifest_path(root, vendor);
    let state = match sized(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Census::Absent,
        Err(e) => Census::Unreadable {
            reason: e.to_string(),
        },
        Ok(Err(reason)) => Census::Unreadable { reason },
        Ok(Ok(bytes)) => {
            // A file shorter than the header region is not sliced past its end;
            // it is handed over as it is, and the manifest decoder refuses it
            // by name. Splitting with a checked split rather than a range keeps
            // `clippy::indexing_slicing` satisfied without a branch that says
            // "this cannot happen".
            let (header, entries) = bytes
                .split_at_checked(HEADER_LEN_USIZE)
                .unwrap_or((bytes.as_slice(), &[]));
            match Manifest::open(vendor, header, entries) {
                Ok(manifest) => Census::Held {
                    manifest: Box::new(manifest),
                },
                Err(why) => Census::Unreadable {
                    reason: why.to_string(),
                },
            }
        }
    };
    VendorCensus {
        vendor,
        path,
        state,
    }
}

/// The bytes at `path`, if this reader may hold them.
///
/// Three outcomes, and they are deliberately three: `Err` is the file system
/// speaking — including the `NotFound` that is the ordinary pre-ingest state —
/// `Ok(Err)` is this reader refusing a file that exists, and `Ok(Ok)` is the
/// bytes. Collapsing the first two would make an absent manifest and an absurd
/// one the same fact on the page, which is the distinction this whole module is
/// built around.
///
/// THE SIZE IS CHECKED BEFORE THE READ. `read` on a file this process cannot
/// hold is not an error it can report — it is an allocator failure or an OOM
/// kill, and neither reaches the operator as "that manifest is too big".
fn sized(path: &Path) -> std::io::Result<Result<Vec<u8>, String>> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_MANIFEST_BYTES {
        return Ok(Err(format!(
            "{size} bytes; the largest manifest this build can write is \
             {MAX_MANIFEST_BYTES} and this reader refuses more"
        )));
    }
    Ok(Ok(std::fs::read(path)?))
}

/// Every vendor's manifest, read off disk once.
#[must_use]
pub fn read_all(root: &Path) -> Vec<VendorCensus> {
    Vendor::ALL
        .into_iter()
        .map(|v| read_vendor(root, v))
        .collect()
}

/// The month `back` months before the one `today` falls in.
///
/// Arithmetic on a month ordinal, not a loop over a calendar: `back = 0` is
/// this month and `back = 35` is three years ago, and both cost the same.
/// `None` before 1970, which is the one direction the store cannot address.
#[must_use]
pub fn month_back(today: Day, back: usize) -> Option<YearMonth> {
    // A `Day` is validated into 1970..=9999, so the month ordinal is at most
    // 9,999·12 + 11 = 119,999. It cannot overflow a `u32`, and writing it as a
    // checked multiply would add a failure arm no input could ever reach — the
    // kind of branch `CLAUDE.md` §4 calls a test that asserts nothing.
    let ordinal = u32::from(today.year()) * 12 + u32::from(today.month() - 1);
    let moved = u32::try_from(back)
        .ok()
        .and_then(|b| ordinal.checked_sub(b))?;
    // `moved <= ordinal <= 119,999`, so the year is at most 9,999 and the month
    // is 1..=12 — both inside their types by construction. `YearMonth::new`
    // re-checks them and is the ONE refusal: it is what turns a month before
    // 1970, which the store cannot address, into `None`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "moved is bounded above by 119,999 because Day is validated \
                  into 1970..=9999, so moved/12 is at most 9,999 and fits u16, \
                  and moved%12 + 1 is 1..=12 and fits u8. YearMonth::new \
                  re-checks both and the tests drive its refusal."
    )]
    let (year, month) = ((moved / 12) as u16, (moved % 12 + 1) as u8);
    YearMonth::new(year, month).ok()
}

/// The manifest key for one instrument's month of one-minute bars.
///
/// `None` for an instrument the manifest cannot key — an option or a future
/// carries an expiry the manifest's key does not, and the coverage grid is
/// about spot series.
#[must_use]
pub fn entry_key(key: &InstrumentKey, month: YearMonth) -> Option<EntryKey> {
    match key.kind {
        Kind::Index | Kind::Equity => Some(EntryKey {
            exchange: key.exchange,
            segment: key.segment,
            symbol: key.underlying,
            timeframe: Timeframe::MINUTE_1,
            month,
        }),
        Kind::Future { .. } | Kind::Option { .. } => None,
    }
}

/// One cell of the coverage grid: an instrument, a month, and each vendor's
/// row count for it.
#[derive(Debug, Clone)]
pub struct Coverage {
    /// The instrument the row is about.
    pub key: InstrumentKey,
    /// The month.
    pub month: YearMonth,
    /// One entry per vendor, in [`Vendor::ALL`] order. `None` is "not held".
    pub rows: Vec<(Vendor, Option<u64>)>,
}

impl Coverage {
    /// Whether any vendor holds this month at all.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.rows.iter().any(|&(_, n)| n.is_some())
    }
}

/// The coverage rows one page of the grid shows.
///
/// # Why this is arithmetic and not a filter
///
/// The grid is `instruments × months` in a fixed order, so the row at ordinal
/// `i` is instrument `i / GRID_MONTHS` at `i % GRID_MONTHS` months back. That
/// makes the offset a division rather than a walk, so a request for page 5
/// touches the 200 rows of page 5 and nothing else — `docs/07-o1-
/// architecture.md` layer 12, and the same bound `crates/api/src/render.rs`
/// states for the instruments table.
///
/// `skip` and `take` are ordinals into that product, not indices into a
/// collection: nothing is built for the rows that are not shown.
#[must_use]
pub fn coverage_page(
    instruments: &[InstrumentKey],
    censuses: &[VendorCensus],
    today: Day,
    skip: usize,
    take: usize,
) -> Vec<Coverage> {
    let mut out = Vec::with_capacity(take);
    for ordinal in skip..skip.saturating_add(take) {
        let Some(key) = instruments.get(ordinal / GRID_MONTHS) else {
            break;
        };
        let Some(month) = month_back(today, ordinal % GRID_MONTHS) else {
            continue;
        };
        let Some(entry) = entry_key(key, month) else {
            continue;
        };
        out.push(Coverage {
            key: *key,
            month,
            rows: censuses
                .iter()
                .map(|c| (c.vendor, c.rows_for(&entry)))
                .collect(),
        });
    }
    out
}

/// How many rows the whole grid has.
///
/// A multiplication, so the pager knows the last page without building it.
#[must_use]
pub fn grid_rows(instruments: usize) -> usize {
    instruments.saturating_mul(GRID_MONTHS)
}

/// The instruments the coverage grid is built over, in a fixed order.
///
/// The index series, which is what the store holds spot bars for. Sorted, so
/// the grid is byte-identical between reloads — a `HashMap` iteration order is
/// not, and the ordinal arithmetic above would then address a different row
/// every time.
#[must_use]
pub fn grid_instruments<'a>(keys: impl Iterator<Item = &'a InstrumentKey>) -> Vec<InstrumentKey> {
    let mut out: Vec<InstrumentKey> = keys
        .filter(|k| k.segment == Segment::Index && k.kind == Kind::Index)
        .copied()
        .collect();
    out.sort_unstable();
    out
}

/// The two instruments the engine sweeps, for a page that has no universe.
///
/// `CLAUDE.md` §1 fixes the surface at exactly these two. Used when the masters
/// were never read, so the coverage grid still shows the two rows that matter
/// rather than nothing at all.
#[must_use]
pub fn swept_instruments() -> Vec<InstrumentKey> {
    ["BANKNIFTY", "NIFTY"]
        .into_iter()
        .filter_map(|s| Symbol::new(s).ok())
        .map(|underlying| InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Index,
            underlying,
            kind: Kind::Index,
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::*;
    use pull::manifest::{Entry, ManifestHeader};

    fn day(y: u16, m: u8, d: u8) -> Day {
        Day::new(y, m, d).expect("a real date")
    }

    fn nifty() -> InstrumentKey {
        InstrumentKey::index(Exchange::Nse, "NIFTY").expect("valid")
    }

    /// A directory to put a manifest in, named after the test.
    fn root(name: &str) -> PathBuf {
        let dir = crate::scratch::path(&format!("census-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("manifest")).expect("mkdir");
        dir
    }

    /// Writes `bytes` as one vendor's manifest under `dir`.
    fn write(dir: &Path, vendor: Vendor, bytes: &[u8]) {
        std::fs::write(manifest_path(dir, vendor), bytes).expect("write");
    }

    /// A manifest file holding exactly the header image given, and `entries`.
    fn file(header: &[u8; 64], entries: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_LEN_USIZE];
        out.get_mut(..64).expect("room").copy_from_slice(header);
        out.extend_from_slice(entries);
        out
    }

    #[test]
    fn an_absent_manifest_names_the_path_and_is_never_zeros() {
        // The ordinary state of a fresh install. It must not read as a census
        // of nothing, because "nothing has been ingested" and "the counter says
        // zero" are different claims.
        let dir = root("absent");
        let census = read_vendor(&dir, Vendor::Groww);
        assert_eq!(census.state.name(), "absent");
        assert_eq!(census.counters(), None, "no counters, not zeroed counters");
        assert_eq!(census.generation(), None);
        assert_eq!(census.degraded(), None);
        assert!(census.is_loud());
        let note = census.note();
        assert!(note.contains("UNAVAILABLE"), "{note}");
        assert!(note.contains("groww"), "{note}");
        assert!(
            note.contains(&census.path.display().to_string()),
            "the path that was looked at is named: {note}"
        );
        // And a probe against an absent census answers "not held", not zero.
        let key = entry_key(&nifty(), YearMonth::new(2026, 7).expect("valid")).expect("keyable");
        assert_eq!(census.rows_for(&key), None);
    }

    #[test]
    fn a_manifest_whose_counters_are_zero_reads_as_zero_and_is_not_loud() {
        // A genesis header with no entries: the store exists, nothing is in it.
        // This is a real answer, and it is a DIFFERENT answer from an absent
        // file — so it is quiet, and its counters are readable.
        let dir = root("zero");
        let header = ManifestHeader::genesis(Vendor::Dhan);
        write(&dir, Vendor::Dhan, &file(&header.image(), &[]));

        let census = read_vendor(&dir, Vendor::Dhan);
        assert_eq!(
            census.counters(),
            Some((0, 0, 0)),
            "zero months, zero rows, zero entries — read, not assumed"
        );
        assert_eq!(census.generation(), Some(0));
        assert_eq!(census.degraded(), None);
        assert!(!census.is_loud(), "an empty store is not a failure");
        let note = census.note();
        assert!(note.contains("0 month(s), 0 row(s)"), "{note}");
        assert!(!note.contains("UNAVAILABLE"), "{note}");
    }

    #[test]
    fn a_manifest_that_will_not_load_is_loud_and_names_the_refusal() {
        let dir = root("unreadable");
        // A file of the right length whose header slots are zeros: the magic
        // does not match, so nothing about it decodes.
        write(&dir, Vendor::Groww, &vec![0u8; HEADER_LEN_USIZE]);
        let census = read_vendor(&dir, Vendor::Groww);
        assert_eq!(census.state.name(), "unreadable");
        assert_eq!(census.counters(), None);
        assert_eq!(census.generation(), None);
        assert!(census.is_loud());
        let note = census.note();
        assert!(note.contains("UNREADABLE"), "{note}");
        assert!(note.contains("groww"), "{note}");

        // A file too short to hold even one header slot is refused by the same
        // path rather than sliced past its end.
        let dir = root("short");
        write(&dir, Vendor::Groww, b"BRUTEXM1");
        assert_eq!(read_vendor(&dir, Vendor::Groww).state.name(), "unreadable");
    }

    #[test]
    fn a_manifest_larger_than_this_reader_holds_is_refused_before_it_is_read() {
        // `std::fs::read` on a file this process cannot hold is not an error it
        // can report -- it is an allocator failure or an OOM kill. Every
        // refusal `pull::manifest` owns is decided AFTER the bytes are in
        // memory, so the size is checked first and the refusal names both
        // numbers.
        //
        // Sparse: `set_len` allocates no blocks, so this costs no disk, and the
        // file is never read -- which is exactly the property under test.
        let dir = root("toobig");
        let p = manifest_path(&dir, Vendor::Groww);
        let f = std::fs::File::create(&p).expect("create");
        f.set_len(MAX_MANIFEST_BYTES + 1).expect("set_len");
        drop(f);
        let census = read_vendor(&dir, Vendor::Groww);
        assert_eq!(
            census.state.name(),
            "unreadable",
            "not absent, and not held"
        );
        let note = census.note();
        assert!(
            note.contains(&(MAX_MANIFEST_BYTES + 1).to_string())
                && note.contains(&MAX_MANIFEST_BYTES.to_string()),
            "the refusal names what arrived and what the bound is: {note}"
        );
        assert!(note.contains("UNREADABLE"), "and it is loud: {note}");
        assert!(census.is_loud());
        assert_eq!(
            census.counters(),
            None,
            "no counters, never zeroed counters"
        );

        // EXACTLY AT THE BOUND IS NOT OVER IT. A file of precisely
        // `MAX_MANIFEST_BYTES` is read and then refused by the manifest decoder
        // for its zeroed header -- a different reason, which proves the size
        // arm did not fire. The boundary is `>` and not `>=`.
        let f = std::fs::File::create(&p).expect("create");
        f.set_len(MAX_MANIFEST_BYTES).expect("set_len");
        drop(f);
        let at_bound = read_vendor(&dir, Vendor::Groww).note();
        assert!(at_bound.contains("UNREADABLE"), "{at_bound}");
        assert!(
            !at_bound.contains("this reader refuses more"),
            "the size arm fired at the bound itself: {at_bound}"
        );
        std::fs::remove_file(&p).expect("cleanup");
    }

    #[test]
    fn a_recovered_census_says_what_it_stepped_over_and_stays_loud() {
        // A crash between a data write and its flush leaves one header slot
        // damaged and the other intact. The census loads from the survivor —
        // which is the whole point of alternating slots — and it must say so:
        // its counters are the RECOVERED generation's, and a month a later
        // generation had committed is not in them.
        let dir = root("degraded");
        // Slot 0: a real header image with one covered byte flipped, so its
        // checksum no longer verifies and it is stepped over.
        let mut damaged = ManifestHeader::genesis(Vendor::Groww).image();
        damaged[16] ^= 0xFF;
        // Slot 1: generation 1, valid, counting nothing. Generation 1 belongs
        // in slot 1 because writes alternate on `generation % SLOT_COUNT`.
        let good = ManifestHeader {
            generation: 1,
            ..ManifestHeader::genesis(Vendor::Groww)
        }
        .image();

        let mut bytes = vec![0u8; HEADER_LEN_USIZE];
        bytes.get_mut(..64).expect("room").copy_from_slice(&damaged);
        bytes
            .get_mut(16_384..16_448)
            .expect("room")
            .copy_from_slice(&good);
        write(&dir, Vendor::Groww, &bytes);

        let census = read_vendor(&dir, Vendor::Groww);
        assert_eq!(
            census.counters(),
            Some((0, 0, 0)),
            "the survivor's counters"
        );
        assert_eq!(census.generation(), Some(1));
        let why = census.degraded().expect("something was stepped over");
        assert!(!why.is_empty(), "and it says what: {why}");
        assert!(census.is_loud(), "a recovered census is not a clean one");
        let note = census.note();
        assert!(note.contains("DEGRADED CENSUS"), "{note}");
        assert!(note.contains("recovered generation 1"), "{note}");
        assert!(note.contains(&why), "the refusal itself is carried: {note}");
    }

    #[test]
    fn a_manifest_path_that_cannot_be_read_at_all_is_unreadable_not_absent() {
        // A directory where a file should be. Not `NotFound`, so it must not be
        // reported as "nothing has been ingested" — that would send an operator
        // to run an ingest instead of to fix the path.
        let dir = root("notafile");
        std::fs::create_dir_all(manifest_path(&dir, Vendor::Dhan)).expect("mkdir");
        let census = read_vendor(&dir, Vendor::Dhan);
        assert_eq!(
            census.state.name(),
            "unreadable",
            "a directory is not an absence"
        );
        assert!(census.is_loud());
        let note = census.note();
        assert!(note.contains("UNREADABLE"), "{note}");
    }

    #[test]
    fn a_census_that_holds_a_month_answers_a_probe_in_one_read() {
        let dir = root("held");
        let month = YearMonth::new(2026, 7).expect("valid");
        let key = entry_key(&nifty(), month).expect("keyable");
        let entry = Entry {
            key,
            rows: 8_250,
            first_ts_micros: 1,
            last_ts_micros: 2,
        };
        let header = ManifestHeader {
            generation: 4,
            n_valid: 1,
            n_keys: 1,
            total_rows: 8_250,
            ..ManifestHeader::genesis(Vendor::Groww)
        };
        write(&dir, Vendor::Groww, &file(&header.image(), &entry.image()));

        let census = read_vendor(&dir, Vendor::Groww);
        assert_eq!(census.state.name(), "held");
        assert_eq!(census.counters(), Some((1, 8_250, 1)));
        assert_eq!(census.generation(), Some(4));
        assert!(!census.is_loud());
        assert_eq!(census.rows_for(&key), Some(8_250));

        // A month it does not hold answers "not held" rather than zero.
        let other = entry_key(&nifty(), YearMonth::new(2026, 6).expect("valid")).expect("keyable");
        assert_eq!(census.rows_for(&other), None);

        // And it reaches the grid.
        let grid = coverage_page(&[nifty()], &[census], day(2026, 7, 15), 0, 3);
        assert_eq!(grid.len(), 3, "this month and the two before it");
        assert_eq!(grid[0].month, month);
        assert_eq!(grid[0].rows, vec![(Vendor::Groww, Some(8_250))]);
        assert!(grid[0].is_held());
        assert!(!grid[1].is_held(), "June is not held, and says so");
    }

    #[test]
    fn every_vendor_is_looked_for_and_the_month_arithmetic_walks_backwards() {
        let dir = root("all");
        let all = read_all(&dir);
        assert_eq!(all.len(), Vendor::ALL.len());
        assert_eq!(all[0].vendor, Vendor::Groww);
        assert_eq!(all[1].vendor, Vendor::Dhan);

        // Zero months back is this month; the year boundary is arithmetic.
        let jan = day(2026, 1, 15);
        assert_eq!(
            month_back(jan, 0).expect("this month").to_string(),
            "2026-01"
        );
        assert_eq!(
            month_back(jan, 1).expect("last month").to_string(),
            "2025-12"
        );
        assert_eq!(
            month_back(jan, 13).expect("a year back").to_string(),
            "2024-12"
        );
        assert_eq!(
            month_back(day(2026, 12, 1), GRID_MONTHS - 1)
                .expect("three years")
                .to_string(),
            "2024-01"
        );
        // Before 1970 there is no month the store can address, and it is `None`
        // rather than a wrapped date. Three ways to fall off that edge, and all
        // three answer the same way: the month is refused, never invented.
        assert_eq!(month_back(day(1970, 1, 1), 1), None, "one month too far");
        assert_eq!(
            month_back(day(1970, 1, 1), 30_000),
            None,
            "past the epoch by more months than there have been"
        );
        assert_eq!(
            month_back(day(2026, 8, 7), usize::MAX),
            None,
            "a count that does not even fit the ordinal's width"
        );
        assert_eq!(
            month_back(day(1970, 1, 1), 0)
                .expect("the first")
                .to_string(),
            "1970-01"
        );
    }

    #[test]
    fn the_grid_is_addressed_by_arithmetic_and_pages_without_building_the_rest() {
        let dir = root("grid");
        let census = vec![read_vendor(&dir, Vendor::Groww)];
        let two = swept_instruments();
        assert_eq!(two.len(), 2, "the engine surface is exactly two");
        assert_eq!(two[0].underlying.as_str(), "BANKNIFTY");
        assert_eq!(two[1].underlying.as_str(), "NIFTY");
        assert_eq!(grid_rows(two.len()), 2 * GRID_MONTHS);
        assert_eq!(grid_rows(0), 0);

        let today = day(2026, 8, 7);
        // The first page of the second instrument starts exactly at its own
        // ordinal: the row at `GRID_MONTHS` is instrument 1, month 0.
        let second = coverage_page(&two, &census, today, GRID_MONTHS, 1);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].key, two[1]);
        assert_eq!(second[0].month.to_string(), "2026-08");
        // And the row before it is instrument 0's oldest month.
        let boundary = coverage_page(&two, &census, today, GRID_MONTHS - 1, 1);
        assert_eq!(boundary[0].key, two[0]);
        assert_eq!(boundary[0].month.to_string(), "2023-09");

        // Asking past the end of the product yields nothing rather than
        // panicking: the pager clamps, but the arithmetic must survive being
        // asked anyway.
        assert!(coverage_page(&two, &census, today, 10_000, 200).is_empty());
        assert!(coverage_page(&[], &census, today, 0, 200).is_empty());
        // A `take` of nothing is nothing, not everything.
        assert!(coverage_page(&two, &census, today, 0, 0).is_empty());

        // A month older than the calendar can name is skipped rather than
        // faked — the grid is honest about reaching the edge.
        let early = coverage_page(&two, &census, day(1970, 2, 1), 0, GRID_MONTHS);
        assert_eq!(early.len(), 2, "only 1970-02 and 1970-01 exist");
    }

    #[test]
    fn the_grid_is_built_over_index_series_only_and_in_a_fixed_order() {
        let equity = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Cash,
            underlying: Symbol::new("RELIANCE").expect("valid"),
            kind: Kind::Equity,
        };
        let banknifty = InstrumentKey::index(Exchange::Nse, "BANKNIFTY").expect("valid");
        // BOTH HALVES OF THE FILTER ARE LOAD-BEARING, and this is the row that
        // proves it: a key whose segment says index and whose kind says equity
        // is a malformed identity, and it belongs in the coverage grid no more
        // than a share does. Without it the `&&` could be an `||` with the
        // suite green.
        let mismatched = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Index,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Equity,
        };
        let keys = [nifty(), equity, banknifty, mismatched];
        let grid = grid_instruments(keys.iter());
        assert_eq!(
            grid.len(),
            2,
            "the equity is not an index series, and neither is a key that \
             claims to be both: {grid:?}"
        );
        assert!(
            !grid.contains(&mismatched),
            "an index segment with an equity kind is not an index series"
        );
        assert!(!grid.contains(&equity));
        assert!(grid.windows(2).all(|w| w[0] < w[1]), "sorted: {grid:?}");

        // An option carries an expiry the manifest key does not, so it is not
        // keyable and the grid never asks for it.
        let month = YearMonth::new(2026, 7).expect("valid");
        assert!(entry_key(&equity, month).is_some(), "an equity is keyable");
        assert!(entry_key(&nifty(), month).is_some());
        let opt = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Fno,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Future {
                expiry: brutex_core::instrument::Expiry::new(2026, 7, 30).expect("valid"),
            },
        };
        assert_eq!(entry_key(&opt, month), None);
        // And a grid built over one is empty rather than wrong.
        assert!(coverage_page(&[opt], &[], day(2026, 8, 7), 0, 4).is_empty());
    }
}
