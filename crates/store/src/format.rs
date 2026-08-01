//! Bytes on disk. `docs/02-store-format.md` is the authority; this follows it.
//!
//! ```text
//! byte 0                    64                                        EOF
//!   ├──────── header ────────┼─── record 0 ─┼─── record 1 ─┼── … ──────┤
//!            64 bytes            56 bytes       56 bytes
//! ```
//!
//! **Address of record *i*: `base + 64 + i·56`.** An add, a multiply, a load.
//! No search, no decode, no allocation — which is the entire reason the format
//! exists and the only thing that makes bar lookup O(1).

use brutex_core::price::Paisa;

/// Bytes of header before the first record.
pub const HEADER_LEN: u64 = 64;

/// Bytes per record. Read it from the header; never assume it.
pub const RECORD_STRIDE: u64 = 56;

/// Identifies a version-1 bar file.
pub const MAGIC: [u8; 8] = *b"BRUTEXB1";

/// The only format version this code writes.
pub const FORMAT_VERSION: u16 = 1;

/// Bytes per checksummed block.
pub const BLOCK_LEN: u64 = 4096;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
    /// The byte offset of record `index`.
    ///
    /// This is the whole performance claim in one line: an add, a multiply, a
    /// load, independent of file size. Gate 8's `read_ratio` measures it at
    /// 1×, 10× and 100×.
    ///
    /// # Errors
    ///
    /// [`FormatError::OffsetOverflow`] if the offset would exceed `u64`. A
    /// wrapped offset would address a different, valid-looking record.
    ///
    /// # Examples
    ///
    /// ```
    /// # use store::format::Bar;
    /// assert_eq!(Bar::offset_of(0)?, 64);
    /// assert_eq!(Bar::offset_of(1)?, 120);
    /// assert_eq!(Bar::offset_of(400_000)?, 22_400_064);
    /// # Ok::<(), store::format::FormatError>(())
    /// ```
    pub const fn offset_of(index: u64) -> Result<u64, FormatError> {
        match index.checked_mul(RECORD_STRIDE) {
            Some(scaled) => match scaled.checked_add(HEADER_LEN) {
                Some(off) => Ok(off),
                None => Err(FormatError::OffsetOverflow),
            },
            None => Err(FormatError::OffsetOverflow),
        }
    }

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
}

/// Something about the bytes is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FormatError {
    /// A record offset would not fit in `u64`.
    OffsetOverflow,
    /// The first eight bytes are not [`MAGIC`].
    NotABarFile,
    /// The header names a version this build does not know.
    ///
    /// Refused rather than guessed: `docs/02-store-format.md` mints a new
    /// version for every layout change precisely so old files stay readable at
    /// their own stride, and guessing defeats that.
    UnknownVersion(u16),
    /// The header names a stride other than the one its version defines.
    StrideMismatch(u16),
    /// The file is shorter than a header.
    TooShortForHeader,
    /// `n_valid` claims more records than the file can hold.
    ///
    /// Trusting the counter over the file length would read past the end and
    /// return whatever bytes happened to be there.
    CounterExceedsFile,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OffsetOverflow => f.write_str("record offset overflows u64"),
            Self::NotABarFile => f.write_str("magic is not BRUTEXB1"),
            Self::UnknownVersion(v) => write!(f, "unknown format version {v}"),
            Self::StrideMismatch(s) => write!(f, "record stride {s} is not the version's stride"),
            Self::TooShortForHeader => f.write_str("file is shorter than its header"),
            Self::CounterExceedsFile => {
                f.write_str("n_valid claims more records than the file holds")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// The 64-byte header.
///
/// `n_valid` is the commit counter and is published **last** — see
/// `docs/05-decisions.md` D-0004. A reader treats records `0..n_valid` as the
/// whole file, so a half-written record at the tail is not merely unlikely,
/// it is **unobservable**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Format version.
    pub format_version: u16,
    /// Bytes per record, as this file declares them.
    pub record_stride: u16,
    /// Bit 0: block checksums present.
    pub flags: u32,
    /// The commit counter: how many records are readable.
    pub n_valid: u64,
    /// Timestamp of record 0, for a cheap range reject.
    pub first_ts_micros: i64,
    /// Timestamp of record `n_valid - 1`.
    pub last_ts_micros: i64,
    /// Resolved from the path; a cross-check, never the index.
    pub symbol_id: u32,
    /// Seconds per bar. 60 for one minute, 1 for one second.
    pub timeframe_secs: u32,
}

/// Bit 0 of [`Header::flags`]: per-block checksums are present.
pub const FLAG_CHECKSUMS: u32 = 1;

impl Header {
    /// Whether this file carries per-block checksums.
    #[must_use]
    pub const fn has_checksums(&self) -> bool {
        self.flags & FLAG_CHECKSUMS != 0
    }

    /// Validates a header against a known file length.
    ///
    /// # Errors
    ///
    /// [`FormatError`] for a wrong magic, an unknown version, a stride that
    /// disagrees with its version, a file too short to hold a header, or a
    /// counter that claims more records than the file can contain.
    pub const fn validate(&self, magic: [u8; 8], file_len: u64) -> Result<(), FormatError> {
        // A const fn cannot iterate a slice comparison, so compare the words.
        let m = u64::from_le_bytes(magic);
        let want = u64::from_le_bytes(MAGIC);
        if m != want {
            return Err(FormatError::NotABarFile);
        }
        if self.format_version != FORMAT_VERSION {
            return Err(FormatError::UnknownVersion(self.format_version));
        }
        if self.record_stride as u64 != RECORD_STRIDE {
            return Err(FormatError::StrideMismatch(self.record_stride));
        }
        if file_len < HEADER_LEN {
            return Err(FormatError::TooShortForHeader);
        }
        // The counter must not claim more than the bytes can hold.
        let capacity = (file_len - HEADER_LEN) / RECORD_STRIDE;
        if self.n_valid > capacity {
            return Err(FormatError::CounterExceedsFile);
        }
        Ok(())
    }

    /// How many whole records the file can hold, ignoring the counter.
    #[must_use]
    pub const fn capacity_for(file_len: u64) -> u64 {
        if file_len < HEADER_LEN {
            0
        } else {
            (file_len - HEADER_LEN) / RECORD_STRIDE
        }
    }

    /// Bytes past the last whole record.
    ///
    /// A non-zero remainder means the file was interrupted mid-record.
    /// `docs/02-store-format.md` §7 truncates to the last whole record and
    /// logs the discarded byte count — loudly, never by ignoring it.
    #[must_use]
    pub const fn ragged_tail_bytes(file_len: u64) -> u64 {
        if file_len < HEADER_LEN {
            file_len
        } else {
            (file_len - HEADER_LEN) % RECORD_STRIDE
        }
    }

    /// Which checksum block holds record `index`.
    ///
    /// # Errors
    ///
    /// [`FormatError::OffsetOverflow`] if the record's offset overflows.
    pub const fn block_of(index: u64) -> Result<u64, FormatError> {
        match Bar::offset_of(index) {
            Ok(off) => Ok(off / BLOCK_LEN),
            Err(e) => Err(e),
        }
    }
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

    fn header(n_valid: u64) -> Header {
        Header {
            format_version: FORMAT_VERSION,
            record_stride: 56,
            flags: FLAG_CHECKSUMS,
            n_valid,
            first_ts_micros: 0,
            last_ts_micros: 0,
            symbol_id: 1,
            timeframe_secs: 60,
        }
    }

    #[test]
    fn the_record_is_exactly_the_documented_shape() {
        // S-01. These are compile-time assertions above; this test exists so
        // the invariant table has a named test beside it and so a reader sees
        // the numbers written out.
        assert_eq!(size_of::<Bar>(), 56);
        assert_eq!(align_of::<Bar>(), 8);
        assert_eq!(RECORD_STRIDE, 56);
        assert_eq!(HEADER_LEN, 64);
        assert_eq!(&MAGIC, b"BRUTEXB1");
    }

    #[test]
    fn addressing_is_arithmetic_and_flat() {
        // The O(1) claim, spelled out. Bar 400,000 costs the same as bar 0.
        assert_eq!(Bar::offset_of(0).expect("fits"), 64);
        assert_eq!(Bar::offset_of(1).expect("fits"), 120);
        assert_eq!(Bar::offset_of(2).expect("fits"), 176);
        assert_eq!(Bar::offset_of(400_000).expect("fits"), 22_400_064);
        // Every step is exactly one stride.
        for i in 0..1000u64 {
            let a = Bar::offset_of(i).expect("fits");
            let b = Bar::offset_of(i + 1).expect("fits");
            assert_eq!(b - a, RECORD_STRIDE);
        }
    }

    #[test]
    fn an_overflowing_index_is_refused_not_wrapped() {
        // A wrapped offset addresses a DIFFERENT, valid-looking record. That
        // is worse than an error, so it must never happen silently.
        assert_eq!(
            Bar::offset_of(u64::MAX),
            Err(FormatError::OffsetOverflow),
            "the multiply must be checked"
        );
        // The exact boundary: the largest index whose offset still fits.
        let last_ok = (u64::MAX - HEADER_LEN) / RECORD_STRIDE;
        assert!(Bar::offset_of(last_ok).is_ok());
        assert_eq!(
            Bar::offset_of(last_ok + 1),
            Err(FormatError::OffsetOverflow),
            "one past the boundary must fail the ADD, not wrap"
        );
    }

    #[test]
    fn open_interest_null_is_distinct_from_zero() {
        // S-08. Spot indices store the sentinel; a derivative may store a real
        // zero. Conflating them makes the two series indistinguishable on the
        // one field that tells them apart.
        let spot = Bar {
            open_interest: OI_NULL,
            ..Bar::default()
        };
        let derivative = Bar {
            open_interest: 0,
            ..Bar::default()
        };
        assert!(spot.oi_is_null());
        assert!(!derivative.oi_is_null());
        assert_eq!(spot.oi(), None);
        assert_eq!(derivative.oi(), Some(0));
        assert_ne!(spot, derivative);
        assert_eq!(OI_NULL, i64::MIN);
    }

    #[test]
    fn ohlc_sanity_accepts_real_bars_and_rejects_impossible_ones() {
        // The real first bar of 2024-06-03 from the lake, in paisa.
        let real = Bar {
            ts_micros: 1_717_386_300_000_000,
            open: 2_333_870,
            high: 2_333_870,
            low: 2_308_370,
            close: 2_310_955,
            volume: 0,
            open_interest: OI_NULL,
        };
        assert!(real.ohlc_is_sane());
        assert_eq!(real.close_price().raw(), 2_310_955);

        // A flat bar is legal: high == low == open == close.
        assert!(Bar::default().ohlc_is_sane());

        // High below another field, and low above one, are both impossible.
        assert!(!Bar { high: 1, ..real }.ohlc_is_sane());
        assert!(
            !Bar {
                low: 9_999_999,
                ..real
            }
            .ohlc_is_sane()
        );
        assert!(
            !Bar {
                open: 9_999_999,
                ..real
            }
            .ohlc_is_sane(),
            "an open above the high is impossible"
        );
        assert!(
            !Bar {
                close: 9_999_999,
                ..real
            }
            .ohlc_is_sane()
        );
        assert!(!Bar { open: -1, ..real }.ohlc_is_sane(), "below the low");
        assert!(!Bar { close: -1, ..real }.ohlc_is_sane());
    }

    #[test]
    fn a_good_header_validates() {
        // 64-byte header plus exactly 3 records.
        assert_eq!(header(3).validate(MAGIC, 64 + 3 * 56), Ok(()));
        assert!(header(3).has_checksums());
        assert!(
            !Header {
                flags: 0,
                ..header(3)
            }
            .has_checksums()
        );
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        assert_eq!(
            header(0).validate(*b"NOTBRUTE", 64),
            Err(FormatError::NotABarFile)
        );
    }

    #[test]
    fn an_unknown_version_refuses_rather_than_guessing() {
        // S-09. A future version has a different stride; guessing would read
        // fields from the wrong offsets and return plausible nonsense.
        let h = Header {
            format_version: 2,
            ..header(0)
        };
        assert_eq!(
            h.validate(MAGIC, 64),
            Err(FormatError::UnknownVersion(2)),
            "the version must be named in the error"
        );
        let h0 = Header {
            format_version: 0,
            ..header(0)
        };
        assert_eq!(h0.validate(MAGIC, 64), Err(FormatError::UnknownVersion(0)));
    }

    #[test]
    fn a_stride_that_disagrees_with_its_version_is_refused() {
        let h = Header {
            record_stride: 64,
            ..header(0)
        };
        assert_eq!(h.validate(MAGIC, 64), Err(FormatError::StrideMismatch(64)));
    }

    #[test]
    fn a_file_too_short_for_a_header_is_refused() {
        assert_eq!(
            header(0).validate(MAGIC, 63),
            Err(FormatError::TooShortForHeader)
        );
        assert_eq!(header(0).validate(MAGIC, 64), Ok(()), "exactly 64 is legal");
    }

    #[test]
    fn a_counter_claiming_more_than_the_file_holds_is_refused() {
        // Trusting the counter over the length reads past the end and returns
        // whatever bytes were there -- plausible integers, silently wrong.
        assert_eq!(
            header(4).validate(MAGIC, 64 + 3 * 56),
            Err(FormatError::CounterExceedsFile)
        );
        assert_eq!(header(3).validate(MAGIC, 64 + 3 * 56), Ok(()));
        // A ragged tail does not make a valid counter invalid.
        assert_eq!(header(3).validate(MAGIC, 64 + 3 * 56 + 17), Ok(()));
    }

    #[test]
    fn capacity_and_ragged_tail_agree_with_the_bytes() {
        assert_eq!(Header::capacity_for(0), 0);
        assert_eq!(Header::capacity_for(63), 0);
        assert_eq!(Header::capacity_for(64), 0);
        assert_eq!(Header::capacity_for(64 + 55), 0);
        assert_eq!(Header::capacity_for(64 + 56), 1);
        assert_eq!(Header::capacity_for(64 + 56 * 100), 100);

        assert_eq!(Header::ragged_tail_bytes(64), 0);
        assert_eq!(Header::ragged_tail_bytes(64 + 56), 0);
        assert_eq!(
            Header::ragged_tail_bytes(64 + 56 + 17),
            17,
            "17 bytes of a torn record"
        );
        assert_eq!(
            Header::ragged_tail_bytes(30),
            30,
            "a file shorter than a header is all tail"
        );
    }

    #[test]
    fn a_block_holds_seventy_three_whole_records() {
        // docs/02-store-format.md section 6: 4 KiB holds 73 records with 8
        // bytes of slack. If that ever stops being true the sidecar CRC layout
        // is wrong, so it is pinned here.
        assert_eq!(BLOCK_LEN / RECORD_STRIDE, 73);
        assert_eq!(BLOCK_LEN % RECORD_STRIDE, 8);

        assert_eq!(Header::block_of(0).expect("fits"), 0);
        // Record 71 starts at 64 + 71*56 = 4040, still in block 0.
        assert_eq!(Header::block_of(71).expect("fits"), 0);
        // Record 72 starts at 4096, the first byte of block 1.
        assert_eq!(Header::block_of(72).expect("fits"), 1);
        assert_eq!(Header::block_of(u64::MAX), Err(FormatError::OffsetOverflow));
    }

    fn assert_error<E: std::error::Error>(_: &E) {}

    #[test]
    fn every_format_error_renders_a_distinct_reason() {
        let all = [
            FormatError::OffsetOverflow,
            FormatError::NotABarFile,
            FormatError::UnknownVersion(7),
            FormatError::StrideMismatch(64),
            FormatError::TooShortForHeader,
            FormatError::CounterExceedsFile,
        ];
        let rendered: Vec<String> = all.iter().map(ToString::to_string).collect();
        for (i, a) in rendered.iter().enumerate() {
            assert!(!a.is_empty());
            for (j, b) in rendered.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "variants {i} and {j} render identically");
                }
            }
        }
        assert!(rendered[2].contains('7'), "the version must be visible");
        assert!(rendered[3].contains("64"), "the stride must be visible");
        assert_error(&FormatError::NotABarFile);
    }
}
