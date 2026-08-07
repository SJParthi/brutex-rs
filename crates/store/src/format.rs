//! The record, the shared constants, and every error the bytes can produce.
//!
//! ```text
//! byte 0                 32768                                      EOF
//!   ├──── header region ────┼─── record 0 ─┼─── record 1 ─┼── … ───────┤
//!    2 slots × 16384 spacing    56 bytes       56 bytes
//! ```
//!
//! **Address of record *i*: `header_len + i·stride`.** An add, a multiply, a
//! load. No search, no decode, no allocation — which is the entire reason the
//! format exists and the only thing that makes bar lookup O(1).
//!
//! The two numbers in that formula are **not** read from this module by the
//! read path. They come from [`crate::layout::Layout`], selected by the file's
//! own `format_version`. The constants here are version 2's definition, and
//! `store::unit::the_constants_are_the_current_versions_layout` pins them to
//! it.
//!
//! # Why this is version 2 and not an edited version 1
//!
//! `CLAUDE.md` §3 rule 8: *store format versions are never mutated in place*.
//! Version 1 — the geometry `docs/02-store-format.md` described before this
//! change — had a **64-byte header region holding one slot**, an 8-byte
//! `header_crc` at offset 48 covering bytes 0..48, and a byte-addressed
//! 4096-byte checksum block. Every one of those numbers is different now: the
//! header region is two slots spaced a failure unit apart, the checksum is
//! four bytes at offset 56 covering everything except itself, a `generation`
//! field was inserted at offset 16, and the block is counted in records.
//!
//! Keeping `MAGIC = b"BRUTEXB1"` and `FORMAT_VERSION = 1` across that would
//! make two incompatible geometries answer to one number, which is precisely
//! what [`crate::layout`]'s dispatch exists to make impossible. So the change
//! mints a version: [`MAGIC`] is `b"BRUTEXB2"` and [`FORMAT_VERSION`] is 2.
//!
//! Version 1 is **retired**, not deleted — see [`RETIRED_VERSIONS`]. It is
//! refused by number, naming the reason, so a version-1 file is never read at
//! version 2's offsets and never reported as a destroyed header.

use brutex_core::price::Paisa;

/// Bytes of fields in one header slot. A **family** constant: every format
/// version fills 64 bytes at the start of its slot.
///
/// This is what makes a header self-describing without knowing its version
/// first. A reader that cannot yet know the layout still knows where each slot
/// begins and how much of it to checksum, so it can decode whichever slots
/// survived a crash and let the magic and version inside them decide the rest.
/// Freezing the slot size is the price of that bootstrap, and it is cheap: a
/// new field takes reserved space in a new version, never more slot.
pub const SLOT_LEN: usize = 64;

/// Bytes from the start of one header slot to the start of the next. A
/// **family** constant, for the same bootstrap reason as [`SLOT_LEN`].
///
/// # Why 16384 and not 64
///
/// The two-slot header is only worth having if a write to one slot cannot
/// damage the other. Storage does not fail at byte granularity: the smallest
/// unit a device programs or a filesystem writes back is a block or a page,
/// and a partially programmed unit takes **everything in it** with it. At
/// 64-byte spacing both slots share one such unit, so the redundancy is
/// nominal — which is exactly why ext4 and F2FS separate their redundant
/// superblocks and checkpoint packs by whole blocks rather than by bytes.
///
/// Measured on the two hosts this repository runs on:
///
/// | Host | Device block | Page |
/// |---|---|---|
/// | Apple M4 Pro, macOS, APFS (`diskutil info /`, `sysctl hw.pagesize`) | 4096 | 16384 |
/// | GitHub CI runner, `x86_64` Linux, ext4 | 4096 | 4096 |
///
/// 16384 is the largest of those, so one slot per 16384 bytes puts the two
/// slots in different failure units on **both** hosts. It is a constant rather
/// than a query of the host, because a geometry that differed between the two
/// would make the same bytes verify differently on each — `CLAUDE.md` §3
/// rule 5, the same argument that rejected a page-sized [`BLOCK_LEN`].
///
/// *Rejected — 4096.* It equals the device block on both hosts but is smaller
/// than this machine's 16384-byte page, and page-granular writeback is a real
/// failure unit. The saving is 16 KiB per file, against a month file of
/// roughly 440 KiB.
///
/// **Honest limit:** this separates the slots by every unit that was measured.
/// It is not a proof. A failure whose granularity exceeds 16384 bytes — an
/// erase block, a dead device — takes both slots, and no arrangement inside
/// one file survives that. See the limits this crate reports.
pub const SLOT_STRIDE: u64 = 16_384;

/// The most header slots any version may declare. A **family** bound.
///
/// [`crate::header::Header::read_region`] must know how much of a buffer can
/// possibly be header before it knows the version, or a long buffer lets
/// record bytes audition as header slots. This is that bound.
pub const MAX_SLOT_COUNT: u64 = 2;

/// [`MAX_SLOT_COUNT`] as a `usize`, for iterator bounds.
///
/// Written as a literal in both widths rather than converted: this workspace
/// denies casts that can truncate, and a fallible conversion here would have a
/// failure arm no test could ever reach. The assertion below keeps the two
/// honest.
pub const MAX_SLOTS: usize = 2;

const _: () = assert!(MAX_SLOT_COUNT == 2 && MAX_SLOTS == 2);

/// The seven bytes shared by every version's magic.
///
/// Checked before the version is, so a file that is not a bar file at all is
/// named as such instead of being reported as an absurd version number.
pub const MAGIC_FAMILY: [u8; 7] = *b"BRUTEXB";

/// Identifies a version-2 bar file.
pub const MAGIC: [u8; 8] = *b"BRUTEXB2";

/// The only format version this build writes.
pub const FORMAT_VERSION: u16 = 2;

/// Versions that existed and are no longer readable by this build.
///
/// Version 1 described a 64-byte single-slot header with an 8-byte checksum at
/// offset 48 and a byte-addressed 4096-byte block. That header shape is not
/// this one, so decoding a version-1 file with this decoder would lift every
/// field from the wrong offset.
///
/// It is listed rather than forgotten so the refusal can say *why*. A version
/// that is merely absent from [`crate::layout::Layout::KNOWN`] reports
/// [`FormatError::UnknownVersion`], which reads as "this file is from the
/// future"; a retired version reports [`FormatError::RetiredVersion`], which
/// is the truth. `CLAUDE.md` §3 rule 8 makes the number append-only: it is
/// never reused for a different geometry.
///
/// No version-1 file can exist — version 1 never had a reader or a writer in
/// this repository, only constants. The entry costs one comparison and removes
/// the only way this build could misread one if that assumption is wrong.
///
/// A fixed-size array rather than a slice so the retirement test can be a
/// `const fn` that destructures it: retiring a second version changes the
/// length, which is a compile error at every destructuring site rather than a
/// silent drift.
pub const RETIRED_VERSIONS: [u16; 1] = [1];

/// Version 2: bytes of header region before the first record.
///
/// [`MAX_SLOT_COUNT`] slots at [`SLOT_STRIDE`] spacing. One slot could not be
/// updated atomically — see [`crate::header`].
///
/// Spelled as a literal rather than the product because that product needs a
/// `usize`→`u64` cast, and this workspace denies casts that can truncate.
/// `store::unit::the_constants_are_the_current_versions_layout` proves the
/// relationship instead, with a checked conversion.
pub const HEADER_LEN: u64 = 32_768;

/// Version 2: header slots. Writes alternate between them.
pub const SLOT_COUNT: u64 = 2;

/// Version 2: bytes per record. Read it from the layout; never assume it.
pub const RECORD_STRIDE: u64 = 56;

/// Version 2: whole records covered by one checksum block.
pub const RECORDS_PER_BLOCK: u64 = 73;

/// Version 2: bytes per checksummed block.
///
/// A whole multiple of [`RECORD_STRIDE`], which is the entire point: a record
/// cannot begin in one block and end in the next, so verifying it needs one
/// checksum and never two. See [`crate::layout`].
pub const BLOCK_LEN: u64 = RECORD_STRIDE * RECORDS_PER_BLOCK;

const _: () = assert!(BLOCK_LEN.is_multiple_of(RECORD_STRIDE));
const _: () = assert!(BLOCK_LEN / RECORD_STRIDE == RECORDS_PER_BLOCK);
const _: () = assert!(RECORDS_PER_BLOCK > 0);
const _: () = assert!(HEADER_LEN == SLOT_COUNT * SLOT_STRIDE);
const _: () = assert!(SLOT_COUNT <= MAX_SLOT_COUNT);
const _: () = assert!(BLOCK_LEN == 4088);
const _: () = assert!(SLOT_STRIDE >= 16_384);

/// Bit 0 of [`Header::flags`]: per-block checksums are present.
///
/// A file that sets it carries a sidecar of one [`crate::block`] checksum per
/// block; a file that does not carries none, and a reader must say so rather
/// than reporting an unverified block as verified.
///
/// [`Header::flags`]: crate::header::Header::flags
pub const FLAG_CHECKSUMS: u32 = 1;

/// Open interest is absent, as opposed to genuinely zero.
///
/// Spot indices carry no open interest and store this sentinel. Conflating it
/// with a real `0` would make a derivative series and an index series
/// indistinguishable on a field that decides which is which.
pub const OI_NULL: i64 = i64::MIN;

/// One bar. Seven `i64`, 56 bytes, no padding.
///
/// `#[repr(C)]` fixes the field order so the layout is the format rather than
/// a compiler choice. The assertions below are compile errors, not tests —
/// `docs/04-invariants.md` S-01.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bar {
    /// Microseconds since the Unix epoch, UTC. The **open** of the bar's
    /// interval — verified against 1,706,290 lake bars, not assumed.
    pub ts_micros: i64,
    /// Open, in paisa.
    pub open: i64,
    /// High, in paisa.
    pub high: i64,
    /// Low, in paisa.
    pub low: i64,
    /// Close, in paisa.
    pub close: i64,
    /// Contracts or shares. `0` is a real zero, not an absence.
    pub volume: i64,
    /// Open interest, or [`OI_NULL`] when absent.
    pub open_interest: i64,
}

const _: () = assert!(size_of::<Bar>() == 56);
const _: () = assert!(align_of::<Bar>() == 8);
const _: () = assert!(RECORD_STRIDE == 56);

impl Bar {
    /// Whether open interest is absent rather than zero.
    #[must_use]
    pub const fn oi_is_null(&self) -> bool {
        self.open_interest == OI_NULL
    }

    /// Open interest as a value, or `None` when absent.
    #[must_use]
    pub const fn oi(&self) -> Option<i64> {
        if self.oi_is_null() {
            None
        } else {
            Some(self.open_interest)
        }
    }

    /// Whether the OHLC values are internally consistent.
    ///
    /// Checked at the write boundary, never at read: a file that already holds
    /// an impossible bar is corrupt, and the CRC is what detects that. This
    /// exists so an impossible bar never reaches the disk in the first place.
    ///
    /// Note what it deliberately does **not** claim: an all-zero record
    /// satisfies it. Zeros are a legal flat bar with a real open interest of
    /// zero, so this can never distinguish a bar from a lost write. That is
    /// [`crate::block`]'s job, and it is why `FLAG_CHECKSUMS` is not
    /// decoration.
    #[must_use]
    pub const fn ohlc_is_sane(&self) -> bool {
        let hi_ok = self.high >= self.open && self.high >= self.low && self.high >= self.close;
        let lo_ok = self.low <= self.open && self.low <= self.close;
        hi_ok && lo_ok
    }

    /// The close, as a price.
    #[must_use]
    pub const fn close_price(&self) -> Paisa {
        Paisa::from_raw(self.close)
    }

    /// The 56-byte image of this record, little-endian, in field order.
    ///
    /// # Why an explicit encoder rather than a cast of the struct
    ///
    /// `#[repr(C)]` fixes the field *order*, not the byte order, so a struct
    /// reinterpreted as bytes is the host's endianness — and this file is meant
    /// to be the same bytes on every host, which is the argument
    /// `docs/02-store-format.md` already makes about the block length. Casting
    /// would also need `unsafe`, and every crate here carries
    /// `#![forbid(unsafe_code)]`. Little-endian matches the header, whose every
    /// field `crate::header` writes with `to_le_bytes`, so one file has one byte
    /// order rather than two.
    ///
    /// It lives here rather than in the crate that ingests, because
    /// `docs/02-store-format.md` §3 is the authority for these bytes and a
    /// second encoder in `crates/pull` would be a second definition of the
    /// format, free to drift. `CLAUDE.md` §5's arrow points one way: `pull`
    /// depends on `store`.
    #[must_use]
    pub fn image(&self) -> [u8; RECORD_LEN] {
        let mut out = [0u8; RECORD_LEN];
        write_at(&mut out, 0, self.ts_micros.to_le_bytes());
        write_at(&mut out, 8, self.open.to_le_bytes());
        write_at(&mut out, 16, self.high.to_le_bytes());
        write_at(&mut out, 24, self.low.to_le_bytes());
        write_at(&mut out, 32, self.close.to_le_bytes());
        write_at(&mut out, 40, self.volume.to_le_bytes());
        write_at(&mut out, 48, self.open_interest.to_le_bytes());
        out
    }

    /// Decodes one record.
    ///
    /// There is nothing to validate here and that is deliberate:
    /// `docs/02-store-format.md` §3 — *"a record has no structure that a lost
    /// write violates"*. An all-zero record is a legal flat bar. Detecting a
    /// lost write is [`crate::block`]'s job, and range validation happens at the
    /// ingest boundary before a byte is written, never on the way back in.
    ///
    /// # Errors
    ///
    /// [`FormatError::RecordTooShort`] when fewer than [`RECORD_STRIDE`] bytes
    /// are offered. Refused rather than zero-filled: a short tail is
    /// `store::unit`'s ragged-file case, and inventing the missing bytes would
    /// manufacture a bar nobody wrote.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut image = [0u8; RECORD_LEN];
        if bytes.len() < RECORD_LEN {
            return Err(FormatError::RecordTooShort { len: bytes.len() });
        }
        for (dst, src) in image.iter_mut().zip(bytes.iter()) {
            *dst = *src;
        }
        Ok(Self {
            ts_micros: i64::from_le_bytes(le_bytes(&image, 0)),
            open: i64::from_le_bytes(le_bytes(&image, 8)),
            high: i64::from_le_bytes(le_bytes(&image, 16)),
            low: i64::from_le_bytes(le_bytes(&image, 24)),
            close: i64::from_le_bytes(le_bytes(&image, 32)),
            volume: i64::from_le_bytes(le_bytes(&image, 40)),
            open_interest: i64::from_le_bytes(le_bytes(&image, 48)),
        })
    }
}

/// [`RECORD_STRIDE`] as a length, for the record image.
///
/// Written as a literal in both widths rather than converted, for the reason
/// `crate::header` gives about every other paired constant: a conversion here
/// would carry a failure arm no test could reach. The assertion keeps the two
/// honest.
pub const RECORD_LEN: usize = 56;

const _: () = assert!(RECORD_STRIDE == 56 && RECORD_LEN == 56);

/// Writes `src` at `offset` in a record image.
///
/// Index-free, because `clippy::indexing_slicing` is denied across this
/// workspace and an encoder is exactly where a panicking index would eventually
/// be reached by an offset that moved.
fn write_at<const N: usize>(out: &mut [u8; RECORD_LEN], offset: usize, src: [u8; N]) {
    for (dst, byte) in out.iter_mut().skip(offset).zip(src) {
        *dst = byte;
    }
}

/// Reads `N` little-endian bytes at `offset` from a record image.
fn le_bytes<const N: usize>(image: &[u8; RECORD_LEN], offset: usize) -> [u8; N] {
    let mut out = [0u8; N];
    for (dst, src) in out.iter_mut().zip(image.iter().skip(offset)) {
        *dst = *src;
    }
    out
}

/// Something about the bytes is wrong.
///
/// Every variant names the value it saw. A refusal that does not say what it
/// refused sends the reader back to a hex dump, which is where a "just retry
/// it" habit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FormatError {
    /// A byte offset would not fit in `u64`.
    ///
    /// Refused rather than wrapped: a wrapped offset addresses a *different,
    /// valid-looking* record.
    OffsetOverflow,
    /// A slot buffer is shorter than [`SLOT_LEN`].
    SlotTooShort {
        /// Bytes the caller supplied.
        len: usize,
    },
    /// A record buffer is shorter than [`RECORD_STRIDE`].
    ///
    /// Distinct from [`FormatError::SlotTooShort`] because a short *record* and
    /// a short *slot* are two different files being wrong in two different
    /// places, and one message for both sends a reader to the wrong offset.
    RecordTooShort {
        /// Bytes the caller supplied.
        len: usize,
    },
    /// The header region is shorter than its version's slot count demands.
    ///
    /// A short region silently returns whichever older commit happened to fit,
    /// which is a fallback that hides a failure. Refused instead.
    HeaderRegionTooShort {
        /// Whole slots the region actually holds.
        slots: u64,
        /// Whole slots the version declares.
        need: u64,
    },
    /// The first seven bytes are not [`MAGIC_FAMILY`].
    NotABarFile,
    /// The header names a version this build does not know.
    ///
    /// Refused rather than guessed: `docs/02-store-format.md` mints a new
    /// version for every layout change precisely so old files stay readable at
    /// their own stride, and guessing defeats that.
    UnknownVersion(u16),
    /// The header names a version this build knows of and cannot decode.
    ///
    /// Distinct from [`FormatError::UnknownVersion`] because the two send an
    /// operator to opposite places: an unknown version means the file is newer
    /// than the build, a retired one means it is older. See
    /// [`RETIRED_VERSIONS`].
    RetiredVersion(u16),
    /// The magic's version byte disagrees with the `format_version` field.
    ///
    /// Two independent statements of the same fact; when they differ the file
    /// is not what either of them claims.
    MagicVersionMismatch(u16),
    /// The header names a stride other than the one its version defines.
    StrideMismatch(u16),
    /// The file is shorter than its version's header region.
    TooShortForHeader,
    /// `n_valid` claims more records than the file can hold.
    ///
    /// Trusting the counter over the file length would read past the end and
    /// return whatever bytes happened to be there.
    CounterExceedsFile,
    /// Appending would push `n_valid` past `u64::MAX`.
    CounterOverflow,
    /// The generation counter cannot be advanced again.
    ///
    /// Refused rather than wrapped, because a wrapped generation would make
    /// the *oldest* slot win and silently un-commit records.
    GenerationExhausted,
    /// An append's timestamps do not follow the records already committed.
    ///
    /// Bars are append-only in time. A batch that begins at or before the
    /// file's last timestamp is a re-pull landing on top of newer data, which
    /// no checksum can detect afterwards because the bytes are well formed.
    TimestampsOutOfOrder {
        /// The last timestamp already committed, or the batch's own first.
        previous: i64,
        /// The timestamp that did not follow it.
        next: i64,
    },
    /// A slot's stored checksum does not match its bytes.
    SlotChecksum {
        /// The checksum the slot carries.
        stored: u32,
        /// The checksum its bytes actually produce.
        computed: u32,
    },
    /// A block's stored checksum does not match its bytes.
    BlockChecksum {
        /// Which block.
        block: u64,
        /// The checksum the sidecar carries.
        stored: u32,
        /// The checksum the block's committed bytes actually produce.
        computed: u32,
    },
    /// A block index past the last block the commit counter covers.
    ///
    /// Verifying it would checksum bytes nobody committed and report the
    /// result as if it meant something.
    BlockNotCommitted {
        /// The block asked for.
        block: u64,
        /// How many blocks the counter covers.
        blocks: u64,
    },
    /// A caller offered a block's bytes at a length the geometry does not give
    /// that block.
    BlockLengthMismatch {
        /// Which block.
        block: u64,
        /// Bytes the caller supplied.
        len: usize,
        /// Bytes the block covers under the current commit counter.
        need: u64,
    },
    /// The file declares no block checksums, so a block cannot be verified.
    ///
    /// Reported rather than passed: "verified" and "there was nothing to
    /// verify against" are not the same answer.
    ChecksumsAbsent,
    /// A slot's generation says it should live in a different slot.
    ///
    /// A writer that puts a commit in the wrong slot can overwrite the only
    /// surviving copy of the previous one, so this is refused rather than
    /// tolerated.
    SlotPositionMismatch {
        /// Where `generation % slot_count` says it belongs.
        expected: u64,
        /// Where it was found.
        found: u64,
    },
    /// A declared layout is not a geometry a file could have.
    ///
    /// Refused at declaration rather than guarded at every use: a zero
    /// `records_per_block` is a division fault, and a header region that is
    /// not a whole number of slots puts a slot at an offset no reader looks
    /// at.
    DegenerateLayout {
        /// Which field of the declaration is impossible.
        field: &'static str,
    },
    /// No slot in the header region decoded.
    ///
    /// Not a torn tail — a torn tail is unobservable. This means every copy of
    /// the header is damaged, and there is nothing to fall back to that would
    /// not be a guess.
    NoValidHeader,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::OffsetOverflow => f.write_str("record offset overflows u64"),
            Self::SlotTooShort { len } => {
                write!(f, "header slot is {len} bytes, needs {SLOT_LEN}")
            }
            Self::RecordTooShort { len } => {
                write!(f, "record is {len} bytes, needs {RECORD_STRIDE}")
            }
            Self::HeaderRegionTooShort { slots, need } => {
                write!(f, "header region holds {slots} whole slots, needs {need}")
            }
            Self::NotABarFile => f.write_str("magic does not begin BRUTEXB"),
            Self::UnknownVersion(v) => write!(f, "unknown format version {v}"),
            Self::RetiredVersion(v) => {
                write!(f, "format version {v} is retired and cannot be decoded")
            }
            Self::MagicVersionMismatch(v) => {
                write!(f, "magic does not belong to format version {v}")
            }
            Self::StrideMismatch(s) => write!(f, "record stride {s} is not the version's stride"),
            Self::TooShortForHeader => f.write_str("file is shorter than its header region"),
            Self::CounterExceedsFile => {
                f.write_str("n_valid claims more records than the file holds")
            }
            Self::CounterOverflow => f.write_str("n_valid would overflow u64"),
            Self::GenerationExhausted => f.write_str("header generation cannot advance past u64"),
            Self::TimestampsOutOfOrder { previous, next } => {
                write!(f, "timestamp {next} does not follow {previous}")
            }
            Self::SlotChecksum { stored, computed } => {
                write!(f, "header slot checksum {stored:#010x} != {computed:#010x}")
            }
            Self::BlockChecksum {
                block,
                stored,
                computed,
            } => write!(
                f,
                "block {block} checksum {stored:#010x} != {computed:#010x}"
            ),
            Self::BlockNotCommitted { block, blocks } => {
                write!(f, "block {block} is past the {blocks} blocks committed")
            }
            Self::BlockLengthMismatch { block, len, need } => {
                write!(f, "block {block} was offered {len} bytes, covers {need}")
            }
            Self::ChecksumsAbsent => f.write_str("the file declares no block checksums"),
            Self::SlotPositionMismatch { expected, found } => {
                write!(
                    f,
                    "header slot {found} holds a commit belonging in {expected}"
                )
            }
            Self::DegenerateLayout { field } => {
                write!(f, "layout field {field} is not a geometry a file can have")
            }
            Self::NoValidHeader => f.write_str("no header slot survived; the header is unreadable"),
        }
    }
}

impl std::error::Error for FormatError {}
