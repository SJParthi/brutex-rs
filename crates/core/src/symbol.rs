//! A fixed-width symbol, so that hashing one is genuinely constant time.
//!
//! # Why not `String`
//!
//! Deduplication is meant to be O(1). Hashing a `String` is O(len) — bounded
//! in practice, but bounded by a value nothing in the type enforces. A vendor
//! that one day emits a 4 KiB identifier would silently make every dedup probe
//! 200× more expensive, and no test would notice because nothing would be
//! *wrong*, only slow.
//!
//! [`Symbol`] is a fixed 24-byte array. Hashing it is a fixed number of machine
//! words regardless of what a vendor sends, and a symbol that does not fit is a
//! loud refusal at the boundary rather than a silent cost later.
//!
//! # Why 24
//!
//! The longest identifier observed across all four vendors is
//! `NIFTYTOTALMCAP` (14) among index symbols, and `NIFTYSMALLCAP250` (16) is
//! the longest in the lake's `NSE/INDEX` listing. Option *contracts* are not
//! stored here — they decompose into an underlying plus expiry, strike and
//! side, which is what keeps this field short. 24 leaves headroom without
//! crossing a cache line when combined with the rest of an
//! [`crate::instrument::InstrumentKey`].

use crate::error::InstrumentError;
use std::fmt;

/// The maximum number of bytes a [`Symbol`] can hold.
pub const SYMBOL_CAPACITY: usize = 24;

/// A fixed-width, ASCII-uppercase instrument symbol.
///
/// Two symbols are equal iff their bytes are equal. Unused trailing bytes are
/// always zero, so equality and hashing never depend on uninitialised padding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol {
    bytes: [u8; SYMBOL_CAPACITY],
    len: u8,
}

impl Symbol {
    /// Builds a symbol from ASCII text.
    ///
    /// Input is uppercased, because vendors are not consistent about case and
    /// `nifty` and `NIFTY` are the same instrument. Anything that is not an
    /// ASCII letter, digit, `-`, `_` or `&` is refused rather than stripped:
    /// silently dropping a character would map two different instruments onto
    /// one identity, which is the exact failure deduplication exists to
    /// prevent.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] if the input is empty, longer than
    /// [`SYMBOL_CAPACITY`], or contains a byte outside the allowed set.
    pub fn new(text: &str) -> Result<Self, InstrumentError> {
        let src = text.as_bytes();
        if src.is_empty() || src.len() > SYMBOL_CAPACITY {
            return Err(InstrumentError::Malformed);
        }

        // Zip rather than index: `indexing_slicing` is denied workspace-wide
        // because an index that can panic is an index that will, and this
        // function runs on every symbol every vendor ever sends.
        let mut bytes = [0u8; SYMBOL_CAPACITY];
        for (dst, &c) in bytes.iter_mut().zip(src.iter()) {
            *dst = match c {
                b'a'..=b'z' => c - b'a' + b'A',
                b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'&' => c,
                _ => return Err(InstrumentError::Malformed),
            };
        }

        // Safe: len was bounds-checked against SYMBOL_CAPACITY (24) above.
        #[allow(clippy::cast_possible_truncation)]
        let len = src.len() as u8;
        Ok(Self { bytes, len })
    }

    /// The symbol as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let n = usize::from(self.len);
        // The constructor admits only ASCII, and ASCII is valid UTF-8.
        core::str::from_utf8(self.bytes.get(..n).unwrap_or(&[])).unwrap_or("")
    }

    /// The number of bytes in use.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Always false — a `Symbol` cannot be constructed empty.
    ///
    /// Present because clippy asks for it next to [`Symbol::len`], and it
    /// documents the invariant rather than implying emptiness is reachable.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Symbol({})", self.as_str())
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
    fn round_trips_every_engine_symbol() {
        for s in ["NIFTY", "BANKNIFTY", "SENSEX", "INDIAVIX"] {
            let sym = Symbol::new(s).expect("valid");
            assert_eq!(sym.as_str(), s);
            assert_eq!(sym.len(), s.len());
            assert!(!sym.is_empty());
        }
    }

    #[test]
    fn uppercases_because_vendors_disagree_about_case() {
        let lower = Symbol::new("nifty").expect("valid");
        let upper = Symbol::new("NIFTY").expect("valid");
        let mixed = Symbol::new("NiFtY").expect("valid");
        assert_eq!(lower, upper, "case must not split one instrument into two");
        assert_eq!(mixed, upper);
        assert_eq!(lower.as_str(), "NIFTY");
    }

    #[test]
    fn refuses_rather_than_stripping_a_bad_byte() {
        // Stripping would map "NIF TY" and "NIFTY" onto one identity, which is
        // precisely the collision deduplication exists to prevent.
        for bad in ["NIF TY", "NIFTY!", "NIFTY.NFO", "निफ्टी", "NIFTY\0"] {
            assert_eq!(
                Symbol::new(bad),
                Err(InstrumentError::Malformed),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn refuses_empty_and_oversize() {
        assert_eq!(Symbol::new(""), Err(InstrumentError::Malformed));
        let too_long = "A".repeat(SYMBOL_CAPACITY + 1);
        assert_eq!(Symbol::new(&too_long), Err(InstrumentError::Malformed));
        // Exactly at capacity is accepted.
        let exact = "A".repeat(SYMBOL_CAPACITY);
        assert_eq!(Symbol::new(&exact).expect("valid").len(), SYMBOL_CAPACITY);
    }

    #[test]
    fn accepts_the_longest_real_lake_symbols() {
        // Verified present in ~/.brutex/lake/bars/NSE/INDEX by the lake survey.
        for s in ["NIFTYSMALLCAP250", "NIFTYTOTALMCAP", "NIFTYMIDCAP150"] {
            assert!(Symbol::new(s).is_ok(), "{s} must fit");
        }
    }

    #[test]
    fn padding_never_affects_identity() {
        // Two symbols of different length must not collide through padding,
        // and equal symbols built separately must hash identically.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let hash = |s: &Symbol| {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };

        let a = Symbol::new("NIFTY").expect("valid");
        let b = Symbol::new("NIFTY").expect("valid");
        let c = Symbol::new("NIFTYY").expect("valid");
        assert_eq!(hash(&a), hash(&b));
        assert_ne!(hash(&a), hash(&c));
        assert_ne!(a, c);
    }

    #[test]
    fn renders_for_humans_and_for_debug() {
        let s = Symbol::new("BANKNIFTY").expect("valid");
        assert_eq!(s.to_string(), "BANKNIFTY");
        assert_eq!(format!("{s:?}"), "Symbol(BANKNIFTY)");
    }
}
