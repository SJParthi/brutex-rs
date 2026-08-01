//! The only way a store path is built.
//!
//! `docs/05-decisions.md` D-0019 locks the shape:
//!
//! ```text
//! bars/<vendor>/<exchange>/<segment>/<symbol>/<timeframe>/<yyyy-mm>.bin
//! bars/groww/NSE/INDEX/NIFTY/1min/2024-06.bin
//! bars/dhan/NSE/INDEX/NIFTY/1min/2024-06.bin
//! ```
//!
//! The vendor is the **first** segment so a vendor can be added, re-pulled or
//! deleted by touching one directory and nothing else. That was prose and a
//! directory naming habit; `docs/04-invariants.md` X-12 ("each vendor writes
//! only under its own path prefix") named a test that did not exist, and
//! nothing in code held the property up.
//!
//! # What the type enforces
//!
//! * The first segment is a [`Vendor`], not a string. A `&str` there could
//!   name any vendor, or none: code holding `Vendor::Groww` could write into
//!   `bars/dhan/…` and nothing would refuse it, and D-0019 deliberately keeps
//!   no provenance field inside the file, so it would be indistinguishable
//!   afterwards. `CLAUDE.md` §6's reasoning applies exactly — a parameter that
//!   can be set can be set wrongly and silently.
//! * The vendor segment is written first, always, by the one renderer.
//! * Every remaining segment is checked before a [`StorePath`] exists at all,
//!   so a path that could leave its vendor prefix is unconstructible rather
//!   than unlikely. A separator, a `..`, a leading `/`, or an empty segment is
//!   refused **by name** — see [`PathError`].
//! * Segments are **case-canonical**: the vendor is lower case because
//!   `Vendor::as_str` is, and the exchange, segment and symbol are upper case
//!   because `brutex_core`'s types are. A non-canonical segment is refused,
//!   not folded.
//!
//! # Why case is refused rather than tolerated
//!
//! Measured: on this repository's development machine (APFS, case-insensitive)
//! `bars/groww/…` and `bars/GROWW/…` are **one** file, so writing the second
//! destroys the first — one vendor overwriting another through the very API
//! that exists to make that impossible. On the CI runner (ext4,
//! case-sensitive) the same two inputs are **two** directory trees, silently
//! splitting one vendor's history in half. Same code, two different silent
//! failures, chosen by the host. That is the identical argument that rejected
//! a host-dependent block length, applied to the path — `CLAUDE.md` §3 rule 5.
//!
//! `brutex_core::symbol::Symbol::new` already upper-cases so `nifty` and
//! `NIFTY` cannot become two instruments; [`StorePath::new`] taking a raw
//! `&str` was a public door around that normalisation.
//!
//! # What it does **not** enforce
//!
//! [`StorePath::to_path_buf`] guarantees a **lexical** property. No component
//! is `..`, none is absolute, so the rendered path cannot climb out of
//! `root/bars/<vendor>/` *as text*. A symlink at any component defeats that,
//! and nothing in this crate resolves or refuses one — a `bars/groww` symlink
//! pointing at `bars/dhan` sends every groww write into dhan's files while
//! satisfying every assertion the isolation test makes. Closing it needs
//! `openat` with `O_NOFOLLOW` per component in a writer that does not exist
//! yet. Stated here rather than implied away.
//!
//! `store::unit::vendor_prefix_isolated` is the test X-12 names.
//!
//! # Cost
//!
//! Construction is a bounded scan of at most [`MAX_SEGMENT_LEN`] bytes per
//! segment over a fixed number of segments — constant, independent of how many
//! instruments, vendors or months exist. It allocates nothing:
//! [`StorePath`] borrows its segments and [`std::fmt::Display`] writes them
//! straight into the caller's sink. [`StorePath::to_path_buf`] is the one
//! allocating entry point, and it takes two bounded allocations — counted
//! there, not rounded down to a nicer number.

use std::fmt;
use std::path::{Path, PathBuf};

use brutex_core::instrument::{InstrumentKey, Kind};
use brutex_core::symbol::SYMBOL_CAPACITY;
use brutex_core::vendor::Vendor;

/// The directory every bar file lives under.
pub const STORE_ROOT: &str = "bars";

/// The longest a single path segment may be.
///
/// Matches `brutex_core::symbol::SYMBOL_CAPACITY`, because the symbol is the
/// longest segment and a store path that could not hold a legal symbol would
/// be a second, quieter limit on what can be stored. The assertion below is
/// the whole of that promise: raising `SYMBOL_CAPACITY` without raising this
/// is a compile error, not a refusal discovered by a `for_key` call on the one
/// symbol long enough to trip it.
pub const MAX_SEGMENT_LEN: usize = 24;

const _: () = assert!(MAX_SEGMENT_LEN == SYMBOL_CAPACITY);

/// The longest a timeframe segment can be.
///
/// The timeframe is not caller text — it is one of [`Timeframe::KNOWN`], so
/// its bound is the longest name in that table rather than
/// [`MAX_SEGMENT_LEN`]. `store::unit::a_maximal_path_fits_the_declared_bound`
/// checks the table against it.
pub const MAX_TIMEFRAME_LEN: usize = 4;

/// The longest vendor segment.
///
/// Derived from `Vendor::ALL` rather than written down: a vendor with a longer
/// name would otherwise widen the longest legal path without widening the
/// bound. Adding a vendor changes `Vendor::ALL`'s length, which makes the
/// destructuring in `longest_vendor` a compile error rather than a silent
/// drift — which matters, because `crates/core` is not this crate's to watch.
///
/// It is written down and then **checked against the table**, rather than
/// computed from it. A computed maximum would silently widen [`MAX_LEN`] the
/// day a longer vendor appeared; the assertion below is a compile error
/// instead, at the moment `Vendor::ALL` changes — and `crates/core` is not
/// this crate's to watch. Adding a vendor also changes the table's length,
/// which makes the destructuring itself a compile error.
///
/// `store::unit::every_vendor_is_a_legal_segment` proves the bound is reached,
/// so it is tight rather than merely sufficient.
pub const MAX_VENDOR_LEN: usize = 5;

const _: () = {
    let [groww, dhan] = Vendor::ALL;
    assert!(groww.as_str().len() <= MAX_VENDOR_LEN);
    assert!(dhan.as_str().len() <= MAX_VENDOR_LEN);
};
const _: () = assert!(MAX_VENDOR_LEN <= MAX_SEGMENT_LEN);

/// The longest file extension, dot included.
///
/// Checked against [`FileKind::ALL`] the same way and for the same reason: a
/// new sibling file with a longer extension is a compile error here rather
/// than a path that quietly exceeds [`MAX_LEN`].
pub const MAX_EXTENSION_LEN: usize = 5;

const _: () = {
    let [bars, checksums, overlay, lock] = FileKind::ALL;
    assert!(bars.extension().len() <= MAX_EXTENSION_LEN);
    assert!(checksums.extension().len() <= MAX_EXTENSION_LEN);
    assert!(overlay.extension().len() <= MAX_EXTENSION_LEN);
    assert!(lock.extension().len() <= MAX_EXTENSION_LEN);
};

/// The length of a rendered path, root excluded, when every segment is at its
/// cap.
///
/// Tight, not merely sufficient: the longest legal path is exactly this many
/// bytes, and `store::unit::a_maximal_path_fits_the_declared_bound` asserts
/// the equality by building one. A bound with slack in it is a bound nobody
/// notices going wrong.
pub const MAX_LEN: usize = STORE_ROOT.len()
    + 1 + MAX_VENDOR_LEN          // "/groww"
    + 3 * (1 + MAX_SEGMENT_LEN)   // exchange, segment, symbol
    + 1 + MAX_TIMEFRAME_LEN       // "/1min"
    + 1 + 7                       // "/yyyy-mm"
    + MAX_EXTENSION_LEN;

/// Which case a segment is canonically written in.
///
/// Not a preference: it is which of `brutex_core`'s types renders that
/// segment. `Vendor::as_str` is lower case, `Exchange::as_str`,
/// `Segment::as_str` and `Symbol` are upper case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentCase {
    /// ASCII letters must be lower case, as `Vendor::as_str` renders them.
    Lower,
    /// ASCII letters must be upper case, as `Symbol` renders them.
    Upper,
}

/// Why a path was refused.
///
/// Every variant names the offending field, because "invalid path" sends the
/// operator to guess which of six segments was wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathError {
    /// A segment was empty. An empty segment collapses two directory levels
    /// into one and silently files data somewhere else.
    EmptySegment {
        /// Which segment.
        field: &'static str,
    },
    /// A segment was `.` or `..`.
    ///
    /// `..` is the escape: one of them leaves the vendor prefix, two leave the
    /// store. Refused by name rather than by the byte rule below so the reason
    /// is legible in the error.
    Traversal {
        /// Which segment.
        field: &'static str,
    },
    /// A segment held a byte outside the allowed set.
    ///
    /// The set is ASCII letters, digits, `-`, `_` and `&` — the same set
    /// `brutex_core::symbol::Symbol` admits. A separator (`/` or `\`) lands
    /// here, which is also what refuses an absolute path: it begins with one.
    IllegalByte {
        /// Which segment.
        field: &'static str,
        /// The byte that was refused.
        byte: u8,
    },
    /// A segment held a letter in the wrong case.
    ///
    /// Refused rather than folded, and refused rather than accepted: two
    /// segments differing only in case are two files on a case-sensitive
    /// filesystem and one file on a case-insensitive one, so tolerating them
    /// makes the on-disk result depend on the host.
    NotCanonicalCase {
        /// Which segment.
        field: &'static str,
        /// The letter that was in the wrong case.
        byte: u8,
    },
    /// A segment was longer than [`MAX_SEGMENT_LEN`].
    SegmentTooLong {
        /// Which segment.
        field: &'static str,
        /// How long it was.
        len: usize,
    },
    /// No timeframe of that length is defined.
    ///
    /// `docs/05-decisions.md` D-0015: minute bars only, until a minute-level
    /// result earns the upgrade. A new timeframe is a new row in
    /// [`Timeframe::KNOWN`], not a string a caller invents.
    UnknownTimeframe {
        /// The length asked for, in seconds.
        secs: u32,
    },
    /// A month outside `1..=12`.
    MonthOutOfRange {
        /// The month asked for.
        month: u8,
    },
    /// A year outside `1970..=9999`.
    ///
    /// Below 1970 no bar can exist — timestamps are microseconds since the
    /// Unix epoch. Above 9999 the four-digit rendering would lie.
    YearOutOfRange {
        /// The year asked for.
        year: u16,
    },
    /// The instrument needs a contract name, which this builder does not form.
    ///
    /// D-0019 files a future or an option under its **contract**, not its
    /// underlying — `NIFTY` alone would put every expiry in one directory.
    /// Forming a contract segment needs the expiry and strike rendering that
    /// belongs beside `InstrumentKey`, and it does not exist yet. Refused by
    /// name rather than filed under the underlying, which would silently merge
    /// distinct series.
    ContractPathUnsupported,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::EmptySegment { field } => write!(f, "path segment {field} is empty"),
            Self::Traversal { field } => {
                write!(f, "path segment {field} is a traversal and would escape")
            }
            Self::IllegalByte { field, byte } => {
                write!(f, "path segment {field} holds byte {byte:#04x}")
            }
            Self::NotCanonicalCase { field, byte } => {
                write!(
                    f,
                    "path segment {field} holds byte {byte:#04x} in the wrong case"
                )
            }
            Self::SegmentTooLong { field, len } => {
                write!(
                    f,
                    "path segment {field} is {len} bytes, max {MAX_SEGMENT_LEN}"
                )
            }
            Self::UnknownTimeframe { secs } => write!(f, "no timeframe of {secs} seconds"),
            Self::MonthOutOfRange { month } => write!(f, "month {month} is not 1..=12"),
            Self::YearOutOfRange { year } => write!(f, "year {year} is not 1970..=9999"),
            Self::ContractPathUnsupported => {
                f.write_str("a futures or options path needs a contract segment")
            }
        }
    }
}

impl std::error::Error for PathError {}

/// A bar length, as both a number of seconds and a path segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timeframe {
    secs: u32,
    name: &'static str,
}

impl Timeframe {
    /// One-minute bars — the only timeframe D-0015 admits.
    pub const MINUTE_1: Self = Self {
        secs: 60,
        name: "1min",
    };

    /// Every timeframe this build stores.
    pub const KNOWN: &'static [Self] = &[Self::MINUTE_1];

    /// The timeframe of `secs` seconds.
    ///
    /// # Errors
    ///
    /// [`PathError::UnknownTimeframe`] naming the length. There is no
    /// derived-name fallback: a directory called `300s` that no reader looks
    /// for is data written into a hole.
    pub fn from_secs(secs: u32) -> Result<Self, PathError> {
        Self::KNOWN
            .iter()
            .copied()
            .find(|tf| tf.secs == secs)
            .ok_or(PathError::UnknownTimeframe { secs })
    }

    /// Seconds per bar. Matches `Header::timeframe_secs`.
    #[must_use]
    pub const fn secs(self) -> u32 {
        self.secs
    }

    /// The path segment.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.name
    }
}

/// The month a bar file covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    year: u16,
    month: u8,
}

impl YearMonth {
    /// Builds a month.
    ///
    /// # Errors
    ///
    /// [`PathError::YearOutOfRange`] outside `1970..=9999`, or
    /// [`PathError::MonthOutOfRange`] outside `1..=12`.
    pub const fn new(year: u16, month: u8) -> Result<Self, PathError> {
        if year < 1970 || year > 9999 {
            return Err(PathError::YearOutOfRange { year });
        }
        if month == 0 || month > 12 {
            return Err(PathError::MonthOutOfRange { month });
        }
        Ok(Self { year, month })
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month, `1..=12`.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// Which of a month's sibling files this path names.
///
/// `docs/02-store-format.md` §6, §8 and §9. The overlay is a separate file at
/// its own stride precisely so the base record stays 56 bytes forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    /// The bar records.
    Bars,
    /// The sidecar block checksums — one per [`crate::layout::Layout`] block,
    /// at the same block index. See [`crate::block`].
    Checksums,
    /// Computed overlay fields, at their own stride.
    Overlay,
    /// The advisory lock a writer holds for the month.
    ///
    /// `docs/02-store-format.md` §9: "One writer per file, enforced by an
    /// advisory lock, and the lock is a leaf — never held while acquiring
    /// another." [`crate::header::Header::commit`]'s crash argument is
    /// conditioned on exactly one writer, so the lock needs a name that is
    /// derived the same way every other sibling is, rather than invented at
    /// the call site by string concatenation.
    Lock,
}

impl FileKind {
    /// Every sibling file a month has.
    pub const ALL: [Self; 4] = [Self::Bars, Self::Checksums, Self::Overlay, Self::Lock];

    /// The file extension, dot included.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Bars => ".bin",
            Self::Checksums => ".crc",
            Self::Overlay => ".ovl",
            Self::Lock => ".lock",
        }
    }
}

/// The pieces a store path is built from.
///
/// A struct rather than eight positional arguments: `exchange` and `segment`
/// are both short uppercase strings, and swapping them at a call site would
/// build a valid-looking path to the wrong place with nothing to catch it.
#[derive(Debug, Clone, Copy)]
pub struct PathParts<'a> {
    /// The vendor. Becomes the first segment under [`STORE_ROOT`].
    ///
    /// A [`Vendor`], not a string: X-12 says each vendor writes only under its
    /// own prefix, and a `&str` cannot carry that.
    pub vendor: Vendor,
    /// The exchange, e.g. `NSE`. Upper case.
    pub exchange: &'a str,
    /// The exchange segment, e.g. `INDEX`. Upper case.
    pub segment: &'a str,
    /// The symbol or contract, e.g. `NIFTY`. Upper case.
    pub symbol: &'a str,
    /// The bar length.
    pub timeframe: Timeframe,
    /// The month the file covers.
    pub month: YearMonth,
    /// Which sibling file.
    pub file: FileKind,
}

/// A validated store path. The only thing that renders one.
///
/// Borrows its segments, so building one allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StorePath<'a> {
    vendor: Vendor,
    exchange: &'a str,
    segment: &'a str,
    symbol: &'a str,
    timeframe: Timeframe,
    month: YearMonth,
    file: FileKind,
}

impl<'a> StorePath<'a> {
    /// Validates the parts and fixes the vendor as the first segment.
    ///
    /// The vendor is not re-checked: it is a [`Vendor`], a closed set of
    /// lower-case literals, and `store::unit::every_vendor_is_a_legal_segment`
    /// drives this module's own [`check_segment`] over all of them. A runtime
    /// check would carry a refusal arm no input could reach.
    ///
    /// # Errors
    ///
    /// [`PathError`] naming the first segment that is empty, a traversal,
    /// over-long, in the wrong case, or holds a byte outside the allowed set.
    /// An absolute path and a nested path both land on the separator byte.
    ///
    /// # Examples
    ///
    /// ```
    /// # use brutex_core::vendor::Vendor;
    /// # use store::path::{FileKind, PathError, PathParts, StorePath, Timeframe, YearMonth};
    /// let path = StorePath::new(PathParts {
    ///     vendor: Vendor::Groww,
    ///     exchange: "NSE",
    ///     segment: "INDEX",
    ///     symbol: "NIFTY",
    ///     timeframe: Timeframe::MINUTE_1,
    ///     month: YearMonth::new(2024, 6)?,
    ///     file: FileKind::Bars,
    /// })?;
    /// assert_eq!(path.to_string(), "bars/groww/NSE/INDEX/NIFTY/1min/2024-06.bin");
    ///
    /// let base = PathParts {
    ///     vendor: Vendor::Groww,
    ///     exchange: "NSE",
    ///     segment: "INDEX",
    ///     symbol: "NIFTY",
    ///     timeframe: Timeframe::MINUTE_1,
    ///     month: YearMonth::new(2024, 6)?,
    ///     file: FileKind::Bars,
    /// };
    ///
    /// // A symbol that tries to climb out is refused, not sanitised.
    /// assert!(StorePath::new(PathParts { symbol: "../dhan", ..base }).is_err());
    ///
    /// // And so is one that differs from the canonical form only in case,
    /// // because "nifty" and "NIFTY" are two files on ext4 and one on APFS.
    /// assert_eq!(
    ///     StorePath::new(PathParts { symbol: "nifty", ..base }),
    ///     Err(PathError::NotCanonicalCase { field: "symbol", byte: b'n' }),
    /// );
    /// # Ok::<(), PathError>(())
    /// ```
    pub fn new(parts: PathParts<'a>) -> Result<Self, PathError> {
        check_segment("exchange", parts.exchange, SegmentCase::Upper)?;
        check_segment("segment", parts.segment, SegmentCase::Upper)?;
        check_segment("symbol", parts.symbol, SegmentCase::Upper)?;
        Ok(Self {
            vendor: parts.vendor,
            exchange: parts.exchange,
            segment: parts.segment,
            symbol: parts.symbol,
            timeframe: parts.timeframe,
            month: parts.month,
            file: parts.file,
        })
    }

    /// The path for one vendor's copy of one instrument.
    ///
    /// Takes the canonical identity from `crates/core` rather than three loose
    /// strings, so the exchange, segment and symbol cannot disagree with the
    /// key they came from.
    ///
    /// # Errors
    ///
    /// [`PathError::ContractPathUnsupported`] for a future or an option, which
    /// D-0019 files under a contract segment this builder does not form. Plus
    /// anything [`StorePath::new`] refuses.
    pub fn for_key(
        vendor: Vendor,
        key: &'a InstrumentKey,
        timeframe: Timeframe,
        month: YearMonth,
        file: FileKind,
    ) -> Result<Self, PathError> {
        match key.kind {
            Kind::Index | Kind::Equity => Self::new(PathParts {
                vendor,
                exchange: key.exchange.as_str(),
                segment: key.segment.as_str(),
                symbol: key.underlying.as_str(),
                timeframe,
                month,
                file,
            }),
            _ => Err(PathError::ContractPathUnsupported),
        }
    }

    /// The vendor this path belongs to.
    #[must_use]
    pub const fn vendor(self) -> Vendor {
        self.vendor
    }

    /// The vendor segment — the first segment under [`STORE_ROOT`].
    #[must_use]
    pub const fn vendor_segment(self) -> &'static str {
        self.vendor.as_str()
    }

    /// The bar length.
    #[must_use]
    pub const fn timeframe(self) -> Timeframe {
        self.timeframe
    }

    /// The month.
    #[must_use]
    pub const fn month(self) -> YearMonth {
        self.month
    }

    /// Which sibling file.
    #[must_use]
    pub const fn file(self) -> FileKind {
        self.file
    }

    /// The path under `root`.
    ///
    /// Two allocations, both bounded: the rendered relative path, and the
    /// buffer it is joined into — reserved up front from [`MAX_LEN`] so
    /// neither grows. Not one: `root` is an `OsStr`, which is not required to
    /// be UTF-8, so the relative path cannot be rendered straight into it
    /// through `fmt::Write`. Counted honestly rather than rounded down.
    ///
    /// # What this guarantees, exactly
    ///
    /// **Lexically**, no component is `.` or `..` and none is absolute, so the
    /// rendered text cannot name anything outside `root/bars/<vendor>/`.
    ///
    /// **On a filesystem, it guarantees nothing.** A symlink at any component
    /// redirects the whole subtree: with `root/bars/groww` linked to
    /// `root/bars/dhan`, a write through a groww path lands in dhan's file
    /// while still satisfying `starts_with(root/bars/groww)` and holding no
    /// `Component::ParentDir`. This crate does not resolve or refuse links —
    /// that needs `openat` with `O_NOFOLLOW` per component, in a writer that
    /// does not exist yet, halting loudly and naming the linked component. The
    /// earlier wording here claimed the filesystem property outright, which
    /// was a failure hidden behind a claim.
    #[must_use]
    pub fn to_path_buf(self, root: &Path) -> PathBuf {
        let mut out = PathBuf::with_capacity(root.as_os_str().len() + 1 + MAX_LEN);
        out.push(root);
        out.push(self.to_string());
        out
    }
}

impl fmt::Display for StorePath<'_> {
    /// The one renderer. The vendor is written before anything that varies.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}/{}/{}/{}/{}{}",
            STORE_ROOT,
            self.vendor.as_str(),
            self.exchange,
            self.segment,
            self.symbol,
            self.timeframe.as_str(),
            self.month,
            self.file.extension(),
        )
    }
}

/// Refuses anything that is not a plain, self-contained, canonically-cased
/// directory name.
///
/// Public so the vendor table can be driven through the same function
/// [`StorePath::new`] uses, rather than through a paraphrase of it in a test.
///
/// The illegal-byte scan runs over the whole segment **before** the case scan,
/// so a segment that is both mis-cased and an escape attempt is reported as
/// the escape. A traversal is the security-relevant fact; the case is a
/// reproducibility one.
///
/// # Errors
///
/// [`PathError::EmptySegment`], [`PathError::SegmentTooLong`],
/// [`PathError::Traversal`], [`PathError::IllegalByte`] or
/// [`PathError::NotCanonicalCase`], checked in that order.
pub fn check_segment(field: &'static str, text: &str, case: SegmentCase) -> Result<(), PathError> {
    if text.is_empty() {
        return Err(PathError::EmptySegment { field });
    }
    if text.len() > MAX_SEGMENT_LEN {
        return Err(PathError::SegmentTooLong {
            field,
            len: text.len(),
        });
    }
    if text == "." || text == ".." {
        return Err(PathError::Traversal { field });
    }
    if let Some(byte) = text
        .bytes()
        .find(|b| !matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'&'))
    {
        return Err(PathError::IllegalByte { field, byte });
    }
    let wrong_case = match case {
        SegmentCase::Lower => text.bytes().find(u8::is_ascii_uppercase),
        SegmentCase::Upper => text.bytes().find(u8::is_ascii_lowercase),
    };
    match wrong_case {
        Some(byte) => Err(PathError::NotCanonicalCase { field, byte }),
        None => Ok(()),
    }
}
