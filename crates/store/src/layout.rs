//! One version's geometry, and the dispatch that selects it.
//!
//! # The stride is not a global
//!
//! `docs/02-store-format.md` says "a format change mints a **new version** and
//! old files stay readable at their own stride". That sentence was prose: the
//! read path used one stride constant and dispatched on nothing, so a version
//! it did not know would either have been read at the current stride or not at
//! all. Both are wrong. Reading a version-2 file at version 1's stride returns
//! fields lifted from the wrong offsets — plausible integers, silently wrong,
//! which is the exact failure `docs/00-charter.md` prohibition 6 exists to
//! forbid.
//!
//! Every geometric question is therefore a method on a [`Layout`], and the
//! only way to obtain a `Layout` is [`Layout::for_version`], which refuses an
//! unknown version **by number** and a retired one by name.
//!
//! # Adding version 3
//!
//! Add a row to [`Layout::KNOWN`], **built by [`Layout::declared`]**. That is
//! the whole change, and it is the reason `declared` exists: a hand-written
//! struct literal would bypass every guard [`Layout::declare`] applies, and
//! `slot_count >= 2` is not a style rule — it is the entire crash-atomicity
//! guarantee. `declared` is a `const fn` whose assertions are compile errors,
//! so a degenerate row cannot be written down at all.
//!
//! Resolution is a search of that table for a matching `version`, so a new row
//! cannot alter what an existing row resolves to — proven by
//! `store::unit::a_known_version_keeps_its_stride_after_a_new_one_exists`,
//! which runs the production resolver against a table that already contains a
//! second version.
//!
//! # A record never straddles a checksum block
//!
//! `docs/02-store-format.md` §6 checksums a block at a time, and the earlier
//! byte-addressed 4096-byte block did not divide the 56-byte stride: 4096/56
//! is 73.14, so a record could begin in one block and end in the next. Record
//! 145 was the cited case — 8 of its bytes in one block, 48 in the next — and
//! verifying it against either block alone checks part of a record and calls
//! the whole of it good.
//!
//! The block is therefore counted in **records**, not bytes:
//! [`Layout::records_per_block`] whole records, so [`Layout::block_len`] is a
//! whole multiple of the stride and a straddle is arithmetically impossible.
//! `store::geometry::no_record_straddles_a_block` walks the first 5,000
//! indices and proves it, record 145 included.
//!
//! ## Why not a padded 4096-byte block
//!
//! `docs/05-decisions.md` D-0005 said "one checksum per 4 KiB block", and
//! `docs/02-store-format.md` §6 described it as "73 whole records with 8 bytes
//! of slack" — a *padded* block, which also never straddles. It was not taken,
//! and the reason is arithmetic rather than taste: padding the block means
//! block *b* starts at `header_len + b·4096` while record `73b` starts at
//! `header_len + 73b·56 = header_len + b·4088`. The two only agree if the
//! record array carries the same 8 bytes of padding every 73 records, and then
//! the address of record *i* stops being `header_len + i·56` and becomes
//! `header_len + (i/73)·4096 + (i%73)·56` — a division and two multiplies
//! instead of a multiply and an add. `CLAUDE.md` §3 rule 4 asks for a constant
//! per-operation cost, and this is still constant, but it is a strictly worse
//! constant bought with nothing: the 8 bytes are wasted either way.
//! `store::geometry::a_padded_four_kib_block_would_cost_a_division` is that
//! comparison as a test.

use crate::format::{
    BLOCK_LEN, FORMAT_VERSION, FormatError, HEADER_LEN, MAGIC, MAGIC_FAMILY, MAX_SLOT_COUNT,
    RECORD_STRIDE, RECORDS_PER_BLOCK, RETIRED_VERSIONS, SLOT_COUNT, SLOT_STRIDE,
};

/// Whether `version` is one this build knows of and refuses to decode.
///
/// Destructures [`RETIRED_VERSIONS`] rather than scanning it, so retiring a
/// second version is a compile error here instead of a silently unchecked
/// number.
const fn version_is_retired(version: u16) -> bool {
    let [first] = RETIRED_VERSIONS;
    version == first
}

/// The geometry of one format version.
///
/// Fields are private: a `Layout` with a zero `records_per_block` would make
/// [`Layout::block_of`] a division fault, and a `Layout` whose magic did not
/// match its version would defeat the cross-check in
/// [`crate::header::Header::decode`]. Both are unrepresentable outside this
/// module — and inside it, [`Layout::declared`] makes them compile errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Layout {
    version: u16,
    magic: [u8; 8],
    slot_count: u64,
    record_stride: u64,
    records_per_block: u64,
}

impl Layout {
    /// Version 2 — the format `docs/02-store-format.md` describes.
    ///
    /// Built through [`Layout::declared`], not written as a literal, so it is
    /// checked by the same rule every other row is.
    pub const V2: Self = Self::declared(
        FORMAT_VERSION_2,
        MAGIC,
        SLOT_COUNT,
        RECORD_STRIDE,
        RECORDS_PER_BLOCK,
    );

    /// Every version this build can read, in ascending order.
    ///
    /// Adding a version is adding a row built by [`Layout::declared`]. Nothing
    /// else in this module names a version number, so a new row cannot change
    /// how an old one resolves.
    pub const KNOWN: &'static [Self] = &[Self::V2];

    /// The version this build writes. Older versions are read, never written.
    pub const CURRENT: Self = Self::V2;

    /// Declares a format version's geometry at compile time.
    ///
    /// This is how a row of [`Layout::KNOWN`] is written. It applies exactly
    /// the rule [`Layout::declare`] applies, but as a `const` assertion, so a
    /// degenerate row does not compile rather than failing at a call site that
    /// may never run.
    ///
    /// # Panics
    ///
    /// At **compile time**, when the geometry is one no file could have. The
    /// message does not name the field, because a `const` panic cannot format
    /// one; [`Layout::declare`] does, and
    /// `store::unit::a_degenerate_layout_is_refused_at_declaration` walks every
    /// field through it.
    #[must_use]
    pub const fn declared(
        version: u16,
        magic: [u8; 8],
        slot_count: u64,
        record_stride: u64,
        records_per_block: u64,
    ) -> Self {
        assert!(
            degenerate_field(version, magic, slot_count, record_stride, records_per_block)
                .is_none(),
            "a Layout row is not a geometry a file can have; see Layout::declare",
        );
        Self {
            version,
            magic,
            slot_count,
            record_stride,
            records_per_block,
        }
    }

    /// Declares a format version's geometry, refusing a degenerate one.
    ///
    /// The fallible twin of [`Layout::declared`], for a geometry that is not a
    /// compile-time constant —
    /// `store::unit::a_known_version_keeps_its_stride_after_a_new_one_exists`
    /// builds its hypothetical next version through this door, so that test
    /// drives the same [`Layout::resolve`] the read path drives rather than a
    /// paraphrase of it.
    ///
    /// # Errors
    ///
    /// [`FormatError::DegenerateLayout`] naming the field, for a retired or
    /// zero version, a zero stride, a zero records-per-block (which would make
    /// [`Layout::block_of`] a division fault), a slot count below 2 or above
    /// [`crate::format::MAX_SLOT_COUNT`], a block length that overflows `u64`,
    /// or a magic outside [`crate::format::MAGIC_FAMILY`].
    pub const fn declare(
        version: u16,
        magic: [u8; 8],
        slot_count: u64,
        record_stride: u64,
        records_per_block: u64,
    ) -> Result<Self, FormatError> {
        match degenerate_field(version, magic, slot_count, record_stride, records_per_block) {
            Some(field) => Err(FormatError::DegenerateLayout { field }),
            None => Ok(Self {
                version,
                magic,
                slot_count,
                record_stride,
                records_per_block,
            }),
        }
    }

    /// The layout for `version`, or a refusal naming the version.
    ///
    /// # Errors
    ///
    /// [`FormatError::RetiredVersion`] for a version this build knows of and
    /// cannot decode, or [`FormatError::UnknownVersion`] carrying the version
    /// it saw. There is no fallback to [`Layout::CURRENT`]: a version this
    /// build does not know has a layout this build cannot infer, and inferring
    /// it wrongly returns plausible nonsense rather than an error.
    ///
    /// # Examples
    ///
    /// ```
    /// # use store::{format::FormatError, layout::Layout};
    /// assert_eq!(Layout::for_version(2)?.record_stride(), 56);
    /// assert_eq!(Layout::for_version(9), Err(FormatError::UnknownVersion(9)));
    /// // Version 1 existed and is not decodable by this build. It is named as
    /// // retired rather than reported as unknown or, worse, read at version
    /// // 2's offsets.
    /// assert_eq!(Layout::for_version(1), Err(FormatError::RetiredVersion(1)));
    /// # Ok::<(), FormatError>(())
    /// ```
    pub fn for_version(version: u16) -> Result<Self, FormatError> {
        Self::resolve(Self::KNOWN, version)
    }

    /// Resolves `version` against an explicit table.
    ///
    /// [`Layout::for_version`] is this function applied to [`Layout::KNOWN`].
    /// It is public so a test can prove the additive property against a table
    /// that already holds a second version, running the same code the read
    /// path runs rather than a paraphrase of it.
    ///
    /// # Errors
    ///
    /// [`FormatError::RetiredVersion`] or [`FormatError::UnknownVersion`]. The
    /// retirement is checked before the table, so a caller cannot resurrect a
    /// retired version by putting a row in its own table.
    pub fn resolve(table: &[Self], version: u16) -> Result<Self, FormatError> {
        if version_is_retired(version) {
            return Err(FormatError::RetiredVersion(version));
        }
        table
            .iter()
            .copied()
            .find(|layout| layout.version == version)
            .ok_or(FormatError::UnknownVersion(version))
    }

    /// This layout's version number.
    #[must_use]
    pub const fn version(self) -> u16 {
        self.version
    }

    /// The eight magic bytes a file of this version begins with.
    #[must_use]
    pub const fn magic(self) -> [u8; 8] {
        self.magic
    }

    /// How many header slots this version keeps.
    #[must_use]
    pub const fn slot_count(self) -> u64 {
        self.slot_count
    }

    /// Bytes from one header slot to the next.
    ///
    /// A family constant, not a per-version one — see
    /// [`crate::format::SLOT_STRIDE`] for why it is a whole failure unit.
    #[must_use]
    pub const fn slot_stride(self) -> u64 {
        SLOT_STRIDE
    }

    /// Bytes of header region before record 0.
    ///
    /// Cannot overflow: [`Layout::declare`] caps `slot_count` at
    /// [`crate::format::MAX_SLOT_COUNT`].
    #[must_use]
    pub const fn header_len(self) -> u64 {
        self.slot_count * SLOT_STRIDE
    }

    /// Bytes per record.
    #[must_use]
    pub const fn record_stride(self) -> u64 {
        self.record_stride
    }

    /// Whole records covered by one checksum block.
    #[must_use]
    pub const fn records_per_block(self) -> u64 {
        self.records_per_block
    }

    /// Bytes per checksum block — always a whole multiple of the stride.
    ///
    /// This is the block's **nominal** length. The last block of a file holds
    /// only as many records as the commit counter covers; see
    /// [`Layout::covered_byte_range`], which is the range a checksum is
    /// actually taken over.
    #[must_use]
    pub const fn block_len(self) -> u64 {
        self.record_stride * self.records_per_block
    }

    /// The byte offset of record `index`.
    ///
    /// This is the whole performance claim in one line: an add, a multiply, a
    /// load, independent of file size.
    ///
    /// # Errors
    ///
    /// [`FormatError::OffsetOverflow`] if the offset would exceed `u64`. A
    /// wrapped offset would address a different, valid-looking record.
    ///
    /// # Examples
    ///
    /// ```
    /// # use store::{format::FormatError, layout::Layout};
    /// let v2 = Layout::V2;
    /// assert_eq!(v2.offset_of(0)?, 32_768);
    /// assert_eq!(v2.offset_of(1)?, 32_824);
    /// assert_eq!(v2.offset_of(400_000)?, 22_432_768);
    /// # Ok::<(), FormatError>(())
    /// ```
    pub const fn offset_of(self, index: u64) -> Result<u64, FormatError> {
        match index.checked_mul(self.record_stride) {
            Some(scaled) => match scaled.checked_add(self.header_len()) {
                Some(offset) => Ok(offset),
                None => Err(FormatError::OffsetOverflow),
            },
            None => Err(FormatError::OffsetOverflow),
        }
    }

    /// The half-open byte range `[start, end)` that record `index` occupies.
    ///
    /// # Errors
    ///
    /// [`FormatError::OffsetOverflow`] if either end would exceed `u64`.
    pub const fn record_byte_range(self, index: u64) -> Result<(u64, u64), FormatError> {
        match self.offset_of(index) {
            Ok(start) => match start.checked_add(self.record_stride) {
                Some(end) => Ok((start, end)),
                None => Err(FormatError::OffsetOverflow),
            },
            Err(e) => Err(e),
        }
    }

    /// Which checksum block holds record `index`. Every byte of it.
    ///
    /// Infallible and exact: the block is counted in records, so this is one
    /// division and the answer is a single block, never a range. A caller can
    /// therefore verify a record by reading one checksum — the property that
    /// makes `docs/02-store-format.md` §6's "verification is O(1) per read"
    /// true rather than approximately true.
    #[must_use]
    pub const fn block_of(self, index: u64) -> u64 {
        index / self.records_per_block
    }

    /// How many blocks a commit counter of `n_valid` covers.
    ///
    /// The last of them is partial unless `n_valid` is a whole multiple of
    /// [`Layout::records_per_block`].
    #[must_use]
    pub const fn blocks_for(self, n_valid: u64) -> u64 {
        let whole = n_valid / self.records_per_block;
        if n_valid.is_multiple_of(self.records_per_block) {
            whole
        } else {
            whole + 1
        }
    }

    /// How many committed records live in `block`.
    ///
    /// [`Layout::records_per_block`] for every block but the last, and the
    /// remainder for the last.
    ///
    /// # Errors
    ///
    /// [`FormatError::BlockNotCommitted`] for a block past the counter.
    /// Answering "zero" instead would let a caller checksum nothing and
    /// report the result as a verified block.
    pub const fn records_in_block(self, block: u64, n_valid: u64) -> Result<u64, FormatError> {
        let blocks = self.blocks_for(n_valid);
        if block >= blocks {
            return Err(FormatError::BlockNotCommitted { block, blocks });
        }
        // `block < blocks` and `blocks <= n_valid.div_ceil(rpb)`, so the
        // product is at most `n_valid` rounded down and cannot overflow.
        let before = block * self.records_per_block;
        let rest = n_valid - before;
        if rest > self.records_per_block {
            Ok(self.records_per_block)
        } else {
            Ok(rest)
        }
    }

    /// The half-open byte range `[start, end)` a checksum for `block` covers.
    ///
    /// **This, not [`Layout::block_byte_range`], is the checksum domain.** The
    /// tail block of a file holds only the records the commit counter covers,
    /// so its nominal range ends past EOF for every file whose record count is
    /// not a whole multiple of [`Layout::records_per_block`] — 72 of every 73
    /// states a file passes through.
    ///
    /// Because the domain is a function of `n_valid`, the same block's
    /// checksum means something different before and after an append that
    /// lands in it: the writer recomputes the tail block's checksum on every
    /// commit that touches it, and a reader verifies against the `n_valid` it
    /// read from the header. Both sides derive the length from the same
    /// counter, which is what makes the answer reproducible rather than
    /// timing-dependent.
    ///
    /// # Errors
    ///
    /// [`FormatError::BlockNotCommitted`] for a block past the counter, or
    /// [`FormatError::OffsetOverflow`] if either end would exceed `u64`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use store::{format::FormatError, layout::Layout};
    /// let v2 = Layout::V2;
    /// // One record committed: the tail block covers 56 bytes, not 4088.
    /// assert_eq!(v2.covered_byte_range(0, 1)?, (32_768, 32_824));
    /// // A whole block: nominal and covered agree.
    /// assert_eq!(v2.covered_byte_range(0, 73)?, v2.block_byte_range(0)?);
    /// # Ok::<(), FormatError>(())
    /// ```
    pub const fn covered_byte_range(
        self,
        block: u64,
        n_valid: u64,
    ) -> Result<(u64, u64), FormatError> {
        let records = match self.records_in_block(block, n_valid) {
            Ok(records) => records,
            Err(e) => return Err(e),
        };
        let start = match block.checked_mul(self.block_len()) {
            Some(scaled) => match scaled.checked_add(self.header_len()) {
                Some(start) => start,
                None => return Err(FormatError::OffsetOverflow),
            },
            None => return Err(FormatError::OffsetOverflow),
        };
        // `records <= records_per_block`, so this product is at most
        // `block_len`, which `declare` already proved fits. Written without a
        // `checked_mul` on purpose: the failure arm would be unreachable, and
        // an arm no input can reach is an arm no test can close.
        let len = records * self.record_stride;
        match start.checked_add(len) {
            Some(end) => Ok((start, end)),
            None => Err(FormatError::OffsetOverflow),
        }
    }

    /// The half-open byte range `[start, end)` block `block` nominally covers.
    ///
    /// Nominal means *if the block were full*. For the tail block of a real
    /// file this names bytes that are not in the file; use
    /// [`Layout::covered_byte_range`] to checksum, and this only to reason
    /// about the geometry.
    ///
    /// # Errors
    ///
    /// [`FormatError::OffsetOverflow`] if either end would exceed `u64`.
    pub const fn block_byte_range(self, block: u64) -> Result<(u64, u64), FormatError> {
        let len = self.block_len();
        match block.checked_mul(len) {
            Some(scaled) => match scaled.checked_add(self.header_len()) {
                Some(start) => match start.checked_add(len) {
                    Some(end) => Ok((start, end)),
                    None => Err(FormatError::OffsetOverflow),
                },
                None => Err(FormatError::OffsetOverflow),
            },
            None => Err(FormatError::OffsetOverflow),
        }
    }

    /// The byte offset of the slot a commit at `generation` is written to.
    ///
    /// Writes alternate, so the slot holding the previous commit is never the
    /// slot being written — which is the whole of the crash argument in
    /// [`crate::header`]. The spacing is [`crate::format::SLOT_STRIDE`], so
    /// the slot being written and the slot holding the previous commit are in
    /// different device blocks and different pages.
    #[must_use]
    pub const fn slot_offset(self, generation: u64) -> u64 {
        (generation % self.slot_count) * SLOT_STRIDE
    }

    /// How many whole records the file can hold, ignoring the counter.
    ///
    /// Saturating rather than branching on `file_len < header_len`: a file
    /// with no room for a header holds no records, which is the same answer
    /// the branch gave, and a branch whose two arms agree at the boundary is
    /// a branch no test can distinguish. `ragged_tail_bytes` is where a file
    /// too short to hold a header is reported, and it does branch.
    #[must_use]
    pub const fn capacity_for(self, file_len: u64) -> u64 {
        file_len.saturating_sub(self.header_len()) / self.record_stride
    }

    /// Bytes past the last whole record.
    ///
    /// A non-zero remainder means the file was interrupted mid-record.
    /// `docs/02-store-format.md` §7 truncates to the last whole record and
    /// logs the discarded byte count — loudly, never by ignoring it.
    #[must_use]
    pub const fn ragged_tail_bytes(self, file_len: u64) -> u64 {
        if file_len < self.header_len() {
            file_len
        } else {
            (file_len - self.header_len()) % self.record_stride
        }
    }
}

/// Version 2's number, named so [`Layout::V2`] does not import a constant that
/// reads as "whatever version this build writes".
///
/// If version 3 is ever minted, [`crate::format::FORMAT_VERSION`] becomes 3
/// and `Layout::V2` must keep saying 2. A row that read the moving constant
/// would silently renumber an existing geometry — `CLAUDE.md` §3 rule 8.
const FORMAT_VERSION_2: u16 = 2;

const _: () = assert!(FORMAT_VERSION_2 == 2);
const _: () = assert!(Layout::CURRENT.version() == FORMAT_VERSION);

/// The first field of a declaration that is not a geometry a file can have.
///
/// One function so [`Layout::declared`] and [`Layout::declare`] cannot drift:
/// a guard added here is a compile error for the const rows and an `Err` for
/// the fallible door, in the same commit.
const fn degenerate_field(
    version: u16,
    magic: [u8; 8],
    slot_count: u64,
    record_stride: u64,
    records_per_block: u64,
) -> Option<&'static str> {
    if version == 0 || version_is_retired(version) {
        return Some("version");
    }
    if record_stride == 0 {
        return Some("record_stride");
    }
    if records_per_block == 0 {
        return Some("records_per_block");
    }
    if record_stride.checked_mul(records_per_block).is_none() {
        return Some("block_len");
    }
    if !magic_is_family(magic) {
        return Some("magic");
    }
    // Two slots is the minimum that lets a commit be written without
    // overwriting the only surviving copy of the previous one, and
    // MAX_SLOT_COUNT is the most a reader will look at before it knows the
    // version -- a row above it would put a slot where nothing looks.
    if slot_count < 2 || slot_count > MAX_SLOT_COUNT {
        return Some("slot_count");
    }
    None
}

/// Whether `magic`'s first seven bytes are [`MAGIC_FAMILY`].
///
/// Destructured rather than iterated because this runs in `const` context,
/// where an iterator is not available and an index would be a denied lint.
const fn magic_is_family(magic: [u8; 8]) -> bool {
    let [m0, m1, m2, m3, m4, m5, m6, _] = magic;
    let [f0, f1, f2, f3, f4, f5, f6] = MAGIC_FAMILY;
    m0 == f0 && m1 == f1 && m2 == f2 && m3 == f3 && m4 == f4 && m5 == f5 && m6 == f6
}

const _: () = assert!(Layout::V2.block_len() == BLOCK_LEN);
const _: () = assert!(Layout::V2.header_len() == HEADER_LEN);
const _: () = assert!(
    Layout::V2
        .block_len()
        .is_multiple_of(Layout::V2.record_stride())
);
const _: () = assert!(Layout::V2.records_per_block() > 0);
const _: () = assert!(Layout::V2.slot_count() > 1);
const _: () = assert!(Layout::V2.slot_count() <= MAX_SLOT_COUNT);
const _: () = assert!(Layout::V2.slot_offset(0) == 0);
const _: () = assert!(Layout::V2.slot_offset(1) == SLOT_STRIDE);
