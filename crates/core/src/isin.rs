//! An ISO 6166 ISIN — fixed width, check-digit verified, and deliberately
//! **not** part of instrument identity.
//!
//! # Why an ISIN is a cross-check and never a key component
//!
//! An ISIN is the one field two vendors can be compared on: it is issued by a
//! national numbering agency, not by a broker, so it is the same string in
//! every master that carries it. That makes it the ideal *check* — and the
//! worst possible *key component*.
//!
//! [`crate::instrument::InstrumentKey`] derives [`Hash`] and [`Eq`] over every
//! field and is used directly as a `HashMap` key; that collision **is** the
//! deduplication (`docs/05-decisions.md` D-0015). If the ISIN were a field of
//! the key, then two vendors disagreeing about one instrument's ISIN would
//! produce **two different keys**, the merge map would hold them as two
//! separate instruments, and the disagreement would never be seen by anyone.
//! A dedup mechanism that splits silently on the exact field you added to make
//! it more precise is the failure `CLAUDE.md` §4 forbids: a fallback that
//! hides a failure.
//!
//! Kept *beside* the key, the same disagreement is a single loud refusal that
//! names the key and both ISINs — which is what D-0020 already requires of
//! every other vendor disagreement.
//!
//! # Why the check digit is verified rather than trusted
//!
//! Refusing a malformed ISIN is only useful if "malformed" means more than
//! "the wrong number of characters". The check digit is what turns a
//! transcription error into a refusal instead of a plausible-looking
//! identifier that silently points at nothing. Measured on the real masters:
//! exactly **one** row in either file fails the check digit — `IN1520250085`,
//! a state development loan — and it is a debt listing that the equity gate in
//! [`crate::vendor`] declines before an ISIN is ever parsed. The check is
//! therefore free in practice and loud the day it is not.
//!
//! Nothing here normalises. There is no padding, no case folding, no trimming:
//! a value that is not an ISIN is refused, because inventing one is how a
//! wrong identity enters a cross-check whose entire job is to catch wrong
//! identities.

use crate::error::InstrumentError;
use std::fmt;

/// The number of characters in an ISIN. ISO 6166 fixes this at 12.
pub const ISIN_LEN: usize = 12;

/// A validated ISO 6166 International Securities Identification Number.
///
/// Fixed 12 ASCII bytes and [`Copy`], so hashing one is a constant number of
/// machine words — the same reason [`crate::symbol::Symbol`] is fixed width.
/// A vendor that starts sending a longer identifier is a loud refusal at the
/// boundary rather than a silently more expensive probe later.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Isin {
    bytes: [u8; ISIN_LEN],
}

impl Isin {
    /// Parses and validates an ISIN.
    ///
    /// The shape ISO 6166 requires: two uppercase letters of ISO 3166 country
    /// code, nine uppercase alphanumeric characters of national number, and
    /// one decimal check digit — which is verified, not assumed.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] for any length other than
    /// [`ISIN_LEN`], any character outside the position's alphabet, or a check
    /// digit that does not match the value computed from the other eleven.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::isin::Isin;
    /// // CHOLAFIN, the share.
    /// assert_eq!(Isin::new("INE121A01024")?.as_str(), "INE121A01024");
    /// // The same string with one digit changed no longer verifies.
    /// assert!(Isin::new("INE121A01025").is_err());
    /// # Ok::<(), brutex_core::error::InstrumentError>(())
    /// ```
    pub fn new(text: &str) -> Result<Self, InstrumentError> {
        let src = text.as_bytes();
        if src.len() != ISIN_LEN {
            return Err(InstrumentError::Malformed);
        }

        // Zip rather than index: `indexing_slicing` is denied workspace-wide,
        // and this runs on every equity row of every master.
        let mut bytes = [0u8; ISIN_LEN];
        for (i, (dst, &c)) in bytes.iter_mut().zip(src.iter()).enumerate() {
            let allowed = match i {
                // ISO 3166 country code. Lowercase is refused rather than
                // upcased: `in` is not `IN`, it is a vendor bug.
                0 | 1 => c.is_ascii_uppercase(),
                // The check digit is decimal.
                n if n == ISIN_LEN - 1 => c.is_ascii_digit(),
                // The national number, e.g. `E121A01024`.
                _ => c.is_ascii_uppercase() || c.is_ascii_digit(),
            };
            if !allowed {
                return Err(InstrumentError::Malformed);
            }
            *dst = c;
        }

        // An irrefutable array pattern: the length is fixed at ISIN_LEN, so
        // this introduces no branch that no test could ever reach. `split_last`
        // would have returned an Option whose `None` arm is unreachable and
        // therefore permanently uncovered.
        let [.., check] = bytes;
        if expected_check_digit(&bytes) != check - b'0' {
            return Err(InstrumentError::Malformed);
        }
        Ok(Self { bytes })
    }

    /// The ISIN as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // The constructor admits only ASCII, and ASCII is valid UTF-8.
        std::str::from_utf8(&self.bytes).unwrap_or("")
    }
}

/// The check digit ISO 6166 requires for the first eleven characters.
///
/// Every letter is expanded to its two-digit ordinal (`A` = 10 … `Z` = 35),
/// then the Luhn mod-10 sum is taken over the expanded string with **the
/// rightmost digit doubled** — it is the digit adjacent to the check digit.
fn expected_check_digit(isin: &[u8; ISIN_LEN]) -> u8 {
    let digits = || {
        isin.iter().take(ISIN_LEN - 1).flat_map(|&c| {
            let v = if c.is_ascii_digit() {
                c - b'0'
            } else {
                c - b'A' + 10
            };
            // A letter expands to TWO digits, a digit to one. Yielding a pair
            // and dropping the leading digit unless the value is ten or more
            // keeps this allocation-free — no `Vec`, no scratch buffer, no
            // bounds check.
            std::iter::once(v / 10)
                .filter(move |_| v >= 10)
                .chain(std::iter::once(v % 10))
        })
    };

    let n = digits().count();
    // Each term is at most 9 — a doubled 9 is 18, which folds to 1 + 8 — and
    // there are at most 22 terms, so the sum is at most 198 and an 8-bit
    // accumulator cannot overflow. Stated rather than assumed, because a cast
    // here would be a `cast_possible_truncation` the workspace denies.
    let sum: u8 = digits()
        .enumerate()
        .map(|(i, d)| {
            // Position counts from the RIGHT of the expanded string.
            if (n - 1 - i) % 2 == 0 {
                let doubled = d * 2;
                doubled / 10 + doubled % 10
            } else {
                d
            }
        })
        .sum();
    (10 - sum % 10) % 10
}

impl fmt::Display for Isin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Isin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Isin({})", self.as_str())
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

    #[test]
    fn accepts_the_real_isins_this_engine_meets() {
        // Every one of these is copied from a real master row.
        for (text, what) in [
            ("INE002A01018", "RELIANCE, the equity"),
            ("INE121A01024", "CHOLAFIN, the equity"),
            (
                "INE121A08PJ0",
                "CHOLAFIN, the 7.5% NCD — letters in the tail",
            ),
            ("INE775A08105", "MOTHERSON, the NCD"),
            ("INE086A13016", "ELECTCAST, the warrant"),
            ("INE657B01025", "BLUECHIP, whose Groww symbol is suffixed"),
            ("INF179KC1JG3", "HDFCLIQUID, an ETF — an INF issuer prefix"),
            (
                "US0378331005",
                "a non-Indian ISIN, so the check is not IN-only",
            ),
        ] {
            assert!(Isin::new(text).is_ok(), "{text} ({what}) must parse");
        }
    }

    #[test]
    fn the_one_real_row_with_a_bad_check_digit_is_refused() {
        // Measured: across both masters exactly one row carries an ISIN whose
        // check digit does not verify — a state development loan. It is a debt
        // listing, so the equity gate declines it before an ISIN is ever
        // parsed; this test is what proves the digit itself is checked rather
        // than the row merely being unreachable.
        assert_eq!(Isin::new("IN1520250085"), Err(InstrumentError::Malformed));
        // The same string with the digit the algorithm actually computes is
        // accepted, which pins down that ONE character is what was wrong.
        assert!(Isin::new("IN1520250086").is_ok());
    }

    #[test]
    fn every_wrong_length_is_refused_rather_than_padded() {
        for bad in [
            "",
            "INE002A0101",
            "INE002A010188",
            "INE002A01018 ",
            " INE002A01018",
        ] {
            assert_eq!(
                Isin::new(bad),
                Err(InstrumentError::Malformed),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn each_position_enforces_its_own_alphabet() {
        // Country code must be two uppercase letters.
        assert_eq!(Isin::new("1NE002A01018"), Err(InstrumentError::Malformed));
        assert_eq!(Isin::new("I2E002A01018"), Err(InstrumentError::Malformed));
        assert_eq!(
            Isin::new("ine002a01018"),
            Err(InstrumentError::Malformed),
            "lowercase is refused, never upcased"
        );
        // The national number is alphanumeric; punctuation is not.
        assert_eq!(Isin::new("INE002-01018"), Err(InstrumentError::Malformed));
        // The check digit is decimal.
        assert_eq!(Isin::new("INE002A0101A"), Err(InstrumentError::Malformed));
        // Twelve BYTES but eleven characters: it survives the length check and
        // is refused by the alphabet rather than by a lucky length.
        assert_eq!("INE002A010é".len(), ISIN_LEN, "the length check passes it");
        assert_eq!(Isin::new("INE002A010é"), Err(InstrumentError::Malformed));
    }

    #[test]
    fn a_single_wrong_digit_anywhere_fails_the_check() {
        // A transcription error is exactly what the check digit exists to
        // catch, and it is the reason this is verified rather than trusted.
        for bad in [
            "INE002A01017",
            "INE002A01118",
            "INE003A01018",
            "INF002A01018",
        ] {
            assert_eq!(
                Isin::new(bad),
                Err(InstrumentError::Malformed),
                "{bad} must fail the check digit"
            );
        }
    }

    #[test]
    fn renders_for_humans_and_for_debug() {
        let i = Isin::new("INE002A01018").expect("valid");
        assert_eq!(i.as_str(), "INE002A01018");
        assert_eq!(i.to_string(), "INE002A01018");
        assert_eq!(format!("{i:?}"), "Isin(INE002A01018)");
    }

    #[test]
    fn identity_is_the_twelve_bytes_and_nothing_else() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let hash = |i: &Isin| {
            let mut h = DefaultHasher::new();
            i.hash(&mut h);
            h.finish()
        };
        let a = Isin::new("INE121A01024").expect("valid");
        let b = Isin::new("INE121A01024").expect("valid");
        let c = Isin::new("INE121A08PJ0").expect("valid");
        assert_eq!(a, b);
        assert_eq!(hash(&a), hash(&b));
        assert_ne!(
            a, c,
            "the share and its issuer's NCD are NOT the same paper"
        );
        assert_ne!(hash(&a), hash(&c));
        // Ord is the byte order, so a BTreeMap key sorts the way the strings
        // do — `…A01024` before `…A08PJ0`.
        assert!(a < c);
        assert_eq!(a.cmp(&c), std::cmp::Ordering::Less);
    }

    #[test]
    fn the_check_digit_matches_the_published_worked_example() {
        // ISO 6166's own worked example is US0378331005: the expansion is
        // 30 28 037833100, the Luhn sum is 45, and the check digit is 5.
        let body = *b"US0378331005";
        assert_eq!(expected_check_digit(&body), 5);
        // And the same body with a deliberately wrong digit still computes 5,
        // which is what makes the comparison in `new` meaningful.
        let wrong = *b"US0378331000";
        assert_eq!(expected_check_digit(&wrong), 5);
    }
}
