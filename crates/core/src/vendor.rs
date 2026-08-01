//! Turning one vendor's instrument row into the canonical [`InstrumentKey`].
//!
//! # Why this reads columns and never parses the display symbol
//!
//! Every vendor ships a human-facing trading symbol, and it is tempting to
//! parse it. Real rows from the primary broker's own master show why that is a
//! trap:
//!
//! | Trading symbol | Real `expiry_date` column |
//! |---|---|
//! | `NIFTY2680419450CE` | `2026-08-04` — `26` year, `8` month, `04` day, weekly |
//! | `BANKNIFTY25DEC27000PE` | `2025-12-24` — `25` year, `DEC` month, **no day at all** |
//!
//! Two different encodings in one file, and the monthly form does not carry the
//! day, so the expiry is **not recoverable from the symbol**. Worse,
//! `BANKNIFTY25SEP…` happens to expire on the 25th, so a day-first reading and
//! a year-first reading agree on that row and disagree on the other — the kind
//! of coincidence that hides a bug through a whole test suite.
//!
//! The master already carries `expiry_date`, `strike_price` and
//! `underlying_symbol` as separate structured columns. Reading them is exact,
//! it is O(1) per row, and it makes the symbology question disappear rather
//! than answering it. The display symbol is never an input to identity.
//!
//! # Prices
//!
//! Strikes arrive in **rupees** and are stored in **paisa**. `27000` in the
//! master is `2_700_000` here. One missed multiplication makes every strike
//! wrong by a factor of a hundred, so the conversion goes through
//! [`Paisa::from_rupees_half_up`] like every other price.

use crate::error::InstrumentError;
use crate::instrument::{Exchange, Expiry, InstrumentKey, Kind, OptionSide, Segment};
use crate::price::Paisa;
use crate::symbol::Symbol;

/// Which vendor a row came from.
///
/// This is the first segment of the store path — `docs/05-decisions.md`
/// D-0019 — so each vendor owns a completely independent series and can be
/// added, re-pulled or deleted without touching any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Vendor {
    /// Primary broker.
    Groww,
    /// Secondary broker.
    Dhan,
}

impl Vendor {
    /// The path segment for this vendor.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Groww => "groww",
            Self::Dhan => "dhan",
        }
    }
}

/// One row of a vendor instrument master, already split into fields.
///
/// Borrowed rather than owned: a master has hundreds of thousands of rows and
/// the vast majority are rejected, so allocating for each one would be work
/// done to throw away.
#[derive(Debug, Clone, Copy)]
pub struct MasterRow<'a> {
    /// Exchange code, e.g. `NSE`.
    pub exchange: &'a str,
    /// Segment code, e.g. `CASH` or `FNO`.
    pub segment: &'a str,
    /// The underlying symbol — `NIFTY` for a NIFTY option.
    pub underlying: &'a str,
    /// Instrument type: `IDX`, `EQ`, `FUT`, `CE`, `PE`.
    pub instrument_type: &'a str,
    /// Expiry as `YYYY-MM-DD`, empty for cash and index rows.
    pub expiry: &'a str,
    /// Strike in **rupees**, empty for anything that is not an option.
    pub strike_rupees: &'a str,
}

/// A row was skipped, and why.
///
/// Skipping is not failing. A master holds every instrument the vendor knows
/// about, and most of them are legitimately not ours. The reason is carried so
/// an ingest can report *what* it declined rather than a bare count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Skip {
    /// Not an exchange this engine stores. `docs/05-decisions.md` D-0017.
    ForeignExchange,
    /// An exchange test instrument, not a real listing.
    TestInstrument,
    /// A segment this engine does not store, such as commodity.
    ForeignSegment,
}

/// The outcome of reading one master row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// A real instrument this engine stores.
    Keep(InstrumentKey),
    /// A row deliberately declined, with the reason.
    Skipped(Skip),
}

/// Exchange test listings carry these markers in the underlying symbol.
///
/// Real rows observed in the primary broker's master include
/// `031NSETEST36DECFUT` and `061NSETEST36DECFUT`, whose underlyings are
/// `031NSETEST` and `061NSETEST`. Storing them would put fabricated
/// instruments beside real ones, and they would be indistinguishable later.
const TEST_MARKERS: [&str; 2] = ["NSETEST", "BSETEST"];

/// Reads one master row into a canonical key.
///
/// # Errors
///
/// [`InstrumentError`] when a field is present but malformed — an unparseable
/// expiry, a strike that is not a number, an unknown instrument type. A
/// malformed row is an error rather than a skip: skipping is for rows that are
/// *validly* not ours, and quietly dropping a row we failed to understand is
/// how an instrument silently vanishes from a universe.
pub fn decode_master_row(row: MasterRow<'_>) -> Result<Decoded, InstrumentError> {
    // Only NSE is stored. D-0017. An unparseable exchange is skipped rather
    // than an error: a master legitimately lists venues we do not store.
    if !matches!(Exchange::parse(row.exchange), Ok(Exchange::Nse)) {
        return Ok(Decoded::Skipped(Skip::ForeignExchange));
    }
    let exchange = Exchange::Nse;

    if TEST_MARKERS.iter().any(|m| row.underlying.contains(m)) {
        return Ok(Decoded::Skipped(Skip::TestInstrument));
    }

    // The vendor's own segment column is used ONLY to decline what we do not
    // store. It is deliberately NOT used as our segment, because it does not
    // mean the same thing: the primary broker files spot indices under
    // `CASH`, so copying that column would put NIFTY in the equities
    // directory and make `is_sweepable` false for the two instruments the
    // engine exists to sweep. That was a real test failure, not a
    // hypothetical.
    if Segment::parse(row.segment).is_err() {
        return Ok(Decoded::Skipped(Skip::ForeignSegment));
    }

    let underlying = Symbol::new(row.underlying)?;

    // Our segment and our kind are both derived from the instrument type,
    // which is the only field whose meaning is stable across vendors.
    let (segment, kind) = match row.instrument_type {
        "IDX" => (Segment::Index, Kind::Index),
        "EQ" => (Segment::Cash, Kind::Equity),
        "FUT" => (
            Segment::Fno,
            Kind::Future {
                expiry: parse_expiry(row.expiry)?,
            },
        ),
        "CE" | "PE" => (
            Segment::Fno,
            Kind::Option {
                expiry: parse_expiry(row.expiry)?,
                strike: parse_strike(row.strike_rupees)?,
                side: if row.instrument_type == "CE" {
                    OptionSide::Call
                } else {
                    OptionSide::Put
                },
            },
        ),
        _ => return Err(InstrumentError::Malformed),
    };

    Ok(Decoded::Keep(InstrumentKey {
        exchange,
        segment,
        underlying,
        kind,
    }))
}

/// Parses an `YYYY-MM-DD` expiry from the master's own column.
///
/// # Errors
///
/// [`InstrumentError::Malformed`] on any shape other than exactly
/// `YYYY-MM-DD` with numeric parts, or on a date that does not exist.
fn parse_expiry(text: &str) -> Result<Expiry, InstrumentError> {
    let mut parts = text.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(InstrumentError::Malformed);
    };
    // Fixed widths, so a value like `2026-8-4` is refused rather than guessed
    // at — a vendor that changes its date format should be a loud failure.
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(InstrumentError::Malformed);
    }
    let year: u16 = y.parse().map_err(|_| InstrumentError::Malformed)?;
    let month: u8 = m.parse().map_err(|_| InstrumentError::Malformed)?;
    let day: u8 = d.parse().map_err(|_| InstrumentError::Malformed)?;
    Expiry::new(year, month, day)
}

/// Converts a rupee strike to paisa.
///
/// # Errors
///
/// [`InstrumentError::Malformed`] if the value is not a number or does not fit
/// in `i64` paisa.
fn parse_strike(text: &str) -> Result<Paisa, InstrumentError> {
    let rupees: f64 = text.parse().map_err(|_| InstrumentError::Malformed)?;
    Paisa::from_rupees_half_up(rupees).map_err(|_| InstrumentError::Malformed)
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

    /// The kept key, or `None` if the row was skipped.
    ///
    /// Returning an Option rather than destructuring with `else { panic!() }`
    /// keeps every branch reachable, so the coverage gate stays honest: an
    /// unreachable panic arm is an uncovered region that no test can ever
    /// exercise.
    fn kept(d: Decoded) -> Option<InstrumentKey> {
        match d {
            Decoded::Keep(k) => Some(k),
            Decoded::Skipped(_) => None,
        }
    }

    /// The option fields, or `None` if the kind is not an option.
    fn as_option(k: Kind) -> Option<(Expiry, Paisa, OptionSide)> {
        match k {
            Kind::Option {
                expiry,
                strike,
                side,
            } => Some((expiry, strike, side)),
            Kind::Index | Kind::Equity | Kind::Future { .. } => None,
        }
    }

    /// Builds a row with sensible blanks, so each test states only what it means.
    fn row<'a>(
        exchange: &'a str,
        segment: &'a str,
        underlying: &'a str,
        ty: &'a str,
        expiry: &'a str,
        strike: &'a str,
    ) -> MasterRow<'a> {
        MasterRow {
            exchange,
            segment,
            underlying,
            instrument_type: ty,
            expiry,
            strike_rupees: strike,
        }
    }

    #[test]
    fn the_test_helpers_cover_their_negative_arms() {
        // These two helpers exist so no test needs an unreachable panic arm.
        // Their None branches must themselves be exercised, or they become
        // the uncovered regions they were introduced to remove.
        assert!(kept(Decoded::Skipped(Skip::TestInstrument)).is_none());
        assert!(kept(Decoded::Skipped(Skip::ForeignExchange)).is_none());
        assert!(as_option(Kind::Index).is_none());
        assert!(as_option(Kind::Equity).is_none());
        assert!(
            as_option(Kind::Future {
                expiry: Expiry::new(2026, 9, 29).expect("valid"),
            })
            .is_none()
        );
    }

    #[test]
    fn vendor_path_segments_are_stable() {
        assert_eq!(Vendor::Groww.as_str(), "groww");
        assert_eq!(Vendor::Dhan.as_str(), "dhan");
        assert_ne!(Vendor::Groww, Vendor::Dhan);
    }

    #[test]
    fn the_two_engine_indices_decode() {
        // Exactly as they appear in the real master:
        //   NSE,NIFTY,NIFTY,NSE-NIFTY,NIFTY 50,IDX,CASH,...
        for sym in ["NIFTY", "BANKNIFTY"] {
            let got =
                decode_master_row(row("NSE", "CASH", sym, "IDX", "", "")).expect("well formed");
            let key = kept(got).expect("must be kept");
            assert_eq!(key.kind, Kind::Index);
            assert!(key.is_sweepable(), "{sym} is one of the two swept");
        }
    }

    #[test]
    fn a_real_weekly_option_row_decodes_from_columns_not_the_symbol() {
        // Real row: trading_symbol NIFTY2680419450CE, expiry_date 2026-08-04,
        // strike_price 19450. The symbol encodes 26|8|04 and the monthly form
        // encodes no day at all -- which is exactly why identity comes from
        // the columns and never from the display string.
        let got = decode_master_row(row("NSE", "FNO", "NIFTY", "CE", "2026-08-04", "19450"))
            .expect("well formed");
        let key = kept(got).expect("must be kept");
        let (expiry, strike, side) = as_option(key.kind).expect("must be an option");
        assert_eq!((expiry.year(), expiry.month(), expiry.day()), (2026, 8, 4));
        assert_eq!(strike.raw(), 1_945_000, "19450 rupees is 1,945,000 paisa");
        assert_eq!(side, OptionSide::Call);
        assert!(!key.is_sweepable(), "options are stored, never swept");
    }

    #[test]
    fn a_real_monthly_option_row_keeps_the_day_the_column_gives() {
        // BANKNIFTY25DEC27000PE expires 2025-12-24. A day-first reading of
        // "25DEC" would produce the 25th. The column says the 24th, and the
        // column wins -- this test fails if anyone reintroduces symbol parsing.
        let got = decode_master_row(row("NSE", "FNO", "BANKNIFTY", "PE", "2025-12-24", "27000"))
            .expect("well formed");
        let key = kept(got).expect("must be kept");
        let (expiry, strike, _) = as_option(key.kind).expect("must be an option");
        assert_eq!(expiry.day(), 24, "the column says 24, not the symbol's 25");
        assert_eq!(strike.raw(), 2_700_000);
    }

    #[test]
    fn strike_conversion_is_rupees_to_paisa() {
        // A missed multiply here makes every strike wrong by 100x.
        for (rupees, paisa) in [
            ("27000", 2_700_000_i64),
            ("19450", 1_945_000),
            ("96", 9_600),
        ] {
            let got = decode_master_row(row("NSE", "FNO", "NIFTY", "CE", "2026-08-04", rupees))
                .expect("well formed");
            let key = kept(got).expect("kept");
            let (_, strike, _) = as_option(key.kind).expect("option");
            assert_eq!(strike.raw(), paisa, "{rupees} rupees");
        }
    }

    #[test]
    fn a_future_decodes_and_is_never_sweepable() {
        // Real row: 360ONE26SEPFUT, expiry_date 2026-09-29.
        let got = decode_master_row(row("NSE", "FNO", "360ONE", "FUT", "2026-09-29", ""))
            .expect("well formed");
        let key = kept(got).expect("kept");
        assert_eq!(
            key.kind,
            Kind::Future {
                expiry: Expiry::new(2026, 9, 29).expect("valid")
            }
        );
        assert!(!key.is_sweepable());
    }

    #[test]
    fn exchange_test_instruments_are_skipped() {
        // Real rows: 031NSETEST36DECFUT and 061NSETEST36DECFUT. Storing these
        // would put fabricated instruments beside real ones, indistinguishable
        // afterwards.
        for u in ["031NSETEST", "061NSETEST", "BSETEST01"] {
            assert_eq!(
                decode_master_row(row("NSE", "FNO", u, "FUT", "2036-11-27", "")).expect("ok"),
                Decoded::Skipped(Skip::TestInstrument),
                "{u} must be skipped"
            );
        }
    }

    #[test]
    fn bse_and_unknown_exchanges_are_skipped_not_stored() {
        // D-0017 -- NSE only.
        assert_eq!(
            decode_master_row(row("BSE", "CASH", "SENSEX", "IDX", "", "")).expect("ok"),
            Decoded::Skipped(Skip::ForeignExchange)
        );
        assert_eq!(
            decode_master_row(row("MCX", "COMMODITY", "GOLD", "FUT", "2026-08-05", ""))
                .expect("ok"),
            Decoded::Skipped(Skip::ForeignExchange)
        );
    }

    #[test]
    fn commodity_segment_is_skipped() {
        assert_eq!(
            decode_master_row(row("NSE", "COMMODITY", "GOLD", "FUT", "2026-08-05", ""))
                .expect("ok"),
            Decoded::Skipped(Skip::ForeignSegment)
        );
    }

    #[test]
    fn an_equity_decodes_and_is_stored_not_swept() {
        let got = decode_master_row(row("NSE", "CASH", "RELIANCE", "EQ", "", "")).expect("ok");
        let key = kept(got).expect("kept");
        assert_eq!(key.kind, Kind::Equity);
        assert!(!key.is_sweepable(), "D-0018: stored, not swept");
    }

    #[test]
    fn a_malformed_row_errors_rather_than_being_skipped_silently() {
        // Skipping is for rows that are VALIDLY not ours. A row we failed to
        // understand must be loud, or an instrument vanishes without trace.
        assert!(decode_master_row(row("NSE", "FNO", "NIFTY", "XX", "2026-08-04", "1")).is_err());
        assert!(decode_master_row(row("NSE", "FNO", "NIFTY", "CE", "not-a-date", "1")).is_err());
        assert!(decode_master_row(row("NSE", "FNO", "NIFTY", "CE", "2026-08-04", "abc")).is_err());
        assert!(decode_master_row(row("NSE", "FNO", "NIF TY", "FUT", "2026-08-04", "")).is_err());
    }

    #[test]
    fn a_right_length_but_non_numeric_date_part_is_refused() {
        // Distinct from the wrong-LENGTH cases below: these have exactly the
        // 4-2-2 shape, so they pass the width check and must be caught by the
        // numeric parse. Without this, a vendor emitting "20X6-08-04" would
        // reach Expiry::new with whatever a lenient parse produced.
        for bad in ["20X6-08-04", "2026-0X-04", "2026-08-0X", "----------"] {
            assert!(
                decode_master_row(row("NSE", "FNO", "NIFTY", "FUT", bad, "")).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn expiry_column_must_be_exactly_yyyy_mm_dd() {
        // A vendor that changes its date format is a loud failure, not a guess.
        for bad in [
            "2026-8-4",
            "26-08-04",
            "2026/08/04",
            "2026-08",
            "",
            "2026-08-04-01",
        ] {
            assert!(
                decode_master_row(row("NSE", "FNO", "NIFTY", "FUT", bad, "")).is_err(),
                "{bad} must be refused"
            );
        }
        // And an impossible date is refused by Expiry itself.
        assert!(decode_master_row(row("NSE", "FNO", "NIFTY", "FUT", "2026-02-31", "")).is_err());
    }

    #[test]
    fn both_vendors_spelling_one_contract_produce_one_key() {
        // THIS is the deduplication, at the row level. The two brokers ship
        // different column layouts; once decoded they are the same key.
        let from_groww =
            decode_master_row(row("NSE", "FNO", "NIFTY", "CE", "2026-08-04", "19450")).expect("ok");
        // Same contract, lower-case underlying, strike with a trailing decimal
        // -- both of which a different vendor legitimately emits.
        let from_dhan =
            decode_master_row(row("NSE", "FNO", "nifty", "CE", "2026-08-04", "19450.0"))
                .expect("ok");
        assert_eq!(from_groww, from_dhan, "one contract, one identity");
    }
}
