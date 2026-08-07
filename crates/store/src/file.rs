//! The write path: a month's bar file, opened, appended to, and read back.
//!
//! Every other module in this crate is arithmetic over bytes somebody else
//! holds. This one is the only place the crate touches the operating system,
//! and it exists because the crate whose entire job is bytes on disk could not
//! put a byte on disk: an audit found no `std::fs` and no `File::` anywhere
//! under `crates/store/src`.
//!
//! # The five calls, and what is durable after each
//!
//! Durability is stated per call rather than left to the reader, because
//! "after which call are the bars safe" is the only question that matters
//! after a power loss.
//!
//! | Call | Durable when it returns `Ok` |
//! |---|---|
//! | [`BarFile::open_or_create`] on a **new** month | the whole 32768-byte header region, and the file's *name* in its directory — both `fsync`ed, the directory one included |
//! | [`BarFile::open_or_create`] on an **existing** month | nothing new; it only reads |
//! | [`BarFile::append`] returning [`Appended::Committed`] | every appended record **and** the header slot that publishes them, in that order, with an `fsync` after each |
//! | [`BarFile::append`] returning [`Appended::AlreadyPresent`] | nothing was written; the batch was already the file's tail, byte for byte |
//! | [`BarFile::read_record`] | nothing; it writes nothing |
//!
//! The order inside [`BarFile::append`] is `docs/02-store-format.md` §5, and
//! the two `fsync`s are not decoration. The header slot published before its
//! records are durable is the *likely* reordering, not the exotic one — the
//! header page is re-dirtied on every commit and is therefore the hottest
//! writeback candidate in the file. [`crate::header::Commit::durable_through`]
//! is the offset the first `fsync` must cover, and it is exactly the end of the
//! records this commit publishes: `store::write::the_committed_length_is_exactly_durable_through`
//! asserts the file's length against it after every commit of a ladder.
//!
//! # No writable mapping, and why the ban is load-bearing here
//!
//! `CLAUDE.md` §4 bans a writable memory mapping and this module is where that
//! ban is either honoured or broken. A mapping that runs out of space raises
//! `SIGBUS`, which is delivered asynchronously and cannot be caught by any
//! construct in any language — so the process dies mid-write and the operator
//! gets a core file instead of a refusal. Every write here is an ordinary
//! positional write syscall, which **returns** `ENOSPC` as a value, and that
//! value is turned into [`StoreError::DiskFull`] and handed back.
//!
//! # One writer per month, enforced by an advisory lock
//!
//! [`crate::header::Header::commit`]'s crash argument is conditioned on
//! exactly one writer, and `docs/02-store-format.md` §9 names the mechanism:
//! an advisory lock on the [`crate::path::FileKind::Lock`] sibling, taken
//! before the bar file is even measured and held for the life of the
//! [`BarFile`]. A second [`BarFile`] on the same month is refused by name with
//! [`StoreError::Locked`] rather than allowed to interleave commits.
//!
//! **What the lock does not protect against, stated rather than implied away:**
//!
//! * A process that opens the `.bin` directly and writes to it. The lock is
//!   advisory: it constrains writers that ask, which is every writer that goes
//!   through this type and no other.
//! * A network filesystem whose lock is a fiction. `flock` over NFS is
//!   emulated, and over some mounts it is a no-op that reports success.
//!   `docs/02-store-format.md` §9 already says to keep the store on local disk.
//! * A symlink at any path component. [`crate::path::StorePath`] guarantees a
//!   **lexical** property only, and this module resolves nothing: it opens the
//!   rendered path with an ordinary `open`, so `bars/groww` linked at
//!   `bars/dhan` sends one vendor's writes into another's file with the lock
//!   held on the wrong month. Closing it needs `openat` with `O_NOFOLLOW` per
//!   component.
//! * A crash. The lock is released by the kernel when the process dies, which
//!   is the property a lock *file* created with `O_EXCL` would not have — that
//!   was the alternative, and it was rejected because a crashed writer would
//!   leave a stale lock that only an operator could clear.
//!
//! # This build writes files with no block checksums, on purpose
//!
//! [`crate::format::FLAG_CHECKSUMS`] is clear in every file this module
//! creates, and the [`crate::path::FileKind::Checksums`] sidecar is not
//! created. `docs/02-store-format.md` §6 provides for exactly that state: a
//! file whose flag is clear carries no sidecar and a verification request
//! against it is **refused** rather than answered, which
//! [`crate::block::verify`] already does by returning
//! [`crate::format::FormatError::ChecksumsAbsent`].
//!
//! It is a hole and it is named as one. What it costs: a lost write to the
//! record extent is undetectable, because an all-zero record is a legal flat
//! bar. What closing it needs is *not* more code here — it is a sidecar entry
//! that says which record count it covers. The tail block's checksum domain is
//! a function of `n_valid` ([`crate::layout::Layout::covered_byte_range`]), so
//! a writer that re-seals the tail block on every commit has a window between
//! the sidecar write and the header commit in which a crash leaves an entry
//! computed over a record count the header does not yet claim — and the next
//! reader reports a healthy file as corrupt. Inventing an entry format to close
//! that is a `docs/02-store-format.md` change with a decision entry behind it,
//! not something to slip into a writer.

use std::fmt;
use std::fs::{self, File, TryLockError};
use std::io::{self, ErrorKind};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use crate::format::{Bar, FormatError, HEADER_LEN, MAX_SLOT_COUNT, RECORD_LEN, SLOT_STRIDE};
use crate::header::Header;
use crate::layout::Layout;
use crate::path::{FileKind, StorePath};

/// The largest header region the format family can declare, as a length.
///
/// Written as a literal in both widths rather than converted, the way every
/// other paired constant in this crate is: a fallible conversion here would
/// carry a failure arm no input could reach. The assertions keep the two
/// honest, and they are stated against the **family** bound rather than
/// against version 2, so a version with fewer slots still reads correctly and
/// a version with more is a compile error here.
const REGION_LEN: usize = 32_768;

const _: () = assert!(REGION_LEN == 32_768);
const _: () = assert!(MAX_SLOT_COUNT * SLOT_STRIDE == 32_768);
const _: () = assert!(HEADER_LEN <= 32_768);

/// [`REGION_LEN`] in the width an offset is measured in.
const REGION_LEN_U64: u64 = 32_768;

const _: () = assert!(REGION_LEN_U64 == 32_768);

/// Which syscall refused, so an error names the operation and not only the
/// path.
///
/// "Permission denied" against a month file is three different bugs depending
/// on whether it was the directory, the lock or the records that refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    /// Creating the directory tree the month lives in.
    CreateDir,
    /// Opening a file.
    Open,
    /// Taking the month's advisory lock.
    Lock,
    /// Reading a file's length.
    Measure,
    /// Reading bytes at an offset.
    Read,
    /// Writing bytes at an offset.
    Write,
    /// Flushing to stable storage.
    Sync,
}

impl Action {
    /// The word this action is named by in a refusal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateDir => "creating the directory",
            Self::Open => "opening",
            Self::Lock => "locking",
            Self::Measure => "measuring",
            Self::Read => "reading",
            Self::Write => "writing",
            Self::Sync => "syncing",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a bar file could not be opened, appended to or read.
///
/// Every variant names the path, the operation, or both. A refusal that says
/// only "I/O error" sends an operator to `strace`, which is where a "just retry
/// it" habit comes from.
///
/// The host's own error is reduced to its [`ErrorKind`] and its `errno` rather
/// than kept: those two are `Copy` and comparable, so a test asserts an exact
/// value instead of matching a message the platform is free to reword.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreError {
    /// The path names a sibling that is not the bar records.
    ///
    /// Refused rather than silently redirected to `.bin`: a caller that asked
    /// for the overlay and got the records back would corrupt the one it meant
    /// to write.
    NotABarPath {
        /// The sibling that was asked for.
        found: FileKind,
    },
    /// Another writer holds this month's advisory lock.
    Locked {
        /// The lock file.
        path: PathBuf,
    },
    /// The disk filled. `ENOSPC`.
    ///
    /// This is the error a writable mapping could not have returned — it would
    /// have raised `SIGBUS` instead and killed the process mid-write. See the
    /// module documentation.
    DiskFull {
        /// The file being written.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
    },
    /// The host refused permission. `EACCES`.
    Denied {
        /// The file or directory.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
    },
    /// The filesystem is mounted read-only. `EROFS`.
    ReadOnly {
        /// The file or directory.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
    },
    /// A directory sits where the file belongs. `EISDIR`.
    IsADirectory {
        /// The path that is a directory.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
    },
    /// A file sits where a directory component belongs. `ENOTDIR`.
    NotADirectory {
        /// The path whose parent is not a directory.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
    },
    /// The path is not there. `ENOENT`.
    Missing {
        /// The path that does not exist.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
    },
    /// The host refused for a reason this module does not classify.
    ///
    /// Carries the kind and the raw `errno` rather than collapsing to "I/O
    /// error", so an unclassified failure is still diagnosable and the
    /// classifier can be widened by whoever meets one.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// Which operation refused.
        action: Action,
        /// The host's classification.
        kind: ErrorKind,
        /// The raw `errno`, when the host supplied one.
        code: Option<i32>,
    },
    /// A write accepted fewer bytes than it was offered and then stopped
    /// accepting any.
    ///
    /// A *partial* write is not this: it is ordinary, and the loop resumes at
    /// the exact offset the host stopped at. This is the case where the host
    /// takes zero bytes twice — there is no offset to resume from and
    /// retrying forever would hang.
    ShortWrite {
        /// The file being written.
        path: PathBuf,
        /// Where the stall happened.
        offset: u64,
        /// Bytes still owed at that offset.
        asked: usize,
        /// Bytes the host had accepted before it stalled.
        wrote: usize,
    },
    /// A read returned fewer bytes than the file's own geometry promised.
    ///
    /// The file was truncated under an open handle: the header says the
    /// records are there and the bytes are not.
    ShortRead {
        /// The file being read.
        path: PathBuf,
        /// Where the read stopped.
        offset: u64,
        /// Bytes still owed at that offset.
        asked: usize,
        /// Bytes the host had returned before it ran out.
        read: usize,
    },
    /// The file's length is not the header region plus a whole number of
    /// records.
    ///
    /// The file was interrupted mid-record. `docs/02-store-format.md` §7
    /// describes truncating to the last whole record and logging the discarded
    /// count; this module refuses instead, and the reason is that truncation is
    /// a **destructive write** performed on an operator's file at open time,
    /// and this crate has no logging sink to be loud through. `CLAUDE.md` §4
    /// admits either half — degrade loudly, *or* refuse — and refusing is the
    /// half that cannot lose a byte.
    RaggedTail {
        /// The bar file.
        path: PathBuf,
        /// Its length.
        len: u64,
        /// Bytes past the last whole record.
        extra: u64,
    },
    /// The header disagrees with the bytes, or with itself.
    ///
    /// Carries [`FormatError`], which names which disagreement it was:
    /// [`FormatError::CounterExceedsFile`] for a header claiming more records
    /// than the file can hold, [`FormatError::NoValidHeader`] when every slot
    /// is damaged, [`FormatError::TimestampsOutOfOrder`] for a batch that does
    /// not follow what is committed, and so on.
    Format {
        /// The bar file.
        path: PathBuf,
        /// What the bytes said.
        source: FormatError,
    },
    /// The file was written for a different instrument.
    ///
    /// `symbol_id` is a cross-check, never the index — this is the check.
    SymbolMismatch {
        /// The bar file.
        path: PathBuf,
        /// What the header carries.
        stored: u32,
        /// What the caller asked for.
        asked: u32,
    },
    /// The file was written at a different bar length.
    TimeframeMismatch {
        /// The bar file.
        path: PathBuf,
        /// Seconds per bar the header carries.
        stored: u32,
        /// Seconds per bar the path names.
        asked: u32,
    },
    /// A record index at or past the commit counter.
    ///
    /// Bytes past `n_valid` are not "empty", they are **not there**: they may
    /// be a half-written batch from a crash, and returning them as a bar would
    /// hand a sweep data nobody committed.
    NotCommitted {
        /// The index asked for.
        index: u64,
        /// How many records are committed.
        n_valid: u64,
    },
    /// An append with no records in it.
    ///
    /// Refused rather than treated as a no-op: publishing nothing still
    /// advances the generation and rewrites a header slot, so a caller looping
    /// over empty days would rewrite the header once per day for no bars. A day
    /// with no bars is a census outcome, not a commit.
    EmptyBatch,
    /// A batch whose own timestamps do not strictly increase.
    ///
    /// Refused at the write boundary, because afterwards it is well-formed
    /// bytes that no checksum can catch.
    BatchNotOrdered {
        /// Index within the batch of the bar that did not follow.
        at: u64,
        /// The timestamp before it.
        previous: i64,
        /// The timestamp that did not follow.
        next: i64,
    },
    /// A batch holding a bar whose OHLC cannot have happened.
    ///
    /// [`Bar::ohlc_is_sane`] exists "so an impossible bar never reaches the
    /// disk in the first place"; this is the call site that makes that true.
    ImpossibleBar {
        /// Index within the batch.
        at: u64,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotABarPath { found } => {
                write!(
                    f,
                    "path names the {} sibling, not the records",
                    found.extension()
                )
            }
            Self::Locked { path } => {
                write!(f, "another writer holds {}", path.display())
            }
            Self::DiskFull { path, action } => {
                write!(f, "disk full {action} {}", path.display())
            }
            Self::Denied { path, action } => {
                write!(f, "permission denied {action} {}", path.display())
            }
            Self::ReadOnly { path, action } => {
                write!(f, "read-only filesystem {action} {}", path.display())
            }
            Self::IsADirectory { path, action } => {
                write!(f, "{} is a directory, {action} it", path.display())
            }
            Self::NotADirectory { path, action } => {
                write!(
                    f,
                    "a path component of {} is not a directory, {action} it",
                    path.display()
                )
            }
            Self::Missing { path, action } => {
                write!(f, "{} does not exist, {action} it", path.display())
            }
            Self::Io {
                path,
                action,
                kind,
                code,
            } => write!(
                f,
                "{action} {} failed: {kind:?} (errno {code:?})",
                path.display()
            ),
            Self::ShortWrite {
                path,
                offset,
                asked,
                wrote,
            } => write!(
                f,
                "{} stalled at offset {offset} with {asked} bytes still owed after {wrote} accepted",
                path.display()
            ),
            Self::ShortRead {
                path,
                offset,
                asked,
                read,
            } => write!(
                f,
                "{} ended at offset {offset} with {asked} bytes owed after {read}",
                path.display()
            ),
            Self::RaggedTail { path, len, extra } => write!(
                f,
                "{} is {len} bytes, {extra} past the last whole record",
                path.display()
            ),
            Self::Format { path, source } => write!(f, "{}: {source}", path.display()),
            Self::SymbolMismatch {
                path,
                stored,
                asked,
            } => write!(f, "{} holds symbol {stored}, not {asked}", path.display()),
            Self::TimeframeMismatch {
                path,
                stored,
                asked,
            } => write!(
                f,
                "{} holds {stored}-second bars, not {asked}",
                path.display()
            ),
            Self::NotCommitted { index, n_valid } => {
                write!(f, "record {index} is past the {n_valid} committed")
            }
            Self::EmptyBatch => f.write_str("an append with no records in it"),
            Self::BatchNotOrdered { at, previous, next } => {
                write!(f, "batch record {at}: {next} does not follow {previous}")
            }
            Self::ImpossibleBar { at } => {
                write!(f, "batch record {at} has impossible OHLC")
            }
        }
    }
}

impl std::error::Error for StoreError {}

/// What an append did.
///
/// Two outcomes rather than one, because "we wrote it" and "it was already
/// there" are different facts and a caller counting bars needs to tell them
/// apart. Collapsing them into `Ok(())` is the shape of a silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Appended {
    /// The batch was written and published.
    Committed {
        /// The index the first record of the batch landed at.
        first_index: u64,
        /// The commit counter after the append.
        n_valid: u64,
    },
    /// The batch was already the file's tail, byte for byte. Nothing was
    /// written and no generation was spent.
    ///
    /// This is what makes a re-run of the same pull safe: the second append
    /// leaves the file byte-identical, which
    /// `store::write::re_appending_the_same_batch_leaves_the_file_byte_identical`
    /// proves by checksumming the whole file either side.
    AlreadyPresent {
        /// The index the first record of the batch already sits at.
        first_index: u64,
        /// The commit counter, unchanged.
        n_valid: u64,
    },
}

/// One month's bar file, open for reading and appending.
///
/// Holds the month's advisory lock for as long as it lives. Dropping it closes
/// the records and then releases the lock, in that order — the fields are
/// declared in that order and Rust drops them in declaration order.
#[derive(Debug)]
pub struct BarFile {
    /// The records.
    bars: File,
    /// Where they are, for every refusal this type can produce.
    bars_path: PathBuf,
    /// The geometry, taken from the file's own `format_version`.
    layout: Layout,
    /// The committed state, re-read from the file on open.
    header: Header,
    /// The advisory lock, held for its **drop** and never read again.
    ///
    /// Underscored because that is what it is: closing this descriptor is what
    /// releases the month, so the field is live for its whole life and is
    /// touched exactly once, by the compiler-generated drop. Declared last so
    /// the records are closed before the lock is released.
    _lock: File,
}

impl BarFile {
    /// Opens a month's bar file, creating it and its directory if absent.
    ///
    /// A file that does not exist, and one that exists with zero bytes, are
    /// the same thing to this function: both get a fresh 32768-byte header
    /// region. A zero-byte file is what a crash between `open` and the first
    /// write leaves behind, and it can hold no record by definition, so
    /// initialising it loses nothing. Any other length is read, never
    /// overwritten.
    ///
    /// The header is always **read back from the file** before this returns,
    /// including on the create path, so the caller never holds a header that
    /// only the process believes in.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotABarPath`] for a path naming a sibling other than the
    /// records. [`StoreError::Locked`] when another writer holds the month.
    /// [`StoreError::Denied`], [`StoreError::ReadOnly`],
    /// [`StoreError::IsADirectory`], [`StoreError::NotADirectory`],
    /// [`StoreError::DiskFull`] or [`StoreError::Io`] from the host.
    /// [`StoreError::RaggedTail`] for a file interrupted mid-record,
    /// [`StoreError::Format`] for a header that disagrees with the file's
    /// length or is damaged in every slot, and
    /// [`StoreError::SymbolMismatch`] or [`StoreError::TimeframeMismatch`] for
    /// a file written for something else.
    pub fn open_or_create(
        root: &Path,
        path: StorePath<'_>,
        symbol_id: u32,
    ) -> Result<Self, StoreError> {
        if path.file() != FileKind::Bars {
            return Err(StoreError::NotABarPath { found: path.file() });
        }
        let timeframe_secs = path.timeframe().secs();
        let bars_path = path.to_path_buf(root);
        let lock_path = path.with_file(FileKind::Lock).to_path_buf(root);

        // A rendered store path always has six parent components, so the
        // fallback is unreachable; it is written as one anyway because the
        // alternative is a refusal arm no input can produce. Reaching it would
        // create the root and then fail loudly on the open below.
        let dir = bars_path.parent().unwrap_or(root).to_path_buf();
        fault(fs::create_dir_all(&dir), &dir, Action::CreateDir)?;

        // The lock is taken before the bar file is opened, let alone measured.
        // Everything below this line assumes exactly one writer.
        let lock = fault(open_rw(&lock_path), &lock_path, Action::Open)?;
        if let Err(refusal) = lock.try_lock() {
            return Err(lock_fault(&lock_path, refusal));
        }

        let bars = fault(open_rw(&bars_path), &bars_path, Action::Open)?;
        let mut len = fault(bars.metadata(), &bars_path, Action::Measure)?.len();
        if len == 0 {
            initialise(&bars, &bars_path, symbol_id, timeframe_secs)?;
            fsync_dir(&dir)?;
            len = fault(bars.metadata(), &bars_path, Action::Measure)?.len();
        }

        let header = read_header(&bars, &bars_path, len)?;
        let layout = refused(Layout::for_version(header.format_version), &bars_path)?;
        let extra = layout.ragged_tail_bytes(len);
        if extra != 0 {
            return Err(StoreError::RaggedTail {
                path: bars_path,
                len,
                extra,
            });
        }
        if header.symbol_id != symbol_id {
            return Err(StoreError::SymbolMismatch {
                path: bars_path,
                stored: header.symbol_id,
                asked: symbol_id,
            });
        }
        if header.timeframe_secs != timeframe_secs {
            return Err(StoreError::TimeframeMismatch {
                path: bars_path,
                stored: header.timeframe_secs,
                asked: timeframe_secs,
            });
        }
        Ok(Self {
            bars,
            bars_path,
            layout,
            header,
            _lock: lock,
        })
    }

    /// The committed header, as the file's own bytes describe it.
    #[must_use]
    pub const fn header(&self) -> Header {
        self.header
    }

    /// The geometry this file's version defines.
    #[must_use]
    pub const fn layout(&self) -> Layout {
        self.layout
    }

    /// How many records are committed.
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.header.n_valid
    }

    /// The bar file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.bars_path
    }

    /// Appends a batch and publishes it.
    ///
    /// The sequence is `docs/02-store-format.md` §5: the records are written
    /// at `header_len + n_valid · stride`, flushed, and only then is the
    /// 64-byte header slot written and flushed. Nothing is written at all
    /// until the batch has been surveyed and the next header computed, so a
    /// refusal leaves the file byte-identical.
    ///
    /// **Re-appending the file's own tail is a no-op, not a duplicate.** A
    /// batch whose first timestamp does not follow the committed range is
    /// compared, record by record, against the records already at those
    /// indices: identical means [`Appended::AlreadyPresent`] and no write at
    /// all, different means a refusal. A re-pull of the same day is therefore
    /// safe and leaves the same bytes, which is `CLAUDE.md` §3 rule 5.
    ///
    /// # Errors
    ///
    /// [`StoreError::EmptyBatch`], [`StoreError::BatchNotOrdered`] or
    /// [`StoreError::ImpossibleBar`] before anything is written.
    /// [`StoreError::Format`] carrying
    /// [`FormatError::TimestampsOutOfOrder`] for a batch that overlaps the
    /// committed range with different bars, or
    /// [`FormatError::CounterOverflow`] and
    /// [`FormatError::GenerationExhausted`] at the counters' ends. Anything
    /// the host refuses: [`StoreError::DiskFull`], [`StoreError::ShortWrite`],
    /// [`StoreError::Denied`], [`StoreError::Io`].
    pub fn append(&mut self, batch: &[Bar]) -> Result<Appended, StoreError> {
        let (first_ts, last_ts) = survey(batch)?;
        let count = len_u64(batch.len());

        // `Header::advance` is the single authority on whether a batch follows
        // what is committed. Asking it first, rather than repeating its
        // comparison here, is what keeps the two from drifting apart — a
        // duplicate of that condition would also be a branch no test could
        // tell from the original.
        let next = match self.header.advance(count, first_ts, last_ts) {
            Ok(next) => next,
            Err(source) => {
                // It does not follow. If it *is* what is already there, byte
                // for byte, this is a re-run of the same pull rather than a
                // conflict, and the safe answer is to write nothing.
                if self.tail_matches(batch, count)? {
                    return Ok(Appended::AlreadyPresent {
                        first_index: self.header.n_valid.saturating_sub(count),
                        n_valid: self.header.n_valid,
                    });
                }
                return Err(StoreError::Format {
                    path: self.bars_path.clone(),
                    source,
                });
            }
        };
        let commit = refused(next.commit(), &self.bars_path)?;
        let first_index = self.header.n_valid;
        let at = refused(self.layout.offset_of(first_index), &self.bars_path)?;

        let mut image = Vec::with_capacity(batch.len().saturating_mul(RECORD_LEN));
        for bar in batch {
            image.extend_from_slice(&bar.image());
        }

        // Step 1 and step 3 of §5: the records, then the barrier. Every byte
        // below `commit.durable_through` is on stable storage when the second
        // write is issued.
        write_fully(&self.bars, &self.bars_path, at, &image)?;
        fault(self.bars.sync_all(), &self.bars_path, Action::Sync)?;

        // Step 4 and step 5: one write of one self-checked 64-byte unit, into
        // the slot that does not hold the previous commit.
        write_fully(&self.bars, &self.bars_path, commit.offset, &commit.bytes)?;
        fault(self.bars.sync_all(), &self.bars_path, Action::Sync)?;

        self.header = commit.header;
        Ok(Appended::Committed {
            first_index,
            n_valid: self.header.n_valid,
        })
    }

    /// Reads one record by index, in O(1).
    ///
    /// One multiply, one add, one positional read of [`RECORD_LEN`] bytes. No
    /// scan, no directory walk, no index file — the address *is* the index,
    /// which is the whole reason the format exists, and the work does not move
    /// with the number of records in the file.
    /// `store::write::a_record_reads_back_at_its_computed_offset` pins the
    /// offset against [`Layout::offset_of`] at both ends of a 375-record file
    /// and across two checksum blocks.
    ///
    /// **Read that bound precisely.** The *operation* is constant; the read
    /// syscall underneath it is not free and its latency is the device's, not
    /// this crate's. Nothing here measures the device, and no bench in this
    /// repository times a syscall — `crates/store/benches/ratio.rs` measures
    /// the arithmetic and the checksum. The syscall cost is UNVERIFIED.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotCommitted`] for an index at or past the commit
    /// counter. [`StoreError::ShortRead`] when the file was truncated under
    /// this handle, and anything the host refuses.
    pub fn read_record(&self, index: u64) -> Result<Bar, StoreError> {
        if index >= self.header.n_valid {
            return Err(StoreError::NotCommitted {
                index,
                n_valid: self.header.n_valid,
            });
        }
        let at = refused(self.layout.offset_of(index), &self.bars_path)?;
        let mut image = [0u8; RECORD_LEN];
        read_fully(&self.bars, &self.bars_path, at, &mut image)?;
        refused(Bar::decode(&image), &self.bars_path)
    }

    /// Whether the last `count` committed records are exactly this batch.
    ///
    /// Reads `count` records, so it costs the batch and not the file. It runs
    /// only when a batch overlaps the committed range, which is the re-pull
    /// case; an ordinary forward append never enters it.
    fn tail_matches(&self, batch: &[Bar], count: u64) -> Result<bool, StoreError> {
        let Some(start) = self.header.n_valid.checked_sub(count) else {
            return Ok(false);
        };
        for (offset, bar) in batch.iter().enumerate() {
            let stored = self.read_record(start.saturating_add(len_u64(offset)))?;
            if stored != *bar {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Checks a batch and returns its first and last timestamps.
///
/// Every refusal here happens before a byte is written, which is the point:
/// `docs/02-store-format.md` §9 puts range validation "at the ingest boundary,
/// before a byte is written", and this is that boundary for the two properties
/// the bytes themselves cannot carry — an impossible bar and a batch out of
/// order are both well-formed records afterwards.
fn survey(batch: &[Bar]) -> Result<(i64, i64), StoreError> {
    let Some(first) = batch.first() else {
        return Err(StoreError::EmptyBatch);
    };
    let mut previous: Option<i64> = None;
    for (offset, bar) in batch.iter().enumerate() {
        if !bar.ohlc_is_sane() {
            return Err(StoreError::ImpossibleBar {
                at: len_u64(offset),
            });
        }
        if let Some(earlier) = previous
            && bar.ts_micros <= earlier
        {
            return Err(StoreError::BatchNotOrdered {
                at: len_u64(offset),
                previous: earlier,
                next: bar.ts_micros,
            });
        }
        previous = Some(bar.ts_micros);
    }
    // The batch is non-empty and strictly increasing, so the running
    // timestamp is the last one.
    Ok((first.ts_micros, previous.unwrap_or(first.ts_micros)))
}

/// Writes a fresh header region: zeros, then the genesis commit.
///
/// The zeros are written rather than left as a hole, so the whole region is
/// allocated before the first record is. A file that cannot fit its own header
/// says so now, with [`StoreError::DiskFull`], rather than on the commit after
/// the first successful append.
fn initialise(
    dst: &File,
    path: &Path,
    symbol_id: u32,
    timeframe_secs: u32,
) -> Result<(), StoreError> {
    let genesis = Header::genesis(symbol_id, timeframe_secs, 0);
    let commit = refused(genesis.commit(), path)?;
    let region = vec![0u8; REGION_LEN];
    write_fully(dst, path, 0, &region)?;
    write_fully(dst, path, commit.offset, &commit.bytes)?;
    fault(dst.sync_all(), path, Action::Sync)
}

/// Reads the header region and returns the committed header.
///
/// Reads at most [`REGION_LEN`] bytes — never the whole file. A longer read
/// would let record bytes audition as header slots, which is the reason
/// [`Header::read_region`] bounds itself at [`MAX_SLOT_COUNT`] positions.
fn read_header(src: &File, path: &Path, len: u64) -> Result<Header, StoreError> {
    let want = len.min(REGION_LEN_U64);
    let mut region = vec![0u8; usize::try_from(want).unwrap_or(REGION_LEN)];
    read_fully(src, path, 0, &mut region)?;
    refused(Header::read_region(&region, len), path)
}

/// Flushes a directory, so a newly created file's **name** is durable.
///
/// Without it the bars can be on stable storage inside a file the directory
/// does not yet mention after a crash, which is the same as not having them.
fn fsync_dir(dir: &Path) -> Result<(), StoreError> {
    let handle = fault(File::open(dir), dir, Action::Open)?;
    fault(handle.sync_all(), dir, Action::Sync)
}

/// Opens for reading and writing, creating but never truncating.
fn open_rw(path: &Path) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// A file, seen as the two positional calls this module makes of it.
///
/// It exists so the failure arms below can be **exercised**. `ENOSPC`,
/// `EROFS`, a partial write and a write that accepts nothing are all things a
/// developer's disk will not do on request, and a refusal path that no test
/// enters is a refusal path that is wrong the first time it runs. The
/// implementation for [`File`] is the two syscalls and nothing else.
trait Positional {
    /// One `pwrite`. Returns how many bytes the host accepted.
    fn put(&self, offset: u64, buf: &[u8]) -> io::Result<usize>;
    /// One `pread`. Returns how many bytes the host returned.
    fn get(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

impl Positional for File {
    fn put(&self, offset: u64, buf: &[u8]) -> io::Result<usize> {
        FileExt::write_at(self, buf, offset)
    }

    fn get(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        FileExt::read_at(self, buf, offset)
    }
}

/// Writes every byte, resuming a partial write at the offset it stopped at.
///
/// A short write is ordinary POSIX behaviour and the answer to it is to
/// continue, not to fail. The failure is a write that accepts **zero** bytes:
/// there is no progress to resume from, and looping on it would hang, which is
/// the one failure a test suite cannot report.
fn write_fully<P: Positional>(
    dst: &P,
    path: &Path,
    offset: u64,
    bytes: &[u8],
) -> Result<(), StoreError> {
    let mut done = 0usize;
    while let Some(rest) = bytes.get(done..).filter(|rest| !rest.is_empty()) {
        // Saturating rather than checked: `offset` came from the geometry,
        // which already refused an overflow, and a saturated offset is one no
        // host accepts a write at — so it becomes a loud refusal by the arm
        // below rather than a wrong write, and there is no arm here that no
        // input can reach.
        let at = offset.saturating_add(len_u64(done));
        match dst.put(at, rest) {
            Ok(0) => {
                return Err(StoreError::ShortWrite {
                    path: path.to_path_buf(),
                    offset: at,
                    asked: rest.len(),
                    wrote: done,
                });
            }
            Ok(taken) => done = done.saturating_add(taken),
            Err(refusal) if refusal.kind() == ErrorKind::Interrupted => {}
            Err(refusal) => return Err(classify(path, Action::Write, &refusal)),
        }
    }
    Ok(())
}

/// Reads every byte, resuming a short read at the offset it stopped at.
///
/// A read that returns zero is end of file: the caller asked for bytes the
/// file's own header promised and the file does not have them, which means it
/// was truncated under this handle.
fn read_fully<P: Positional>(
    src: &P,
    path: &Path,
    offset: u64,
    buf: &mut [u8],
) -> Result<(), StoreError> {
    let mut done = 0usize;
    while let Some(rest) = buf.get_mut(done..).filter(|rest| !rest.is_empty()) {
        let at = offset.saturating_add(len_u64(done));
        let owed = rest.len();
        match src.get(at, rest) {
            Ok(0) => {
                return Err(StoreError::ShortRead {
                    path: path.to_path_buf(),
                    offset: at,
                    asked: owed,
                    read: done,
                });
            }
            Ok(got) => done = done.saturating_add(got),
            Err(refusal) if refusal.kind() == ErrorKind::Interrupted => {}
            Err(refusal) => return Err(classify(path, Action::Read, &refusal)),
        }
    }
    Ok(())
}

/// The one place an [`io::Error`] becomes a [`StoreError`].
///
/// One function rather than a closure at every call site: a closure per site
/// is a refusal arm per site, and most of them are arms the host will never
/// take on a developer's machine. Here there is one arm per *kind*, and the
/// tests below drive all of them.
fn classify(path: &Path, action: Action, refusal: &io::Error) -> StoreError {
    let path = path.to_path_buf();
    match refusal.kind() {
        ErrorKind::StorageFull => StoreError::DiskFull { path, action },
        ErrorKind::PermissionDenied => StoreError::Denied { path, action },
        ErrorKind::ReadOnlyFilesystem => StoreError::ReadOnly { path, action },
        ErrorKind::IsADirectory => StoreError::IsADirectory { path, action },
        ErrorKind::NotADirectory => StoreError::NotADirectory { path, action },
        ErrorKind::NotFound => StoreError::Missing { path, action },
        kind => StoreError::Io {
            path,
            action,
            kind,
            code: refusal.raw_os_error(),
        },
    }
}

/// A failed lock attempt, named.
///
/// `WouldBlock` is the whole point of the lock and is not an I/O error; every
/// other reason is.
fn lock_fault(path: &Path, refusal: TryLockError) -> StoreError {
    match refusal {
        TryLockError::WouldBlock => StoreError::Locked {
            path: path.to_path_buf(),
        },
        TryLockError::Error(host) => classify(path, Action::Lock, &host),
    }
}

/// Lifts an [`io::Result`] into this module's errors.
fn fault<T>(result: io::Result<T>, path: &Path, action: Action) -> Result<T, StoreError> {
    match result {
        Ok(value) => Ok(value),
        Err(refusal) => Err(classify(path, action, &refusal)),
    }
}

/// Lifts a [`FormatError`] into this module's errors, naming the file.
fn refused<T>(result: Result<T, FormatError>, path: &Path) -> Result<T, StoreError> {
    match result {
        Ok(value) => Ok(value),
        Err(source) => Err(StoreError::Format {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// A length as a `u64`, without a cast this workspace denies.
///
/// The saturating answer is only reachable on a target where `usize` is wider
/// than 64 bits, and there it is a length no file has — every caller either
/// compares it against a real length or offers it to the host, both of which
/// refuse loudly. Same argument as [`crate::block`]'s byte count.
fn len_u64(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use std::cell::RefCell;

    use super::{
        Action, Positional, StoreError, classify, len_u64, lock_fault, read_fully, write_fully,
    };
    use std::fs::TryLockError;
    use std::io::{self, ErrorKind};
    use std::path::Path;

    /// What the fake host should do on the next call.
    #[derive(Debug, Clone, Copy)]
    enum Step {
        /// Accept or return this many bytes.
        Take(usize),
        /// Fail with this kind.
        Fail(ErrorKind),
    }

    /// A host that does exactly what a test tells it to.
    ///
    /// This is a fake and it is used **only** where the real condition cannot
    /// be produced: no developer disk fills on request, no filesystem becomes
    /// read-only on request, and no regular file accepts zero bytes on
    /// request. Every condition that *can* be made — a directory where a file
    /// belongs, a read-only directory, a truncated file, a second writer — is
    /// made for real in `crates/store/tests/write.rs`.
    struct Script {
        steps: RefCell<Vec<Step>>,
        seen: RefCell<Vec<(u64, usize)>>,
    }

    impl Script {
        fn new(steps: &[Step]) -> Self {
            Self {
                steps: RefCell::new(steps.iter().rev().copied().collect()),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn next(&self, offset: u64, len: usize) -> io::Result<usize> {
            self.seen.borrow_mut().push((offset, len));
            match self.steps.borrow_mut().pop() {
                Some(Step::Take(n)) => Ok(n.min(len)),
                Some(Step::Fail(kind)) => Err(io::Error::from(kind)),
                None => Ok(len),
            }
        }
    }

    impl Positional for Script {
        fn put(&self, offset: u64, buf: &[u8]) -> io::Result<usize> {
            self.next(offset, buf.len())
        }

        fn get(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            self.next(offset, buf.len())
        }
    }

    fn path() -> &'static Path {
        Path::new("/tmp/brutex-store-fake.bin")
    }

    #[test]
    fn a_partial_write_resumes_at_the_offset_it_stopped_at() {
        // Two scripted partials, and then a host that takes whatever is left:
        // the loop must not depend on how the remainder is handed back.
        let host = Script::new(&[Step::Take(3), Step::Take(2)]);
        assert_eq!(write_fully(&host, path(), 100, &[7u8; 10]), Ok(()));
        assert_eq!(
            *host.seen.borrow(),
            vec![(100, 10), (103, 7), (105, 5)],
            "each resume starts where the last one stopped"
        );
    }

    #[test]
    fn a_write_that_accepts_nothing_is_refused_rather_than_retried_forever() {
        let host = Script::new(&[Step::Take(4), Step::Take(0)]);
        assert_eq!(
            write_fully(&host, path(), 0, &[1u8; 9]),
            Err(StoreError::ShortWrite {
                path: path().to_path_buf(),
                offset: 4,
                asked: 5,
                wrote: 4,
            })
        );
    }

    #[test]
    fn an_interrupted_write_is_retried_at_the_same_offset() {
        let host = Script::new(&[Step::Fail(ErrorKind::Interrupted), Step::Take(6)]);
        assert_eq!(write_fully(&host, path(), 8, &[2u8; 6]), Ok(()));
        assert_eq!(*host.seen.borrow(), vec![(8, 6), (8, 6)]);
    }

    #[test]
    fn a_full_disk_mid_write_is_returned_and_never_signalled() {
        // The whole argument for banning a writable mapping, as a value: an
        // ordinary write RETURNS this. A mapping would have raised SIGBUS.
        let host = Script::new(&[Step::Take(2), Step::Fail(ErrorKind::StorageFull)]);
        assert_eq!(
            write_fully(&host, path(), 32_768, &[3u8; 56]),
            Err(StoreError::DiskFull {
                path: path().to_path_buf(),
                action: Action::Write,
            })
        );
    }

    #[test]
    fn an_empty_write_touches_the_host_at_all() {
        let host = Script::new(&[]);
        assert_eq!(write_fully(&host, path(), 0, &[]), Ok(()));
        assert!(
            host.seen.borrow().is_empty(),
            "nothing to write means no syscall"
        );
    }

    #[test]
    fn a_short_read_resumes_and_a_read_of_nothing_is_end_of_file() {
        let host = Script::new(&[Step::Take(2), Step::Take(3)]);
        let mut buf = [0u8; 5];
        assert_eq!(read_fully(&host, path(), 16, &mut buf), Ok(()));
        assert_eq!(*host.seen.borrow(), vec![(16, 5), (18, 3)]);

        let host = Script::new(&[Step::Take(2), Step::Take(0)]);
        let mut buf = [0u8; 56];
        assert_eq!(
            read_fully(&host, path(), 32_768, &mut buf),
            Err(StoreError::ShortRead {
                path: path().to_path_buf(),
                offset: 32_770,
                asked: 54,
                read: 2,
            })
        );
    }

    #[test]
    fn an_interrupted_read_is_retried_and_a_refused_read_is_classified() {
        let host = Script::new(&[Step::Fail(ErrorKind::Interrupted), Step::Take(4)]);
        let mut buf = [0u8; 4];
        assert_eq!(read_fully(&host, path(), 0, &mut buf), Ok(()));

        let host = Script::new(&[Step::Fail(ErrorKind::PermissionDenied)]);
        let mut buf = [0u8; 4];
        assert_eq!(
            read_fully(&host, path(), 0, &mut buf),
            Err(StoreError::Denied {
                path: path().to_path_buf(),
                action: Action::Read,
            })
        );
    }

    #[test]
    fn every_classified_kind_gets_its_own_name() {
        // Two of these — a full disk and a read-only mount — cannot be
        // produced on a developer's machine without privileged mounting, so
        // this is where the mapping is checked. It proves the classifier, not
        // the kernel: `crates/store/tests/write.rs` produces the other five
        // for real.
        let cases = [
            (
                ErrorKind::StorageFull,
                StoreError::DiskFull {
                    path: path().to_path_buf(),
                    action: Action::Write,
                },
            ),
            (
                ErrorKind::PermissionDenied,
                StoreError::Denied {
                    path: path().to_path_buf(),
                    action: Action::Write,
                },
            ),
            (
                ErrorKind::ReadOnlyFilesystem,
                StoreError::ReadOnly {
                    path: path().to_path_buf(),
                    action: Action::Write,
                },
            ),
            (
                ErrorKind::IsADirectory,
                StoreError::IsADirectory {
                    path: path().to_path_buf(),
                    action: Action::Write,
                },
            ),
            (
                ErrorKind::NotADirectory,
                StoreError::NotADirectory {
                    path: path().to_path_buf(),
                    action: Action::Write,
                },
            ),
            (
                ErrorKind::NotFound,
                StoreError::Missing {
                    path: path().to_path_buf(),
                    action: Action::Write,
                },
            ),
        ];
        for (kind, want) in cases {
            assert_eq!(
                classify(path(), Action::Write, &io::Error::from(kind)),
                want,
                "{kind:?}"
            );
        }

        // Anything unclassified keeps the kind and the errno rather than
        // collapsing to "I/O error".
        let other = io::Error::from_raw_os_error(9);
        assert_eq!(
            classify(path(), Action::Sync, &other),
            StoreError::Io {
                path: path().to_path_buf(),
                action: Action::Sync,
                kind: other.kind(),
                code: Some(9),
            }
        );
    }

    #[test]
    fn a_lock_that_would_block_is_a_held_month_and_nothing_else_is() {
        assert_eq!(
            lock_fault(path(), TryLockError::WouldBlock),
            StoreError::Locked {
                path: path().to_path_buf(),
            }
        );
        assert_eq!(
            lock_fault(
                path(),
                TryLockError::Error(io::Error::from(ErrorKind::PermissionDenied))
            ),
            StoreError::Denied {
                path: path().to_path_buf(),
                action: Action::Lock,
            }
        );
    }

    #[test]
    fn a_length_crosses_widths_without_a_cast() {
        assert_eq!(len_u64(0), 0);
        assert_eq!(len_u64(56), 56);
        assert_eq!(len_u64(usize::MAX), u64::MAX);
    }
}
