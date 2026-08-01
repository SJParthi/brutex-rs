//! CRC-32C (Castagnoli) — the one checksum this store computes.
//!
//! # Why the polynomial lives here rather than in a dependency
//!
//! The whole function is four lines of arithmetic and it is *checkable*: every
//! published description of this polynomial carries the same check value for
//! the input `b"123456789"`, and [`CHECK_VALUE`] is asserted against this
//! implementation by `store::unit::the_crc_matches_the_published_check_value`.
//! A dependency would have to be taken on faith unless the same check ran
//! anyway.
//!
//! # Why CRC at all
//!
//! `docs/05-decisions.md` D-0005. A flipped bit in a raw `i64` price yields a
//! *different, plausible* price. There is no parse to fail and no structure to
//! violate, so without a checksum the corruption is silent, permanent, and
//! inherited by every result derived from it.

/// The reflected CRC-32C generator polynomial: `0x1EDC_6F41`, bit-reversed.
///
/// Reflected form is used because the loop shifts right, which is what makes
/// it a shift and a mask rather than a bit-order fixup per byte.
const POLYNOMIAL: u32 = 0x82F6_3B78;

/// CRC-32C of `b"123456789"`.
///
/// This is the check value that identifies the Castagnoli polynomial. It is a
/// published constant, not a value read out of this implementation — the test
/// that compares them therefore verifies the code, not the constant.
pub const CHECK_VALUE: u32 = 0xE306_9283;

/// CRC-32C over a byte stream.
///
/// Takes an iterator rather than a slice so a caller can check a
/// *discontiguous* range without copying — the header slot checksums every
/// byte of its slot except the four bytes holding the checksum itself, which
/// is two slices and one allocation if this took `&[u8]`.
///
/// # Examples
///
/// ```
/// # use store::crc::{crc32c, CHECK_VALUE};
/// assert_eq!(crc32c(*b"123456789"), CHECK_VALUE);
/// ```
#[must_use]
pub fn crc32c<I>(bytes: I) -> u32
where
    I: IntoIterator<Item = u8>,
{
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8u8 {
            // `0 - (crc & 1)` is all-ones when the low bit is set and all-zeros
            // when it is not, so the polynomial is applied without a branch.
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (POLYNOMIAL & mask);
        }
    }
    !crc
}
