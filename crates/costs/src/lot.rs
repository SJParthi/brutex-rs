//! Lot arithmetic: how many units a contract carries, and what that is worth.
//!
//! # Why the lot size is dated
//!
//! It moved seven times between 2021 and 2026. A backtest that used one lot
//! size across that window would mis-size a NIFTY position by **three times**
//! between April and November 2024 (25 units against 75) and a BANKNIFTY one by
//! more than two (15 against 35). Every charge in this crate that scales with
//! quantity would be wrong by the same factor, in the same direction, on every
//! trade in the window — a systematic error, not noise that averages out.
//!
//! So the lot size is a dated table with the same refusal contract as a
//! statutory rate: before the source's recorded history begins there is **no
//! lot size**, and a date landing there produces a [`Refusal`] rather than the
//! first recorded value stretched backwards.
//!
//! That last point is a deliberate divergence from the source's *documentation*
//! and a faithful port of its *code*. The source's own module docstring says
//! "for dates before the first transition, returns the first recorded value";
//! its `get_lot_size` raises instead. The two disagree, and this port follows
//! the code, because the docstring describes exactly the silent extrapolation
//! `CLAUDE.md` §4 bans. Recorded in `docs/06-limits.md`.
//!
//! # The arithmetic is multiplication and nothing else
//!
//! `quantity = lots × lot_size`, `notional = unit price × quantity`. Two
//! `checked_mul`s. There is no loop, no accumulation over legs and no summing
//! over a chain — a position of one lot and a position of a thousand cost the
//! same to compute. Every overflow is refused by name; nothing wraps and
//! nothing saturates, because a saturated quantity is a position size nobody
//! asked for.

use brutex_core::instrument::{Exchange, InstrumentKey};
use brutex_core::price::Paisa;

use crate::dated::{DatedRow, DatedTable, boundary};
use crate::day::TradeDay;
use crate::error::{CostError, Refusal};
use crate::venue::SweptSlot;

/// How to close a lot-size refusal.
const LOT_REMEDIATION: &str = "source the exchange circular or bulletin fixing the lot size for \
     the window and add ONE dated row (effective date, lot size, citation) to the table in \
     crates/costs/src/lot.rs, with a docs/05-decisions.md ledger entry — zero mechanism change";

/// The first day the shipped lot sizes are citation-grounded.
///
/// The source's recorded history begins here, and it labels this first row a
/// "pre-Jul 2021 default (extended back to start of backfill)" — a value it
/// carried backwards to its own data floor. This crate does not carry it
/// further: [`TradeDay`] reaches to 1990 because `core`'s expiry calendar does,
/// and 1990..2020 has no evidence at all.
pub const LOT_VERIFIED_FROM: TradeDay = boundary!(2021 - 1 - 1);

/// The prose carried by every lot-size refusal row.
const LOT_GAP: &str = "UNVERIFIED — the options lot size before 2021-01-01. The source's recorded \
     history (`brutex/options/lot_size_history.py`, TRACK2_OPTIONS_SPEC §8.3, citation-grounded \
     2026-05-16) begins 2021-01-01 and labels that first row itself a value extended back to its \
     own backfill floor; no exchange circular fixing an earlier lot size was retrieved. \
     `TradeDay` reaches back to 1990 because `core`'s expiry calendar does, so this window has no \
     row. NOTE: the source's module docstring says a pre-history date returns the first recorded \
     value; its code raises instead, and this port follows the code — the docstring describes the \
     silent extrapolation `CLAUDE.md` §4 bans.";

/// How many units of the underlying one options contract carries.
///
/// Strictly positive by construction. A lot of zero would make every position
/// worth nothing and every charge zero, which is a bug that looks like a
/// profitable strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct LotSize(u32);

impl LotSize {
    /// Wraps a lot size. Crate-private, because the only lot sizes that exist
    /// are the ones the dated table below was built from, and a caller that
    /// could mint one could size a position off a number no circular carries.
    const fn new_const(units: u32) -> Self {
        Self(units)
    }

    /// The number of units, always strictly positive.
    #[must_use]
    pub const fn units(self) -> u32 {
        self.0
    }

    /// The number of units, widened for the arithmetic that consumes it.
    // A widening cast from an unsigned type: no truncation and no sign loss are
    // possible, so only the stylistic `cast_lossless` needs excusing — and
    // `i64::from` is not callable in a `const fn` on the pinned toolchain.
    #[allow(clippy::cast_lossless)]
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.0 as i64
    }
}

impl core::fmt::Display for LotSize {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The lot-size history of each swept underlying, keyed on `core`'s own order.
///
/// Every row is the source's `_LOT_HISTORY_*` table with its citation carried
/// across, and the SENSEX table the source also holds is deliberately **not**
/// here: `CLAUDE.md` §1 fixes the engine surface at two NSE instruments, and a
/// third row would be a scope change smuggled in as data. See
/// [`crate::venue`], which makes the same argument about the venue map.
///
/// The array's length is [`SweptSlot::COUNT`], so widening the engine surface
/// stops this file compiling until it gains its table.
const LOT_SIZES: [DatedTable<LotSize>; SweptSlot::COUNT] = [
    DatedTable {
        subject: "options lot size (NIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: LOT_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, LOT_GAP),
        later: [
            Some(DatedRow::verified(
                LOT_VERIFIED_FROM,
                LotSize::new_const(75),
                "75 — the pre-Jul-2021 NSE historic lot, which the source extended back to its \
                 backfill floor; TRACK2_OPTIONS_SPEC §8.3 row 1 (`NSE historic`).",
            )),
            Some(DatedRow::verified(
                boundary!(2021 - 7 - 29),
                LotSize::new_const(50),
                "50 — the July 2021 expiry rebasing; TRACK2_OPTIONS_SPEC §8.3 (`NSE bulletin`).",
            )),
            Some(DatedRow::verified(
                boundary!(2024 - 4 - 26),
                LotSize::new_const(25),
                "25 — effective 26-Apr-2024; TRACK2_OPTIONS_SPEC §8.3 (`NSE FAOP circular`).",
            )),
            Some(DatedRow::verified(
                boundary!(2024 - 11 - 20),
                LotSize::new_const(75),
                "75 — effective 20-Nov-2024, the SEBI minimum-notional rule; \
                 `SEBI/HO/MRD-PoD2/CIR/P/2024/00181`.",
            )),
            Some(DatedRow::verified(
                boundary!(2026 - 1 - 1),
                LotSize::new_const(65),
                "65 — January 2026 onward; TRACK2_OPTIONS_SPEC §8.3 (`NSE Jan-2026 circular`).",
            )),
        ],
    },
    DatedTable {
        subject: "options lot size (BANKNIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: LOT_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, LOT_GAP),
        later: [
            Some(DatedRow::verified(
                LOT_VERIFIED_FROM,
                LotSize::new_const(25),
                "25 — the pre-Jun-2023 lot, which the source extended back to its backfill floor; \
                 TRACK2_OPTIONS_SPEC §8.3 row 1 (`NSE historic`).",
            )),
            Some(DatedRow::verified(
                boundary!(2023 - 6 - 30),
                LotSize::new_const(15),
                "15 — effective 30-Jun-2023; TRACK2_OPTIONS_SPEC §8.3 \
                 (`Zerodha Marketintel #353422`).",
            )),
            Some(DatedRow::verified(
                boundary!(2024 - 11 - 20),
                LotSize::new_const(30),
                "30 — effective 20-Nov-2024, the SEBI minimum-notional rule; \
                 `SEBI/HO/MRD-PoD2/CIR/P/2024/00181`.",
            )),
            Some(DatedRow::verified(
                boundary!(2025 - 7 - 1),
                LotSize::new_const(35),
                "35 — the July 2025 transitional lot; TRACK2_OPTIONS_SPEC §8.3 \
                 (`NSE bulletins`).",
            )),
            Some(DatedRow::verified(
                boundary!(2025 - 12 - 30),
                LotSize::new_const(30),
                "30 — the 30-Dec-2025 end-of-day revert; TRACK2_OPTIONS_SPEC §8.3 \
                 (`NSE FAOP70616`).",
            )),
        ],
    },
];

// ---------------------------------------------------------------------------
// COMPILE-TIME structural proof, and the slot binding.
//
// A table that anchors late, does not ascend, has a hole or carries a blank
// citation does not compile. And slot 0 must be the underlying `core` names
// first: a reordered SWEPT would size a BANKNIFTY position with NIFTY's lot,
// which on 2024-06-01 is 15 units charged as 25. It stops compiling instead.
// ---------------------------------------------------------------------------
const _: () = assert!(LOT_SIZES[0].is_shipping_shape());
const _: () = assert!(LOT_SIZES[1].is_shipping_shape());
const _: () = assert!(crate::dated::str_eq(InstrumentKey::SWEPT[0].1, "NIFTY"));
const _: () = assert!(crate::dated::str_eq(InstrumentKey::SWEPT[1].1, "BANKNIFTY"));

/// The lot size in force for `slot` on `day`.
///
/// # Errors
///
/// [`Refusal`] for a day before [`LOT_VERIFIED_FROM`], naming the window, the
/// citation gap and the remedy. There is no default lot size.
///
/// # Examples
///
/// ```
/// use brutex_core::symbol::Symbol;
/// use costs::day::TradeDay;
/// use costs::lot::lot_size_on;
/// use costs::venue::swept_slot;
///
/// let nifty = swept_slot(Symbol::new("NIFTY")?)?;
///
/// // The 2024 rebasings, either side of the boundary that tripled the lot.
/// assert_eq!(lot_size_on(nifty, TradeDay::new(2024, 11, 19)?)?.units(), 25);
/// assert_eq!(lot_size_on(nifty, TradeDay::new(2024, 11, 20)?)?.units(), 75);
///
/// // Before the recorded history there is a refusal, never a guess.
/// assert!(lot_size_on(nifty, TradeDay::new(2020, 12, 31)?).is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
// The slot's inner index is produced only by `venue::swept_slot`, which returns
// an index into `InstrumentKey::SWEPT`, and the array above has exactly that
// length by its own type. The access is in bounds by construction.
#[allow(clippy::indexing_slicing)]
pub fn lot_size_on(slot: SweptSlot, day: TradeDay) -> Result<LotSize, Refusal> {
    LOT_SIZES[slot.index()].value_on(day)
}

/// How many units of the underlying `lots` contracts carry.
///
/// One multiplication.
///
/// # Errors
///
/// * [`CostError::NotPositive`] when `lots` is zero or negative. A trade of no
///   contracts is not a trade, and a negative lot count is a short expressed in
///   the wrong place — the side belongs on the order, not on the quantity.
/// * [`CostError::Overflow`] when the product leaves `i64`. Refused, never
///   saturated.
///
/// # Examples
///
/// ```
/// use costs::lot::contract_quantity;
/// # use brutex_core::symbol::Symbol;
/// # use costs::day::TradeDay;
/// # use costs::venue::swept_slot;
/// let lot = costs::lot::lot_size_on(
///     swept_slot(Symbol::new("NIFTY")?)?,
///     TradeDay::new(2024, 11, 20)?,
/// )?;
/// assert_eq!(contract_quantity(4, lot)?, 300, "four lots of 75");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn contract_quantity(lots: i64, lot_size: LotSize) -> Result<i64, CostError> {
    if lots <= 0 {
        return Err(CostError::NotPositive {
            quantity: "lots",
            value: lots,
        });
    }
    lots.checked_mul(lot_size.as_i64())
        .ok_or(CostError::Overflow {
            operation: "lots x lot size",
        })
}

/// What `quantity` units are worth at `unit_price`.
///
/// One multiplication. The result is paisa, like every other money value in
/// this repository.
///
/// # Errors
///
/// * [`CostError::NotPositive`] when `quantity` or `unit_price` is not strictly
///   positive. A zero premium is not a quotable price — the option tick is five
///   paisa ([`crate::rate::TICK`]), so the smallest quotable premium is five
///   paisa, and zero is refused rather than priced.
/// * [`CostError::Overflow`] when the product leaves `i64`.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::lot::notional;
///
/// // A premium of 125.50 on 75 units is 9,412.50.
/// assert_eq!(notional(Paisa::from_raw(125_50), 75)?.raw(), 9_412_50);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn notional(unit_price: Paisa, quantity: i64) -> Result<Paisa, CostError> {
    if unit_price.raw() <= 0 {
        return Err(CostError::NotPositive {
            quantity: "unit price",
            value: unit_price.raw(),
        });
    }
    if quantity <= 0 {
        return Err(CostError::NotPositive {
            quantity: "quantity",
            value: quantity,
        });
    }
    unit_price
        .raw()
        .checked_mul(quantity)
        .map(Paisa::from_raw)
        .ok_or(CostError::Overflow {
            operation: "unit price x quantity",
        })
}

#[cfg(test)]
// A money literal is written `rupees_paisa` — `24_012_00` reads as the twelve
// rupees over twenty-four thousand a circular or a screen would show, where
// `2_401_200` reads as nothing at all. The grouping is deliberate.
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::inconsistent_digit_grouping
)]
mod tests {
    use super::*;

    use brutex_core::symbol::Symbol;

    use crate::dated::Dated;
    use crate::venue::swept_slot;

    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    fn slot(text: &str) -> SweptSlot {
        swept_slot(Symbol::new(text).expect("a valid symbol")).expect("a swept underlying")
    }

    #[test]
    fn every_transition_and_the_day_before_it_are_the_source_figures() {
        // Both tables, every boundary, on the day and the day before. The
        // boundary is INCLUSIVE: the transition date is the first day of the
        // new lot, not the last day of the old one.
        // (underlying, transition date, lot on the day, lot the day before —
        // zero meaning the day before is the pre-history refusal).
        type LotPin = (&'static str, (u16, u8, u8), u32, u32);
        let pins: &[LotPin] = &[
            ("NIFTY", (2021, 1, 1), 75, 0), // 0 = a refusal on the day before
            ("NIFTY", (2021, 7, 29), 50, 75),
            ("NIFTY", (2024, 4, 26), 25, 50),
            ("NIFTY", (2024, 11, 20), 75, 25),
            ("NIFTY", (2026, 1, 1), 65, 75),
            ("BANKNIFTY", (2021, 1, 1), 25, 0),
            ("BANKNIFTY", (2023, 6, 30), 15, 25),
            ("BANKNIFTY", (2024, 11, 20), 30, 15),
            ("BANKNIFTY", (2025, 7, 1), 35, 30),
            ("BANKNIFTY", (2025, 12, 30), 30, 35),
        ];
        for &(underlying, (y, m, d), on, before) in pins {
            let subject = slot(underlying);
            let boundary = day(y, m, d);
            assert_eq!(
                lot_size_on(subject, boundary).map(LotSize::units),
                Ok(on),
                "{underlying} on {boundary}"
            );
            let yesterday = boundary.plus_days(-1).expect("inside the window");
            if before == 0 {
                let refusal = lot_size_on(subject, yesterday)
                    .expect_err("the day before recorded history refuses");
                assert_eq!(refusal.verified_from(), Some(boundary));
            } else {
                assert_eq!(
                    lot_size_on(subject, yesterday).map(LotSize::units),
                    Ok(before),
                    "{underlying} on {yesterday}"
                );
            }
        }
    }

    #[test]
    fn the_two_underlyings_carry_different_lots_on_the_same_day() {
        // A single hardcoded lot would size one of the two wrongly on every
        // trade. On 2024-06-01 the gap is 25 against 15 — a 67% error.
        let on = day(2024, 6, 1);
        assert_eq!(lot_size_on(slot("NIFTY"), on).map(LotSize::units), Ok(25));
        assert_eq!(
            lot_size_on(slot("BANKNIFTY"), on).map(LotSize::units),
            Ok(15)
        );
        // And on 2025-08-01, 75 against 35.
        let later = day(2025, 8, 1);
        assert_eq!(
            lot_size_on(slot("NIFTY"), later).map(LotSize::units),
            Ok(75)
        );
        assert_eq!(
            lot_size_on(slot("BANKNIFTY"), later).map(LotSize::units),
            Ok(35)
        );
    }

    #[test]
    fn the_pre_history_window_refuses_on_every_day_of_it_and_never_after() {
        let mut refused = 0u32;
        let mut priced = 0u32;
        for underlying in ["NIFTY", "BANKNIFTY"] {
            let subject = slot(underlying);
            for today in crate::day::every_representable_day() {
                match lot_size_on(subject, today) {
                    Ok(lot) => {
                        assert!(!today.before(LOT_VERIFIED_FROM), "{today} is pre-history");
                        assert!(lot.units() > 0, "a lot of zero is not a contract");
                        priced += 1;
                    }
                    Err(refusal) => {
                        assert!(today.before(LOT_VERIFIED_FROM), "{today} has a row");
                        assert_eq!(refusal.window_start(), TradeDay::MIN);
                        assert_eq!(refusal.verified_from(), Some(LOT_VERIFIED_FROM));
                        assert_eq!(refusal.remediation(), LOT_REMEDIATION);
                        assert!(refusal.to_string().contains(underlying));
                        refused += 1;
                    }
                }
            }
        }
        // 1990-01-01 .. 2020-12-31 is 11,323 days, on each of two underlyings.
        assert_eq!(refused, 2 * 11_323);
        assert_eq!(priced, 2 * (40_542 - 11_323));
        assert_eq!(LOT_VERIFIED_FROM, day(2021, 1, 1));
    }

    #[test]
    fn every_shipped_lot_row_is_positive_and_keyed_on_cores_order() {
        assert_eq!(LOT_SIZES.len(), SweptSlot::COUNT);
        let mut verified_rows = 0u32;
        for (index, table) in LOT_SIZES.iter().enumerate() {
            assert!(table.is_shipping_shape(), "table {index}");
            assert!(
                table.subject.contains(InstrumentKey::SWEPT[index].1),
                "table {index} names the wrong underlying: {}",
                table.subject
            );
            assert_eq!(table.rows().count(), 6, "one refusal anchor and five rows");
            for row in table.rows() {
                match row.value {
                    Dated::Verified(lot) => {
                        assert!(lot.units() > 0, "a lot of zero is not a contract");
                        assert!(lot.units() <= 100, "no shipped lot is that large");
                        verified_rows += 1;
                    }
                    Dated::Unverified => assert!(row.start.same_day(TradeDay::MIN)),
                }
            }
        }
        assert_eq!(verified_rows, 10);
    }

    #[test]
    fn the_quantity_is_the_product_and_nothing_else() {
        let lot = LotSize::new_const(75);
        assert_eq!(contract_quantity(1, lot), Ok(75));
        assert_eq!(contract_quantity(4, lot), Ok(300));
        assert_eq!(contract_quantity(1_000, lot), Ok(75_000));
        // Every lot count from 1 to 1,000 is exactly lots x units — the
        // statement that this does not loop, accumulate or round.
        for lots in 1i64..=1_000 {
            assert_eq!(contract_quantity(lots, lot), Ok(lots * 75));
        }
        // And it scales with the lot, which is the whole reason the lot is
        // dated: the same order is 25 units in June 2024 and 75 in December.
        let june = lot_size_on(slot("NIFTY"), day(2024, 6, 1)).expect("priced");
        let december = lot_size_on(slot("NIFTY"), day(2024, 12, 1)).expect("priced");
        assert_eq!(contract_quantity(2, june), Ok(50));
        assert_eq!(contract_quantity(2, december), Ok(150));
    }

    #[test]
    fn a_lot_count_that_is_not_a_trade_is_refused_by_name() {
        let lot = LotSize::new_const(75);
        for lots in [0i64, -1, -75, i64::MIN] {
            assert_eq!(
                contract_quantity(lots, lot),
                Err(CostError::NotPositive {
                    quantity: "lots",
                    value: lots
                }),
                "{lots} lots is not a trade"
            );
        }
    }

    #[test]
    fn a_quantity_past_i64_is_refused_rather_than_wrapped() {
        // The overflow the operator asked for by name. A wrapped quantity is a
        // negative position; a saturated one is a size nobody ordered.
        let lot = LotSize::new_const(75);
        assert_eq!(
            contract_quantity(i64::MAX, lot),
            Err(CostError::Overflow {
                operation: "lots x lot size"
            })
        );
        assert_eq!(
            contract_quantity(i64::MAX / 75 + 1, lot),
            Err(CostError::Overflow {
                operation: "lots x lot size"
            })
        );
        // One lot below the edge still answers, which proves the refusal is the
        // boundary and not the neighbourhood.
        assert_eq!(
            contract_quantity(i64::MAX / 75, lot),
            Ok((i64::MAX / 75) * 75)
        );
        // A lot of one can never overflow, whatever the lot count.
        assert_eq!(
            contract_quantity(i64::MAX, LotSize::new_const(1)),
            Ok(i64::MAX)
        );
    }

    #[test]
    fn the_notional_is_the_product_of_price_and_quantity() {
        assert_eq!(
            notional(Paisa::from_raw(125_50), 75),
            Ok(Paisa::from_raw(9_412_50))
        );
        assert_eq!(notional(Paisa::from_raw(5), 1), Ok(Paisa::from_raw(5)));
        // The tick is five paisa, so the smallest quotable premium on the
        // largest shipped lot is a real, small number.
        assert_eq!(
            notional(crate::rate::TICK, LotSize::new_const(75).as_i64()),
            Ok(Paisa::from_raw(375))
        );
        // A whole round trip's worth of arithmetic, composed: lots -> quantity
        // -> notional, with the lot read off the dated table.
        let lot = lot_size_on(slot("BANKNIFTY"), day(2025, 8, 1)).expect("priced");
        let quantity = contract_quantity(3, lot).expect("a real trade");
        assert_eq!(quantity, 105, "three lots of 35");
        assert_eq!(
            notional(Paisa::from_raw(200_00), quantity),
            Ok(Paisa::from_raw(21_000_00))
        );
    }

    #[test]
    fn a_non_positive_price_or_quantity_is_refused_by_name() {
        for bad in [0i64, -1, i64::MIN] {
            assert_eq!(
                notional(Paisa::from_raw(bad), 75),
                Err(CostError::NotPositive {
                    quantity: "unit price",
                    value: bad
                })
            );
            assert_eq!(
                notional(Paisa::from_raw(100), bad),
                Err(CostError::NotPositive {
                    quantity: "quantity",
                    value: bad
                })
            );
        }
    }

    #[test]
    fn a_notional_past_i64_is_refused_rather_than_wrapped() {
        assert_eq!(
            notional(Paisa::from_raw(i64::MAX), 2),
            Err(CostError::Overflow {
                operation: "unit price x quantity"
            })
        );
        assert_eq!(
            notional(Paisa::from_raw(2), i64::MAX),
            Err(CostError::Overflow {
                operation: "unit price x quantity"
            })
        );
        assert_eq!(
            notional(Paisa::from_raw(1), i64::MAX),
            Ok(Paisa::from_raw(i64::MAX))
        );
    }

    #[test]
    fn a_lot_size_reads_renders_and_hashes_as_the_number_it_is() {
        use std::collections::HashSet;

        let lot = LotSize::new_const(75);
        assert_eq!(lot.units(), 75);
        assert_eq!(lot.as_i64(), 75);
        assert_eq!(lot.to_string(), "75");
        assert_eq!(format!("{lot:?}"), "LotSize(75)");
        assert!(LotSize::new_const(25) < lot);
        let mut set = HashSet::new();
        assert!(set.insert(lot));
        assert!(!set.insert(LotSize::new_const(75)));
        assert!(set.insert(LotSize::new_const(25)));
        assert_eq!(set.len(), 2);
    }
}
