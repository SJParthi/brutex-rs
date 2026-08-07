//! Which exchange's circular governs a trade.
//!
//! The exchange transaction charge and the IPFT are venue-scoped, because the
//! circulars that fix them are: NSE `FA64232` and the BSE notice of
//! 27-Sep-2024 are two documents about two exchanges. So the rate tables in
//! [`crate::regime`] are keyed on [`Exchange`] and not on an instrument name.
//!
//! # Why this crate names no instrument of its own
//!
//! The predecessor held its own instrument-to-exchange map, with
//! `NIFTY` and `BANKNIFTY` on NSE and `SENSEX` on BSE, and refused anything
//! else. Transcribing that map here would write a third instrument name into a
//! repository whose `CLAUDE.md` §1 fixes the engine surface at exactly two,
//! both on NSE — a scope change smuggled in as a lookup table.
//!
//! So this module holds **no table of its own**. It reads
//! [`InstrumentKey::SWEPT`], which is `CLAUDE.md` §1's set expressed as data in
//! `crates/core`, and refuses everything outside it. Widening the costable set
//! is then the same edit as widening the engine surface, which is the point:
//! the two cannot drift.
//!
//! A caller costing something outside that set — the BSE index history that
//! `CLAUDE.md` §1 keeps on disk, for instance — already knows its venue and
//! passes the [`Exchange`] to [`crate::regime::exchange_charge_rate`]
//! directly. The BSE rows exist and are reachable; what does not exist is this
//! crate guessing which venue an unfamiliar symbol trades on.
//!
//! # Cost
//!
//! [`InstrumentKey::SWEPT`] is a fixed-size array of two entries and a
//! [`Symbol`] is at most [`brutex_core::symbol::SYMBOL_CAPACITY`] bytes wide.
//! The resolution is therefore two comparisons of at most 24 bytes each —
//! bounded by two compile-time constants, and by no input.

use brutex_core::instrument::{Exchange, InstrumentKey};
use brutex_core::symbol::Symbol;

use crate::error::CostError;

/// The exchange whose circulars govern trades on `underlying`.
///
/// The underlying, not the contract: a NIFTY option is costed under the NSE
/// circulars because NIFTY is an NSE index, whatever the contract is called.
///
/// # Errors
///
/// [`CostError::UnknownInstrument`] for anything outside
/// [`InstrumentKey::SWEPT`]. This is a refusal, not a failure to parse — the
/// symbol may be entirely valid and stored. There is no default venue, because
/// a default venue would charge an NSE rate on a BSE trade and say nothing.
///
/// # Examples
///
/// ```
/// use brutex_core::instrument::Exchange;
/// use brutex_core::symbol::Symbol;
/// use costs::error::CostError;
/// use costs::venue::exchange_for_underlying;
///
/// let nifty = Symbol::new("NIFTY")?;
/// assert_eq!(exchange_for_underlying(nifty), Ok(Exchange::Nse));
///
/// let sensex = Symbol::new("SENSEX")?;
/// assert_eq!(
///     exchange_for_underlying(sensex),
///     Err(CostError::UnknownInstrument),
/// );
/// # Ok::<(), brutex_core::error::InstrumentError>(())
/// ```
pub fn exchange_for_underlying(underlying: Symbol) -> Result<Exchange, CostError> {
    swept_slot(underlying).map(SweptSlot::exchange)
}

/// Which slot of [`InstrumentKey::SWEPT`] an underlying occupies.
///
/// The lot size, the expiry weekday and the strike step differ between the two
/// swept indices, so their tables need a key. This is that key, and it is
/// deliberately **`core`'s own ordering** rather than an enum of this crate's:
///
/// * The tables are fixed-size arrays of length `InstrumentKey::SWEPT.len()`.
///   If `CLAUDE.md` §1 ever widens the engine surface, `core`'s array grows and
///   every table in this crate **stops compiling** until it gains the row —
///   rather than silently pricing a third index with the first one's lot size.
/// * A slot is an index, so selecting a table is arithmetic and not a search.
///   `docs/07-o1-architecture.md` law 4.
///
/// The inner index is private and is only ever produced by scanning
/// `InstrumentKey::SWEPT`, so it is always a valid index into any array of that
/// length. That is what lets the table accessors index without a bounds check
/// that could never fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SweptSlot(usize);

impl SweptSlot {
    /// How many slots there are — [`InstrumentKey::SWEPT`]'s own length.
    ///
    /// Every per-underlying table in this crate is an array of exactly this
    /// many entries, asserted at compile time beside each one.
    pub const COUNT: usize = InstrumentKey::SWEPT.len();

    /// The index into [`InstrumentKey::SWEPT`], always below [`Self::COUNT`].
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }

    /// The venue whose circulars govern this underlying.
    #[must_use]
    pub fn exchange(self) -> Exchange {
        self.entry().0
    }

    /// The underlying's symbol, as `core` spells it.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        self.entry().1
    }

    /// The row this slot names.
    // `self.0` is produced only by `swept_slot`, which returns an index into
    // `InstrumentKey::SWEPT` itself, and the field is private to this module.
    // The access is therefore in bounds by construction; a `get` here would add
    // a branch no input could take and a coverage hole where the answer went.
    #[allow(clippy::indexing_slicing)]
    fn entry(self) -> (Exchange, &'static str) {
        InstrumentKey::SWEPT[self.0]
    }
}

/// The slot `underlying` occupies in [`InstrumentKey::SWEPT`].
///
/// # Errors
///
/// [`CostError::UnknownInstrument`] for anything outside the swept set — see
/// [`exchange_for_underlying`], which is this function with the slot discarded.
///
/// # Examples
///
/// ```
/// use brutex_core::symbol::Symbol;
/// use costs::venue::swept_slot;
///
/// let slot = swept_slot(Symbol::new("BANKNIFTY")?)?;
/// assert_eq!(slot.symbol(), "BANKNIFTY");
/// assert!(slot.index() < costs::venue::SweptSlot::COUNT);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn swept_slot(underlying: Symbol) -> Result<SweptSlot, CostError> {
    for (index, &(_, swept)) in InstrumentKey::SWEPT.iter().enumerate() {
        if underlying.as_str() == swept {
            return Ok(SweptSlot(index));
        }
    }
    Err(CostError::UnknownInstrument)
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

    fn symbol(text: &str) -> Symbol {
        Symbol::new(text).expect("a valid symbol")
    }

    #[test]
    fn both_swept_underlyings_resolve_to_nse() {
        assert_eq!(exchange_for_underlying(symbol("NIFTY")), Ok(Exchange::Nse));
        assert_eq!(
            exchange_for_underlying(symbol("BANKNIFTY")),
            Ok(Exchange::Nse)
        );
        // Symbol uppercases, because vendors disagree about case and one
        // instrument must not become two.
        assert_eq!(exchange_for_underlying(symbol("nifty")), Ok(Exchange::Nse));
    }

    #[test]
    fn an_underlying_outside_the_engine_surface_is_refused_and_never_defaulted() {
        for text in [
            // The predecessor's third instrument. BSE rows exist and are
            // reachable by exchange; this crate still does not name it.
            "SENSEX",
            // Reference-only, per CLAUDE.md section 1.
            "INDIAVIX",
            // Stored, never swept.
            "RELIANCE",
            "NIFTYNXT50",
            "NIFTYSMALLCAP250",
            // A near miss, which is the case a prefix match would get wrong.
            "NIFTYY",
            "BANKNIFT",
        ] {
            assert_eq!(
                exchange_for_underlying(symbol(text)),
                Err(CostError::UnknownInstrument),
                "{text} must be refused, not defaulted onto a venue"
            );
        }
    }

    #[test]
    fn a_slot_is_cores_own_index_and_round_trips_to_cores_own_row() {
        assert_eq!(SweptSlot::COUNT, 2);
        assert_eq!(SweptSlot::COUNT, InstrumentKey::SWEPT.len());
        for (index, &(exchange, swept)) in InstrumentKey::SWEPT.iter().enumerate() {
            let slot = swept_slot(symbol(swept)).expect("a swept underlying");
            assert_eq!(slot.index(), index, "{swept} is at core's index {index}");
            assert_eq!(slot.symbol(), swept);
            assert_eq!(slot.exchange(), exchange);
            assert!(slot.index() < SweptSlot::COUNT);
        }
        // The two slots are distinct, ordered, and hashable — a table keyed on
        // them must not collapse the two indices into one entry.
        let nifty = swept_slot(symbol("NIFTY")).expect("swept");
        let banknifty = swept_slot(symbol("BANKNIFTY")).expect("swept");
        assert_ne!(nifty, banknifty);
        assert!(nifty < banknifty);
        assert_eq!(format!("{nifty:?}"), "SweptSlot(0)");
    }

    #[test]
    fn an_unswept_underlying_has_no_slot_either() {
        for text in ["SENSEX", "INDIAVIX", "RELIANCE", "NIFTYY"] {
            assert_eq!(
                swept_slot(symbol(text)),
                Err(CostError::UnknownInstrument),
                "{text} must have no slot, so no table can be indexed for it"
            );
        }
    }

    #[test]
    fn the_costable_set_is_the_engine_surface_itself_and_cannot_drift_from_it() {
        // This crate holds no instrument table. If CLAUDE.md section 1 ever
        // widens, core's SWEPT widens, and this crate follows without an edit
        // — which is exactly why it must be the thing being read.
        assert_eq!(InstrumentKey::SWEPT.len(), 2);
        for &(exchange, swept) in &InstrumentKey::SWEPT {
            assert_eq!(exchange, Exchange::Nse, "{swept} must be an NSE symbol");
            assert_eq!(
                exchange_for_underlying(symbol(swept)),
                Ok(exchange),
                "{swept} is swept and must therefore be costable"
            );
        }
    }
}
