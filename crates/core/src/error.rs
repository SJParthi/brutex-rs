//! Error types.
//!
//! Every error in this crate names what went wrong and refuses to carry a
//! substitute value. `docs/00-charter.md` prohibition 6: degrade loudly and
//! name the reason, or refuse — never silently. An error variant that means
//! "something was wrong, here is a default anyway" does not exist here.
//!
//! These are plain enums with a hand-written [`core::fmt::Display`]. A derive
//! macro would be a dependency, and this crate has none.

use core::fmt;

/// A price could not be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PriceError {
    /// The value was NaN or an infinity.
    ///
    /// A vendor that sends this is malfunctioning. There is no sensible
    /// substitute, so the bar carrying it is refused rather than repaired.
    NotFinite,
    /// The scaled paisa value does not fit in an `i64`.
    ///
    /// Raised in preference to letting a float-to-integer cast saturate, which
    /// would turn an absurd quote into a plausible extreme price.
    OutOfRange,
    /// An arithmetic operation on two prices would have wrapped.
    Overflow,
}

impl fmt::Display for PriceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::NotFinite => "price is not a finite number",
            Self::OutOfRange => "price does not fit in i64 paisa",
            Self::Overflow => "price arithmetic would overflow i64",
        };
        f.write_str(msg)
    }
}

impl core::error::Error for PriceError {}

/// An instrument could not be parsed or is not one this engine accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InstrumentError {
    /// The exchange segment of the identifier was not recognised.
    UnknownExchange,
    /// The symbol is not one of the three the engine sweeps.
    ///
    /// This is not a parse failure. The symbol may be perfectly valid and
    /// stored — futures, options and single stocks all are — but
    /// `docs/00-charter.md` section 1 fixes the swept set at exactly three,
    /// and widening it requires a decision-ledger entry rather than a caller
    /// passing a different string.
    NotSweepable,
    /// The identifier was empty or malformed.
    Malformed,
}

impl fmt::Display for InstrumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::UnknownExchange => "unknown exchange",
            Self::NotSweepable => {
                "instrument is storable but not sweepable; the engine surface is fixed at three"
            }
            Self::Malformed => "malformed instrument identifier",
        };
        f.write_str(msg)
    }
}

impl core::error::Error for InstrumentError {}

/// A calendar question could not be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CalendarError {
    /// The date is outside the years the calendar covers.
    ///
    /// The calendar is transcribed data, not a rule engine. It knows the years
    /// it was given and refuses the rest rather than extrapolating a holiday
    /// pattern, which is how a wrong session count enters a result set.
    OutOfCoverage,
    /// The date is not a valid calendar date.
    NotADate,
}

impl fmt::Display for CalendarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::OutOfCoverage => "date is outside the calendar's covered years",
            Self::NotADate => "not a valid calendar date",
        };
        f.write_str(msg)
    }
}

impl core::error::Error for CalendarError {}

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
    fn every_price_error_renders_a_distinct_reason() {
        let all = [
            PriceError::NotFinite,
            PriceError::OutOfRange,
            PriceError::Overflow,
        ];
        let rendered: Vec<String> = all.iter().map(ToString::to_string).collect();
        for (i, a) in rendered.iter().enumerate() {
            assert!(!a.is_empty(), "variant {i} rendered empty");
            for (j, b) in rendered.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "variants {i} and {j} render identically");
                }
            }
        }
    }

    #[test]
    fn every_instrument_error_renders_a_distinct_reason() {
        let all = [
            InstrumentError::UnknownExchange,
            InstrumentError::NotSweepable,
            InstrumentError::Malformed,
        ];
        let rendered: Vec<String> = all.iter().map(ToString::to_string).collect();
        for (i, a) in rendered.iter().enumerate() {
            assert!(!a.is_empty());
            for (j, b) in rendered.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn every_calendar_error_renders_a_distinct_reason() {
        let all = [CalendarError::OutOfCoverage, CalendarError::NotADate];
        let rendered: Vec<String> = all.iter().map(ToString::to_string).collect();
        assert_ne!(rendered[0], rendered[1]);
        assert!(!rendered[0].is_empty());
        assert!(!rendered[1].is_empty());
    }

    #[test]
    fn errors_are_std_errors() {
        fn assert_error<E: core::error::Error>(_: &E) {}
        assert_error(&PriceError::NotFinite);
        assert_error(&InstrumentError::Malformed);
        assert_error(&CalendarError::NotADate);
    }
}
