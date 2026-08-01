//! Instrument identity — the one key every vendor resolves to.
//!
//! Five sources spell the same contract five incompatible ways:
//!
//! | Source | The same NIFTY option |
//! |---|---|
//! | primary broker | a symbol string |
//! | secondary broker | a **numeric security id** |
//! | tick vendor A, CSV | `NIFTY03JUL2522800CE.NFO` |
//! | tick vendor A, websocket | `OPTIDX_NIFTY_03JUL2025_CE_22800` |
//! | tick vendor B | a **filename** — its rows carry no identifier at all |
//!
//! [`InstrumentKey`] is what they all decode into. Two vendors naming one
//! contract produce one key and therefore **collide by design** — that
//! collision *is* the deduplication. See `docs/05-decisions.md` D-0015.
//!
//! Every field is fixed-width and every one derives [`Hash`], so a key hashes
//! in a constant number of machine words no matter which vendor it came from.
//! That is what makes the O(1) dedup claim true rather than aspirational.
//!
//! # Storable is not sweepable
//!
//! `docs/00-charter.md` §1 stores futures, options and single stocks but
//! sweeps exactly three instruments. [`InstrumentKey::is_sweepable`] is the
//! one place that distinction is decided, so widening it is a visible edit
//! rather than a caller passing a different string.

use crate::error::InstrumentError;
use crate::price::Paisa;
use crate::symbol::Symbol;
use std::fmt;

/// The exchange an instrument trades on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Exchange {
    /// National Stock Exchange.
    Nse,
    /// Bombay Stock Exchange.
    Bse,
}

impl Exchange {
    /// The path segment used by the store layout.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nse => "NSE",
            Self::Bse => "BSE",
        }
    }

    /// Parses an exchange from its path segment.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::UnknownExchange`] for anything else. There is no
    /// "default exchange": guessing one would file an instrument under a
    /// venue it does not trade on.
    pub fn parse(text: &str) -> Result<Self, InstrumentError> {
        match text {
            "NSE" => Ok(Self::Nse),
            "BSE" => Ok(Self::Bse),
            _ => Err(InstrumentError::UnknownExchange),
        }
    }
}

/// The exchange segment, which is also a directory in the store layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    /// Spot indices.
    Index,
    /// Cash equities.
    Cash,
    /// Futures and options.
    Fno,
}

impl Segment {
    /// The path segment used by the store layout.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Index => "INDEX",
            Self::Cash => "CASH",
            Self::Fno => "FNO",
        }
    }

    /// Parses a segment from its path segment.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] for anything else.
    pub fn parse(text: &str) -> Result<Self, InstrumentError> {
        match text {
            "INDEX" => Ok(Self::Index),
            "CASH" => Ok(Self::Cash),
            "FNO" => Ok(Self::Fno),
            _ => Err(InstrumentError::Malformed),
        }
    }
}

/// Which side of an option contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OptionSide {
    /// Call.
    Call,
    /// Put.
    Put,
}

impl OptionSide {
    /// The two-letter suffix every Indian vendor uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Call => "CE",
            Self::Put => "PE",
        }
    }
}

/// A contract expiry date.
///
/// Stored as year, month, day in that field order so the derived [`Ord`] is
/// chronological without a hand-written comparator.
///
/// This is deliberately **not** a general date type. It validates only what an
/// expiry needs, and it refuses an impossible date rather than normalising it
/// — 31 February would otherwise silently become 3 March and file a contract
/// under a month it never traded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Expiry {
    year: u16,
    month: u8,
    day: u8,
}

impl Expiry {
    /// Builds an expiry, refusing an impossible date.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] if the year is outside 1990..=2100, the
    /// month outside 1..=12, or the day outside the real length of that month
    /// including leap years.
    pub const fn new(year: u16, month: u8, day: u8) -> Result<Self, InstrumentError> {
        if year < 1990 || year > 2100 || month < 1 || month > 12 || day < 1 {
            return Err(InstrumentError::Malformed);
        }
        if day > Self::days_in_month(year, month) {
            return Err(InstrumentError::Malformed);
        }
        Ok(Self { year, month, day })
    }

    /// Days in a given month, honouring the Gregorian leap rule.
    const fn days_in_month(year: u16, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            // A century is a leap year only when divisible by 400, which is
            // why 1900 was not and 2000 was.
            2 if year.is_multiple_of(4)
                && (!year.is_multiple_of(100) || year.is_multiple_of(400)) =>
            {
                29
            }
            2 => 28,
            _ => 0,
        }
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }
    /// The month, 1..=12.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }
    /// The day of month.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }
}

impl fmt::Display for Expiry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// What kind of instrument this is, carrying the fields that kind requires.
///
/// Modelling the variants this way makes an illegal combination
/// *unrepresentable*: an index cannot accidentally carry a strike, and an
/// option cannot exist without one. A flat struct with optional fields would
/// have allowed both, and the check would have moved to review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A spot index. No expiry, no strike.
    Index,
    /// A cash equity. No expiry, no strike.
    Equity,
    /// A futures contract.
    Future {
        /// Contract expiry.
        expiry: Expiry,
    },
    /// An options contract.
    Option {
        /// Contract expiry.
        expiry: Expiry,
        /// Strike price, in paisa.
        strike: Paisa,
        /// Call or put.
        side: OptionSide,
    },
}

/// The canonical identity of one tradable instrument.
///
/// Equality and hashing are structural over every field, so this type is
/// directly usable as a `HashMap` key — which is what makes duplicate
/// rejection a single probe rather than a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstrumentKey {
    /// Trading venue.
    pub exchange: Exchange,
    /// Exchange segment.
    pub segment: Segment,
    /// Underlying symbol — `NIFTY` for a NIFTY option, not the contract name.
    pub underlying: Symbol,
    /// Instrument kind and its kind-specific fields.
    pub kind: Kind,
}

impl InstrumentKey {
    /// The three instruments the engine sweeps.
    ///
    /// `docs/00-charter.md` §1. India VIX is deliberately absent: it is stored
    /// and stamped onto trades, but it never enters the condition vocabulary,
    /// ranking, or run identity.
    pub const SWEPT: [(Exchange, &'static str); 3] = [
        (Exchange::Nse, "NIFTY"),
        (Exchange::Nse, "BANKNIFTY"),
        (Exchange::Bse, "SENSEX"),
    ];

    /// Builds a spot index key.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] if the symbol is not valid.
    pub fn index(exchange: Exchange, underlying: &str) -> Result<Self, InstrumentError> {
        Ok(Self {
            exchange,
            segment: Segment::Index,
            underlying: Symbol::new(underlying)?,
            kind: Kind::Index,
        })
    }

    /// Whether the sweep engine may operate on this instrument.
    ///
    /// Storable and sweepable are different questions. Everything is storable;
    /// exactly three things are sweepable, and widening that set requires a
    /// `docs/05-decisions.md` entry rather than a different argument here.
    #[must_use]
    pub fn is_sweepable(&self) -> bool {
        if self.segment != Segment::Index || self.kind != Kind::Index {
            return false;
        }
        Self::SWEPT
            .iter()
            .any(|&(ex, sym)| ex == self.exchange && self.underlying.as_str() == sym)
    }

    /// Refuses an instrument the engine may not sweep.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::NotSweepable`] when [`Self::is_sweepable`] is false.
    /// The error says "storable but not sweepable" rather than "unknown",
    /// because the instrument may be perfectly valid and present.
    pub fn require_sweepable(&self) -> Result<(), InstrumentError> {
        if self.is_sweepable() {
            Ok(())
        } else {
            Err(InstrumentError::NotSweepable)
        }
    }
}

impl fmt::Display for InstrumentKey {
    /// Renders the canonical name used as a store path segment.
    ///
    /// The format is chosen so that sorting the names sorts by underlying,
    /// then expiry, then strike — which is the order a human reads an option
    /// chain in.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.exchange.as_str(), self.underlying)?;
        match self.kind {
            Kind::Index | Kind::Equity => Ok(()),
            Kind::Future { expiry } => write!(f, "-{expiry}-FUT"),
            Kind::Option {
                expiry,
                strike,
                side,
            } => write!(f, "-{}-{}-{}", expiry, strike.raw(), side.as_str()),
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
    use std::collections::HashMap;

    fn nifty() -> InstrumentKey {
        InstrumentKey::index(Exchange::Nse, "NIFTY").expect("valid")
    }

    #[test]
    fn exchange_and_segment_round_trip_through_path_segments() {
        for e in [Exchange::Nse, Exchange::Bse] {
            assert_eq!(Exchange::parse(e.as_str()), Ok(e));
        }
        for s in [Segment::Index, Segment::Cash, Segment::Fno] {
            assert_eq!(Segment::parse(s.as_str()), Ok(s));
        }
        assert_eq!(
            Exchange::parse("MCX"),
            Err(InstrumentError::UnknownExchange)
        );
        assert_eq!(
            Exchange::parse("nse"),
            Err(InstrumentError::UnknownExchange)
        );
        assert_eq!(Segment::parse("CURRENCY"), Err(InstrumentError::Malformed));
    }

    #[test]
    fn option_side_renders_the_vendor_suffix() {
        assert_eq!(OptionSide::Call.as_str(), "CE");
        assert_eq!(OptionSide::Put.as_str(), "PE");
    }

    #[test]
    fn expiry_refuses_impossible_dates_rather_than_normalising() {
        // 31 February would silently become 3 March under a normalising date
        // type, filing a contract under a month it never traded in.
        assert_eq!(Expiry::new(2025, 2, 31), Err(InstrumentError::Malformed));
        assert_eq!(Expiry::new(2025, 13, 1), Err(InstrumentError::Malformed));
        assert_eq!(Expiry::new(2025, 0, 1), Err(InstrumentError::Malformed));
        assert_eq!(Expiry::new(2025, 1, 0), Err(InstrumentError::Malformed));
        assert_eq!(Expiry::new(2025, 4, 31), Err(InstrumentError::Malformed));
        assert_eq!(Expiry::new(1989, 1, 1), Err(InstrumentError::Malformed));
        assert_eq!(Expiry::new(2101, 1, 1), Err(InstrumentError::Malformed));
    }

    #[test]
    fn expiry_honours_the_gregorian_leap_rule() {
        assert!(Expiry::new(2024, 2, 29).is_ok(), "2024 is a leap year");
        assert_eq!(
            Expiry::new(2025, 2, 29),
            Err(InstrumentError::Malformed),
            "2025 is not"
        );
        assert!(Expiry::new(2000, 2, 29).is_ok(), "2000 divisible by 400");
        assert_eq!(
            Expiry::new(2100, 2, 29),
            Err(InstrumentError::Malformed),
            "2100 is divisible by 100 but not 400"
        );
    }

    #[test]
    fn expiry_orders_chronologically_and_renders_iso() {
        let a = Expiry::new(2025, 7, 3).expect("valid");
        let b = Expiry::new(2025, 7, 31).expect("valid");
        let c = Expiry::new(2026, 1, 1).expect("valid");
        assert!(a < b && b < c);
        assert_eq!(a.to_string(), "2025-07-03");
        assert_eq!((a.year(), a.month(), a.day()), (2025, 7, 3));
    }

    #[test]
    fn exactly_three_instruments_are_sweepable() {
        assert!(nifty().is_sweepable());
        assert!(
            InstrumentKey::index(Exchange::Nse, "BANKNIFTY")
                .expect("valid")
                .is_sweepable()
        );
        assert!(
            InstrumentKey::index(Exchange::Bse, "SENSEX")
                .expect("valid")
                .is_sweepable()
        );
        assert_eq!(InstrumentKey::SWEPT.len(), 3);
    }

    #[test]
    fn india_vix_is_storable_but_never_sweepable() {
        // The charter stores and stamps VIX but keeps it out of the condition
        // vocabulary, ranking and run identity.
        let vix = InstrumentKey::index(Exchange::Nse, "INDIAVIX").expect("valid");
        assert!(!vix.is_sweepable());
        assert_eq!(vix.require_sweepable(), Err(InstrumentError::NotSweepable));
    }

    #[test]
    fn the_right_symbol_on_the_wrong_exchange_is_not_sweepable() {
        // SENSEX is a BSE index. The same name under NSE is a different thing
        // and must not inherit sweepability from the string alone.
        let wrong = InstrumentKey::index(Exchange::Nse, "SENSEX").expect("valid");
        assert!(!wrong.is_sweepable());
        let also_wrong = InstrumentKey::index(Exchange::Bse, "NIFTY").expect("valid");
        assert!(!also_wrong.is_sweepable());
    }

    #[test]
    fn derivatives_are_storable_but_never_sweepable() {
        let fut = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Fno,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Future {
                expiry: Expiry::new(2025, 7, 31).expect("valid"),
            },
        };
        assert!(!fut.is_sweepable());

        let opt = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Fno,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Option {
                expiry: Expiry::new(2025, 7, 3).expect("valid"),
                strike: Paisa::from_raw(2_280_000),
                side: OptionSide::Call,
            },
        };
        assert!(!opt.is_sweepable());
        assert_eq!(opt.require_sweepable(), Err(InstrumentError::NotSweepable));
    }

    #[test]
    fn two_spellings_of_one_contract_collide_by_design() {
        // This IS the deduplication. One vendor sends lower case, another
        // upper; both must land on a single map entry.
        let from_vendor_a = InstrumentKey::index(Exchange::Nse, "NIFTY").expect("valid");
        let from_vendor_b = InstrumentKey::index(Exchange::Nse, "nifty").expect("valid");
        assert_eq!(from_vendor_a, from_vendor_b);

        let mut seen: HashMap<InstrumentKey, u32> = HashMap::new();
        *seen.entry(from_vendor_a).or_insert(0) += 1;
        *seen.entry(from_vendor_b).or_insert(0) += 1;
        assert_eq!(seen.len(), 1, "one contract must occupy one slot");
        assert_eq!(seen.get(&from_vendor_a), Some(&2));
    }

    #[test]
    fn contracts_differing_in_one_field_never_collide() {
        let base = |expiry, strike, side| InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Fno,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Option {
                expiry,
                strike: Paisa::from_raw(strike),
                side,
            },
        };
        let e1 = Expiry::new(2025, 7, 3).expect("valid");
        let e2 = Expiry::new(2025, 7, 10).expect("valid");

        let mut seen = std::collections::HashSet::new();
        for k in [
            base(e1, 2_280_000, OptionSide::Call),
            base(e1, 2_280_000, OptionSide::Put), // side differs
            base(e1, 2_285_000, OptionSide::Call), // strike differs
            base(e2, 2_280_000, OptionSide::Call), // expiry differs
        ] {
            seen.insert(k);
        }
        assert_eq!(seen.len(), 4, "every field must participate in identity");
    }

    #[test]
    fn canonical_names_render_for_the_store_path() {
        assert_eq!(nifty().to_string(), "NSE-NIFTY");

        let fut = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Fno,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Future {
                expiry: Expiry::new(2025, 7, 31).expect("valid"),
            },
        };
        assert_eq!(fut.to_string(), "NSE-NIFTY-2025-07-31-FUT");

        let opt = InstrumentKey {
            exchange: Exchange::Bse,
            segment: Segment::Fno,
            underlying: Symbol::new("SENSEX").expect("valid"),
            kind: Kind::Option {
                expiry: Expiry::new(2025, 7, 3).expect("valid"),
                strike: Paisa::from_raw(8_140_000),
                side: OptionSide::Put,
            },
        };
        assert_eq!(opt.to_string(), "BSE-SENSEX-2025-07-03-8140000-PE");
    }

    #[test]
    fn an_equity_is_storable_and_not_sweepable() {
        let eq = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Cash,
            underlying: Symbol::new("HINDALCO").expect("valid"),
            kind: Kind::Equity,
        };
        assert!(!eq.is_sweepable());
        assert_eq!(eq.to_string(), "NSE-HINDALCO");
    }

    #[test]
    fn a_bad_symbol_is_refused_at_construction() {
        assert_eq!(
            InstrumentKey::index(Exchange::Nse, "NIF TY"),
            Err(InstrumentError::Malformed)
        );
        assert_eq!(
            InstrumentKey::index(Exchange::Nse, ""),
            Err(InstrumentError::Malformed)
        );
    }

    #[test]
    fn require_sweepable_accepts_the_three() {
        assert_eq!(nifty().require_sweepable(), Ok(()));
    }
}
