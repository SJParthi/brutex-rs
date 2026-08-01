//! The per-vendor counter file — layer 13 of `docs/07-o1-architecture.md`.
//!
//! # The question this exists to answer in one read
//!
//! *How much do I have?* Against the store's directory tree that is a walk of
//! roughly 248,000 files — one `stat` each — and it gets slower with every
//! ingest, which is precisely the decay `CLAUDE.md` exists to prevent. Law 3:
//! **never scan to answer a question; maintain a counter instead.**
//!
//! So each vendor has one file recording, per `(instrument, timeframe, month)`,
//! the row count and the first and last timestamp, and its header carries the
//! three totals — entries written, distinct keys, rows held — maintained on
//! every write. [`Manifest::total_rows`], [`Manifest::keys`] and
//! [`Manifest::entries`] are field reads.
//!
//! What that does **not** buy, said plainly: a *filtered* census — "how many
//! expired option series" — is still a walk of the manifest's own entries. It
//! is a sequential read of one file rather than 248,000 directory operations,
//! which is the difference the layer is about, but it is not O(1) and is not
//! claimed to be. A per-segment counter in the header would make it one, and it
//! would be a **new field at a new format version**, never a dynamic schema —
//! `CLAUDE.md` §4. See `docs/06-limits.md` §17.
//!
//! # Bytes on disk
//!
//! ```text
//! byte 0                 32768                                     EOF
//!   ├──── header region ────┼── entry 0 ──┼── entry 1 ──┼── … ───────┤
//!    2 slots × 16384 spacing    64 bytes      64 bytes
//! ```
//!
//! **Address of entry *i*: `HEADER_LEN + i·64`.** [`Manifest::offset_of`] is an
//! add and a multiply — law 4, arithmetic beats lookup. Both the header slot and
//! the entry are 64 bytes with a CRC-32C over the first 60, so one checksum
//! helper serves both and neither straddles a cache line.
//!
//! # What is reused from `crates/store`, and what is not
//!
//! Reused, as **code**:
//!
//! * [`store::crc::crc32c`] — the same polynomial, the same table, verified
//!   against the same published check value. Re-deriving it here would be a
//!   second implementation of a checksum, which is the one kind of duplication
//!   that fails silently: two kernels that disagree produce two files that each
//!   verify only against themselves.
//! * [`store::path::Timeframe`] and [`store::path::YearMonth`] — the manifest's
//!   key must be the tuple that names a bar file, and a second definition of
//!   "which timeframes exist" would let the manifest record a month the store
//!   cannot address.
//! * [`store::format::SLOT_LEN`], [`store::format::SLOT_STRIDE`] and
//!   [`store::format::MAX_SLOT_COUNT`] — the header-slot *geometry*, not the
//!   header. The 16,384-byte spacing is the measured failure-unit argument in
//!   `store::format::SLOT_STRIDE`, and it is the same argument here: two slots
//!   64 bytes apart share one device block and the redundancy is nominal.
//!
//! Reused as **reasoning**, deliberately not as code:
//!
//! * *The commit counter is the authority* (D-0004). A reader takes entries
//!   `0..n_valid`, so a torn entry at the tail is not merely unlikely, it is
//!   unobservable.
//! * *A header update is one write of one self-checked unit.* [`Commit`]
//!   carries one offset and one 64-byte buffer, so there is no API here that
//!   can update the counter and the checksum separately.
//! * *Alternating slots.* Commit *g* goes to slot `g % 2`, so consecutive
//!   commits never write the same slot and a crash mid-write leaves the other
//!   one whole.
//!
//! **Not reused: [`store::header::Header`] itself.** Its fields are a bar
//! file's — `symbol_id`, `timeframe_secs`, `first_ts_micros` — and this file
//! holds counters instead. Widening that struct to serve both would make one
//! `format_version` describe two geometries, which is exactly what
//! `store::layout`'s dispatch exists to make impossible. Its discontiguous
//! checksum domain is also not reused: that shape exists because a bar file's
//! slot has a reserved field *after* its checksum, and this one does not, so
//! the checksum sits last and covers one contiguous run.
//!
//! # A torn write is detectable, never silently believed
//!
//! Three independent mechanisms, each covering what the others cannot:
//!
//! | Failure | What catches it |
//! |---|---|
//! | a half-written entry at the tail | `n_valid` — the reader never looks at it |
//! | a half-written or corrupt entry *below* `n_valid` | that entry's own CRC-32C, and then the previous generation is tried |
//! | a half-written header slot | that slot's own CRC-32C, and the other slot |
//! | a header published before the entries it counts | [`ManifestHeader::validate`] against the region, then [`Manifest::load`] walking down the generations |
//! | a header whose counters do not match its entries | [`Manifest::load`] recomputes both and refuses a disagreement |
//!
//! The last one is what makes the counter worth having. A counter that is never
//! checked against the thing it counts is a number, not a measurement — and
//! this whole module exists so that number can be trusted without a walk. The
//! check runs once, on load, where a walk is already being paid.
//!
//! **Falling back is never silent.** Every generation stepped over on the way
//! to the one that loaded is reported by [`Manifest::degraded_reason`], because
//! `CLAUDE.md` §4 admits exactly two behaviours and "returned `Ok` on a file it
//! had just proved was damaged" is neither of them. Until D-0036 the fault was
//! computed, stored in a local, and dropped on the success path: a manifest
//! whose newest header slot failed its checksum loaded silently at the previous
//! generation, a committed month vanished from the census, and no API said so.
//!
//! # What the counters do NOT check: the bar files themselves
//!
//! The header is checked against **this file's own entries** and against
//! nothing else. `bars/` is never consulted, and no API here can tell that an
//! entry claiming 7,312 rows describes a month with no bar file at all, or that
//! a bar file written by a pull that crashed before its append is missing from
//! the census. The manifest is a record of what a writer *said* it wrote.
//!
//! That is inherent rather than an oversight: confirming it means walking
//! `bars/`, which is the ~248,000 directory operations this layer exists to
//! avoid, so a reconciliation pass is a deliberate, occasional, O(files)
//! operation and is not built. `docs/06-limits.md` §17 records it as an
//! open gap rather than leaving it implied.
//!
//! # Nothing here writes
//!
//! Like `crates/store`, this crate performs no I/O. [`Manifest::record`] returns
//! an [`Append`] — one entry image at one offset — and a [`Commit`] — one slot
//! image at one offset, with the byte offset through which the entry must
//! already be durable. A writable memory mapping stays banned: it raises
//! `SIGBUS` on a full disk and a signal cannot be caught.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use brutex_core::error::InstrumentError;
use brutex_core::instrument::{Exchange, Segment};
use brutex_core::symbol::{SYMBOL_CAPACITY, Symbol};
use brutex_core::vendor::Vendor;
use store::crc::crc32c;
use store::format::{MAX_SLOT_COUNT, MAX_SLOTS, SLOT_LEN, SLOT_STRIDE};
use store::path::{MAX_VENDOR_LEN, PathError, Timeframe, YearMonth};

/// The directory manifests live in, beside `bars/`.
pub const MANIFEST_ROOT: &str = "manifest";

/// The extension one manifest carries.
pub const MANIFEST_EXTENSION: &str = ".man";

/// Identifies a version-1 manifest.
///
/// `BRUTEXM` rather than `BRUTEXB`: a manifest and a bar file must never be
/// mistaken for one another, and the family check is the first thing either
/// decoder does.
pub const MAGIC: [u8; 8] = *b"BRUTEXM1";

/// The seven bytes shared by every manifest version.
pub const MAGIC_FAMILY: [u8; 7] = *b"BRUTEXM";

/// The only format version this build reads or writes.
pub const FORMAT_VERSION: u16 = 1;

/// Bytes per entry, and per header slot. One image size serves both.
pub const IMAGE_LEN: usize = 64;

/// Bytes per entry, in the width an offset is computed in.
pub const ENTRY_STRIDE: u64 = 64;

/// Bytes per entry, in the width the header stores it.
pub const ENTRY_STRIDE_U16: u16 = 64;

/// Header slots. Writes alternate between them.
pub const SLOT_COUNT: u64 = 2;

/// Bytes of header region before the first entry.
///
/// Spelled as a literal rather than the product, because that product needs a
/// `usize`→`u64` conversion and this workspace denies a cast that can truncate.
/// The assertion below is what keeps it honest.
pub const HEADER_LEN: u64 = 32_768;

/// The most entries one manifest may hold.
///
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary.
/// Measured scale is ~248,000 files across every vendor and expiry;
/// 2,097,152 is 8.4× that and caps one manifest at 134 MB. The refusal names
/// the number, so an operator who legitimately outgrows it is told what to
/// raise rather than being handed an allocation the size of the file.
pub const MAX_ENTRIES: u64 = 2_097_152;

/// [`MAX_ENTRIES`] as a `usize`, for the map reservation.
///
/// Written as a literal in both widths rather than converted, for the same
/// reason [`HEADER_LEN`] is: there is no cast to deny and no fallible
/// conversion whose failure arm no test could ever reach. The assertion keeps
/// the two honest.
const MAX_ENTRIES_LEN: usize = 2_097_152;

/// [`store::format::SLOT_STRIDE`] as a `usize`, for slicing a region.
const SLOT_STRIDE_LEN: usize = 16_384;

const _: () = assert!(MAX_ENTRIES == 2_097_152 && MAX_ENTRIES_LEN == 2_097_152);
const _: () = assert!(SLOT_STRIDE == 16_384 && SLOT_STRIDE_LEN == 16_384);
const _: () = assert!(HEADER_LEN == SLOT_COUNT * SLOT_STRIDE);
const _: () = assert!(SLOT_COUNT <= MAX_SLOT_COUNT);
const _: () = assert!(IMAGE_LEN == SLOT_LEN);
const _: () = assert!(ENTRY_STRIDE == 64 && ENTRY_STRIDE_U16 == 64);
const _: () = assert!(MAGIC[0] == MAGIC_FAMILY[0] && MAGIC[7] == b'1');

/// Byte offset of the checksum, in both an entry and a header slot.
const OFF_CRC: usize = 60;

/// Byte offset of `magic` in a header slot.
const H_MAGIC: usize = 0;
/// Byte offset of `format_version` in a header slot.
const H_VERSION: usize = 8;
/// Byte offset of `entry_stride` in a header slot.
const H_STRIDE: usize = 10;
/// Byte offset of `generation` in a header slot.
const H_GENERATION: usize = 16;
/// Byte offset of `n_valid` in a header slot.
const H_N_VALID: usize = 24;
/// Byte offset of `n_keys` in a header slot.
const H_N_KEYS: usize = 32;
/// Byte offset of `total_rows` in a header slot.
const H_TOTAL_ROWS: usize = 40;
/// Byte offset of the vendor segment in a header slot.
const H_VENDOR: usize = 48;
/// Bytes the vendor segment occupies.
const H_VENDOR_LEN: usize = 8;

const _: () = assert!(MAX_VENDOR_LEN <= H_VENDOR_LEN);
const _: () = assert!(H_VENDOR + H_VENDOR_LEN <= OFF_CRC);

/// Byte offset of the symbol in an entry.
const E_SYMBOL: usize = 0;
/// Byte offset of `rows` in an entry.
const E_ROWS: usize = 24;
/// Byte offset of `first_ts_micros` in an entry.
const E_FIRST_TS: usize = 32;
/// Byte offset of `last_ts_micros` in an entry.
const E_LAST_TS: usize = 40;
/// Byte offset of `timeframe_secs` in an entry.
const E_TIMEFRAME: usize = 48;
/// Byte offset of `year` in an entry.
const E_YEAR: usize = 52;
/// Byte offset of `month` in an entry.
const E_MONTH: usize = 54;
/// Byte offset of the exchange code in an entry.
const E_EXCHANGE: usize = 55;
/// Byte offset of the segment code in an entry.
const E_SEGMENT: usize = 56;

const _: () = assert!(E_SYMBOL + SYMBOL_CAPACITY == E_ROWS);
const _: () = assert!(E_SEGMENT < OFF_CRC);
const _: () = assert!(OFF_CRC + 4 == IMAGE_LEN);

/// The path of one vendor's manifest under `root`.
///
/// One allocation, bounded: the root, one directory, one file name whose
/// longest form is [`MAX_VENDOR_LEN`] plus the extension. The vendor segment is
/// a [`Vendor`], not a string, for the reason `store::path` gives — a `&str`
/// there could name any vendor, or none.
#[must_use]
pub fn manifest_path(root: &Path, vendor: Vendor) -> PathBuf {
    let mut out = PathBuf::with_capacity(
        root.as_os_str().len()
            + 2
            + MANIFEST_ROOT.len()
            + MAX_VENDOR_LEN
            + MANIFEST_EXTENSION.len(),
    );
    out.push(root);
    out.push(MANIFEST_ROOT);
    out.push(format!("{}{MANIFEST_EXTENSION}", vendor.as_str()));
    out
}

/// Why one entry's bytes are not an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EntryFault {
    /// Fewer than [`IMAGE_LEN`] bytes were offered.
    TooShort {
        /// Bytes the caller supplied.
        len: usize,
    },
    /// The stored checksum does not match the bytes.
    Checksum {
        /// The checksum the entry carries.
        stored: u32,
        /// The checksum its bytes actually produce.
        computed: u32,
    },
    /// The symbol field is not UTF-8.
    SymbolNotUtf8,
    /// The symbol field is not a symbol.
    BadSymbol(InstrumentError),
    /// The symbol field is not in the canonical form a [`Symbol`] renders.
    ///
    /// Refused rather than folded, and for the reason `store::path` gives about
    /// case: an entry written as `nifty` and read back as `NIFTY` is a key that
    /// does not match itself, so the census would count one month twice.
    SymbolNotCanonical,
    /// The exchange code is not one this build defines.
    UnknownExchange {
        /// The code that was found.
        code: u8,
    },
    /// The segment code is not one this build defines.
    UnknownSegment {
        /// The code that was found.
        code: u8,
    },
    /// No timeframe of that length exists — `store::path::Timeframe::KNOWN`.
    UnknownTimeframe {
        /// The length found, in seconds.
        secs: u32,
    },
    /// The year and month are not a month.
    BadMonth(PathError),
    /// The entry records no rows.
    ///
    /// A manifest is a census of what exists. An entry claiming zero rows
    /// describes a file with nothing in it, which is a write that should never
    /// have been recorded rather than a fact to be stored.
    EmptyEntry,
    /// The entry's own timestamps run backwards.
    TimestampsOutOfOrder {
        /// The first timestamp.
        first: i64,
        /// The last timestamp, which did not follow it.
        last: i64,
    },
}

impl fmt::Display for EntryFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooShort { len } => write!(f, "entry is {len} bytes, needs {IMAGE_LEN}"),
            Self::Checksum { stored, computed } => {
                write!(f, "entry checksum {stored:#010x} != {computed:#010x}")
            }
            Self::SymbolNotUtf8 => f.write_str("the symbol field is not UTF-8"),
            Self::BadSymbol(e) => write!(f, "the symbol field is not a symbol: {e}"),
            Self::SymbolNotCanonical => f.write_str("the symbol field is not in canonical form"),
            Self::UnknownExchange { code } => write!(f, "exchange code {code} is not defined"),
            Self::UnknownSegment { code } => write!(f, "segment code {code} is not defined"),
            Self::UnknownTimeframe { secs } => write!(f, "no timeframe of {secs} seconds"),
            Self::BadMonth(e) => write!(f, "not a month: {e}"),
            Self::EmptyEntry => f.write_str("the entry records no rows"),
            Self::TimestampsOutOfOrder { first, last } => {
                write!(f, "timestamp {last} does not follow {first}")
            }
        }
    }
}

impl std::error::Error for EntryFault {}

/// Why a manifest was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ManifestError {
    /// An entry ordinal is past [`MAX_ENTRIES`].
    ///
    /// Refused rather than allowed to wrap. Bounding the *ordinal* rather than
    /// checking the multiplication afterwards is what lets every offset
    /// computed inside this module be plain arithmetic: past this gate the
    /// product cannot overflow, so there is no failure arm downstream for a
    /// test to be unable to reach.
    OrdinalOutOfRange {
        /// The ordinal that was refused.
        ordinal: u64,
        /// The bound.
        limit: u64,
    },
    /// A header slot buffer is shorter than [`IMAGE_LEN`].
    SlotTooShort {
        /// Bytes the caller supplied.
        len: usize,
    },
    /// The first seven bytes are not [`MAGIC_FAMILY`].
    NotAManifest,
    /// The slot names a version this build does not know.
    UnknownVersion(u16),
    /// The slot names an entry stride its version does not define.
    StrideMismatch(u16),
    /// The slot's stored checksum does not match its bytes.
    SlotChecksum {
        /// The checksum the slot carries.
        stored: u32,
        /// The checksum its bytes actually produce.
        computed: u32,
    },
    /// The slot's vendor field is not a vendor this build reads.
    UnknownVendor,
    /// The manifest belongs to a different vendor than the one asked for.
    ///
    /// The file name carries the vendor and so does the header; when they
    /// disagree, one of them is a typo and appending would file one vendor's
    /// census under another's name.
    VendorMismatch {
        /// The vendor asked for.
        asked: Vendor,
        /// The vendor the header names.
        found: Vendor,
    },
    /// A slot's generation says it belongs in a different slot.
    SlotPositionMismatch {
        /// Where `generation % SLOT_COUNT` says it belongs.
        expected: u64,
        /// Where it was found.
        found: u64,
    },
    /// The header region is shorter than [`SLOT_COUNT`] whole slots.
    HeaderRegionTooShort {
        /// Whole slots the region actually holds.
        slots: u64,
        /// Whole slots this version declares.
        need: u64,
    },
    /// No slot in the header region decoded.
    NoValidHeader,
    /// `n_valid` claims more entries than [`MAX_ENTRIES`].
    TooManyEntries {
        /// The counter that was refused.
        n_valid: u64,
        /// The bound.
        limit: u64,
    },
    /// `n_valid` claims more entries than the region can hold.
    CounterExceedsRegion {
        /// The counter that was refused.
        n_valid: u64,
        /// Whole entries the region holds.
        capacity: u64,
    },
    /// `n_keys` claims more distinct keys than there are entries.
    KeyCountExceedsEntries {
        /// Distinct keys claimed.
        keys: u64,
        /// Entries claimed.
        entries: u64,
    },
    /// Appending would push `n_valid` past `u64::MAX`.
    CounterOverflow,
    /// The generation counter cannot be advanced again.
    ///
    /// Refused rather than wrapped: a wrapped generation makes the *oldest*
    /// slot win and silently un-commits entries.
    GenerationExhausted,
    /// The row total would pass `u64::MAX`.
    RowTotalOverflow,
    /// One entry could not be decoded.
    Entry {
        /// Which entry.
        ordinal: u64,
        /// What was wrong with it.
        fault: EntryFault,
    },
    /// A later entry for one key records fewer rows than an earlier one.
    ///
    /// `CLAUDE.md` §3 rule 8 — history is append-only. A month whose row count
    /// went down is a re-pull that lost data, and the manifest is the only
    /// place that fact is visible without re-reading the bar file.
    RowCountWentBackwards {
        /// Which entry.
        ordinal: u64,
        /// Rows the earlier entry recorded.
        previous: u64,
        /// Rows this entry records.
        next: u64,
    },
    /// A later entry for one key ends before an earlier one did.
    KeyTimestampsOutOfOrder {
        /// Which entry.
        ordinal: u64,
        /// The last timestamp already recorded.
        previous: i64,
        /// The timestamp that did not follow it.
        next: i64,
    },
    /// The header's key count is not the number of distinct keys present.
    KeyCountDisagrees {
        /// What the header claims.
        header: u64,
        /// What the entries hold.
        entries: u64,
    },
    /// The header's row total is not the sum of the entries' row counts.
    RowTotalDisagrees {
        /// What the header claims.
        header: u64,
        /// What the entries hold.
        entries: u64,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::OrdinalOutOfRange { ordinal, limit } => {
                write!(
                    f,
                    "entry ordinal {ordinal} is past the {limit} this build holds"
                )
            }
            Self::SlotTooShort { len } => {
                write!(f, "header slot is {len} bytes, needs {IMAGE_LEN}")
            }
            Self::NotAManifest => f.write_str("magic does not begin BRUTEXM"),
            Self::UnknownVersion(v) => write!(f, "unknown manifest version {v}"),
            Self::StrideMismatch(s) => write!(f, "entry stride {s} is not this version's stride"),
            Self::SlotChecksum { stored, computed } => {
                write!(f, "header slot checksum {stored:#010x} != {computed:#010x}")
            }
            Self::UnknownVendor => f.write_str("the header names no vendor this build reads"),
            Self::VendorMismatch { asked, found } => write!(
                f,
                "this manifest belongs to {}, not {}",
                found.as_str(),
                asked.as_str()
            ),
            Self::SlotPositionMismatch { expected, found } => write!(
                f,
                "header slot {found} holds a commit belonging in {expected}"
            ),
            Self::HeaderRegionTooShort { slots, need } => {
                write!(f, "header region holds {slots} whole slots, needs {need}")
            }
            Self::NoValidHeader => f.write_str("no header slot survived; the header is unreadable"),
            Self::TooManyEntries { n_valid, limit } => {
                write!(f, "n_valid is {n_valid}; this build holds at most {limit}")
            }
            Self::CounterExceedsRegion { n_valid, capacity } => {
                write!(f, "n_valid is {n_valid}; the region holds {capacity}")
            }
            Self::KeyCountExceedsEntries { keys, entries } => {
                write!(f, "{keys} distinct keys across {entries} entries")
            }
            Self::CounterOverflow => f.write_str("n_valid would overflow u64"),
            Self::GenerationExhausted => f.write_str("generation cannot advance past u64"),
            Self::RowTotalOverflow => f.write_str("the row total would overflow u64"),
            Self::Entry { ordinal, fault } => write!(f, "entry {ordinal}: {fault}"),
            Self::RowCountWentBackwards {
                ordinal,
                previous,
                next,
            } => write!(
                f,
                "entry {ordinal} records {next} rows where {previous} were already recorded"
            ),
            Self::KeyTimestampsOutOfOrder {
                ordinal,
                previous,
                next,
            } => write!(
                f,
                "entry {ordinal}: timestamp {next} does not follow {previous}"
            ),
            Self::KeyCountDisagrees { header, entries } => write!(
                f,
                "the header claims {header} distinct keys; the entries hold {entries}"
            ),
            Self::RowTotalDisagrees { header, entries } => write!(
                f,
                "the header claims {header} rows; the entries hold {entries}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

/// The exchange's code on disk.
///
/// Append-only, like every other number this repository writes down: a code is
/// never reused for a different exchange. BSE is defined even though `CLAUDE.md`
/// §1 sweeps and pulls NSE alone, because BSE data already on disk is not
/// deleted and a census that could not name it would under-report the store.
fn exchange_code(exchange: Exchange) -> u8 {
    match exchange {
        Exchange::Nse => 1,
        Exchange::Bse => 2,
    }
}

/// The exchange a code names.
fn exchange_of(code: u8) -> Result<Exchange, EntryFault> {
    match code {
        1 => Ok(Exchange::Nse),
        2 => Ok(Exchange::Bse),
        _ => Err(EntryFault::UnknownExchange { code }),
    }
}

/// The segment's code on disk. Append-only.
fn segment_code(segment: Segment) -> u8 {
    match segment {
        Segment::Index => 1,
        Segment::Cash => 2,
        Segment::Fno => 3,
    }
}

/// The segment a code names.
fn segment_of(code: u8) -> Result<Segment, EntryFault> {
    match code {
        1 => Ok(Segment::Index),
        2 => Ok(Segment::Cash),
        3 => Ok(Segment::Fno),
        _ => Err(EntryFault::UnknownSegment { code }),
    }
}

/// What one manifest entry is keyed by.
///
/// Exactly the tuple that names one bar file, minus the vendor, which the
/// manifest itself carries. Structural `Eq` and `Hash` over every field, so it
/// is directly a `HashMap` key and a lookup is one probe rather than a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryKey {
    /// Trading venue.
    pub exchange: Exchange,
    /// Exchange segment.
    pub segment: Segment,
    /// The symbol, fixed width — `docs/07-o1-architecture.md` layer 1.
    pub symbol: Symbol,
    /// The bar length.
    pub timeframe: Timeframe,
    /// The month the bar file covers.
    pub month: YearMonth,
}

/// One month of one instrument, as the manifest records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entry {
    /// What this entry is about.
    pub key: EntryKey,
    /// Rows in that bar file.
    pub rows: u64,
    /// Timestamp of the first row.
    pub first_ts_micros: i64,
    /// Timestamp of the last row.
    pub last_ts_micros: i64,
}

impl Entry {
    /// Whether this entry is internally consistent.
    ///
    /// Run before an entry is written **and** after one is read, because the
    /// checksum proves the bytes are the bytes that were written and says
    /// nothing about whether they were ever sensible.
    ///
    /// # Errors
    ///
    /// [`EntryFault::EmptyEntry`] or [`EntryFault::TimestampsOutOfOrder`].
    pub const fn check(&self) -> Result<(), EntryFault> {
        if self.rows == 0 {
            return Err(EntryFault::EmptyEntry);
        }
        if self.last_ts_micros < self.first_ts_micros {
            return Err(EntryFault::TimestampsOutOfOrder {
                first: self.first_ts_micros,
                last: self.last_ts_micros,
            });
        }
        Ok(())
    }

    /// The 64-byte image of this entry, checksum computed.
    #[must_use]
    pub fn image(&self) -> [u8; IMAGE_LEN] {
        let mut out = [0u8; IMAGE_LEN];
        write_text(&mut out, E_SYMBOL, self.key.symbol.as_str());
        write_at(&mut out, E_ROWS, self.rows.to_le_bytes());
        write_at(&mut out, E_FIRST_TS, self.first_ts_micros.to_le_bytes());
        write_at(&mut out, E_LAST_TS, self.last_ts_micros.to_le_bytes());
        write_at(
            &mut out,
            E_TIMEFRAME,
            self.key.timeframe.secs().to_le_bytes(),
        );
        write_at(&mut out, E_YEAR, self.key.month.year().to_le_bytes());
        write_at(&mut out, E_MONTH, self.key.month.month().to_le_bytes());
        write_at(
            &mut out,
            E_EXCHANGE,
            exchange_code(self.key.exchange).to_le_bytes(),
        );
        write_at(
            &mut out,
            E_SEGMENT,
            segment_code(self.key.segment).to_le_bytes(),
        );
        seal(&mut out);
        out
    }

    /// Decodes one entry.
    ///
    /// The bytes are copied into a 64-byte array **once**, before anything is
    /// checked, and every field is read from that copy — the same argument
    /// `store::header::Header::decode` makes: the checksum must cover exactly
    /// the bytes the returned value is built from, or a concurrent write
    /// produces a value no checksum ever saw.
    ///
    /// # Errors
    ///
    /// [`EntryFault`], checked in this order: length, checksum, then each field.
    /// The checksum first, because a field refusal from bytes that already
    /// failed their checksum is a diagnosis of corruption as a schema problem.
    pub fn decode(bytes: &[u8]) -> Result<Self, EntryFault> {
        let image = image_of(bytes).map_err(|len| EntryFault::TooShort { len })?;
        verify(&image).map_err(|(stored, computed)| EntryFault::Checksum { stored, computed })?;

        let text = text_at(&image, E_SYMBOL, SYMBOL_CAPACITY).ok_or(EntryFault::SymbolNotUtf8)?;
        let symbol = Symbol::new(text).map_err(EntryFault::BadSymbol)?;
        if symbol.as_str() != text {
            return Err(EntryFault::SymbolNotCanonical);
        }
        let secs = u32::from_le_bytes(le_bytes(&image, E_TIMEFRAME));
        let timeframe =
            Timeframe::from_secs(secs).map_err(|_| EntryFault::UnknownTimeframe { secs })?;
        let year = u16::from_le_bytes(le_bytes(&image, E_YEAR));
        let [month] = le_bytes(&image, E_MONTH);
        let month = YearMonth::new(year, month).map_err(EntryFault::BadMonth)?;
        let [exchange] = le_bytes(&image, E_EXCHANGE);
        let [segment] = le_bytes(&image, E_SEGMENT);

        let entry = Self {
            key: EntryKey {
                exchange: exchange_of(exchange)?,
                segment: segment_of(segment)?,
                symbol,
                timeframe,
                month,
            },
            rows: u64::from_le_bytes(le_bytes(&image, E_ROWS)),
            first_ts_micros: i64::from_le_bytes(le_bytes(&image, E_FIRST_TS)),
            last_ts_micros: i64::from_le_bytes(le_bytes(&image, E_LAST_TS)),
        };
        entry.check()?;
        Ok(entry)
    }
}

/// One header slot's fields.
///
/// The three counters are the whole point of the file. They are maintained on
/// every write and checked against the entries on every load, so reading one is
/// a field read that means something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ManifestHeader {
    /// Format version. Selects nothing yet; refused when unknown.
    pub format_version: u16,
    /// Bytes per entry, as this file declares them.
    pub entry_stride: u16,
    /// Which vendor's census this is. A cross-check against the file name.
    pub vendor: Vendor,
    /// Which commit this slot holds. Higher wins.
    pub generation: u64,
    /// The commit counter: how many entries are readable.
    pub n_valid: u64,
    /// Distinct keys among those entries.
    pub n_keys: u64,
    /// Rows held across every distinct key — the counter law 3 is about.
    pub total_rows: u64,
}

/// One header write: one offset, one buffer, one positional write.
///
/// The type is the guarantee. A commit cannot be expressed as several field
/// updates because there is nowhere to put the second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    /// Which slot this belongs in — `generation % SLOT_COUNT`.
    pub slot: u64,
    /// The byte offset to write at.
    pub offset: u64,
    /// Exactly the bytes to write, checksum included.
    pub bytes: [u8; IMAGE_LEN],
    /// The byte offset through which entry data must already be durable.
    ///
    /// Issuing the slot write before the bytes below this offset are on stable
    /// storage publishes a counter over entries that may not exist. This crate
    /// performs no I/O and cannot issue the barrier; it states the offset so a
    /// writer cannot claim it did not know which one to flush.
    pub durable_through: u64,
    /// The header state this commit publishes.
    pub header: ManifestHeader,
}

/// One entry write: one offset and one buffer, plus the commit that publishes
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Append {
    /// Which entry this is — the index the offset is computed from.
    pub ordinal: u64,
    /// The byte offset to write the entry at.
    pub offset: u64,
    /// Exactly the bytes to write, checksum included.
    pub bytes: [u8; IMAGE_LEN],
    /// The header write that publishes it, to be issued **after** the entry is
    /// durable through [`Commit::durable_through`].
    pub commit: Commit,
}

impl ManifestHeader {
    /// A fresh header for an empty manifest: generation 0, no entries.
    #[must_use]
    pub const fn genesis(vendor: Vendor) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            entry_stride: ENTRY_STRIDE_U16,
            vendor,
            generation: 0,
            n_valid: 0,
            n_keys: 0,
            total_rows: 0,
        }
    }

    /// The header that publishing `appended` more entries produces.
    ///
    /// Advances the generation and the counter together, because they are
    /// written together.
    ///
    /// # Errors
    ///
    /// [`ManifestError::GenerationExhausted`] or
    /// [`ManifestError::CounterOverflow`] — both refusals, never wraps.
    /// [`ManifestError::TooManyEntries`] past [`MAX_ENTRIES`], or
    /// [`ManifestError::KeyCountExceedsEntries`] for a key count no set of
    /// entries could produce.
    pub const fn advance(
        &self,
        appended: u64,
        n_keys: u64,
        total_rows: u64,
    ) -> Result<Self, ManifestError> {
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(ManifestError::GenerationExhausted);
        };
        let Some(n_valid) = self.n_valid.checked_add(appended) else {
            return Err(ManifestError::CounterOverflow);
        };
        if n_valid > MAX_ENTRIES {
            return Err(ManifestError::TooManyEntries {
                n_valid,
                limit: MAX_ENTRIES,
            });
        }
        if n_keys > n_valid {
            return Err(ManifestError::KeyCountExceedsEntries {
                keys: n_keys,
                entries: n_valid,
            });
        }
        Ok(Self {
            generation,
            n_valid,
            n_keys,
            total_rows,
            ..*self
        })
    }

    /// Checks this header against the entries a region can hold.
    ///
    /// # Errors
    ///
    /// [`ManifestError::TooManyEntries`],
    /// [`ManifestError::CounterExceedsRegion`] — trusting the counter over the
    /// region reads past the end and returns whatever was there — or
    /// [`ManifestError::KeyCountExceedsEntries`].
    pub const fn validate(&self, capacity: u64) -> Result<(), ManifestError> {
        if self.n_valid > MAX_ENTRIES {
            return Err(ManifestError::TooManyEntries {
                n_valid: self.n_valid,
                limit: MAX_ENTRIES,
            });
        }
        if self.n_valid > capacity {
            return Err(ManifestError::CounterExceedsRegion {
                n_valid: self.n_valid,
                capacity,
            });
        }
        if self.n_keys > self.n_valid {
            return Err(ManifestError::KeyCountExceedsEntries {
                keys: self.n_keys,
                entries: self.n_valid,
            });
        }
        Ok(())
    }

    /// The single write that publishes this header.
    ///
    /// # Preconditions the caller owns
    ///
    /// 1. **One writer per manifest.** Two writers that read the same
    ///    generation produce the same slot at the same offset. This crate
    ///    performs no I/O and cannot take a lock.
    /// 2. **Flush first.** Every byte below [`Commit::durable_through`] must be
    ///    on stable storage before this write is issued.
    ///
    /// # Errors
    ///
    /// [`ManifestError::TooManyEntries`] if the counter is past
    /// [`MAX_ENTRIES`]. Past that gate every offset below is plain arithmetic
    /// that provably cannot overflow, which is why nothing downstream carries a
    /// failure arm no test could enter.
    pub fn commit(&self) -> Result<Commit, ManifestError> {
        if self.n_valid > MAX_ENTRIES {
            return Err(ManifestError::TooManyEntries {
                n_valid: self.n_valid,
                limit: MAX_ENTRIES,
            });
        }
        Ok(self.commit_within_bounds())
    }

    /// [`ManifestHeader::commit`] for a header whose counter is already known
    /// to be within [`MAX_ENTRIES`].
    ///
    /// Private, and the precondition is not a comment on a public door: the
    /// only caller other than [`ManifestHeader::commit`] is
    /// [`Manifest::record`], which reaches it through
    /// [`ManifestHeader::advance`] — and `advance` refuses a counter past
    /// [`MAX_ENTRIES`] before this can be called.
    fn commit_within_bounds(&self) -> Commit {
        let slot = self.generation % SLOT_COUNT;
        Commit {
            slot,
            offset: slot * SLOT_STRIDE,
            bytes: self.image(),
            durable_through: offset_within_bounds(self.n_valid),
            header: *self,
        }
    }

    /// The 64-byte slot image for this header, checksum computed.
    #[must_use]
    pub fn image(&self) -> [u8; IMAGE_LEN] {
        let mut out = [0u8; IMAGE_LEN];
        write_at(&mut out, H_MAGIC, MAGIC);
        write_at(&mut out, H_VERSION, self.format_version.to_le_bytes());
        write_at(&mut out, H_STRIDE, self.entry_stride.to_le_bytes());
        write_at(&mut out, H_GENERATION, self.generation.to_le_bytes());
        write_at(&mut out, H_N_VALID, self.n_valid.to_le_bytes());
        write_at(&mut out, H_N_KEYS, self.n_keys.to_le_bytes());
        write_at(&mut out, H_TOTAL_ROWS, self.total_rows.to_le_bytes());
        write_text(&mut out, H_VENDOR, self.vendor.as_str());
        seal(&mut out);
        out
    }

    /// Decodes one slot, checksum and all.
    ///
    /// # Errors
    ///
    /// [`ManifestError::SlotTooShort`], [`ManifestError::NotAManifest`],
    /// [`ManifestError::UnknownVersion`], [`ManifestError::SlotChecksum`],
    /// [`ManifestError::StrideMismatch`] or [`ManifestError::UnknownVendor`] —
    /// checked in that order, so the most basic disagreement is the one
    /// reported.
    pub fn decode(slot: &[u8]) -> Result<Self, ManifestError> {
        let image = image_of(slot).map_err(|len| ManifestError::SlotTooShort { len })?;
        let magic: [u8; 8] = le_bytes(&image, H_MAGIC);
        if magic
            .iter()
            .take(MAGIC_FAMILY.len())
            .ne(MAGIC_FAMILY.iter())
        {
            return Err(ManifestError::NotAManifest);
        }
        let format_version = u16::from_le_bytes(le_bytes(&image, H_VERSION));
        if format_version != FORMAT_VERSION || magic != MAGIC {
            return Err(ManifestError::UnknownVersion(format_version));
        }
        verify(&image)
            .map_err(|(stored, computed)| ManifestError::SlotChecksum { stored, computed })?;
        let entry_stride = u16::from_le_bytes(le_bytes(&image, H_STRIDE));
        if entry_stride != ENTRY_STRIDE_U16 {
            return Err(ManifestError::StrideMismatch(entry_stride));
        }
        let name = text_at(&image, H_VENDOR, H_VENDOR_LEN).ok_or(ManifestError::UnknownVendor)?;
        let vendor = Vendor::ALL
            .into_iter()
            .find(|v| v.as_str() == name)
            .ok_or(ManifestError::UnknownVendor)?;
        Ok(Self {
            format_version,
            entry_stride,
            vendor,
            generation: u64::from_le_bytes(le_bytes(&image, H_GENERATION)),
            n_valid: u64::from_le_bytes(le_bytes(&image, H_N_VALID)),
            n_keys: u64::from_le_bytes(le_bytes(&image, H_N_KEYS)),
            total_rows: u64::from_le_bytes(le_bytes(&image, H_TOTAL_ROWS)),
        })
    }

    /// Reads the whole header region and returns every generation that
    /// survived, newest first, together with anything stepped over on the way.
    ///
    /// Each of the first [`store::format::MAX_SLOTS`] slot positions is decoded
    /// independently, and every valid one whose generation the region's entry
    /// capacity supports becomes a candidate, in generation order. A slot that
    /// is empty, stale, half-written, corrupt or in the wrong position is not a
    /// candidate — and the reason it is not is **returned**, in
    /// [`HeaderRead::stepped_over`], rather than dropped.
    ///
    /// It does not condemn the file when the newest slot fails its check
    /// against the capacity. That is the crash where the header became durable
    /// before the entries it counts, and the previous generation is sitting
    /// intact in the other slot.
    ///
    /// # Errors
    ///
    /// [`ManifestError::HeaderRegionTooShort`],
    /// [`ManifestError::NoValidHeader`] when no slot decodes and none gave a
    /// specific reason, the most specific refusal any slot did give, or
    /// [`ManifestError::VendorMismatch`] when the surviving header names
    /// another vendor.
    pub fn read_region(
        region: &[u8],
        capacity: u64,
        vendor: Vendor,
    ) -> Result<HeaderRead, ManifestError> {
        let slots = whole_slots(region);
        if slots < SLOT_COUNT {
            return Err(ManifestError::HeaderRegionTooShort {
                slots,
                need: SLOT_COUNT,
            });
        }

        let mut candidates: Vec<Self> = Vec::with_capacity(MAX_SLOTS);
        let mut fault: Option<ManifestError> = None;
        for (index, chunk) in (0u64..).zip(region.chunks(SLOT_STRIDE_LEN).take(MAX_SLOTS)) {
            match Self::decode(chunk) {
                Ok(header) => {
                    let expected = header.generation % SLOT_COUNT;
                    if expected == index {
                        candidates.push(header);
                    } else {
                        fault.get_or_insert(ManifestError::SlotPositionMismatch {
                            expected,
                            found: index,
                        });
                    }
                }
                Err(refusal) => {
                    if is_specific(refusal) {
                        fault.get_or_insert(refusal);
                    }
                }
            }
        }

        // Newest first. Bounded by the candidate list rather than by the
        // generation strictly decreasing: a loop that leaned on the comparison
        // would hang rather than fail if that comparison stopped being strict,
        // and a hang is the one failure a test suite cannot report.
        candidates.sort_unstable_by_key(|header| std::cmp::Reverse(header.generation));
        let mut newest: Option<Self> = None;
        let mut older: Option<Self> = None;
        // Kept apart from `fault` on purpose. A generation the region cannot
        // support is what an operator needs to hear about first — it is the
        // state a writer published — and it is reported for the NEWEST such
        // generation rather than the last one looked at.
        let mut unsupported: Option<ManifestError> = None;
        for candidate in candidates {
            match candidate.validate(capacity) {
                Ok(()) => {
                    if candidate.vendor != vendor {
                        return Err(ManifestError::VendorMismatch {
                            asked: vendor,
                            found: candidate.vendor,
                        });
                    }
                    // Two slots, so two places to put a survivor. The assertion
                    // below is what keeps that spelling honest: a third slot
                    // would silently overwrite `older` here, so a family that
                    // grew one would be a compile error rather than a lost
                    // generation.
                    if newest.is_none() {
                        newest = Some(candidate);
                    } else {
                        older = Some(candidate);
                    }
                }
                Err(refused) => {
                    unsupported.get_or_insert(refused);
                }
            }
        }
        let stepped_over = unsupported.or(fault);
        let Some(newest) = newest else {
            return Err(stepped_over.unwrap_or(ManifestError::NoValidHeader));
        };
        Ok(HeaderRead {
            newest,
            older,
            stepped_over,
        })
    }
}

const _: () = assert!(MAX_SLOTS == 2);

/// What one header region held: the generations that survived, and what did
/// not.
///
/// Returned by [`ManifestHeader::read_region`] rather than a bare header,
/// because both of the other two answers are load-bearing and both used to be
/// thrown away. The older generation is what makes [`Manifest::load`]'s
/// fall-back real rather than only true for a region that is physically short,
/// and `stepped_over` is what makes the fall-back loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderRead {
    newest: ManifestHeader,
    older: Option<ManifestHeader>,
    stepped_over: Option<ManifestError>,
}

impl HeaderRead {
    /// The highest generation that survived. This is the committed state.
    #[must_use]
    pub const fn newest(&self) -> ManifestHeader {
        self.newest
    }

    /// The generation below it, if the other slot still holds one.
    ///
    /// The recovery point. A commit that became durable before the entries it
    /// counts leaves this one describing exactly the prefix that *is* durable.
    #[must_use]
    pub const fn older(&self) -> Option<ManifestHeader> {
        self.older
    }

    /// A refusal that was stepped over to produce [`HeaderRead::newest`].
    ///
    /// `Some` means the file is damaged even though a header was recovered: a
    /// slot that failed its checksum, one that names a version this build does
    /// not read, one found in the wrong position, or a generation whose counter
    /// the region cannot support. There is at most one, because a fault costs a
    /// slot and there are two.
    #[must_use]
    pub const fn stepped_over(&self) -> Option<ManifestError> {
        self.stepped_over
    }
}

/// One vendor's census, loaded.
///
/// Holds the header — the counters — and an index from key to the newest entry
/// for that key. The index is reserved from the **committed entry count**,
/// which is known before the walk begins and which `validate` has already
/// checked against the region, so it never rehashes and the reservation is
/// proportional to the census rather than to the file it arrived in:
/// `docs/07-o1-architecture.md` layer 3, O(1) **worst case** rather than
/// average. [`Manifest::reserved`] is that number, and M-17 asserts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    header: ManifestHeader,
    index: HashMap<EntryKey, Entry>,
    degraded: Option<ManifestError>,
}

impl Manifest {
    /// An empty manifest for a vendor that has none yet.
    ///
    /// **Private, and D-0036 is the reason.** A writer that called this on a
    /// vendor which already had a manifest produced a census that was silently,
    /// verifiably wrong and still loaded clean: the stale slot 0 wins on
    /// generation, the fresh generation-1 commit lands in slot 1, entry 0 is
    /// overwritten, and — because real index months share a row count — the
    /// recomputed key count and row total still agree with the stale header. So
    /// the only public doors are [`Manifest::open`], which hands out a genesis
    /// only for a file that has nothing in it, and [`Manifest::load`].
    ///
    /// The index reserves nothing, because nothing is known yet to reserve
    /// from. That is a write-path cost — one rehash per doubling as keys are
    /// first recorded — and it is stated rather than hidden: layer 3's
    /// guarantee is about the loaded index, which is the one a query touches.
    /// See `docs/06-limits.md` §17.
    fn genesis(vendor: Vendor) -> Self {
        Self {
            header: ManifestHeader::genesis(vendor),
            index: HashMap::new(),
            degraded: None,
        }
    }

    /// The manifest of a vendor whose file may or may not exist yet.
    ///
    /// Both regions empty is a file with nothing in it, and that — and only
    /// that — is a genesis manifest. Anything else is [`Manifest::load`], so a
    /// writer cannot start a new census over a file that already holds one.
    ///
    /// # Errors
    ///
    /// Whatever [`Manifest::load`] refuses.
    pub fn open(
        vendor: Vendor,
        header_region: &[u8],
        entries: &[u8],
    ) -> Result<Self, ManifestError> {
        if header_region.is_empty() && entries.is_empty() {
            return Ok(Self::genesis(vendor));
        }
        Self::load(vendor, header_region, entries)
    }

    /// Loads a manifest from its header region and its entry region.
    ///
    /// This is the one scan, and it is where the counters are earned: every
    /// entry below `n_valid` is decoded and checksum-verified, the distinct
    /// keys and the row total are recomputed, and a header whose counters do
    /// not match what the entries actually hold is refused. Afterwards every
    /// question this type answers is a field read or one hash probe.
    ///
    /// # It walks *down* the generations, and says that it did
    ///
    /// If the newest committed generation does not describe what is on disk —
    /// its tail entry is zeroed, or garbage, or half-written, all of which are
    /// ordinary results of a crash between a data write and its flush — the
    /// generation below it is tried, because that one counts a prefix of these
    /// same entries and the whole point of alternating slots is that it is
    /// still intact. Whichever generation loads,
    /// [`Manifest::degraded_reason`] carries what was stepped over. Before
    /// D-0036 the fall-back existed only for a region that was physically too
    /// **short**, so the commonest shape of the crash it was written for
    /// condemned the file and left recovery to the ~248,000-file directory walk
    /// this module exists to avoid.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] — a header that does not decode, a counter the region
    /// cannot support, an entry that fails its own checksum or its own checks,
    /// a key whose history goes backwards, or a counter that disagrees with the
    /// entries it claims to count. When no generation survives, the refusal
    /// reported is the **newest** one's: that is the state a writer published,
    /// and `ManifestError` is `Copy` and does not nest.
    pub fn load(
        vendor: Vendor,
        header_region: &[u8],
        entries: &[u8],
    ) -> Result<Self, ManifestError> {
        let capacity = whole_entries(entries);
        let read = ManifestHeader::read_region(header_region, capacity, vendor)?;

        let header = read.newest();
        let published = match Self::walk(header, entries) {
            Ok(index) => {
                return Ok(Self {
                    header,
                    index,
                    degraded: read.stepped_over(),
                });
            }
            Err(fault) => fault,
        };
        // The published generation does not describe these bytes. The one below
        // it counts a prefix of them, and `read_region` has already validated
        // it against the region. Nothing is lost by not also carrying
        // `stepped_over` here: a stepped-over slot costs a candidate, and with
        // two slots a file cannot both have one and still offer an older
        // generation.
        let Some(previous) = read.older() else {
            return Err(published);
        };
        Self::walk(previous, entries)
            .map(|index| Self {
                header: previous,
                index,
                degraded: Some(published),
            })
            .map_err(|_| published)
    }

    /// Decodes every committed entry under one header and checks the counters
    /// against them.
    ///
    /// Split out of [`Manifest::load`] so that walking a second generation is
    /// the same code rather than a paraphrase of it.
    fn walk(
        header: ManifestHeader,
        entries: &[u8],
    ) -> Result<HashMap<EntryKey, Entry>, ManifestError> {
        // Reserved from the COUNTER, which `validate` has already proved is
        // within both `MAX_ENTRIES` and the region's capacity -- not from the
        // region's byte length, which is untrusted input. Sizing it from the
        // length made a one-entry census inside a region at the design ceiling
        // allocate 574 MB, because the reservation followed the file rather
        // than the census. The bound is still known before the loop starts, so
        // the map is still built once and never grows: layer 3. The `unwrap_or`
        // arm cannot be reached — `n_valid` is at most `MAX_ENTRIES`, which
        // fits a `usize` on every target this builds for — and it lives in
        // `Result::unwrap_or`, which is not this repository's code to measure.
        let reserve = usize::try_from(header.n_valid).unwrap_or(MAX_ENTRIES_LEN);
        let mut index: HashMap<EntryKey, Entry> = HashMap::with_capacity(reserve);
        let mut n_keys = 0u64;

        for (ordinal, chunk) in (0u64..header.n_valid).zip(entries.chunks_exact(IMAGE_LEN)) {
            let entry =
                Entry::decode(chunk).map_err(|fault| ManifestError::Entry { ordinal, fault })?;
            match index.insert(entry.key, entry) {
                None => n_keys += 1,
                Some(previous) => {
                    if entry.rows < previous.rows {
                        return Err(ManifestError::RowCountWentBackwards {
                            ordinal,
                            previous: previous.rows,
                            next: entry.rows,
                        });
                    }
                    if entry.last_ts_micros < previous.last_ts_micros {
                        return Err(ManifestError::KeyTimestampsOutOfOrder {
                            ordinal,
                            previous: previous.last_ts_micros,
                            next: entry.last_ts_micros,
                        });
                    }
                }
            }
        }

        let mut total_rows = 0u64;
        for entry in index.values() {
            total_rows = total_rows
                .checked_add(entry.rows)
                .ok_or(ManifestError::RowTotalOverflow)?;
        }
        if n_keys != header.n_keys {
            return Err(ManifestError::KeyCountDisagrees {
                header: header.n_keys,
                entries: n_keys,
            });
        }
        if total_rows != header.total_rows {
            return Err(ManifestError::RowTotalDisagrees {
                header: header.total_rows,
                entries: total_rows,
            });
        }
        Ok(index)
    }

    /// The header, counters and all.
    #[must_use]
    pub const fn header(&self) -> ManifestHeader {
        self.header
    }

    /// What this census had to step over to load, if anything.
    ///
    /// `Some` means the file is damaged and this manifest is what could still
    /// be recovered from it: a header slot that failed its checksum, a
    /// generation whose entries were not all there, a slot in the wrong
    /// position. The counters below are then the *recovered* generation's, and
    /// a month that a later generation had committed is not in them.
    ///
    /// **An operator entry point renders this.** `CLAUDE.md` §4 allows
    /// degrading loudly and naming the reason, or refusing — and this crate
    /// performs no I/O and holds no logger, so the loudest thing available to
    /// it is a value the census carries and a `#[must_use]` on the way to it.
    #[must_use]
    pub const fn degraded_reason(&self) -> Option<ManifestError> {
        self.degraded
    }

    /// How many entries the index reserved room for — layer 3, as a number.
    ///
    /// `docs/07-o1-architecture.md`: *"Layer 3's guarantee is the absence of a
    /// rehash, so the bound is the reservation itself."* A guarantee about an
    /// allocation that nothing can observe is a guarantee nothing can test, so
    /// this exposes it: `pull::unit::the_loaded_index_is_reserved_from_the_census`
    /// asserts that a one-entry census inside a large region reserves for the
    /// census and not for the region.
    #[must_use]
    pub fn reserved(&self) -> usize {
        self.index.capacity()
    }

    /// How many entries have been committed. One field read.
    #[must_use]
    pub const fn entries(&self) -> u64 {
        self.header.n_valid
    }

    /// How many distinct `(instrument, timeframe, month)` keys are held. One
    /// field read — the answer to "how many month files do I have", without a
    /// directory walk.
    #[must_use]
    pub const fn keys(&self) -> u64 {
        self.header.n_keys
    }

    /// How many rows are held across every key. One field read.
    #[must_use]
    pub const fn total_rows(&self) -> u64 {
        self.header.total_rows
    }

    /// The newest entry for one key, if any. One hash probe.
    #[must_use]
    pub fn entry(&self, key: &EntryKey) -> Option<Entry> {
        self.index.get(key).copied()
    }

    /// Records one month, returning the two writes that publish it.
    ///
    /// The manifest is append-only: an update to a key that already exists is a
    /// **new entry**, and the newest wins. Nothing on disk is ever rewritten in
    /// place, so a crash during an update leaves the previous state whole.
    ///
    /// `self` is left untouched unless every check passes, so a refused record
    /// cannot leave the in-memory counters describing a write that never
    /// happened.
    ///
    /// A census that loaded degraded may still be appended to, and that is
    /// deliberate: recovering from a torn commit *is* writing the next
    /// generation, and refusing here would leave the commonest crash with no
    /// way forward at all. What the writer may not do is append without having
    /// looked at [`Manifest::degraded_reason`], which is why that answer is
    /// carried on the value rather than logged and forgotten.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Entry`] for an entry that is not internally consistent,
    /// [`ManifestError::RowCountWentBackwards`] or
    /// [`ManifestError::KeyTimestampsOutOfOrder`] for one that contradicts what
    /// is already recorded for its key, [`ManifestError::RowTotalOverflow`], or
    /// whatever [`ManifestHeader::advance`] and [`ManifestHeader::commit`]
    /// refuse.
    pub fn record(&mut self, entry: Entry) -> Result<Append, ManifestError> {
        let ordinal = self.header.n_valid;
        entry
            .check()
            .map_err(|fault| ManifestError::Entry { ordinal, fault })?;

        let (n_keys, total_rows) = match self.index.get(&entry.key) {
            None => (
                self.header.n_keys + 1,
                self.header
                    .total_rows
                    .checked_add(entry.rows)
                    .ok_or(ManifestError::RowTotalOverflow)?,
            ),
            Some(previous) => {
                if entry.rows < previous.rows {
                    return Err(ManifestError::RowCountWentBackwards {
                        ordinal,
                        previous: previous.rows,
                        next: entry.rows,
                    });
                }
                if entry.last_ts_micros < previous.last_ts_micros {
                    return Err(ManifestError::KeyTimestampsOutOfOrder {
                        ordinal,
                        previous: previous.last_ts_micros,
                        next: entry.last_ts_micros,
                    });
                }
                // Subtraction without a check: the comparison two lines above
                // returned early unless `entry.rows >= previous.rows`, and
                // `previous.rows` was itself added into the total. A
                // `checked_sub` here would carry a failure arm no input could
                // reach, which is a branch nobody has checked.
                (
                    self.header.n_keys,
                    self.header
                        .total_rows
                        .checked_add(entry.rows - previous.rows)
                        .ok_or(ManifestError::RowTotalOverflow)?,
                )
            }
        };

        // `advance` has refused a counter past `MAX_ENTRIES`, so `ordinal` is
        // below it and both offsets below are plain arithmetic. Calling the
        // fallible entry points here would add two `?` arms that no input could
        // ever take — a branch nobody has checked, sitting behind a coverage
        // gate that would still report 100%.
        let header = self.header.advance(1, n_keys, total_rows)?;
        let offset = offset_within_bounds(ordinal);
        let commit = header.commit_within_bounds();

        self.index.insert(entry.key, entry);
        self.header = header;
        Ok(Append {
            ordinal,
            offset,
            bytes: entry.image(),
            commit,
        })
    }

    /// The byte offset of entry `ordinal`.
    ///
    /// `HEADER_LEN + ordinal·64`. An add and a multiply — law 4. There is no
    /// index to consult and nothing to search.
    ///
    /// # Errors
    ///
    /// [`ManifestError::OrdinalOutOfRange`] past [`MAX_ENTRIES`]. The bound is
    /// on the ordinal rather than on the product, so the arithmetic itself is
    /// total: `MAX_ENTRIES · 64 + HEADER_LEN` is 134 MB and nowhere near `u64`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::manifest::{Manifest, ManifestError, HEADER_LEN, MAX_ENTRIES};
    /// assert_eq!(Manifest::offset_of(0)?, HEADER_LEN);
    /// assert_eq!(Manifest::offset_of(1)?, HEADER_LEN + 64);
    /// assert_eq!(Manifest::offset_of(1_000)?, HEADER_LEN + 64_000);
    /// assert!(Manifest::offset_of(MAX_ENTRIES + 1).is_err());
    /// # Ok::<(), ManifestError>(())
    /// ```
    pub const fn offset_of(ordinal: u64) -> Result<u64, ManifestError> {
        if ordinal > MAX_ENTRIES {
            return Err(ManifestError::OrdinalOutOfRange {
                ordinal,
                limit: MAX_ENTRIES,
            });
        }
        Ok(offset_within_bounds(ordinal))
    }
}

/// `HEADER_LEN + ordinal · ENTRY_STRIDE`, for an ordinal already known to be
/// within [`MAX_ENTRIES`].
///
/// Total, not fallible. `MAX_ENTRIES · ENTRY_STRIDE + HEADER_LEN` is 134 MB, so
/// past the bound check there is nothing left that can overflow — and therefore
/// nothing left that needs a failure arm.
const fn offset_within_bounds(ordinal: u64) -> u64 {
    HEADER_LEN + ordinal * ENTRY_STRIDE
}

const _: () = assert!(MAX_ENTRIES * ENTRY_STRIDE + HEADER_LEN < u64::MAX);

/// Whether a refusal says something about the file, or only that a slot is not
/// a header at all.
const fn is_specific(refusal: ManifestError) -> bool {
    !matches!(
        refusal,
        ManifestError::NotAManifest | ManifestError::SlotTooShort { .. }
    )
}

/// How many whole slot positions the region holds, capped at the family bound.
fn whole_slots(region: &[u8]) -> u64 {
    region
        .chunks_exact(SLOT_STRIDE_LEN)
        .take(MAX_SLOTS)
        .fold(0u64, |seen, _| seen + 1)
}

/// How many whole entries the region holds, capped at [`MAX_ENTRIES`].
///
/// Counted by folding rather than by `len()` so the answer is a `u64` without a
/// `usize` conversion whose failure arm no test on a 64-bit host could reach,
/// and bounded so the fold cannot overflow. It walks the region, which is O(n)
/// — it is on the load path, where a walk is already being paid, and it is the
/// last one: every question afterwards is a field read or a hash probe.
fn whole_entries(entries: &[u8]) -> u64 {
    entries
        .chunks_exact(IMAGE_LEN)
        .take(MAX_ENTRIES_LEN)
        .fold(0u64, |seen, _| seen + 1)
}

/// Copies the first [`IMAGE_LEN`] bytes, or reports how few there were.
///
/// One observation of the caller's bytes. Everything downstream reads this
/// array and nothing touches the caller's slice again, so the checksum covers
/// exactly the bytes the decoded value is built from.
fn image_of(bytes: &[u8]) -> Result<[u8; IMAGE_LEN], usize> {
    if bytes.len() < IMAGE_LEN {
        return Err(bytes.len());
    }
    let mut image = [0u8; IMAGE_LEN];
    for (dst, src) in image.iter_mut().zip(bytes.iter()) {
        *dst = *src;
    }
    Ok(image)
}

/// Writes the checksum over bytes `0..60` into bytes `60..64`.
fn seal(image: &mut [u8; IMAGE_LEN]) {
    let crc = crc32c(&covered(image));
    write_at(image, OFF_CRC, crc.to_le_bytes());
}

/// Checks an image's stored checksum, reporting both numbers when it fails.
fn verify(image: &[u8; IMAGE_LEN]) -> Result<(), (u32, u32)> {
    let stored = u32::from_le_bytes(le_bytes(image, OFF_CRC));
    let computed = crc32c(&covered(image));
    if stored == computed {
        Ok(())
    } else {
        Err((stored, computed))
    }
}

/// Every byte an image's checksum covers: `0..60`, one contiguous run.
///
/// **The domain is frozen.** Covering the four checksum bytes as well would be
/// a different number over a different domain, and every image already on disk
/// would fail its check — a change no round-trip test would notice, because a
/// checksum that agrees with itself always agrees with itself.
/// `pull::unit::the_covered_domain_is_the_image_minus_its_checksum` pins it.
///
/// Returned as an owned array rather than as a sub-slice because this workspace
/// denies slice indexing. Sixty bytes copied once per entry is not on a path
/// that repeats: the census is answered from the header.
fn covered(image: &[u8; IMAGE_LEN]) -> [u8; OFF_CRC] {
    let mut head = [0u8; OFF_CRC];
    for (dst, src) in head.iter_mut().zip(image.iter()) {
        *dst = *src;
    }
    head
}

/// Writes `src` at `offset`.
fn write_at<const N: usize>(out: &mut [u8; IMAGE_LEN], offset: usize, src: [u8; N]) {
    for (dst, byte) in out.iter_mut().skip(offset).zip(src) {
        *dst = byte;
    }
}

/// Writes `text` at `offset`, leaving the rest of its field zero.
fn write_text(out: &mut [u8; IMAGE_LEN], offset: usize, text: &str) {
    for (dst, byte) in out.iter_mut().skip(offset).zip(text.bytes()) {
        *dst = byte;
    }
}

/// Reads `N` little-endian bytes at `offset`.
///
/// Index-free by construction — this workspace denies slice indexing, and a
/// decoder is exactly the place a panicking index would eventually be reached
/// by a corrupt file. Every caller has already copied a whole [`IMAGE_LEN`]
/// image, so no read is ever short.
fn le_bytes<const N: usize>(image: &[u8; IMAGE_LEN], offset: usize) -> [u8; N] {
    let mut out = [0u8; N];
    for (dst, src) in out.iter_mut().zip(image.iter().skip(offset)) {
        *dst = *src;
    }
    out
}

/// The NUL-terminated text in a fixed-width field, or `None` if it is not
/// UTF-8.
fn text_at(image: &[u8; IMAGE_LEN], start: usize, len: usize) -> Option<&str> {
    let field = image.get(start..start + len).unwrap_or(&[]);
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    std::str::from_utf8(field.get(..end).unwrap_or(&[])).ok()
}
