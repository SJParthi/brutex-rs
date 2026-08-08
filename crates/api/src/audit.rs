//! What every pull did, written where a restart cannot lose it.
//!
//! # Why this is a file and not a `Vec` behind a lock
//!
//! The operator asked for a record they can read afterwards. An in-memory ring
//! would have been three lines and one `Mutex`, and it would be empty in
//! exactly the situation the record is wanted in: the server fell over, or was
//! restarted, and the question is *what was the last thing it did*. A log that
//! vanishes on restart is a log that is missing when it is needed, so this one
//! is on disk, it is `fsync`-ed before the answer is rendered, and
//! [`Journal::path`] is printed on the page so the claim is checkable rather
//! than taken.
//!
//! It also has to be shared, and a shared mutable buffer inside the server is a
//! lock on the render path — the one thing `crates/api/src/server.rs` says it
//! does not have. An append-only file needs no lock here: the kernel's
//! `O_APPEND` places the bytes, and every reader opens its own handle.
//!
//! # Why the records are fixed stride
//!
//! Because the page must be able to show *the last twenty runs* without reading
//! the first twenty thousand. A newline-delimited text log cannot answer "where
//! does record `n - 20` start" without walking, so the tail of a text log costs
//! the whole log. Every record here is exactly [`RECORD_LEN`] bytes, so the
//! count is `len / 256` — one `metadata` call — and the offset of any record is
//! a multiplication. That is the same argument `docs/02-store-format.md` makes
//! for bars and `crates/pull/src/manifest.rs` makes for entries; this file is
//! the third instance of it and deliberately not a fourth idea.
//! `api::audit::the_tail_reads_only_the_records_it_shows` is the proof, and it
//! is written as a byte-offset assertion rather than as a timing.
//!
//! # What a torn write looks like
//!
//! One record is one `write_all` of 256 bytes to a handle opened `O_APPEND`,
//! followed by `sync_all`. A process killed between the two leaves the bytes
//! placed but unsynced; a process killed *inside* the write can in principle
//! leave a partial record. Both are visible rather than silent: the file length
//! stops being a multiple of [`RECORD_LEN`], which [`Log::Held::torn`] reports
//! by name, and any record whose bytes are wrong fails its CRC-32C and is
//! rendered as a refusal in its own row instead of as a number. **Nothing here
//! repairs a damaged record.** `CLAUDE.md` §4 — degrade loudly and name the
//! reason, or refuse.
//!
//! # What is deliberately not here
//!
//! No rotation, no compaction, and no cap on the file. A pull is an operator
//! action and each one costs 256 bytes, so a hundred thousand pulls is 25.6 MB.
//! Rotation would mean a second file, an ordering rule between the two and a
//! deletion — and `CLAUDE.md` §3 rule 8 makes history append-only. The growth
//! is stated on the page instead, in bytes, so nobody has to guess.

use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pull::ingest::Ingested;
use pull::session::{DropCensus, DropReason, Window};
use store::crc::crc32c;

/// One record, in bytes. Every record is exactly this long, always.
pub const RECORD_LEN: usize = 256;

/// [`RECORD_LEN`] as the width an offset is computed in.
///
/// Written as a literal in both widths rather than converted, for the reason
/// `crates/api/src/census.rs` gives about its own pair: there is no cast to
/// deny and no fallible conversion whose failure arm no test could reach. The
/// assertion below keeps the two honest.
pub const RECORD_LEN_U64: u64 = 256;
const _: () = assert!(RECORD_LEN as u64 == RECORD_LEN_U64);

/// The four bytes a record starts with.
///
/// A file that is not this journal is refused by name rather than decoded into
/// plausible counters.
const MAGIC: [u8; 4] = *b"BXAU";

/// Which layout the record after it is in.
///
/// A version is per record and not per file, so a journal written by two builds
/// is readable by the newer one: an unknown version is refused for that record
/// alone and every other row still renders. `CLAUDE.md` §3 rule 8 — a format
/// version is never mutated in place.
pub const VERSION: u16 = 1;

/// The longest source this record keeps, in bytes.
const SOURCE_CAPACITY: usize = 64;

/// The longest note this record keeps, in bytes.
const NOTE_CAPACITY: usize = 68;

// The field map. Every offset is a constant so the writer and the reader
// cannot disagree about one, which is the failure a hand-counted literal has.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_SCOPE: usize = 6;
const OFF_OUTCOME: usize = 7;
const OFF_AT: usize = 8;
const OFF_ELAPSED: usize = 16;
const OFF_MEMBERS: usize = 24;
const OFF_ROWS_READ: usize = 32;
const OFF_BARS_STORED: usize = 40;
const OFF_ROWS_FOLDED: usize = 48;
const OFF_COUNTED: usize = 56;
const OFF_BEFORE_OPEN: usize = 64;
const OFF_AFTER_CLOSE: usize = 72;
const OFF_BEFORE_WINDOW: usize = 80;
const OFF_AFTER_WINDOW: usize = 88;
const OFF_FAILURES: usize = 96;
const OFF_FROM_DAYS: usize = 104;
const OFF_TO_DAYS: usize = 108;
const OFF_SOURCE_LEN: usize = 112;
const OFF_NOTE_LEN: usize = 114;
const OFF_SOURCE_KEPT: usize = 116;
const OFF_NOTE_KEPT: usize = 117;
const OFF_SOURCE: usize = 120;
const OFF_NOTE: usize = 184;
const OFF_CRC: usize = 252;

/// The layout above adds up to exactly one record, checked here rather than in
/// a comment.
const _: () = assert!(OFF_SOURCE + SOURCE_CAPACITY == OFF_NOTE);
const _: () = assert!(OFF_NOTE + NOTE_CAPACITY == OFF_CRC);
const _: () = assert!(OFF_CRC + 4 == RECORD_LEN);

/// Which form the run came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `/pull/spot`.
    Spot,
    /// `/pull/fno`.
    Fno,
}

impl Scope {
    /// The byte this scope is stored as.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Spot => 0,
            Self::Fno => 1,
        }
    }

    /// The scope a stored byte names.
    #[must_use]
    pub const fn of_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Spot),
            1 => Some(Self::Fno),
            _ => None,
        }
    }

    /// What the page calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::Fno => "expired F&O",
        }
    }
}

/// How the run ended.
///
/// Four states and not three: "it stored bars", "I would not accept the
/// request", "I accepted it and there is no transport to serve it" and "the
/// transport ran and something failed" are four different things to an
/// operator, and collapsing any pair of them loses the diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Bars reached the store and the census counted them.
    Stored,
    /// The request was refused before anything ran.
    Refused,
    /// The request was understood and no transport exists to serve it.
    NotStarted,
    /// Something ran and did not finish clean.
    Failed,
}

impl Outcome {
    /// The byte this outcome is stored as.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Stored => 0,
            Self::Refused => 1,
            Self::NotStarted => 2,
            Self::Failed => 3,
        }
    }

    /// The outcome a stored byte names.
    #[must_use]
    pub const fn of_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Stored),
            1 => Some(Self::Refused),
            2 => Some(Self::NotStarted),
            3 => Some(Self::Failed),
            _ => None,
        }
    }

    /// The one word at the top of a row.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stored => "STORED",
            Self::Refused => "REFUSED",
            Self::NotStarted => "NOT STARTED",
            Self::Failed => "FAILED",
        }
    }

    /// Whether an operator has to be told about this row.
    #[must_use]
    pub const fn is_loud(self) -> bool {
        !matches!(self, Self::Stored)
    }
}

/// Rows that did not become bars, by reason, in the width a record stores.
///
/// A separate type from [`DropCensus`] on purpose. `DropCensus` is a *counter*
/// — its fields are private and `count` is the only way in, so rebuilding one
/// from four totals would mean calling `count` once per drop, which is a walk
/// over a number that was already known. This is the same four facts as a
/// value, so a record read off disk becomes a rendered panel without a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Drops {
    /// Before 09:15 IST.
    pub before_open: u64,
    /// At or after 15:30 IST.
    pub after_close: u64,
    /// Before the operator's window.
    pub before_window: u64,
    /// After the operator's window.
    pub after_window: u64,
}

impl Drops {
    /// The same four counts a filter recorded.
    #[must_use]
    pub fn of_census(census: DropCensus) -> Self {
        Self {
            before_open: u64::from(census.of(DropReason::BeforeSessionOpen)),
            after_close: u64::from(census.of(DropReason::AtOrAfterSessionClose)),
            before_window: u64::from(census.of(DropReason::BeforeWindow)),
            after_window: u64::from(census.of(DropReason::AfterWindow)),
        }
    }

    /// How many were dropped for one reason.
    #[must_use]
    pub const fn of(self, reason: DropReason) -> u64 {
        match reason {
            DropReason::BeforeSessionOpen => self.before_open,
            DropReason::AtOrAfterSessionClose => self.after_close,
            DropReason::BeforeWindow => self.before_window,
            DropReason::AfterWindow => self.after_window,
            // `DropReason` is `#[non_exhaustive]`, so a fifth reason added in
            // `pull` compiles here and reports zero rather than failing to
            // build. It is zero and not a dash because this type carries four
            // counters and a fifth reason has none — the honest answer is that
            // this build never counted it, and `DROP_REASONS` below is what
            // the page iterates, so a reason this build cannot count is also a
            // reason the page never names.
            //
            // NO TEST DRIVES THIS ARM AND NONE CAN. A fifth variant would have
            // to be constructed, and `pull::session::DropReason` is a foreign
            // `#[non_exhaustive]` enum with exactly four constructors. It is
            // named here rather than left for a coverage report to find.
            _ => 0,
        }
    }

    /// How many were dropped in total.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.before_open
            .saturating_add(self.after_close)
            .saturating_add(self.before_window)
            .saturating_add(self.after_window)
    }

    /// The largest single reason, or one, so a share is never divided by zero.
    #[must_use]
    pub const fn peak(self) -> u64 {
        let mut top = self.before_open;
        if self.after_close > top {
            top = self.after_close;
        }
        if self.before_window > top {
            top = self.before_window;
        }
        if self.after_window > top {
            top = self.after_window;
        }
        if top == 0 { 1 } else { top }
    }
}

/// Every reason this build counts, in the order a page shows them.
///
/// [`DropReason`] is `#[non_exhaustive]` so there is no `ALL` to iterate. The
/// four are listed once, here, and every surface reads this rather than writing
/// its own list — which is what stops a page and a record disagreeing about
/// which reasons exist.
pub const DROP_REASONS: [DropReason; 4] = [
    DropReason::BeforeWindow,
    DropReason::AfterWindow,
    DropReason::BeforeSessionOpen,
    DropReason::AtOrAfterSessionClose,
];

/// One pull, as it is written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// When the answer was written, in epoch seconds.
    pub at_unix_secs: i64,
    /// How long the run took, measured across the call that did the work.
    pub elapsed_micros: u64,
    /// Which form it came from.
    pub scope: Scope,
    /// How it ended.
    pub outcome: Outcome,
    /// Members the folder held.
    pub members: u64,
    /// Rows read out of the vendor's files.
    pub rows_read: u64,
    /// Bars written to the store.
    pub bars_stored: u64,
    /// Rows merged into a bar that was already open.
    pub rows_folded: u64,
    /// Slices the census holds after the run.
    pub counted: u64,
    /// Rows dropped, by reason.
    pub drops: Drops,
    /// Members that failed.
    pub failures: u64,
    /// The window's first day, as days from the epoch.
    pub from_days: u32,
    /// The window's last day, as days from the epoch.
    pub to_days: u32,
    /// What was asked for: the target, or the folder.
    pub source: String,
    /// How long `source` was before it was cut, in bytes.
    pub source_bytes: u16,
    /// Why it ended the way it did, in the refusal's own words.
    pub note: String,
    /// How long `note` was before it was cut, in bytes.
    pub note_bytes: u16,
}

impl Record {
    /// A record of a run that never ran.
    #[must_use]
    pub fn refused(
        scope: Scope,
        outcome: Outcome,
        at: SystemTime,
        source: &str,
        note: &str,
    ) -> Self {
        Self {
            at_unix_secs: crate::ingest::epoch_secs(at),
            elapsed_micros: 0,
            scope,
            outcome,
            members: 0,
            rows_read: 0,
            bars_stored: 0,
            rows_folded: 0,
            counted: 0,
            drops: Drops::default(),
            failures: 0,
            from_days: 0,
            to_days: 0,
            source: keep(source, SOURCE_CAPACITY),
            source_bytes: saturating_len(source),
            note: keep(note, NOTE_CAPACITY),
            note_bytes: saturating_len(note),
        }
    }

    /// The window this record is about.
    #[must_use]
    pub fn with_window(mut self, window: Window) -> Self {
        self.from_days = window.from().days_from_epoch();
        self.to_days = window.to().days_from_epoch();
        self
    }

    /// A record of a run that ran.
    ///
    /// Every figure comes off [`Ingested`] rather than being recomputed here: a
    /// second arithmetic over the same counts is a second answer waiting to
    /// disagree with the first.
    #[must_use]
    pub fn of_run(
        scope: Scope,
        at: SystemTime,
        elapsed_micros: u64,
        source: &str,
        window: Window,
        done: &Ingested,
    ) -> Self {
        let note = done.failures.first().map_or_else(
            || {
                if done.balances() {
                    "every row accounted for: stored, folded into an open bar, or dropped"
                        .to_owned()
                } else {
                    "THE BOOKS DO NOT BALANCE — see the run's own page".to_owned()
                }
            },
            |first| format!("{} — {}", first.instrument, first.why),
        );
        let outcome = if done.failures.is_empty() && done.balances() {
            Outcome::Stored
        } else {
            Outcome::Failed
        };
        Self {
            at_unix_secs: crate::ingest::epoch_secs(at),
            elapsed_micros,
            scope,
            outcome,
            members: as_u64(done.members),
            rows_read: as_u64(done.rows_read),
            bars_stored: as_u64(done.bars_stored),
            rows_folded: as_u64(done.rows_folded),
            counted: as_u64(done.counted),
            drops: Drops::of_census(done.census),
            failures: as_u64(done.failures.len()),
            from_days: window.from().days_from_epoch(),
            to_days: window.to().days_from_epoch(),
            source: keep(source, SOURCE_CAPACITY),
            source_bytes: saturating_len(source),
            note: keep(&note, NOTE_CAPACITY),
            note_bytes: saturating_len(&note),
        }
    }

    /// Whether the source on the page is shorter than the one that was asked
    /// for.
    #[must_use]
    pub fn source_was_cut(&self) -> bool {
        usize::from(self.source_bytes) > self.source.len()
    }

    /// Whether the note on the page is shorter than the one that was written.
    #[must_use]
    pub fn note_was_cut(&self) -> bool {
        usize::from(self.note_bytes) > self.note.len()
    }

    /// Exactly the bytes to append, checksum included.
    #[must_use]
    pub fn image(&self) -> [u8; RECORD_LEN] {
        let mut out = [0u8; RECORD_LEN];
        write_at(&mut out, OFF_MAGIC, MAGIC);
        write_at(&mut out, OFF_VERSION, VERSION.to_le_bytes());
        write_at(&mut out, OFF_SCOPE, [self.scope.code()]);
        write_at(&mut out, OFF_OUTCOME, [self.outcome.code()]);
        write_at(&mut out, OFF_AT, self.at_unix_secs.to_le_bytes());
        write_at(&mut out, OFF_ELAPSED, self.elapsed_micros.to_le_bytes());
        write_at(&mut out, OFF_MEMBERS, self.members.to_le_bytes());
        write_at(&mut out, OFF_ROWS_READ, self.rows_read.to_le_bytes());
        write_at(&mut out, OFF_BARS_STORED, self.bars_stored.to_le_bytes());
        write_at(&mut out, OFF_ROWS_FOLDED, self.rows_folded.to_le_bytes());
        write_at(&mut out, OFF_COUNTED, self.counted.to_le_bytes());
        write_at(
            &mut out,
            OFF_BEFORE_OPEN,
            self.drops.before_open.to_le_bytes(),
        );
        write_at(
            &mut out,
            OFF_AFTER_CLOSE,
            self.drops.after_close.to_le_bytes(),
        );
        write_at(
            &mut out,
            OFF_BEFORE_WINDOW,
            self.drops.before_window.to_le_bytes(),
        );
        write_at(
            &mut out,
            OFF_AFTER_WINDOW,
            self.drops.after_window.to_le_bytes(),
        );
        write_at(&mut out, OFF_FAILURES, self.failures.to_le_bytes());
        write_at(&mut out, OFF_FROM_DAYS, self.from_days.to_le_bytes());
        write_at(&mut out, OFF_TO_DAYS, self.to_days.to_le_bytes());
        write_at(&mut out, OFF_SOURCE_LEN, self.source_bytes.to_le_bytes());
        write_at(&mut out, OFF_NOTE_LEN, self.note_bytes.to_le_bytes());
        write_at(&mut out, OFF_SOURCE_KEPT, [kept_byte(&self.source)]);
        write_at(&mut out, OFF_NOTE_KEPT, [kept_byte(&self.note)]);
        write_text(&mut out, OFF_SOURCE, &self.source, SOURCE_CAPACITY);
        write_text(&mut out, OFF_NOTE, &self.note, NOTE_CAPACITY);
        let crc = crc32c(&covered(&out));
        write_at(&mut out, OFF_CRC, crc.to_le_bytes());
        out
    }

    /// One record, decoded and checksum-verified.
    ///
    /// # Errors
    ///
    /// [`RecordFault`], checked in this order: magic, checksum, version, then
    /// each field. The checksum before the version, because a version refusal
    /// read out of bytes that already failed their checksum is a diagnosis of
    /// corruption as a schema problem.
    pub fn decode(image: &[u8; RECORD_LEN]) -> Result<Self, RecordFault> {
        if le4(image, OFF_MAGIC) != MAGIC {
            return Err(RecordFault::NotAnAuditRecord);
        }
        let stored = u32::from_le_bytes(le4(image, OFF_CRC));
        let computed = crc32c(&covered(image));
        if stored != computed {
            return Err(RecordFault::Checksum { stored, computed });
        }
        let version = u16::from_le_bytes(le2(image, OFF_VERSION));
        if version != VERSION {
            return Err(RecordFault::UnknownVersion { version });
        }
        let scope_code = byte(image, OFF_SCOPE);
        let Some(scope) = Scope::of_code(scope_code) else {
            return Err(RecordFault::UnknownScope { code: scope_code });
        };
        let outcome_code = byte(image, OFF_OUTCOME);
        let Some(outcome) = Outcome::of_code(outcome_code) else {
            return Err(RecordFault::UnknownOutcome { code: outcome_code });
        };
        let source = text(
            image,
            OFF_SOURCE,
            byte(image, OFF_SOURCE_KEPT),
            SOURCE_CAPACITY,
        )?;
        let note = text(image, OFF_NOTE, byte(image, OFF_NOTE_KEPT), NOTE_CAPACITY)?;
        Ok(Self {
            at_unix_secs: i64::from_le_bytes(le8(image, OFF_AT)),
            elapsed_micros: u64::from_le_bytes(le8(image, OFF_ELAPSED)),
            scope,
            outcome,
            members: u64::from_le_bytes(le8(image, OFF_MEMBERS)),
            rows_read: u64::from_le_bytes(le8(image, OFF_ROWS_READ)),
            bars_stored: u64::from_le_bytes(le8(image, OFF_BARS_STORED)),
            rows_folded: u64::from_le_bytes(le8(image, OFF_ROWS_FOLDED)),
            counted: u64::from_le_bytes(le8(image, OFF_COUNTED)),
            drops: Drops {
                before_open: u64::from_le_bytes(le8(image, OFF_BEFORE_OPEN)),
                after_close: u64::from_le_bytes(le8(image, OFF_AFTER_CLOSE)),
                before_window: u64::from_le_bytes(le8(image, OFF_BEFORE_WINDOW)),
                after_window: u64::from_le_bytes(le8(image, OFF_AFTER_WINDOW)),
            },
            failures: u64::from_le_bytes(le8(image, OFF_FAILURES)),
            from_days: u32::from_le_bytes(le4(image, OFF_FROM_DAYS)),
            to_days: u32::from_le_bytes(le4(image, OFF_TO_DAYS)),
            source_bytes: u16::from_le_bytes(le2(image, OFF_SOURCE_LEN)),
            source,
            note_bytes: u16::from_le_bytes(le2(image, OFF_NOTE_LEN)),
            note,
        })
    }
}

/// Why one record could not be believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordFault {
    /// The first four bytes are not this journal's.
    NotAnAuditRecord,
    /// The stored checksum does not match the bytes.
    Checksum {
        /// What the record carries.
        stored: u32,
        /// What its bytes actually produce.
        computed: u32,
    },
    /// A layout this build does not know.
    UnknownVersion {
        /// The version the record claims.
        version: u16,
    },
    /// A scope byte outside the two this build writes.
    UnknownScope {
        /// The byte found.
        code: u8,
    },
    /// An outcome byte outside the four this build writes.
    UnknownOutcome {
        /// The byte found.
        code: u8,
    },
    /// A kept length past the field it names.
    KeptPastCapacity {
        /// The length the record claims.
        kept: u8,
        /// The field it has.
        capacity: u8,
    },
    /// Text that is not UTF-8.
    NotText,
}

impl std::fmt::Display for RecordFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::NotAnAuditRecord => f.write_str("not an audit record"),
            Self::Checksum { stored, computed } => {
                write!(f, "record checksum {stored:#010x} != {computed:#010x}")
            }
            Self::UnknownVersion { version } => {
                write!(f, "record version {version}; this build writes {VERSION}")
            }
            Self::UnknownScope { code } => {
                write!(f, "scope byte {code} is not one this build writes")
            }
            Self::UnknownOutcome { code } => {
                write!(f, "outcome byte {code} is not one this build writes")
            }
            Self::KeptPastCapacity { kept, capacity } => {
                write!(f, "kept length {kept} past a {capacity}-byte field")
            }
            Self::NotText => f.write_str("a text field is not UTF-8"),
        }
    }
}

impl std::error::Error for RecordFault {}

/// What the journal file was found to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Log {
    /// No file at that path. The ordinary state before the first pull.
    Absent,
    /// A file that exists and cannot be measured.
    Unreadable {
        /// What refused it, in the refusal's own words.
        reason: String,
    },
    /// A file that is there, with the number of whole records it holds.
    Held {
        /// How many whole records. `len / RECORD_LEN`, one field read.
        records: u64,
        /// How large the file is, so the growth is a number and not a worry.
        bytes: u64,
        /// The bytes past the last whole record, when there are any.
        ///
        /// `Some` is a torn write and is loud. It never changes what the whole
        /// records say — they are before it — so the page shows both.
        torn: Option<u64>,
    },
}

impl Log {
    /// How many whole records are readable.
    #[must_use]
    pub const fn records(&self) -> u64 {
        match *self {
            Self::Held { records, .. } => records,
            Self::Absent | Self::Unreadable { .. } => 0,
        }
    }

    /// Whether an operator has to be told about the file itself.
    #[must_use]
    pub const fn is_loud(&self) -> bool {
        match *self {
            Self::Unreadable { .. } => true,
            Self::Held { torn, .. } => torn.is_some(),
            Self::Absent => false,
        }
    }
}

/// The journal file, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal {
    /// The file every record is appended to.
    pub path: PathBuf,
}

/// The directory the journal lives in, beneath the store root.
///
/// Beside `manifest/`, not inside it: a manifest is a counter this build must
/// be able to rewrite, and this is a history it must never rewrite. Two
/// different rules, two different directories.
#[must_use]
pub fn journal_path(root: &Path) -> PathBuf {
    root.join("audit").join("pull.journal")
}

impl Journal {
    /// The journal beneath a store root.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self {
            path: journal_path(root),
        }
    }

    /// What is there, from one `metadata` call and no read.
    ///
    /// Never fails: every outcome is a state the page renders, for the reason
    /// `crates/api/src/census.rs` gives about an absent manifest.
    #[must_use]
    pub fn look(&self) -> Log {
        match std::fs::metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Log::Absent,
            Err(e) => Log::Unreadable {
                reason: e.to_string(),
            },
            Ok(meta) => {
                let bytes = meta.len();
                let records = bytes / RECORD_LEN_U64;
                let spare = bytes % RECORD_LEN_U64;
                Log::Held {
                    records,
                    bytes,
                    torn: if spare == 0 { None } else { Some(spare) },
                }
            }
        }
    }

    /// Appends one record and makes it durable.
    ///
    /// # Errors
    ///
    /// The failure in its own words, prefixed with the path. A pull whose
    /// record could not be written is a pull the operator has to be told about
    /// on the answer page — silently losing the record of a run that wrote
    /// 9.8 MB of bars is exactly the fallback `CLAUDE.md` §4 forbids.
    ///
    /// # Two arms here are backstops and no test drives them
    ///
    /// `write_all` and `sync_all`. Reaching the first needs a full disk and
    /// the second a failing `fsync`, and neither is a state a developer's disk
    /// enters on request. The two arms that ARE reachable are covered — a file
    /// where the directory has to be, and a path with no parent — and
    /// `crates/store/src/file.rs` solved exactly this shape with a trait so
    /// the arms can be injected. That is the available fix and it is not built
    /// here. `CLAUDE.md` §3 rule 6: said, rather than left to be found.
    pub fn append(&self, record: &Record) -> Result<(), String> {
        let named =
            |what: &str, e: &std::io::Error| format!("{}: {what} — {e}", self.path.display());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| named("cannot create the audit directory", &e))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .map_err(|e| named("cannot open the journal", &e))?;
        file.write_all(&record.image())
            .map_err(|e| named("cannot append the record", &e))?;
        file.sync_all()
            .map_err(|e| named("the record was written and not synced", &e))
    }

    /// One page of records, newest first.
    ///
    /// `skip` and `take` are counted from the **newest** record, so page zero
    /// is the last `take` runs. Only those records are read: the offset is
    /// `(records - skip - take) * RECORD_LEN`, which is a multiplication, and
    /// the read is `take * RECORD_LEN` bytes however long the file is.
    /// `api::audit::the_tail_reads_only_the_records_it_shows` pins the offset
    /// and the length.
    ///
    /// # Errors
    ///
    /// The failure in its own words. A record that will not decode is **not**
    /// an error here — it is an [`Entry`] carrying its own fault, so one
    /// damaged record does not blank the page around it.
    ///
    /// # One arm here is a backstop and no test drives it
    ///
    /// The `seek` refusal. `SeekFrom::Start` takes a `u64` and every value of
    /// it is a legal offset on a regular file — seeking past the end is
    /// defined and succeeds — so on the platforms this builds for there is no
    /// input that reaches it. It is written rather than unwrapped because a
    /// `?` that cannot fire costs nothing and an `unwrap` that cannot fire is
    /// still an `unwrap`. It is named here rather than left for a coverage
    /// report to find, per `CLAUDE.md` §3 rule 6.
    pub fn page(&self, records: u64, skip: u64, take: u64) -> Result<Vec<Entry>, String> {
        let take = take.min(MAX_PAGE_RECORDS);
        let end = records.saturating_sub(skip);
        let start = end.saturating_sub(take);
        let wanted = end.saturating_sub(start);
        if wanted == 0 {
            return Ok(Vec::new());
        }
        // `wanted` is capped at MAX_PAGE_RECORDS above, so this product is at
        // most 51,200 and the conversion cannot fail on any host with a 16-bit
        // `usize` or wider. The arm is a backstop and no test drives it.
        let span = usize::try_from(wanted.saturating_mul(RECORD_LEN_U64))
            .map_err(|e| format!("{wanted} records do not fit this machine's memory: {e}"))?;
        let mut file =
            std::fs::File::open(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        file.seek(std::io::SeekFrom::Start(
            start.saturating_mul(RECORD_LEN_U64),
        ))
        .map_err(|e| {
            format!(
                "{}: cannot seek to record {start} — {e}",
                self.path.display()
            )
        })?;
        let mut buf = vec![0u8; span];
        file.read_exact(&mut buf).map_err(|e| {
            format!(
                "{}: cannot read {wanted} record(s) — {e}",
                self.path.display()
            )
        })?;
        let mut out = Vec::with_capacity(span / RECORD_LEN);
        for (i, chunk) in buf.chunks_exact(RECORD_LEN).enumerate() {
            let mut image = [0u8; RECORD_LEN];
            for (dst, src) in image.iter_mut().zip(chunk.iter()) {
                *dst = *src;
            }
            out.push(Entry {
                ordinal: start.saturating_add(as_u64(i)),
                decoded: Record::decode(&image),
            });
        }
        out.reverse();
        Ok(out)
    }
}

/// The most records one call will ever read.
///
/// A page shows what a person reads; this is the ceiling on the buffer
/// [`Journal::page`] allocates, so a caller that asks for the whole file gets
/// one page of it instead of an allocation the size of the disk.
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary.
pub const MAX_PAGE_RECORDS: u64 = 200;

/// One row of the journal page: which record it is, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its position in the file, counted from the first record ever written.
    pub ordinal: u64,
    /// The record, or why it could not be believed.
    pub decoded: Result<Record, RecordFault>,
}

// ---------------------------------------------------------------------------
// bytes
// ---------------------------------------------------------------------------

/// `n` as a `u64`, saturating rather than wrapping.
///
/// A count that wrapped would report a smaller number than the truth, which is
/// the one direction a count must never be wrong in. On every host this builds
/// for `usize` is 64 bits and the conversion cannot fail, so the arm is a
/// backstop and not a live path.
fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// A text's byte length, saturating at what the record's field can hold.
fn saturating_len(text: &str) -> u16 {
    u16::try_from(text.len()).unwrap_or(u16::MAX)
}

/// The longest prefix of `text` that fits `capacity` bytes and is still text.
///
/// Cut on a character boundary, never mid-sequence: a record holding half a
/// multi-byte character is a record that will not decode, and a page that
/// refuses its own writer's output is worse than a shorter line.
fn keep(text: &str, capacity: usize) -> String {
    if text.len() <= capacity {
        return text.to_owned();
    }
    let mut end = 0;
    for (at, c) in text.char_indices() {
        let next = at.saturating_add(c.len_utf8());
        if next > capacity {
            break;
        }
        end = next;
    }
    text.get(..end).unwrap_or("").to_owned()
}

/// A kept text's length as the byte the record stores.
///
/// [`keep`] never returns more than a capacity, and both capacities are under
/// 255, so the saturation is a backstop. It is written rather than asserted
/// because an assertion here would be a panic on a path that renders a page.
fn kept_byte(text: &str) -> u8 {
    u8::try_from(text.len()).unwrap_or(u8::MAX)
}

/// The text at `offset`, `kept` bytes of it.
fn text(
    image: &[u8; RECORD_LEN],
    offset: usize,
    kept: u8,
    capacity: usize,
) -> Result<String, RecordFault> {
    let kept_usize = usize::from(kept);
    if kept_usize > capacity {
        return Err(RecordFault::KeptPastCapacity {
            kept,
            capacity: u8::try_from(capacity).unwrap_or(u8::MAX),
        });
    }
    let end = offset.saturating_add(kept_usize);
    let bytes = image.get(offset..end).unwrap_or(&[]);
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_ignored| RecordFault::NotText)
}

/// Writes `src` at `offset`.
fn write_at<const N: usize>(out: &mut [u8; RECORD_LEN], offset: usize, src: [u8; N]) {
    for (dst, byte) in out.iter_mut().skip(offset).zip(src) {
        *dst = byte;
    }
}

/// Writes `text` at `offset`, leaving the rest of its field zero.
fn write_text(out: &mut [u8; RECORD_LEN], offset: usize, text: &str, capacity: usize) {
    for (dst, byte) in out.iter_mut().skip(offset).take(capacity).zip(text.bytes()) {
        *dst = byte;
    }
}

/// One byte at `offset`, or zero past the end.
fn byte(image: &[u8; RECORD_LEN], offset: usize) -> u8 {
    image.get(offset).copied().unwrap_or(0)
}

/// Two bytes at `offset`.
fn le2(image: &[u8; RECORD_LEN], offset: usize) -> [u8; 2] {
    let mut out = [0u8; 2];
    for (dst, src) in out.iter_mut().zip(image.iter().skip(offset)) {
        *dst = *src;
    }
    out
}

/// Four bytes at `offset`.
fn le4(image: &[u8; RECORD_LEN], offset: usize) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (dst, src) in out.iter_mut().zip(image.iter().skip(offset)) {
        *dst = *src;
    }
    out
}

/// Eight bytes at `offset`.
fn le8(image: &[u8; RECORD_LEN], offset: usize) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (dst, src) in out.iter_mut().zip(image.iter().skip(offset)) {
        *dst = *src;
    }
    out
}

/// Every byte the checksum covers: `0..252`, one contiguous run.
///
/// **The domain is frozen.** Covering the four checksum bytes as well would be
/// a different number over a different domain, and every record already on disk
/// would fail its check — a change no round-trip test would notice, because a
/// checksum that agrees with itself always agrees with itself.
/// `api::audit::the_covered_domain_is_the_record_minus_its_checksum` pins it.
fn covered(image: &[u8; RECORD_LEN]) -> [u8; OFF_CRC] {
    let mut head = [0u8; OFF_CRC];
    for (dst, src) in head.iter_mut().zip(image.iter()) {
        *dst = *src;
    }
    head
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
    use pull::ingest::Failure;
    use pull::session::Day;

    fn d(y: u16, m: u8, day: u8) -> Day {
        Day::new(y, m, day).expect("a real date")
    }

    fn window() -> Window {
        Window::new(d(2025, 7, 1), d(2025, 7, 1)).expect("forwards")
    }

    fn at(secs: i64) -> SystemTime {
        if secs >= 0 {
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs.unsigned_abs())
        } else {
            SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(secs.unsigned_abs())
        }
    }

    fn census(before_open: u32, after_close: u32, before_w: u32, after_w: u32) -> DropCensus {
        let mut c = DropCensus::new();
        for _ in 0..before_open {
            c.count(DropReason::BeforeSessionOpen);
        }
        for _ in 0..after_close {
            c.count(DropReason::AtOrAfterSessionClose);
        }
        for _ in 0..before_w {
            c.count(DropReason::BeforeWindow);
        }
        for _ in 0..after_w {
            c.count(DropReason::AfterWindow);
        }
        c
    }

    fn run() -> Ingested {
        Ingested {
            members: 194,
            rows_read: 354_675,
            bars_stored: 62_978,
            rows_folded: 291_527,
            counted: 194,
            census: census(0, 170, 0, 0),
            failures: Vec::new(),
        }
    }

    /// A store root nobody else in this process shares, emptied first.
    fn scratch(name: &str) -> PathBuf {
        let dir = crate::scratch::path(name);
        let _ignored = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn a_record_survives_the_round_trip_field_for_field() {
        let record = Record::of_run(
            Scope::Spot,
            at(1_751_356_800),
            4_512_903,
            "/Users/you/Downloads/GDFL/Futures/-III",
            window(),
            &run(),
        );
        let image = record.image();
        assert_eq!(image.len(), 256, "one record is one stride");
        let back = Record::decode(&image).expect("its own bytes");
        assert_eq!(back, record, "every field, not merely the counters");
        assert_eq!(back.bars_stored, 62_978);
        assert_eq!(back.rows_folded, 291_527);
        assert_eq!(back.drops.after_close, 170);
        assert_eq!(back.drops.total(), 170);
        assert_eq!(back.outcome, Outcome::Stored);
        assert_eq!(back.from_days, d(2025, 7, 1).days_from_epoch());
        assert_eq!(back.elapsed_micros, 4_512_903);
    }

    #[test]
    fn the_covered_domain_is_the_record_minus_its_checksum() {
        // A checksum over its own bytes is a number that always agrees with
        // itself. The domain is 0..252 and nothing else, and this is what says
        // so — changing the four checksum bytes must NOT change the covered
        // run, and changing any other byte must.
        let record = Record::refused(Scope::Fno, Outcome::Refused, at(10), "NIFTY", "expired");
        let image = record.image();
        assert_eq!(covered(&image).len(), 252);
        let mut poked = image;
        poked[OFF_CRC] ^= 0xff;
        assert_eq!(
            covered(&poked),
            covered(&image),
            "the checksum bytes are outside the domain"
        );
        let mut elsewhere = image;
        elsewhere[OFF_MEMBERS] ^= 0x01;
        assert_ne!(covered(&elsewhere), covered(&image));
    }

    #[test]
    fn a_flipped_bit_is_refused_by_name_and_both_numbers_are_reported() {
        let record = Record::refused(Scope::Spot, Outcome::NotStarted, at(0), "swept", "no http");
        let mut image = record.image();
        image[OFF_ROWS_READ] ^= 0x40;
        // BOTH NUMBERS ARE NAMED, and the assertion states both rather than
        // matching a shape: the stored checksum is the one still in the poked
        // bytes and the computed one is what those bytes now produce, so an
        // equality here is exact and there is no arm nothing reaches.
        let stored = u32::from_le_bytes(le4(&image, OFF_CRC));
        let computed = crc32c(&covered(&image));
        assert_ne!(stored, computed, "a flipped bit changes the checksum");
        assert_eq!(
            Record::decode(&image),
            Err(RecordFault::Checksum { stored, computed })
        );
        let said = RecordFault::Checksum { stored, computed }.to_string();
        assert!(said.contains("record checksum"), "{said}");
        assert!(said.contains("!="), "and both numbers, not one: {said}");
    }

    #[test]
    fn a_file_that_is_not_this_journal_is_refused_before_its_fields_are_read() {
        let image = [0u8; RECORD_LEN];
        assert_eq!(
            Record::decode(&image),
            Err(RecordFault::NotAnAuditRecord),
            "zeroes are not a record"
        );
        assert_eq!(
            RecordFault::NotAnAuditRecord.to_string(),
            "not an audit record"
        );
    }

    #[test]
    fn a_version_this_build_does_not_know_is_refused_and_named() {
        let mut image = Record::refused(Scope::Spot, Outcome::Refused, at(1), "x", "y").image();
        image[OFF_VERSION] = 9;
        // The checksum has to be re-sealed, or the version arm is unreachable:
        // the checksum is verified first, deliberately.
        let crc = crc32c(&covered(&image));
        for (dst, src) in image.iter_mut().skip(OFF_CRC).zip(crc.to_le_bytes()) {
            *dst = src;
        }
        assert_eq!(
            Record::decode(&image),
            Err(RecordFault::UnknownVersion { version: 9 })
        );
        assert!(
            RecordFault::UnknownVersion { version: 9 }
                .to_string()
                .contains("this build writes 1")
        );
    }

    #[test]
    fn an_unknown_scope_or_outcome_byte_is_refused_rather_than_guessed() {
        for (offset, expect) in [
            (OFF_SCOPE, RecordFault::UnknownScope { code: 7 }),
            (OFF_OUTCOME, RecordFault::UnknownOutcome { code: 7 }),
        ] {
            let mut image = Record::refused(Scope::Spot, Outcome::Refused, at(1), "x", "y").image();
            image[offset] = 7;
            let crc = crc32c(&covered(&image));
            for (dst, src) in image.iter_mut().skip(OFF_CRC).zip(crc.to_le_bytes()) {
                *dst = src;
            }
            assert_eq!(Record::decode(&image), Err(expect));
            assert!(expect.to_string().contains('7'), "{expect:?}");
        }
    }

    #[test]
    fn a_kept_length_past_its_field_is_refused_and_a_broken_byte_is_not_text() {
        let mut image = Record::refused(Scope::Spot, Outcome::Refused, at(1), "x", "y").image();
        image[OFF_SOURCE_KEPT] = 200;
        let crc = crc32c(&covered(&image));
        for (dst, src) in image.iter_mut().skip(OFF_CRC).zip(crc.to_le_bytes()) {
            *dst = src;
        }
        assert_eq!(
            Record::decode(&image),
            Err(RecordFault::KeptPastCapacity {
                kept: 200,
                capacity: 64
            })
        );
        assert_eq!(
            RecordFault::KeptPastCapacity {
                kept: 200,
                capacity: 64
            }
            .to_string(),
            "kept length 200 past a 64-byte field",
            "a refusal an operator can act on names both numbers"
        );

        let mut broken = Record::refused(Scope::Spot, Outcome::Refused, at(1), "abc", "y").image();
        broken[OFF_SOURCE] = 0xff;
        let crc = crc32c(&covered(&broken));
        for (dst, src) in broken.iter_mut().skip(OFF_CRC).zip(crc.to_le_bytes()) {
            *dst = src;
        }
        assert_eq!(Record::decode(&broken), Err(RecordFault::NotText));
        assert_eq!(
            RecordFault::NotText.to_string(),
            "a text field is not UTF-8"
        );

        // THE NOTE FIELD IS CHECKED TOO, not only the source. Two fields share
        // one decoder, so there are two call sites, and a `?` dropped at the
        // second would let a record with unreadable bytes render as an EMPTY
        // reason — which on the page reads exactly like a run that had nothing
        // to say.
        let mut note_broken =
            Record::refused(Scope::Spot, Outcome::Refused, at(1), "x", "why").image();
        note_broken[OFF_NOTE] = 0x80;
        let crc = crc32c(&covered(&note_broken));
        for (dst, src) in note_broken.iter_mut().skip(OFF_CRC).zip(crc.to_le_bytes()) {
            *dst = src;
        }
        assert_eq!(Record::decode(&note_broken), Err(RecordFault::NotText));
    }

    #[test]
    fn an_over_long_source_is_cut_on_a_character_boundary_and_says_it_was_cut() {
        // Every character here is three bytes, so a 64-byte field holds 21 of
        // them and cutting at 64 would land inside the 22nd. A record holding
        // half a character is a record that will not decode.
        let long = "अ".repeat(40);
        assert_eq!(long.len(), 120);
        let record = Record::refused(Scope::Spot, Outcome::Refused, at(1), &long, "y");
        assert_eq!(record.source.len(), 63, "21 whole characters, not 64 bytes");
        assert_eq!(record.source_bytes, 120, "the original length is kept");
        assert!(record.source_was_cut());
        assert!(!record.note_was_cut());
        let back = Record::decode(&record.image()).expect("still decodes");
        assert_eq!(back.source.chars().count(), 21);
        assert_eq!(back.source_bytes, 120);
        assert!(back.source_was_cut());
    }

    #[test]
    fn a_failed_member_becomes_the_note_and_the_outcome_is_not_stored() {
        let mut done = run();
        done.failures.push(Failure {
            instrument: "ABB-III".to_owned(),
            why: "the census refused the record".to_owned(),
        });
        let record = Record::of_run(Scope::Spot, at(5), 1, "folder", window(), &done);
        assert_eq!(record.outcome, Outcome::Failed);
        assert_eq!(record.failures, 1);
        assert!(
            record.note.starts_with("ABB-III — the census"),
            "{}",
            record.note
        );
        assert!(Outcome::Failed.is_loud());
        assert!(!Outcome::Stored.is_loud());
    }

    #[test]
    fn books_that_do_not_balance_are_recorded_as_failed_even_with_no_named_member() {
        let mut done = run();
        done.rows_read += 1; // one row with nowhere to be
        assert!(!done.balances());
        let record = Record::of_run(Scope::Spot, at(5), 1, "folder", window(), &done);
        assert_eq!(record.outcome, Outcome::Failed);
        assert_eq!(
            record.failures, 0,
            "no member failed, and it still does not balance"
        );
        assert!(record.note.contains("DO NOT BALANCE"), "{}", record.note);
    }

    #[test]
    fn an_absent_journal_is_a_state_and_not_a_failure() {
        let root = scratch("audit-absent");
        let journal = Journal::at(&root);
        assert_eq!(journal.look(), Log::Absent);
        assert_eq!(journal.look().records(), 0);
        assert!(!journal.look().is_loud());
        assert!(journal.page(0, 0, 10).expect("no records").is_empty());
        assert!(
            journal.path.ends_with("audit/pull.journal"),
            "{:?}",
            journal.path
        );
    }

    #[test]
    fn appending_creates_the_directory_and_the_count_is_the_length_divided() {
        let root = scratch("audit-append");
        let journal = Journal::at(&root);
        for i in 0..5u32 {
            let record = Record::refused(
                Scope::Spot,
                Outcome::Refused,
                at(i64::from(i)),
                "swept",
                "nothing yet",
            );
            journal.append(&record).expect("appends");
        }
        assert_eq!(
            journal.look(),
            Log::Held {
                records: 5,
                bytes: 5 * 256,
                torn: None,
            },
            "five whole records and nothing past them"
        );
    }

    #[test]
    fn the_tail_reads_only_the_records_it_shows() {
        // The bound this file exists for, asserted as an offset rather than as
        // a timing: page zero of a 40-record journal reads records 30..40 and
        // nothing before them, so the byte span is 10 x 256 whatever the file
        // holds.
        let root = scratch("audit-tail");
        let journal = Journal::at(&root);
        for i in 0..40u32 {
            journal
                .append(&Record::refused(
                    Scope::Spot,
                    Outcome::Refused,
                    at(i64::from(i)),
                    &format!("run-{i}"),
                    "nothing yet",
                ))
                .expect("appends");
        }
        let records = journal.look().records();
        assert_eq!(records, 40);

        let page = journal.page(records, 0, 10).expect("a page");
        assert_eq!(page.len(), 10, "ten rows, not forty");
        assert_eq!(page.first().map(|e| e.ordinal), Some(39), "newest first");
        assert_eq!(page.last().map(|e| e.ordinal), Some(30));
        let newest = page
            .first()
            .expect("a row")
            .decoded
            .clone()
            .expect("decodes");
        assert_eq!(newest.source, "run-39");

        let second = journal.page(records, 10, 10).expect("a page");
        assert_eq!(second.first().map(|e| e.ordinal), Some(29));
        assert_eq!(second.last().map(|e| e.ordinal), Some(20));

        // Past the end is empty rather than an error or a wrap.
        assert!(journal.page(records, 40, 10).expect("empty").is_empty());
        // And a caller asking for more than the ceiling gets the ceiling.
        let all = journal.page(records, 0, u64::MAX).expect("capped");
        assert_eq!(all.len(), 40, "40 records, capped at {MAX_PAGE_RECORDS}");
    }

    #[test]
    fn a_torn_tail_is_named_and_the_whole_records_before_it_still_read() {
        let root = scratch("audit-torn");
        let journal = Journal::at(&root);
        journal
            .append(&Record::refused(
                Scope::Spot,
                Outcome::Stored,
                at(1),
                "whole",
                "",
            ))
            .expect("appends");
        // Half a record, exactly what a process killed inside a write leaves.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&journal.path)
                .expect("opens");
            f.write_all(&[0u8; 100]).expect("writes");
        }
        assert_eq!(
            journal.look(),
            Log::Held {
                records: 1,
                bytes: 356,
                torn: Some(100),
            },
            "the whole record before the tear still counts, and the tear is \
             named in bytes rather than swallowed"
        );
        assert!(journal.look().is_loud(), "a tear is loud");
        let page = journal.page(1, 0, 10).expect("still reads");
        assert_eq!(page.len(), 1);
        assert_eq!(
            page.first()
                .expect("a row")
                .decoded
                .clone()
                .expect("decodes")
                .source,
            "whole"
        );
    }

    #[test]
    fn one_damaged_record_does_not_blank_the_rows_around_it() {
        let root = scratch("audit-damaged");
        let journal = Journal::at(&root);
        for i in 0..3u32 {
            journal
                .append(&Record::refused(
                    Scope::Spot,
                    Outcome::Stored,
                    at(i64::from(i)),
                    &format!("run-{i}"),
                    "",
                ))
                .expect("appends");
        }
        // Corrupt the middle record in place, byte 300 — inside record 1.
        let mut bytes = std::fs::read(&journal.path).expect("reads");
        bytes[300] ^= 0xff;
        std::fs::write(&journal.path, &bytes).expect("writes");

        let page = journal.page(3, 0, 10).expect("a page");
        assert_eq!(page.len(), 3, "all three rows are still there");
        assert!(page[0].decoded.is_ok(), "the newest is fine");
        assert!(page[2].decoded.is_ok(), "the oldest is fine");
        let middle = format!("{:?}", page[1].decoded);
        assert!(
            matches!(page[1].decoded, Err(RecordFault::Checksum { .. })),
            "the middle record is damaged and says so: {middle}"
        );
    }

    #[test]
    fn a_journal_path_that_cannot_be_written_is_a_named_refusal() {
        let root = scratch("audit-refused");
        // A FILE where the audit DIRECTORY has to be. `create_dir_all` refuses
        // it, and the pull page has to say so rather than lose the record.
        std::fs::write(root.join("audit"), b"not a directory").expect("writes");
        let journal = Journal::at(&root);
        let why = journal
            .append(&Record::refused(
                Scope::Spot,
                Outcome::Stored,
                at(1),
                "x",
                "",
            ))
            .expect_err("cannot create the directory");
        assert!(why.contains("cannot create the audit directory"), "{why}");
        assert!(why.contains("pull.journal"), "and names the path: {why}");
        // And looking at it is Unreadable or Absent, never a crash.
        assert!(!matches!(journal.look(), Log::Held { .. }));

        // A PATH WITH NO PARENT AT ALL. The root directory is the one path
        // `Path::parent` answers `None` for, so it is what drives the arm that
        // skips `create_dir_all` — and the open then refuses by name rather
        // than by assuming a directory step happened.
        let rootish = Journal {
            path: PathBuf::from("/"),
        };
        let why = rootish
            .append(&Record::refused(
                Scope::Spot,
                Outcome::Stored,
                at(1),
                "x",
                "",
            ))
            .expect_err("the root directory is not a journal");
        assert!(why.starts_with("/: cannot open the journal — "), "{why}");
    }

    #[test]
    fn a_page_over_a_journal_that_is_not_there_is_a_named_refusal() {
        let root = scratch("audit-missing");
        let journal = Journal::at(&root);
        // `records` says there are ten; the file says otherwise. That mismatch
        // is what a deleted journal between the look and the read looks like.
        let why = journal.page(10, 0, 5).expect_err("no file");
        assert!(why.contains("pull.journal"), "{why}");
    }

    #[test]
    fn a_short_file_refuses_the_read_rather_than_inventing_records() {
        let root = scratch("audit-short");
        let journal = Journal::at(&root);
        std::fs::create_dir_all(journal.path.parent().expect("a parent")).expect("dir");
        std::fs::write(&journal.path, [0u8; 64]).expect("writes");
        let why = journal.page(4, 0, 4).expect_err("not four records");
        assert!(why.contains("cannot read 4 record(s)"), "{why}");
    }

    #[test]
    fn every_drop_reason_this_build_counts_is_carried_and_shared_by_peak() {
        let drops = Drops::of_census(census(3, 40, 0, 10));
        assert_eq!(drops.of(DropReason::BeforeSessionOpen), 3);
        assert_eq!(drops.of(DropReason::AtOrAfterSessionClose), 40);
        assert_eq!(drops.of(DropReason::BeforeWindow), 0);
        assert_eq!(drops.of(DropReason::AfterWindow), 10);
        assert_eq!(drops.total(), 53);
        assert_eq!(drops.peak(), 40);
        assert_eq!(Drops::default().peak(), 1, "never a division by zero");
        assert_eq!(Drops::default().total(), 0);
        assert_eq!(DROP_REASONS.len(), 4);

        // EVERY REASON CAN BE THE PEAK. A `peak` that only ever noticed the
        // first two counters would draw every share bar against the wrong
        // denominator for the other two, and the bar would still look
        // plausible — so each of the four is made the largest in turn.
        assert_eq!(
            Drops::of_census(census(9, 1, 1, 1)).peak(),
            9,
            "before open"
        );
        assert_eq!(
            Drops::of_census(census(1, 9, 1, 1)).peak(),
            9,
            "after close"
        );
        assert_eq!(
            Drops::of_census(census(1, 1, 9, 1)).peak(),
            9,
            "before window"
        );
        assert_eq!(
            Drops::of_census(census(1, 1, 1, 9)).peak(),
            9,
            "after window"
        );
        assert_eq!(
            Drops::of_census(census(1, 1, 9, 1)).of(DropReason::BeforeWindow),
            9
        );
        assert_eq!(Drops::of_census(census(1, 1, 9, 1)).total(), 12);
    }

    #[test]
    fn scopes_and_outcomes_round_trip_through_their_stored_byte() {
        for scope in [Scope::Spot, Scope::Fno] {
            assert_eq!(Scope::of_code(scope.code()), Some(scope));
            assert!(!scope.label().is_empty());
        }
        assert_eq!(Scope::of_code(2), None);
        for outcome in [
            Outcome::Stored,
            Outcome::Refused,
            Outcome::NotStarted,
            Outcome::Failed,
        ] {
            assert_eq!(Outcome::of_code(outcome.code()), Some(outcome));
            assert!(!outcome.label().is_empty());
        }
        assert_eq!(Outcome::of_code(4), None);
        assert_eq!(Scope::Fno.label(), "expired F&O");
        assert_eq!(Outcome::NotStarted.label(), "NOT STARTED");
    }

    #[test]
    fn a_window_can_be_attached_to_a_refusal_after_the_fact() {
        let record = Record::refused(Scope::Fno, Outcome::Refused, at(1), "NIFTY", "expired")
            .with_window(window());
        assert_eq!(record.from_days, d(2025, 7, 1).days_from_epoch());
        assert_eq!(record.to_days, d(2025, 7, 1).days_from_epoch());
        let back = Record::decode(&record.image()).expect("decodes");
        assert_eq!(back.from_days, record.from_days);
    }

    #[test]
    fn a_clock_before_the_epoch_is_recorded_as_a_negative_second_not_a_wrap() {
        let record = Record::refused(Scope::Spot, Outcome::Refused, at(-86_400), "x", "y");
        assert_eq!(record.at_unix_secs, -86_400);
        let back = Record::decode(&record.image()).expect("decodes");
        assert_eq!(back.at_unix_secs, -86_400, "signed, not wrapped");
    }

    #[test]
    fn an_unreadable_log_is_loud_and_a_held_one_is_not() {
        assert!(
            Log::Unreadable {
                reason: "x".to_owned()
            }
            .is_loud()
        );
        assert_eq!(
            Log::Unreadable {
                reason: "x".to_owned()
            }
            .records(),
            0
        );
        assert!(
            !Log::Held {
                records: 3,
                bytes: 768,
                torn: None
            }
            .is_loud()
        );
        assert_eq!(
            Log::Held {
                records: 3,
                bytes: 768,
                torn: None
            }
            .records(),
            3
        );
    }
}
