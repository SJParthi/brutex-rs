//! CRC-32C (Castagnoli) — the one checksum this store computes.
//!
//! # Why the polynomial lives here rather than in a dependency
//!
//! The whole function is arithmetic over one published constant, and it is
//! *checkable*: every published description of this polynomial carries the same
//! check value for the input `b"123456789"`, and [`CHECK_VALUE`] is asserted
//! against this implementation by
//! `store::unit::the_crc_matches_the_published_check_value`. A dependency would
//! have to be taken on faith unless the same check ran anyway.
//!
//! # Why CRC at all
//!
//! `docs/05-decisions.md` D-0005. A flipped bit in a raw `i64` price yields a
//! *different, plausible* price. There is no parse to fail and no structure to
//! violate, so without a checksum the corruption is silent, permanent, and
//! inherited by every result derived from it.
//!
//! # Why a table and not a bit loop — D-0032
//!
//! The first implementation shifted one bit at a time: eight iterations per
//! byte, no table. Measured on this machine (Apple M4 Pro, `rustc` 1.97.1,
//! release profile, min of 201 trials × 200 reps, `black_box` on both sides)
//! over a full 4,088-byte block:
//!
//! | kernel | ns / block | ns / byte |
//! |---|---|---|
//! | bit-by-bit (what this replaced) | 13,952.5 | 3.413 |
//! | 256-entry table | 6,868.3 | 1.680 |
//! | **slice-by-8, this file** | **1,487.5** | **0.364** |
//!
//! A block covers 73 records, so the per-record share falls from 191.1 ns to
//! 20.4 ns. `docs/06-limits.md` §14 records what is still *not* claimed: a CRC
//! is O(n) in the bytes it covers and always will be, and no hardware kernel is
//! used here.
//!
//! # Why this shape and not the two obvious faster ones
//!
//! * **No `core::arch` intrinsic.** Every crate in this workspace carries
//!   `#![forbid(unsafe_code)]`, which an `#[allow]` cannot re-open, so an
//!   intrinsic would need a new crate existing only to hold one `unsafe`
//!   expression. Worse, CI runs on `x86_64` alone: an `aarch64`-gated body is
//!   never compiled, never linted, never tested and never covered there, while
//!   `--fail-under-regions 100` would still report 100% — a gate reporting
//!   success without taking the measurement, which `CLAUDE.md` §3 rule 6
//!   forbids. This kernel is one body on every target, so what CI measures is
//!   what the operator runs.
//! * **No slice-by-16.** Measured beside slice-by-8 on the block size that
//!   actually exists: 1,632.7 ns against 1,487.5 ns at 4,088 bytes, and 19.6 ns
//!   against 16.2 ns at 60 bytes, for twice the table. It wins only past
//!   64 KiB, a length this store never checksums.
//!
//! # Why two entry points instead of an iterator
//!
//! [`crc32c`] takes a slice, because the fast kernel reads eight bytes at a
//! time and an `IntoIterator<Item = u8>` signature *structurally* forbids that
//! — it was the reason the bit loop could not be replaced. The header slot
//! still needs a **discontiguous** domain (every byte except the four holding
//! the checksum), so [`crc32c_split`] takes the two runs and threads one
//! register through both. It is not a convenience: the covered domain of a
//! header slot is bytes `0..56 ‖ 60..64`, and zero-filling the hole to get one
//! contiguous 64-byte buffer would compute a *different number* and condemn
//! every slot already on disk.

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

/// How many bytes the kernel consumes per iteration.
///
/// Also the number of 256-entry lanes in [`TABLE`], hence 8 KiB of rodata.
const LANES: usize = 8;

/// The slice-by-[`LANES`] lookup tables, built at compile time.
///
/// Lane 0 is the ordinary byte table; lane *k* is lane *k−1* advanced by one
/// more zero byte, which is what lets a byte be folded while `k` bytes still
/// stand behind it.
const TABLE: [[u32; 256]; LANES] = build_table();

/// Builds [`TABLE`] by the same bit loop this file used to run at runtime.
///
/// `#[allow]` rather than `.get()` because `const fn` cannot call `<[T]>::get`,
/// and rather than `u8::try_from` because `TryFrom` is not `const` either. The
/// exception is safe in a way a runtime one would not be: every index here is a
/// literal loop bound below a literal length, and const evaluation *is* the
/// check — an out-of-range index would fail the build, not panic in an
/// operator's process.
#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
const fn build_table() -> [[u32; 256]; LANES] {
    let mut table = [[0u32; 256]; LANES];

    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0u8;
        while bit < 8 {
            // `0 - (crc & 1)` is all-ones when the low bit is set and all-zeros
            // when it is not, so the polynomial is applied without a branch.
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (POLYNOMIAL & mask);
            bit += 1;
        }
        table[0][i] = crc;
        i += 1;
    }

    let mut lane = 1usize;
    while lane < LANES {
        let mut i = 0usize;
        while i < 256 {
            let prev = table[lane - 1][i];
            table[lane][i] = (prev >> 8) ^ table[0][(prev & 0xFF) as usize];
            i += 1;
        }
        lane += 1;
    }

    table
}

/// One table lookup, written so that no index and no panic path exist.
///
/// `usize::from(u8)` is at most 255 and every lane holds 256 entries, so the
/// `None` arm cannot be reached by any input. It is spelled as a total
/// expression rather than as `lane[i]` because this workspace denies slice
/// indexing, and a bounds check that can never fail is still a branch no test
/// could enter.
#[inline]
fn look(lane: &[u32; 256], byte: u8) -> u32 {
    lane.get(usize::from(byte)).copied().unwrap_or(0)
}

/// Folds `bytes` into a running CRC register.
///
/// Takes and returns the **un-inverted** register, which is what lets
/// [`crc32c_split`] thread one computation through two discontiguous runs and
/// get the same answer as [`crc32c`] over their concatenation.
fn update(mut crc: u32, bytes: &[u8]) -> u32 {
    let (blocks, tail) = bytes.as_chunks::<LANES>();
    let [l0, l1, l2, l3, l4, l5, l6, l7] = &TABLE;

    for word in blocks {
        // Destructured rather than indexed: the length is fixed by
        // `as_chunks`, so every byte has a name and nothing is bounds-checked.
        let [w0, w1, w2, w3, w4, w5, w6, w7] = *word;
        let [a0, a1, a2, a3] = (u32::from_le_bytes([w0, w1, w2, w3]) ^ crc).to_le_bytes();
        crc = look(l7, a0)
            ^ look(l6, a1)
            ^ look(l5, a2)
            ^ look(l4, a3)
            ^ look(l3, w4)
            ^ look(l2, w5)
            ^ look(l1, w6)
            ^ look(l0, w7);
    }

    for &byte in tail {
        let [low, ..] = crc.to_le_bytes();
        crc = (crc >> 8) ^ look(l0, low ^ byte);
    }

    crc
}

/// CRC-32C over a contiguous run of bytes.
///
/// # Examples
///
/// ```
/// # use store::crc::{crc32c, CHECK_VALUE};
/// assert_eq!(crc32c(b"123456789"), CHECK_VALUE);
/// assert_eq!(crc32c(&[]), 0);
/// ```
#[must_use]
pub fn crc32c(bytes: &[u8]) -> u32 {
    !update(u32::MAX, bytes)
}

/// CRC-32C over two runs, as though they were one.
///
/// The header slot checksums every byte of its 64 except the four holding the
/// checksum itself, and those four sit in the *middle*. This is that domain
/// without a copy and without filling the hole — filling it would be a
/// different number over a different domain, and every slot already written
/// would fail its check on the next read.
///
/// # Examples
///
/// ```
/// # use store::crc::{crc32c, crc32c_split};
/// // Splitting anywhere is the same answer as not splitting.
/// assert_eq!(crc32c_split(b"12345", b"6789"), crc32c(b"123456789"));
/// assert_eq!(crc32c_split(b"", b"123456789"), crc32c(b"123456789"));
/// ```
#[must_use]
pub fn crc32c_split(head: &[u8], tail: &[u8]) -> u32 {
    !update(update(u32::MAX, head), tail)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::{LANES, POLYNOMIAL, TABLE, build_table};

    /// Folds one byte through the bit loop, un-inverted, from a given register.
    fn fold(mut crc: u32, byte: u8) -> u32 {
        crc ^= u32::from(byte);
        for _ in 0..8u8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (POLYNOMIAL & mask);
        }
        crc
    }

    /// This test lives in `src` and not in `tests/` on purpose.
    ///
    /// [`build_table`] runs at **compile** time, so nothing at run time ever
    /// enters it — it was 26 uncovered lines and one uncovered function the
    /// moment it was written, and an integration test cannot reach a private
    /// item to fix that. Calling it here puts the builder under the coverage
    /// gate, and checking it against a derivation written from the polynomial
    /// rather than from the builder is what makes the call worth making: a
    /// table that agrees with itself proves nothing.
    #[test]
    fn the_table_is_the_polynomial_lane_by_lane() {
        let built = build_table();
        assert_eq!(built, TABLE, "the const table is what the builder returns");

        // Lane 0, entry i, is the register left by folding the single byte i
        // into a zero register.
        for (i, entry) in built[0].iter().enumerate() {
            let byte = u8::try_from(i).unwrap();
            assert_eq!(*entry, fold(0, byte), "lane 0 entry {i}");
        }

        // Lane k is lane k-1 advanced by one further zero byte. That identity
        // is the whole reason slice-by-8 works, and getting it wrong is the
        // commonest bug in fast CRC code -- one that stays invisible on any
        // input shorter than the stride.
        for lane in 1..LANES {
            for (i, entry) in built[lane].iter().enumerate() {
                assert_eq!(*entry, fold(built[lane - 1][i], 0), "lane {lane} entry {i}");
            }
        }
    }
}
