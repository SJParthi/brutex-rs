//! The two-slot header, and the commit that is one write of one unit.
//!
//! # What was wrong
//!
//! `docs/05-decisions.md` D-0004 makes the commit counter the authority and
//! publishes it last, so a torn *record* is unobservable. That argument is
//! sound and it survives here unchanged. What it does not cover is a torn
//! *header*: publishing a commit meant writing `n_valid`, then
//! `last_ts_micros`, then `header_crc` as three separate stores. A crash
//! between any two of them left a header whose checksum did not match its own
//! counter — and a header that fails its checksum condemns the **whole file**,
//! not the tail. The counter made the last record safe and the header unsafe.
//!
//! # The shape that fixes it
//!
//! A header **slot** is 64 bytes carrying every field *and* the checksum over
//! them. A file holds [`Layout::slot_count`] slots at
//! [`crate::format::SLOT_STRIDE`] spacing, and commit *g* is written to slot
//! `g % slot_count`. Consecutive commits therefore never write the same slot,
//! and a reader takes the valid slot with the highest generation **that the
//! file's bytes actually support**.
//!
//! ```text
//! byte 0          16384          32768
//!   ├── slot 0 ─────┼── slot 1 ────┤   generation 4 → slot 0
//!    generation 4     generation 3     generation 5 → slot 1
//! ```
//!
//! A crash can land in exactly one of three places, and none of them is a
//! self-inconsistent header:
//!
//! | Crash point | What survives | What a reader sees |
//! |---|---|---|
//! | before the slot write | both slots whole | generation *g−1*: the records committed before |
//! | during the slot write | the *other* slot whole | generation *g−1*, because a partial slot fails its own checksum |
//! | after the slot write | both slots whole | generation *g*: the new records |
//!
//! The reader never sees a count that was not committed — only *g−1* or *g*.
//! `store::fault::a_torn_header_commit_never_reports_an_uncommitted_count`
//! writes every one of the 65 possible prefixes of a commit image and asserts
//! exactly that.
//!
//! The fourth row of that table is the one that was missing: a slot that is
//! **whole but unsupported**, because the records it counts never reached the
//! disk. [`Header::read_region`] falls back to the previous generation for
//! that case too, instead of condemning the file — see
//! `store::fault::kill_between_write_and_commit`.
//!
//! # Why not the alternatives
//!
//! **One aligned word holding counter, timestamp and checksum.** Arithmetically
//! impossible: those are 8 + 8 + 4 bytes and a machine word is 8. Making them
//! fit means truncating the timestamp, which is a fabrication in the one field
//! a range reject depends on.
//!
//! **The counter as the only authority, everything else derived.** It still
//! needs one 8-byte store to be atomic across a power loss, which is a
//! property of the device, not of this program — "in practice atomic" is not a
//! measurement. It also derives `last_ts_micros` by reading record
//! `n_valid − 1`, which cannot be trusted until its block checksum is
//! verified, which needs a header. A dependency loop on the open path.
//!
//! # The three assumptions, named rather than assumed
//!
//! An earlier version of this module claimed two slots assume "only that a
//! write to slot A cannot damage slot B … they are disjoint byte ranges, so
//! that is true on any device". **That was false.** Storage does not fail at
//! byte granularity; it fails at a block or a page, and two slots 64 bytes
//! apart share one of those. The three assumptions the crash table really
//! rests on are:
//!
//! 1. **A write to one slot cannot damage another.** Now structural:
//!    [`crate::format::SLOT_STRIDE`] is 16384 bytes, which is at least the
//!    device block *and* the page size measured on both hosts this repository
//!    builds on. Residual limit: a failure coarser than 16384 bytes takes both
//!    slots, and no arrangement inside one file survives that.
//! 2. **The appended records are durable before the slot write.** Now carried
//!    by the type: [`Commit::durable_through`] is the byte offset a writer
//!    must have flushed before it issues the slot write. This crate performs
//!    no I/O and cannot issue the barrier itself, so it states the offset
//!    instead of leaving the requirement in prose. When the requirement is
//!    broken anyway, [`Header::read_region`] falls back to the previous
//!    generation rather than handing back uncommitted bytes, and
//!    [`crate::block`] is what detects the records that were lost.
//! 3. **Exactly one writer per file.** Two writers reading the same generation
//!    produce the same slot at the same offset and the same record range.
//!    Nothing in this crate can enforce that — an exclusive lock is I/O — so
//!    it is a documented precondition of [`Header::commit`], the lock file has
//!    a name in the path type ([`crate::path::FileKind::Lock`]), and the gap
//!    is reported as a limit rather than implied away.
//!
//! # It is still a read-only mapping plus `pwrite`
//!
//! Nothing here writes. [`Header::commit`] returns a [`Commit`] — one offset
//! and one 64-byte buffer — which a writer hands to a single positional write.
//! A writable mapping remains banned: it raises `SIGBUS` on a full disk, and a
//! signal cannot be caught. **There is no API in this module that can update
//! two header fields in two writes**, because a `Commit` carries one offset
//! and one buffer and there is no other way to produce header bytes.
//!
//! [`Header::decode`] takes its own 64-byte copy of a slot before it checksums
//! anything, so a caller may hand it a `MAP_SHARED` mapping that a writer is
//! concurrently `pwrite`-ing: the checksum covers exactly the bytes the
//! returned `Header` is built from, and a copy that caught a write mid-flight
//! fails that checksum rather than returning half of each.

use crate::crc::crc32c;
use crate::format::{FLAG_CHECKSUMS, FormatError, MAGIC_FAMILY, MAX_SLOTS, SLOT_LEN, SLOT_STRIDE};
use crate::layout::Layout;

/// Byte offset of `magic` within a slot.
const OFF_MAGIC: usize = 0;
/// Byte offset of `format_version` within a slot.
const OFF_VERSION: usize = 8;
/// Byte offset of `record_stride` within a slot.
const OFF_STRIDE: usize = 10;
/// Byte offset of `flags` within a slot.
const OFF_FLAGS: usize = 12;
/// Byte offset of `generation` within a slot.
const OFF_GENERATION: usize = 16;
/// Byte offset of `n_valid` within a slot.
const OFF_N_VALID: usize = 24;
/// Byte offset of `first_ts_micros` within a slot.
const OFF_FIRST_TS: usize = 32;
/// Byte offset of `last_ts_micros` within a slot.
const OFF_LAST_TS: usize = 40;
/// Byte offset of `symbol_id` within a slot.
const OFF_SYMBOL_ID: usize = 48;
/// Byte offset of `timeframe_secs` within a slot.
const OFF_TIMEFRAME: usize = 52;
/// Byte offset of the slot checksum.
const OFF_CRC: usize = 56;
/// Byte offset of the reserved tail, which is zero and stays zero.
const OFF_RESERVED: usize = 60;

const _: () = assert!(OFF_RESERVED < SLOT_LEN);
const _: () = assert!(OFF_CRC + 4 == OFF_RESERVED);

/// [`crate::format::SLOT_STRIDE`] as a `usize`, for slicing a region.
///
/// Written as a literal in both widths rather than converted, so there is no
/// cast to deny and no fallible conversion whose failure arm no test could
/// ever reach. The assertion is what keeps the two honest.
const SLOT_STRIDE_LEN: usize = 16_384;
const _: () = assert!(SLOT_STRIDE == 16_384 && SLOT_STRIDE_LEN == 16_384);

/// [`Layout::CURRENT`]'s record stride, in the width the slot stores it.
///
/// Written as a literal in both widths for the same reason. A future
/// `CURRENT` with a different stride is a compile error here.
const CURRENT_STRIDE: u16 = 56;
const _: () = assert!(CURRENT_STRIDE == 56 && Layout::CURRENT.record_stride() == 56);

/// One header slot's fields.
///
/// `n_valid` is the commit counter — `docs/05-decisions.md` D-0004. A reader
/// treats records `0..n_valid` as the whole file, so a half-written record at
/// the tail is not merely unlikely, it is **unobservable**. `generation`
/// extends that guarantee to the header itself: it says which slot is the
/// newest, so a half-written *header* is unobservable too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Header {
    /// Format version. Selects the layout; never assumed.
    pub format_version: u16,
    /// Bytes per record, as this file declares them.
    pub record_stride: u16,
    /// Bit 0: block checksums present.
    pub flags: u32,
    /// Which commit this slot holds. Higher wins.
    pub generation: u64,
    /// The commit counter: how many records are readable.
    pub n_valid: u64,
    /// Timestamp of record 0, for a cheap range reject.
    ///
    /// Meaningful only when `n_valid > 0`. [`Header::advance`] sets it from
    /// the first batch appended to an empty file and never moves it again, so
    /// a range reject over `[first_ts_micros, last_ts_micros]` describes the
    /// records that are actually there.
    pub first_ts_micros: i64,
    /// Timestamp of record `n_valid - 1`. Meaningful only when `n_valid > 0`.
    pub last_ts_micros: i64,
    /// Resolved from the path; a cross-check, never the index.
    pub symbol_id: u32,
    /// Seconds per bar. 60 for one minute.
    pub timeframe_secs: u32,
}

/// One header write: one offset, one buffer, one `pwrite`.
///
/// The type is the guarantee. A commit cannot be expressed as several field
/// updates because there is nowhere to put the second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    /// Which slot this belongs in — `generation % slot_count`.
    pub slot: u64,
    /// The byte offset to write at.
    pub offset: u64,
    /// Exactly the bytes to write, checksum included.
    pub bytes: [u8; SLOT_LEN],
    /// The byte offset through which record data must already be durable.
    ///
    /// `header_len + n_valid · stride` — the end of the last record this
    /// commit publishes. Issuing the slot write before the bytes below this
    /// offset are on stable storage publishes a counter over data that may not
    /// exist; the header page is re-dirtied on every commit and is therefore
    /// the hottest writeback candidate in the file, so the reordering is the
    /// likely case rather than the exotic one.
    ///
    /// This crate performs no I/O and cannot issue the barrier. It states the
    /// offset so a writer cannot claim it did not know which one to flush.
    pub durable_through: u64,
    /// The header state this commit publishes.
    pub header: Header,
}

impl Header {
    /// A fresh header for an empty file: generation 0, no records.
    ///
    /// The caller writes [`Header::commit`] into a zero-filled header region.
    /// Every other slot stays zero, which fails the magic check and is
    /// therefore not a candidate — an empty slot and a corrupt one are the
    /// same thing to a reader, and both are safe.
    #[must_use]
    pub const fn genesis(symbol_id: u32, timeframe_secs: u32, flags: u32) -> Self {
        Self {
            format_version: Layout::CURRENT.version(),
            record_stride: CURRENT_STRIDE,
            flags,
            generation: 0,
            n_valid: 0,
            first_ts_micros: 0,
            last_ts_micros: 0,
            symbol_id,
            timeframe_secs,
        }
    }

    /// Whether this file carries per-block checksums.
    ///
    /// A file without them cannot have a block verified at all, and
    /// [`crate::block::verify`] says so rather than returning "fine".
    #[must_use]
    pub const fn checksums_present(&self) -> bool {
        self.flags & FLAG_CHECKSUMS != 0
    }

    /// The header that publishing `appended` more records produces.
    ///
    /// Advances the generation and the counter together, because they are
    /// written together, and carries the batch's timestamps into the range the
    /// header advertises. `first_ts_micros` is taken from the **first** batch
    /// appended to an empty file and never moved afterwards; every later batch
    /// only moves `last_ts_micros`. Before this, no method in this crate set
    /// `first_ts_micros` at all, so every header it could commit claimed
    /// record 0 sat at the Unix epoch and a range reject built on it accepted
    /// every file that has ever existed.
    ///
    /// When `appended` is zero there is no batch to describe: the counter and
    /// both timestamps are carried forward unchanged and the two timestamp
    /// arguments are not read. That is a re-publish of the same state under a
    /// new generation, which is idempotent and safe.
    ///
    /// # Errors
    ///
    /// [`FormatError::GenerationExhausted`] or [`FormatError::CounterOverflow`]
    /// — both refusals, never wraps. A wrapped generation makes the *oldest*
    /// slot win and silently un-commits records.
    ///
    /// [`FormatError::TimestampsOutOfOrder`] when the batch's own timestamps
    /// run backwards, or when it begins at or before the last timestamp
    /// already committed. Bars are append-only in time, and a re-pull landing
    /// on top of newer data is well-formed bytes that no checksum can catch
    /// afterwards.
    pub const fn advance(
        &self,
        appended: u64,
        first_ts_micros: i64,
        last_ts_micros: i64,
    ) -> Result<Self, FormatError> {
        let (generation, n_valid) = match (
            self.generation.checked_add(1),
            self.n_valid.checked_add(appended),
        ) {
            (None, _) => return Err(FormatError::GenerationExhausted),
            (Some(_), None) => return Err(FormatError::CounterOverflow),
            (Some(generation), Some(n_valid)) => (generation, n_valid),
        };
        if appended == 0 {
            return Ok(Self {
                generation,
                ..*self
            });
        }
        if last_ts_micros < first_ts_micros {
            return Err(FormatError::TimestampsOutOfOrder {
                previous: first_ts_micros,
                next: last_ts_micros,
            });
        }
        if self.n_valid > 0 && first_ts_micros <= self.last_ts_micros {
            return Err(FormatError::TimestampsOutOfOrder {
                previous: self.last_ts_micros,
                next: first_ts_micros,
            });
        }
        let first = if self.n_valid == 0 {
            first_ts_micros
        } else {
            self.first_ts_micros
        };
        Ok(Self {
            generation,
            n_valid,
            first_ts_micros: first,
            last_ts_micros,
            ..*self
        })
    }

    /// The single write that publishes this header.
    ///
    /// # Preconditions the caller owns
    ///
    /// 1. **One writer per file.** Two writers that read the same generation
    ///    produce the same slot, the same offset and the same record range;
    ///    whichever lands last wins, and a crash during the second destroys
    ///    the slot that held the only copy of the previous commit. Take an
    ///    exclusive lock on [`crate::path::FileKind::Lock`] first. This crate
    ///    performs no I/O and cannot check it.
    /// 2. **Flush first.** Every byte below [`Commit::durable_through`] must
    ///    be on stable storage before this write is issued.
    ///
    /// # Errors
    ///
    /// [`FormatError::UnknownVersion`] or [`FormatError::RetiredVersion`] if
    /// this header names a version this build cannot write,
    /// [`FormatError::StrideMismatch`] if it names a stride its own version
    /// does not define, or [`FormatError::OffsetOverflow`] if the counter puts
    /// the end of the data past `u64`.
    pub fn commit(&self) -> Result<Commit, FormatError> {
        let layout = Layout::for_version(self.format_version)?;
        if u64::from(self.record_stride) != layout.record_stride() {
            return Err(FormatError::StrideMismatch(self.record_stride));
        }
        Ok(Commit {
            slot: self.generation % layout.slot_count(),
            offset: layout.slot_offset(self.generation),
            bytes: self.image(layout.magic()),
            durable_through: layout.offset_of(self.n_valid)?,
            header: *self,
        })
    }

    /// Checks this header against a known file length.
    ///
    /// # Errors
    ///
    /// [`FormatError::StrideMismatch`] for a stride its version does not
    /// define, [`FormatError::TooShortForHeader`] for a file that cannot hold
    /// the header region, [`FormatError::CounterExceedsFile`] for a counter
    /// claiming more records than the bytes can contain — trusting the counter
    /// over the length reads past the end and returns whatever was there — or
    /// [`FormatError::TimestampsOutOfOrder`] for a non-empty file whose
    /// advertised range runs backwards.
    pub fn validate(&self, layout: Layout, file_len: u64) -> Result<(), FormatError> {
        if u64::from(self.record_stride) != layout.record_stride() {
            return Err(FormatError::StrideMismatch(self.record_stride));
        }
        if file_len < layout.header_len() {
            return Err(FormatError::TooShortForHeader);
        }
        if self.n_valid > layout.capacity_for(file_len) {
            return Err(FormatError::CounterExceedsFile);
        }
        if self.n_valid > 0 && self.last_ts_micros < self.first_ts_micros {
            return Err(FormatError::TimestampsOutOfOrder {
                previous: self.first_ts_micros,
                next: self.last_ts_micros,
            });
        }
        Ok(())
    }

    /// Decodes one slot, checksum and all.
    ///
    /// The slot is copied into a 64-byte array **once**, before anything is
    /// checked, and every field is then read from that copy. That is what
    /// makes it safe over a `MAP_SHARED` mapping a writer is `pwrite`-ing: the
    /// checksum verifies exactly the bytes the returned `Header` carries. Were
    /// the buffer re-read after the checksum passed, a write landing in
    /// between would produce a header no checksum ever covered — and with
    /// `#![forbid(unsafe_code)]` there is no fence available to order plain
    /// loads, so the copy is the fix rather than a mitigation.
    ///
    /// # Errors
    ///
    /// [`FormatError::SlotTooShort`], [`FormatError::NotABarFile`],
    /// [`FormatError::UnknownVersion`], [`FormatError::RetiredVersion`],
    /// [`FormatError::MagicVersionMismatch`], [`FormatError::SlotChecksum`] or
    /// [`FormatError::StrideMismatch`] — checked in that order, so the most
    /// basic disagreement is the one reported.
    pub fn decode(slot: &[u8]) -> Result<Self, FormatError> {
        Self::decode_parts(slot).map(|(header, _)| header)
    }

    /// [`Header::decode`], keeping the layout it already resolved.
    ///
    /// [`Header::read_region`] needs both, and re-resolving the version would
    /// add an error path no test could reach — the version was accepted three
    /// lines earlier.
    fn decode_parts(slot: &[u8]) -> Result<(Self, Layout), FormatError> {
        if slot.len() < SLOT_LEN {
            return Err(FormatError::SlotTooShort { len: slot.len() });
        }
        // The one observation of the caller's bytes. Everything below reads
        // this array; nothing below touches `slot` again.
        let mut image = [0u8; SLOT_LEN];
        for (dst, src) in image.iter_mut().zip(slot.iter()) {
            *dst = *src;
        }

        let magic: [u8; 8] = le_bytes(&image, OFF_MAGIC);
        if magic
            .iter()
            .take(MAGIC_FAMILY.len())
            .ne(MAGIC_FAMILY.iter())
        {
            return Err(FormatError::NotABarFile);
        }
        let format_version = u16::from_le_bytes(le_bytes(&image, OFF_VERSION));
        let layout = Layout::for_version(format_version)?;
        if magic != layout.magic() {
            return Err(FormatError::MagicVersionMismatch(format_version));
        }
        let stored = u32::from_le_bytes(le_bytes(&image, OFF_CRC));
        let computed = crc32c(covered(&image));
        if stored != computed {
            return Err(FormatError::SlotChecksum { stored, computed });
        }
        let record_stride = u16::from_le_bytes(le_bytes(&image, OFF_STRIDE));
        if u64::from(record_stride) != layout.record_stride() {
            return Err(FormatError::StrideMismatch(record_stride));
        }
        Ok((
            Self {
                format_version,
                record_stride,
                flags: u32::from_le_bytes(le_bytes(&image, OFF_FLAGS)),
                generation: u64::from_le_bytes(le_bytes(&image, OFF_GENERATION)),
                n_valid: u64::from_le_bytes(le_bytes(&image, OFF_N_VALID)),
                first_ts_micros: i64::from_le_bytes(le_bytes(&image, OFF_FIRST_TS)),
                last_ts_micros: i64::from_le_bytes(le_bytes(&image, OFF_LAST_TS)),
                symbol_id: u32::from_le_bytes(le_bytes(&image, OFF_SYMBOL_ID)),
                timeframe_secs: u32::from_le_bytes(le_bytes(&image, OFF_TIMEFRAME)),
            },
            layout,
        ))
    }

    /// Reads the whole header region and returns the committed state.
    ///
    /// Each of the first [`crate::format::MAX_SLOTS`] slot positions is
    /// decoded independently — each carries its own magic, version and
    /// checksum — and the valid one with the highest generation that the
    /// file's length supports wins. A slot that is empty, stale, half-written,
    /// corrupt or in the wrong position is simply not a candidate.
    ///
    /// Two things it deliberately does **not** do:
    ///
    /// * It does not scan past [`crate::format::MAX_SLOTS`] positions. The
    ///   region a caller passes may be a whole read-only mapping, and every
    ///   aligned run of record bytes would otherwise audition as a header.
    /// * It does not condemn the file when the newest slot fails its check
    ///   against the file length. That is the crash where the header became
    ///   durable before the records it counts, and the previous generation is
    ///   sitting intact in the other slot; returning an error there loses a
    ///   month of bars to recover a tail.
    ///
    /// # Errors
    ///
    /// [`FormatError::HeaderRegionTooShort`] when the region is smaller than
    /// the version's header region — a short region would otherwise return
    /// whichever older commit happened to fit, silently losing every record
    /// committed since. [`FormatError::NoValidHeader`] when no slot decodes
    /// and none gave a specific reason: every copy of the header is damaged,
    /// and anything else this function could return would be a guess. When a
    /// slot did give a specific reason — an unknown or retired version, a
    /// stride disagreement, a failed checksum, or a commit found in the wrong
    /// slot — that reason is reported instead, because "the header is
    /// unreadable" is a false diagnosis of an intact file from another
    /// version. When slots decode but none survives [`Header::validate`], the
    /// newest one's refusal is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use store::{format::FormatError, header::Header};
    /// // An empty file: a zeroed header region, then the genesis commit.
    /// let mut region = vec![0u8; 32_768];
    /// let genesis = Header::genesis(1, 60, 0).commit()?;
    /// assert_eq!(genesis.offset, 0);
    /// assert_eq!(genesis.durable_through, 32_768);
    /// region
    ///     .iter_mut()
    ///     .zip(genesis.bytes)
    ///     .for_each(|(dst, src)| *dst = src);
    ///
    /// let read = Header::read_region(&region, 32_768)?;
    /// assert_eq!(read.n_valid, 0);
    /// assert_eq!(read.generation, 0);
    /// # Ok::<(), FormatError>(())
    /// ```
    pub fn read_region(region: &[u8], file_len: u64) -> Result<Self, FormatError> {
        let (mut header, mut layout) = best_candidate(region, None)?;

        let slots = whole_slots(region);
        if slots < layout.slot_count() {
            return Err(FormatError::HeaderRegionTooShort {
                slots,
                need: layout.slot_count(),
            });
        }

        loop {
            match header.validate(layout, file_len) {
                Ok(()) => return Ok(header),
                Err(refusal) => match best_candidate(region, Some(header.generation)) {
                    Ok((older, older_layout)) => {
                        header = older;
                        layout = older_layout;
                    }
                    Err(_) => return Err(refusal),
                },
            }
        }
    }

    /// The 64-byte slot image for this header, checksum computed.
    fn image(&self, magic: [u8; 8]) -> [u8; SLOT_LEN] {
        let mut out = [0u8; SLOT_LEN];
        let body = magic
            .into_iter()
            .chain(self.format_version.to_le_bytes())
            .chain(self.record_stride.to_le_bytes())
            .chain(self.flags.to_le_bytes())
            .chain(self.generation.to_le_bytes())
            .chain(self.n_valid.to_le_bytes())
            .chain(self.first_ts_micros.to_le_bytes())
            .chain(self.last_ts_micros.to_le_bytes())
            .chain(self.symbol_id.to_le_bytes())
            .chain(self.timeframe_secs.to_le_bytes());
        for (dst, src) in out.iter_mut().zip(body) {
            *dst = src;
        }
        let crc = crc32c(covered(&out));
        for (dst, src) in out.iter_mut().skip(OFF_CRC).zip(crc.to_le_bytes()) {
            *dst = src;
        }
        out
    }
}

/// The best-positioned decoded slot whose generation is below `below`.
///
/// `below` is exclusive and `None` means no bound, so the caller can ask again
/// for the next-newest commit after the newest one failed its check. Because
/// slot position is `generation % slot_count`, two positionally-valid slots
/// can never share a generation, and the bound therefore strictly shrinks the
/// candidate set on every call.
///
/// The error it reports when there is no candidate is the most *specific*
/// refusal any slot produced. A slot that is merely blank fails the magic
/// check, which says nothing; a slot naming a retired version, a wrong stride
/// or a failed checksum says everything, and reporting the blank one instead
/// would diagnose an intact file from another version as a destroyed header.
fn best_candidate(region: &[u8], below: Option<u64>) -> Result<(Header, Layout), FormatError> {
    let mut fault: Option<FormatError> = None;
    let mut best: Option<(Header, Layout)> = None;

    for (index, chunk) in (0u64..).zip(region.chunks(SLOT_STRIDE_LEN).take(MAX_SLOTS)) {
        match Header::decode_parts(chunk) {
            Err(refusal) => {
                if is_specific(refusal) {
                    fault.get_or_insert(refusal);
                }
            }
            Ok((header, layout)) => {
                let expected = header.generation % layout.slot_count();
                if index == expected {
                    if below.is_none_or(|limit| header.generation < limit)
                        && best.is_none_or(|(seen, _)| header.generation > seen.generation)
                    {
                        best = Some((header, layout));
                    }
                } else {
                    fault.get_or_insert(FormatError::SlotPositionMismatch {
                        expected,
                        found: index,
                    });
                }
            }
        }
    }

    best.ok_or(fault.unwrap_or(FormatError::NoValidHeader))
}

/// Whether a refusal says something about the file, or only that a slot is not
/// a header at all.
const fn is_specific(refusal: FormatError) -> bool {
    !matches!(
        refusal,
        FormatError::NotABarFile | FormatError::SlotTooShort { .. }
    )
}

/// How many whole slot positions the region holds, capped at the family bound.
///
/// Counted by folding rather than `count()` so the answer is a `u64` without a
/// `usize` cast this workspace would deny, and bounded by
/// [`crate::format::MAX_SLOTS`] so the fold cannot overflow.
fn whole_slots(region: &[u8]) -> u64 {
    region
        .chunks_exact(SLOT_STRIDE_LEN)
        .take(MAX_SLOTS)
        .fold(0u64, |seen, _| seen + 1)
}

/// Every byte of a slot except the four the checksum itself occupies.
///
/// Covering the reserved tail as well as the fields means a flipped bit
/// *anywhere* in the 64 bytes is detected — there is no window a corruption
/// can land in and be called clean. `store::fault::a_single_bit_flip_in_any_
/// header_byte_is_detected` walks all 512 of them.
fn covered(slot: &[u8]) -> impl Iterator<Item = u8> + '_ {
    slot.iter()
        .take(OFF_CRC)
        .chain(slot.iter().skip(OFF_RESERVED).take(SLOT_LEN - OFF_RESERVED))
        .copied()
}

/// Reads `N` little-endian bytes at `offset`.
///
/// Index-free by construction — this workspace denies slice indexing, and a
/// header decoder is exactly the place a panicking index would eventually be
/// reached by a corrupt file. Callers check `slot.len() >= SLOT_LEN` first, so
/// no read is ever short.
fn le_bytes<const N: usize>(slot: &[u8], offset: usize) -> [u8; N] {
    let mut out = [0u8; N];
    for (dst, src) in out.iter_mut().zip(slot.iter().skip(offset)) {
        *dst = *src;
    }
    out
}
