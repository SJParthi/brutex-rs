//! The per-block checksum: what turns a lost write into a refusal.
//!
//! # Why this module exists
//!
//! [`crate::format::FLAG_CHECKSUMS`] was declared and never implemented, which
//! left a real hole rather than a missing feature. The header commit's whole
//! crash argument assumes the appended records reach stable storage before the
//! slot write. When that assumption is broken — and it is the *likely* break,
//! because page 0 is re-dirtied on every commit and is therefore the hottest
//! writeback candidate in the file — the header names records whose bytes are
//! zeros from a newly allocated extent.
//!
//! Nothing in the record can detect that. An all-zero [`crate::format::Bar`]
//! satisfies `ohlc_is_sane`, and its `open_interest` of zero is a **real**
//! zero rather than [`crate::format::OI_NULL`], so a spot-index file quietly
//! acquires derivative-shaped bars that go on to enter a sweep. A checksum
//! taken when the records were written is the only thing that can say those
//! bytes are not the bytes that were committed.
//!
//! # The domain is the committed prefix, not the nominal block
//!
//! [`Layout::covered_byte_range`] is the range, and both sides derive it from
//! the same `n_valid`. The tail block of a file holds only the records the
//! counter covers, so checksumming its nominal 4088 bytes would read past EOF
//! for 72 of every 73 states a file passes through. A writer recomputes the
//! tail block's checksum on every commit that lands in it; a reader verifies
//! against the `n_valid` it read from the header. Same counter, same length,
//! same answer — `CLAUDE.md` §3 rule 5.
//!
//! # No I/O
//!
//! This module takes bytes and returns numbers. The sidecar file it feeds is
//! [`crate::path::FileKind::Checksums`]; reading and writing it belongs to a
//! writer that does not exist yet.

use crate::crc::crc32c;
use crate::format::FormatError;
use crate::header::Header;
use crate::layout::Layout;

/// The checksum a writer stores for `block`, given that block's bytes.
///
/// `bytes` must be exactly the range [`Layout::covered_byte_range`] names for
/// this block under this counter — offering more or fewer is refused rather
/// than trimmed, because a checksum over the wrong length is a number that
/// will never match again and nobody will know why.
///
/// # Errors
///
/// [`FormatError::BlockNotCommitted`] for a block past the counter,
/// [`FormatError::BlockLengthMismatch`] when `bytes` is not the covered
/// length, or [`FormatError::OffsetOverflow`] from the geometry.
///
/// # Examples
///
/// ```
/// # use store::{block, format::FormatError, layout::Layout};
/// let v2 = Layout::V2;
/// // One record committed: the tail block covers exactly that record.
/// let record = [7u8; 56];
/// let sealed = block::seal(v2, 1, 0, &record)?;
/// assert_eq!(block::seal(v2, 1, 0, &record)?, sealed, "idempotent");
/// // The same bytes at a length the geometry does not give the block.
/// assert!(block::seal(v2, 1, 0, &[7u8; 55]).is_err());
/// # Ok::<(), FormatError>(())
/// ```
pub fn seal(layout: Layout, n_valid: u64, block: u64, bytes: &[u8]) -> Result<u32, FormatError> {
    let need = covered_len(layout, n_valid, block)?;
    let len = byte_count(bytes);
    if len != need {
        return Err(FormatError::BlockLengthMismatch {
            block,
            len: bytes.len(),
            need,
        });
    }
    Ok(crc32c(bytes.iter().copied()))
}

/// Verifies one block's committed bytes against its stored checksum.
///
/// # Errors
///
/// [`FormatError::ChecksumsAbsent`] when the header does not declare
/// checksums — "verified" and "there was nothing to verify against" are
/// different answers and this refuses to conflate them.
/// [`FormatError::BlockChecksum`] naming the block and both numbers when the
/// bytes are not the bytes that were sealed. Plus anything [`seal`] refuses.
///
/// # Examples
///
/// ```
/// # use store::{block, format::{FLAG_CHECKSUMS, FormatError}, header::Header, layout::Layout};
/// let v2 = Layout::V2;
/// let header = Header::genesis(1, 60, FLAG_CHECKSUMS).advance(1, 100, 100)?;
/// let record = [7u8; 56];
/// let sealed = block::seal(v2, header.n_valid, 0, &record)?;
/// assert_eq!(block::verify(&header, v2, 0, &record, sealed), Ok(()));
///
/// // The write was lost and the extent reads as zeros. The bar is "sane"
/// // and its open interest is a real zero; only the checksum knows.
/// assert!(block::verify(&header, v2, 0, &[0u8; 56], sealed).is_err());
/// # Ok::<(), FormatError>(())
/// ```
pub fn verify(
    header: &Header,
    layout: Layout,
    block: u64,
    bytes: &[u8],
    stored: u32,
) -> Result<(), FormatError> {
    if !header.checksums_present() {
        return Err(FormatError::ChecksumsAbsent);
    }
    let computed = seal(layout, header.n_valid, block, bytes)?;
    if computed == stored {
        Ok(())
    } else {
        Err(FormatError::BlockChecksum {
            block,
            stored,
            computed,
        })
    }
}

/// How many bytes `block` covers under `n_valid`.
fn covered_len(layout: Layout, n_valid: u64, block: u64) -> Result<u64, FormatError> {
    let (start, end) = layout.covered_byte_range(block, n_valid)?;
    Ok(end - start)
}

/// The length of `bytes` as a `u64`, without a cast this workspace denies.
///
/// Counted rather than converted: `usize`→`u64` is a widening cast on every
/// target this builds for, so a checked conversion would carry a failure arm
/// no test could ever reach, and coverage would never close.
fn byte_count(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |seen, _| seen.saturating_add(1))
}
