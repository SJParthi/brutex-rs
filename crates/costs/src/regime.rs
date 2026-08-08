//! The dated rate tables, the lookup, and the refusal rows.
//!
//! Four tables ship: securities transaction tax on options premium, the same
//! tax on exercise, and the exchange transaction charge on each of the two
//! venues. They share one lookup, because they are one shape.
//!
//! # The refusal row
//!
//! Two of the four tables have a row that carries **no rate**. Before
//! 2024-10-01 the exchange transaction charge was a member
//! monthly-turnover **slab ladder**; the circulars that fixed those slabs were
//! identified by number and never officially retrieved, and a slab ladder has
//! no honest flat per-trade equivalent anyway. So that window prices nothing.
//! A date landing on it yields a [`Refusal`], and there is no argument, flag,
//! environment variable or default that changes that.
//!
//! The predecessor made this state unrepresentable by convention — its row's
//! rate field was `int | None` and its comment said *"the
//! charge-an-unverified-rate state is UNREPRESENTABLE"*. Here it is
//! unrepresentable by the compiler:
//!
//! * [`Rate::Unverified`] **has no numeric field**. There is nowhere in the
//!   type for a rate to be stored, so there is nothing to read, unwrap or
//!   default.
//! * [`BpsX100`]'s constructor is crate-private, so the only rates in
//!   existence are the ones this crate's `const` tables were built from.
//! * `RegimeTable` is crate-private and every instance is a `const` item in
//!   this file. A caller cannot supply a substitute table with a rate in it.
//!
//! A cited-but-unverified *figure* still exists — it lives in the row's
//! `source` string, as prose, where it can be read and never multiplied.
//!
//! # Why the lookup is O(1), by construction
//!
//! A table is one anchor row plus `[Option<RegimeRow>; MAX_LATER_ROWS]`, and
//! [`MAX_LATER_ROWS`] is `2`. The lookup runs one loop over that fixed-size
//! array: **at most two iterations, for every table and every date, forever**.
//! The trip count is a property of an array length fixed at compile time, not
//! of any argument — `day` is compared, never counted, and the tables are
//! `const` items that exist before any input does. A fifth row is a compile
//! error, not a slower lookup.
//!
//! Two states that would have cost a branch do not exist at all:
//!
//! * **Empty table.** The anchor is a field, not an element, so a table with
//!   no rows cannot be written down.
//! * **Date before the table.** Every table's anchor starts at
//!   [`TradeDay::MIN`], asserted at compile time by the `const` blocks at the
//!   foot of this file, and `TradeDay` cannot represent an earlier day. So
//!   every representable date selects a row.
//!
//! The predecessor used `bisect_right` and its own docstring called
//! that `O(log N)`, "effectively O(1)" because `N <= 3`, explicitly declining
//! to claim a literal bound. That hedge belongs to a variable-length list.
//! There is no list here and no `N` that can grow, so the claim is made
//! plainly and `crates/costs/benches/ratio.rs` measures it.

use brutex_core::instrument::Exchange;

use crate::dated::{boundary, has_content};
use crate::day::TradeDay;
use crate::error::Refusal;
use crate::rate::BpsX100;

/// How many rows a table may carry **after** its anchor row.
///
/// Two, which with the anchor makes [`MAX_REGIME_ROWS`]. This is the number
/// the O(1) claim rests on: it is the length of a fixed-size array, so it
/// bounds the lookup's loop at compile time and no input can raise it.
pub const MAX_LATER_ROWS: usize = 2;

/// How many rows a table may carry in total, anchor included.
pub const MAX_REGIME_ROWS: usize = MAX_LATER_ROWS + 1;

/// What a regime row knows about its rate.
///
/// The two variants are not symmetric, and that asymmetry is the point:
/// [`Self::Verified`] carries a number and [`Self::Unverified`] carries
/// **nothing**. There is no field on the unverified variant for a fabricated
/// rate to occupy, no `Option` to unwrap and no default to fall back to. The
/// citation gap is described in the row's `source` string as prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rate {
    /// A citation-grounded rate.
    Verified(BpsX100),
    /// No rate exists for this window, and none will be invented for it.
    Unverified,
}

/// One dated row: the day it takes effect, its rate or its absence, and its
/// citation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RegimeRow {
    /// The first day this row is in force, inclusive.
    start: TradeDay,
    /// The rate, or the refusal.
    rate: Rate,
    /// The citation for a verified row; for a refusal row, what was
    /// identified and what was never retrieved. Never blank — asserted at
    /// compile time.
    source: &'static str,
}

impl RegimeRow {
    /// A row that carries a rate and the citation it came from.
    const fn verified(start: TradeDay, bps_x100: i64, source: &'static str) -> Self {
        Self {
            start,
            rate: Rate::Verified(BpsX100::new(bps_x100)),
            source,
        }
    }

    /// A row that carries no rate, and the honest account of why.
    const fn unverified(start: TradeDay, source: &'static str) -> Self {
        Self {
            start,
            rate: Rate::Unverified,
            source,
        }
    }

    /// Whether this row is structurally sound: a non-negative rate if it has
    /// one, and a source that says something.
    ///
    /// A `const fn` so the shipped tables are checked by the compiler rather
    /// than by a test that could be deleted.
    const fn is_well_shaped(&self) -> bool {
        let rate_ok = match self.rate {
            Rate::Verified(bps) => bps.get() >= 0,
            Rate::Unverified => true,
        };
        rate_ok && has_content(self.source)
    }
}

/// A dated rate table for one charge.
///
/// Crate-private, and deliberately: this type is the mechanism the refusal
/// contract rests on. If a caller could build one, a caller could build one
/// that prices 2020 at the 2024 rate, and the whole guarantee would be a
/// comment again.
pub(crate) struct RegimeTable {
    /// The human name of the charge, carried into a refusal.
    charge: &'static str,
    /// The venue, when the charge is exchange-scoped.
    exchange: Option<Exchange>,
    /// The row in force from [`TradeDay::MIN`]. A field rather than an array
    /// element, which is what makes "empty table" and "before the table"
    /// unrepresentable.
    anchor: RegimeRow,
    /// Later rows in strictly ascending order, `None`-padded at the end.
    later: [Option<RegimeRow>; MAX_LATER_ROWS],
}

impl RegimeTable {
    /// The rate in force on `day`, or the refusal that stands in its place.
    ///
    /// At most [`MAX_LATER_ROWS`] comparisons. See the module documentation
    /// for why that is a compile-time bound rather than a small `N`.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the row in force carries no verified rate. The refusal
    /// names the window, the citation gap and the remedy.
    fn rate_on(&self, day: TradeDay) -> Result<BpsX100, Refusal> {
        // Start on the anchor: it begins at TradeDay::MIN, so it is in force
        // unless a later row displaces it. There is no "not found" case.
        let mut selected = &self.anchor;
        let mut verified_from = None;
        for row in self.later.iter().flatten() {
            if row.start <= day {
                selected = row;
                // A later row has displaced the one whose successor this was.
                verified_from = None;
            } else if verified_from.is_none() {
                // Rows ascend, so the first row that has not started yet is
                // the successor of whichever row is in force.
                verified_from = Some(row.start);
            }
        }
        match selected.rate {
            Rate::Verified(bps) => Ok(bps),
            Rate::Unverified => Err(Refusal::new(
                self.charge,
                self.exchange,
                day,
                selected.start,
                verified_from,
                selected.source,
            )),
        }
    }

    /// Every row, anchor first, skipping the unused slots.
    fn rows(&self) -> impl Iterator<Item = &RegimeRow> {
        std::iter::once(&self.anchor).chain(self.later.iter().flatten())
    }

    /// The unverified windows this table carries, **derived from the table**.
    ///
    /// One slot per row in table order; `None` where that row carries a
    /// verified rate. Derived rather than written down a second time, so no
    /// report, log line or footer can hold a boundary date that has drifted
    /// from the table it claims to describe.
    fn refusal_windows(&self) -> [Option<RefusalWindow>; MAX_REGIME_ROWS] {
        let mut windows = [None; MAX_REGIME_ROWS];
        let successors = self
            .rows()
            .skip(1)
            .map(|row| Some(row.start))
            .chain(std::iter::once(None));
        for (slot, (row, verified_from)) in windows.iter_mut().zip(self.rows().zip(successors)) {
            if row.rate == Rate::Unverified {
                *slot = Some(RefusalWindow {
                    start: row.start,
                    verified_from,
                });
            }
        }
        windows
    }

    /// Whether the anchor covers every representable day.
    ///
    /// A `const fn` because this is the property the lookup's totality rests
    /// on: if an anchor started later than [`TradeDay::MIN`], every earlier
    /// date would silently be priced at the anchor's rate. Asserted at compile
    /// time for all four shipped tables at the foot of this file.
    const fn anchor_covers_all_days(&self) -> bool {
        self.anchor.start.same_day(TradeDay::MIN)
    }

    /// Whether the rows ascend strictly, with no gap before a populated slot.
    ///
    /// A `const fn`, so the compiler rejects a mis-ordered shipped table. The
    /// match is exhaustive over the fixed array shape and needs no loop and no
    /// indexing — which is only possible because [`MAX_LATER_ROWS`] is a
    /// compile-time constant.
    const fn rows_ascend(&self) -> bool {
        match self.later {
            [None, None] => true,
            [Some(first), None] => self.anchor.start.before(first.start),
            [Some(first), Some(second)] => {
                self.anchor.start.before(first.start) && first.start.before(second.start)
            }
            // A populated slot after an empty one: the table has a hole, and
            // `rows()` would silently close it.
            [None, Some(_)] => false,
        }
    }

    /// Whether every row is structurally sound. A `const fn`, same reason.
    const fn rows_are_well_shaped(&self) -> bool {
        let anchor = self.anchor.is_well_shaped();
        match self.later {
            [None, None] => anchor,
            [Some(first), None] => anchor && first.is_well_shaped(),
            [Some(first), Some(second)] => {
                anchor && first.is_well_shaped() && second.is_well_shaped()
            }
            [None, Some(second)] => anchor && second.is_well_shaped(),
        }
    }

    /// The whole structural contract, in one place, checked by the compiler.
    ///
    /// Every shipped table is asserted against this in a `const` block at the
    /// foot of this file, so a table that anchors too late, does not ascend,
    /// has a hole, carries a negative rate or carries a blank citation does
    /// **not compile**. The tests reach the false arms by building broken
    /// tables, which is the only way to reach them at all.
    const fn is_shipping_shape(&self) -> bool {
        self.anchor_covers_all_days() && self.rows_ascend() && self.rows_are_well_shaped()
    }
}

/// A span of days for which no rate was ever verified.
///
/// Derived from a table by [`exchange_charge_refusal_windows`], never written
/// down twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RefusalWindow {
    start: TradeDay,
    verified_from: Option<TradeDay>,
}

impl RefusalWindow {
    /// The first day of the window, inclusive.
    #[must_use]
    pub const fn start(self) -> TradeDay {
        self.start
    }

    /// The first day a verified rate exists again — the window's exclusive
    /// end. `None` when the window is open-ended.
    #[must_use]
    pub const fn verified_from(self) -> Option<TradeDay> {
        self.verified_from
    }
}

// ---------------------------------------------------------------------------
// Securities transaction tax — options, sell-side premium
// ---------------------------------------------------------------------------

/// The day the Finance Act 2024 rates took effect, and the same day the SEBI
/// true-to-label circular moved the exchange transaction charges.
const OCT_2024: TradeDay = boundary!(2024 - 10 - 1);

/// The day the Finance Act 2026 rates take effect.
const APR_2026: TradeDay = boundary!(2026 - 4 - 1);

/// STT on options premium, sell side. Three regimes, all verified.
const STT_OPTIONS: RegimeTable = RegimeTable {
    charge: "securities transaction tax (options sell premium)",
    exchange: None,
    anchor: RegimeRow::verified(
        TradeDay::MIN,
        6_250,
        "0.0625% — `COSTS_VERIFIED` §1 + DEC-COST-002, verified for the repository's envelope. \
         NOTE, identified and deliberately NOT encoded: Finance Act 2023 reportedly moved options \
         STT from 0.05% to 0.0625% effective 2023-04-01, which would split this window in two. \
         That is UNVERIFIED training knowledge with no retrieved circular, so no second row was \
         invented for it (`MIGRATION.md` #1524 follow-up (iii)).",
    ),
    later: [
        Some(RegimeRow::verified(
            OCT_2024,
            10_000,
            "0.10% — Finance Act 2024 gazette; Zerodha `Z-Connect`; `ClearTax` \
             (`COSTS_VERIFIED` §1).",
        )),
        Some(RegimeRow::verified(
            APR_2026,
            15_000,
            "0.15% — Finance Bill 2026 (DEC-COST-003), VERIFIED via `TaxGuru`, the Wikipedia STT \
             page, and `1Finance` citing the Finance Bill on `indiabudget.gov.in`.",
        )),
    ],
};

/// STT on exercise, charged on intrinsic value. Two regimes, both verified.
///
/// No path in this repository sets an exercise outcome today. The table is
/// carried across because the source carries it and because a rate that is
/// present and dated cannot later be guessed at.
const STT_EXERCISE: RegimeTable = RegimeTable {
    charge: "securities transaction tax on exercise (intrinsic value)",
    exchange: None,
    anchor: RegimeRow::verified(
        TradeDay::MIN,
        12_500,
        "0.125% on intrinsic value before 2026-04-01 (DEC-COST-004; `COSTS_VERIFIED` §1).",
    ),
    later: [
        Some(RegimeRow::verified(
            APR_2026,
            15_000,
            "0.15% on intrinsic value from 2026-04-01 — Finance Act 2026 (DEC-COST-004).",
        )),
        None,
    ],
};

// ---------------------------------------------------------------------------
// Exchange transaction charge — premium, both sides
// ---------------------------------------------------------------------------

/// The NSE exchange transaction charge on options premium, from 2024-10-01.
///
/// 0.03503%, ₹3,503 per crore, both sides. NSE circular `FA64232`, issued
/// under the SEBI true-to-label circular `SEBI/HO/MRD/TPD-1/P/CIR/2024/92`;
/// `COSTS_VERIFIED` §2 row 3 — VERIFIED.
///
/// **UNVERIFIED and deliberately not encoded:** `FA73061` (dated 2026-02-27,
/// effective 2026-03-01) reportedly re-splits this charge and the IPFT as
/// ₹3,552 + ₹1 per crore, leaving the total unchanged at 0.03553%. That would
/// bound this row's window. Until the circular body lands officially, the
/// 3,503 + 50 split stands. `MIGRATION.md` #1524 follow-up (i).
pub const NSE_EXCHANGE_CHARGE: BpsX100 = BpsX100::new(3_503);

/// The BSE exchange transaction charge on options premium, from 2024-10-01.
///
/// 0.03250%, ₹3,250 per crore, both sides, for SENSEX and Bankex. BSE notice
/// of 27-Sep-2024, under the same SEBI true-to-label circular;
/// `COSTS_VERIFIED` §2 row 4 — VERIFIED.
pub const BSE_EXCHANGE_CHARGE: BpsX100 = BpsX100::new(3_250);

/// The exchange transaction charge on NSE. One verified row, and one refusal.
const NSE_EXCHANGE_CHARGE_TABLE: RegimeTable = RegimeTable {
    charge: "exchange transaction charge (options premium)",
    exchange: Some(Exchange::Nse),
    anchor: RegimeRow::unverified(
        TradeDay::MIN,
        "UNVERIFIED — the pre-2024-10-01 NSE F&O options exchange transaction charge was a member \
         monthly-premium-turnover SLAB ladder. Three superseded circulars were IDENTIFIED and \
         NONE was officially retrieved: `NSE/FA/46730` (roughly 2021-01 to 2023-03), `FA56129` \
         (2023-04 to 2024-03) and `FA61137` (2024-04 to 2024-09). Every official host and the web \
         archive were egress-blocked, and a slab ladder has no honest flat per-trade equivalent \
         in any case. Refusal window per DEC-ETC-REGIME-HISTORY-001 (`MIGRATION.md` #1524); \
         Golden Rule 1.",
    ),
    later: [
        Some(RegimeRow::verified(
            OCT_2024,
            3_503,
            "0.03503% — NSE circular `FA64232`, under SEBI true-to-label circular \
             `SEBI/HO/MRD/TPD-1/P/CIR/2024/92`; `COSTS_VERIFIED` §2 row 3 — VERIFIED. NOTE: this \
             window's end may be bounded by `FA73061` (effective 2026-03-01, re-splitting the \
             charge and the IPFT as ₹3,552 + ₹1 per crore with the total unchanged) — UNVERIFIED, \
             NOT encoded, `MIGRATION.md` #1524 follow-up (i).",
        )),
        None,
    ],
};

/// The exchange transaction charge on BSE. One verified row, and one refusal.
const BSE_EXCHANGE_CHARGE_TABLE: RegimeTable = RegimeTable {
    charge: "exchange transaction charge (options premium)",
    exchange: Some(Exchange::Bse),
    anchor: RegimeRow::unverified(
        TradeDay::MIN,
        "UNVERIFIED — the pre-2024-10-01 BSE SENSEX and Bankex options exchange transaction \
         charge. The pre-2023-11 era has NO identified document at all. Notice `20231020-46` \
         (effective 2023-11-01) was a nearest-expiry-scoped slab ladder and notice `20240430-42` \
         (effective 2024-05-13) a slab hike with an active flat-versus-slab source conflict; \
         NEITHER was officially retrieved, and neither a slab nor an expiry-scoped applicability \
         is expressible in a flat table. Part of this window may predate the SENSEX options \
         relaunch, when no contract existed — the refusal is still the correct answer. \
         `MIGRATION.md` #1524.",
    ),
    later: [
        Some(RegimeRow::verified(
            OCT_2024,
            3_250,
            "0.03250% — BSE notice of 27-Sep-2024 (`20240927-37`, under SEBI true-to-label \
             `SEBI/HO/MRD/TPD-1/P/CIR/2024/92`); `COSTS_VERIFIED` §2 row 4 — VERIFIED. The source \
             row cites the notice by date; the number `20240927-37` was not independently \
             reconfirmable — a minor open point, non-blocking.",
        )),
        None,
    ],
};

// ---------------------------------------------------------------------------
// COMPILE-TIME structural proof for every shipped table.
//
// These are not tests. A table that anchors later than TradeDay::MIN, whose
// rows do not ascend, that has a hole, that carries a negative rate or that
// carries a blank citation does not compile. There is no way to ship one and
// find out later.
// ---------------------------------------------------------------------------
const _: () = assert!(STT_OPTIONS.is_shipping_shape());
const _: () = assert!(STT_EXERCISE.is_shipping_shape());
const _: () = assert!(NSE_EXCHANGE_CHARGE_TABLE.is_shipping_shape());
const _: () = assert!(BSE_EXCHANGE_CHARGE_TABLE.is_shipping_shape());

// The two exchange-charge tables must agree with the public constants a caller
// reads directly. Same numbers, one source, checked by the compiler.
const _: () = assert!(NSE_EXCHANGE_CHARGE.get() == 3_503);
const _: () = assert!(BSE_EXCHANGE_CHARGE.get() == 3_250);

// ---------------------------------------------------------------------------
// The public lookups
// ---------------------------------------------------------------------------

/// The STT rate on options sell-side premium in force on `day`.
///
/// The regime is selected by the trade's **entry** date, per the source's
/// `DEC-COST-002`. A boundary date is inclusive: 2024-10-01 is the first day
/// of the 0.10% regime, not the last day of the 0.0625% one.
///
/// # Errors
///
/// [`Refusal`] if the row in force carries no verified rate. Every row of this
/// table is verified today, so this cannot fire — but the signature is the
/// same as the exchange charge's on purpose. A future dated window that cannot
/// be sourced becomes a refusal row and nothing at any call site changes.
///
/// # Examples
///
/// ```
/// use costs::day::TradeDay;
/// use costs::regime::stt_options_rate;
///
/// let before = TradeDay::new(2024, 9, 30)?;
/// let on = TradeDay::new(2024, 10, 1)?;
/// assert_eq!(stt_options_rate(before).map(|r| r.get()), Ok(6_250));
/// assert_eq!(stt_options_rate(on).map(|r| r.get()), Ok(10_000));
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn stt_options_rate(day: TradeDay) -> Result<BpsX100, Refusal> {
    STT_OPTIONS.rate_on(day)
}

/// The STT-on-exercise rate in force on `day`, charged on intrinsic value.
///
/// # Errors
///
/// [`Refusal`] if the row in force carries no verified rate. Every row of this
/// table is verified today; see [`stt_options_rate`] for why the signature
/// still says so.
pub fn stt_exercise_rate(day: TradeDay) -> Result<BpsX100, Refusal> {
    STT_EXERCISE.rate_on(day)
}

/// The exchange transaction charge in force on `day` at `exchange`.
///
/// # Errors
///
/// [`Refusal`] for **every date before 2024-10-01**, on both venues. That is
/// not a defect and it is not a gap waiting to be filled with the current
/// rate: the pre-boundary charge was a slab ladder whose circulars were never
/// retrieved. The refusal names the window, the circulars that were
/// identified, and the one row that would close it.
///
/// # Examples
///
/// ```
/// use brutex_core::instrument::Exchange;
/// use costs::day::TradeDay;
/// use costs::regime::exchange_charge_rate;
///
/// let on = TradeDay::new(2024, 10, 1)?;
/// assert_eq!(exchange_charge_rate(Exchange::Nse, on).map(|r| r.get()), Ok(3_503));
///
/// let before = TradeDay::new(2024, 9, 30)?;
/// let refused = exchange_charge_rate(Exchange::Nse, before)
///     .expect_err("the pre-boundary window has no verified rate");
/// assert_eq!(refused.window_start(), TradeDay::MIN);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn exchange_charge_rate(exchange: Exchange, day: TradeDay) -> Result<BpsX100, Refusal> {
    table_for(exchange).rate_on(day)
}

/// The unverified windows of an exchange's transaction-charge table.
///
/// One slot per row in table order, `None` where the row carries a verified
/// rate. Derived from the table itself so that an operator report cannot
/// quote a boundary the table has moved past.
#[must_use]
pub fn exchange_charge_refusal_windows(
    exchange: Exchange,
) -> [Option<RefusalWindow>; MAX_REGIME_ROWS] {
    table_for(exchange).refusal_windows()
}

/// The exchange-charge table for a venue.
fn table_for(exchange: Exchange) -> &'static RegimeTable {
    match exchange {
        Exchange::Nse => &NSE_EXCHANGE_CHARGE_TABLE,
        Exchange::Bse => &BSE_EXCHANGE_CHARGE_TABLE,
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
    use std::collections::BTreeMap;

    use crate::day::every_representable_day;

    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    fn rate_of(result: Result<BpsX100, Refusal>) -> i64 {
        result.expect("a verified rate").get()
    }

    #[test]
    fn the_boundary_dates_are_the_dates_they_claim_to_be() {
        // The `boundary!` macro builds a TradeDay in a const context, where
        // TradeDay::new cannot be called. This pins the two constructions
        // together, and would fail loudly if a literal were mistyped into
        // something the const constructor rejected and silently anchored.
        assert_eq!(OCT_2024, day(2024, 10, 1));
        assert_eq!(APR_2026, day(2026, 4, 1));
        assert_ne!(OCT_2024, TradeDay::MIN);
        assert_ne!(APR_2026, TradeDay::MIN);
    }

    #[test]
    fn stt_on_options_premium_holds_on_both_sides_of_both_boundaries() {
        assert_eq!(rate_of(stt_options_rate(TradeDay::MIN)), 6_250);
        assert_eq!(rate_of(stt_options_rate(day(2024, 9, 30))), 6_250);
        assert_eq!(rate_of(stt_options_rate(day(2024, 10, 1))), 10_000);
        assert_eq!(rate_of(stt_options_rate(day(2026, 3, 31))), 10_000);
        assert_eq!(rate_of(stt_options_rate(day(2026, 4, 1))), 15_000);
        assert_eq!(rate_of(stt_options_rate(TradeDay::MAX)), 15_000);
    }

    #[test]
    fn stt_on_exercise_holds_on_both_sides_of_its_boundary() {
        assert_eq!(rate_of(stt_exercise_rate(TradeDay::MIN)), 12_500);
        assert_eq!(rate_of(stt_exercise_rate(day(2024, 10, 1))), 12_500);
        assert_eq!(rate_of(stt_exercise_rate(day(2026, 3, 31))), 12_500);
        assert_eq!(rate_of(stt_exercise_rate(day(2026, 4, 1))), 15_000);
        assert_eq!(rate_of(stt_exercise_rate(TradeDay::MAX)), 15_000);
    }

    #[test]
    fn the_exchange_charge_prices_the_boundary_and_refuses_the_day_before_it() {
        for (exchange, want) in [(Exchange::Nse, 3_503), (Exchange::Bse, 3_250)] {
            assert_eq!(
                rate_of(exchange_charge_rate(exchange, day(2024, 10, 1))),
                want,
                "{exchange:?} on the boundary"
            );
            assert_eq!(
                rate_of(exchange_charge_rate(exchange, TradeDay::MAX)),
                want,
                "{exchange:?} still, decades later"
            );
            for refused in [day(2024, 9, 30), day(2020, 6, 1), TradeDay::MIN] {
                let refusal = exchange_charge_rate(exchange, refused)
                    .expect_err("the pre-boundary window carries no rate");
                assert_eq!(refusal.on(), refused);
                assert_eq!(refusal.exchange(), Some(exchange));
                assert_eq!(refusal.window_start(), TradeDay::MIN);
                assert_eq!(refusal.verified_from(), Some(day(2024, 10, 1)));
                assert_eq!(
                    refusal.charge(),
                    "exchange transaction charge (options premium)"
                );
            }
        }
        // The public constants and the tables carry one number, not two.
        assert_eq!(NSE_EXCHANGE_CHARGE.get(), 3_503);
        assert_eq!(BSE_EXCHANGE_CHARGE.get(), 3_250);
    }

    #[test]
    fn the_refusal_names_the_circulars_that_were_identified_and_never_retrieved() {
        let nse = exchange_charge_rate(Exchange::Nse, day(2023, 5, 1)).expect_err("refused");
        let cited = nse.source();
        // Every circular the source identified, and the word that says none of
        // them was retrieved. A refusal that named no document would be a
        // shrug; this one is a work order.
        for token in ["NSE/FA/46730", "FA56129", "FA61137", "SLAB", "NONE"] {
            assert!(cited.contains(token), "the NSE refusal must name {token}");
        }
        assert!(nse.to_string().contains("Refusing to fabricate a value"));

        let bse = exchange_charge_rate(Exchange::Bse, day(2023, 5, 1)).expect_err("refused");
        let cited = bse.source();
        for token in ["20231020-46", "20240430-42", "NEITHER"] {
            assert!(cited.contains(token), "the BSE refusal must name {token}");
        }
        assert!(bse.to_string().contains("(BSE)"));
    }

    #[test]
    fn a_refusal_window_is_derived_from_the_table_and_never_written_twice() {
        for exchange in [Exchange::Nse, Exchange::Bse] {
            let windows = exchange_charge_refusal_windows(exchange);
            let found: Vec<RefusalWindow> = windows.into_iter().flatten().collect();
            assert_eq!(found.len(), 1, "{exchange:?} has exactly one refusal row");
            assert_eq!(found[0].start(), TradeDay::MIN);
            assert_eq!(found[0].verified_from(), Some(day(2024, 10, 1)));
            // The anchor row is the refusal, so slot 0 holds it and the row
            // that carries a rate leaves its slot empty.
            assert!(windows[0].is_some());
            assert!(windows[1].is_none());
            assert!(windows[2].is_none());
        }
        // The tax tables have no refusal rows at all, and say so by carrying
        // no windows rather than by carrying an empty-looking one.
        for table in [&STT_OPTIONS, &STT_EXERCISE] {
            assert_eq!(table.refusal_windows(), [None, None, None]);
        }
    }

    #[test]
    fn an_unverified_row_carries_no_number_for_anything_to_reach() {
        // The observable form of "unrepresentable": the anchor row of both
        // exchange tables matches a variant with no payload, and there is no
        // accessor, no unwrap and no default that yields a BpsX100 from it.
        for table in [&NSE_EXCHANGE_CHARGE_TABLE, &BSE_EXCHANGE_CHARGE_TABLE] {
            assert_eq!(table.anchor.rate, Rate::Unverified);
            assert!(table.rate_on(TradeDay::MIN).is_err());
        }
        for table in [&STT_OPTIONS, &STT_EXERCISE] {
            assert_ne!(table.anchor.rate, Rate::Unverified);
            assert!(table.rate_on(TradeDay::MIN).is_ok());
        }
        assert_eq!(
            format!("{:?}", Rate::Verified(BpsX100::new(7))),
            "Verified(BpsX100(7))"
        );
        assert_eq!(format!("{:?}", Rate::Unverified), "Unverified");
    }

    #[test]
    fn every_shipped_table_passes_the_validation_the_compiler_already_ran() {
        for table in [
            &STT_OPTIONS,
            &STT_EXERCISE,
            &NSE_EXCHANGE_CHARGE_TABLE,
            &BSE_EXCHANGE_CHARGE_TABLE,
        ] {
            assert!(table.is_shipping_shape(), "{} is malformed", table.charge);
            assert!(table.anchor_covers_all_days());
            assert!(table.rows_ascend());
            assert!(table.rows_are_well_shaped());
            assert!(table.rows().count() >= 2);
            assert!(table.rows().count() <= MAX_REGIME_ROWS);
        }
        assert_eq!(MAX_LATER_ROWS, 2);
        assert_eq!(MAX_REGIME_ROWS, 3);
        assert_eq!(STT_OPTIONS.rows().count(), 3);
        assert_eq!(STT_EXERCISE.rows().count(), 2);
        assert_eq!(NSE_EXCHANGE_CHARGE_TABLE.rows().count(), 2);
        assert_eq!(BSE_EXCHANGE_CHARGE_TABLE.rows().count(), 2);
    }

    #[test]
    fn the_validation_rejects_every_way_a_table_can_be_wrong() {
        let good = RegimeRow::verified(TradeDay::MIN, 10, "cited");
        let later = RegimeRow::verified(OCT_2024, 20, "cited");
        let build = |anchor, rows| RegimeTable {
            charge: "test charge",
            exchange: None,
            anchor,
            later: rows,
        };

        // Sound, as a control: a broken case must differ by one thing only.
        assert!(build(good, [Some(later), None]).is_shipping_shape());
        assert!(build(good, [None, None]).is_shipping_shape());
        assert!(
            build(
                good,
                [
                    Some(later),
                    Some(RegimeRow::verified(APR_2026, 30, "cited"))
                ]
            )
            .is_shipping_shape()
        );

        // An anchor that does not cover the start of history: every earlier
        // date would be priced at this row without anything saying so.
        assert!(!build(later, [None, None]).anchor_covers_all_days());
        assert!(!build(later, [None, None]).is_shipping_shape());
        // Rows that do not ascend, and rows that duplicate a start.
        assert!(!build(later, [Some(good), None]).rows_ascend());
        assert!(!build(good, [Some(good), None]).rows_ascend());
        assert!(
            !build(
                good,
                [
                    Some(RegimeRow::verified(APR_2026, 20, "cited")),
                    Some(later)
                ]
            )
            .rows_ascend()
        );
        // A hole: `rows()` would close it silently, so the shape is refused.
        assert!(!build(good, [None, Some(later)]).rows_ascend());
        assert!(!build(good, [None, Some(later)]).is_shipping_shape());
        // A negative rate is not a rate.
        let negative = RegimeRow::verified(OCT_2024, -1, "cited");
        assert!(!negative.is_well_shaped());
        assert!(!build(good, [Some(negative), None]).rows_are_well_shaped());
        assert!(
            !build(
                good,
                [Some(negative), Some(RegimeRow::verified(APR_2026, 1, "c"))]
            )
            .rows_are_well_shaped()
        );
        assert!(
            !build(RegimeRow::verified(TradeDay::MIN, -5, "c"), [None, None]).is_shipping_shape()
        );
        // A row must carry its citation or its citation gap. Never blank.
        assert!(!RegimeRow::verified(OCT_2024, 1, "").is_well_shaped());
        assert!(!RegimeRow::unverified(OCT_2024, "").is_well_shaped());
        assert!(RegimeRow::unverified(OCT_2024, "gap").is_well_shaped());
        // Whitespace-only is not a citation either, and the byte walk in
        // `has_content` is what catches it without `str::trim`.
        let blank = RegimeRow::verified(OCT_2024, 1, "   \t\n ");
        assert!(
            !blank.is_well_shaped(),
            "a blank citation is not a citation"
        );
        assert!(!build(good, [Some(blank), None]).is_shipping_shape());
        assert!(!build(blank, [None, None]).rows_are_well_shaped());
        assert!(has_content("x"));
        assert!(has_content("  x  "));
        assert!(!has_content(""));
        assert!(!has_content(" \t\r\n"));
        // A hole with a well-shaped row in the second slot still shapes: the
        // ordering check is what rejects the hole, and each check answers for
        // itself. Both halves of that arm's conjunction are pinned, because a
        // mutant turning it into a disjunction survived until they were.
        assert!(build(good, [None, Some(later)]).rows_are_well_shaped());
        assert!(!build(good, [None, Some(blank)]).rows_are_well_shaped());
        assert!(!build(blank, [None, Some(later)]).rows_are_well_shaped());
        assert!(!build(blank, [None, Some(blank)]).rows_are_well_shaped());
        // The same, for the arm with one populated slot and for the arm with
        // two: a bad anchor is as fatal as a bad row, in every shape.
        assert!(!build(blank, [Some(later), None]).rows_are_well_shaped());
        assert!(
            !build(
                blank,
                [Some(later), Some(RegimeRow::verified(APR_2026, 1, "c"))]
            )
            .rows_are_well_shaped()
        );
        assert!(
            !build(
                good,
                [Some(later), Some(RegimeRow::verified(APR_2026, 1, " "))]
            )
            .rows_are_well_shaped()
        );
    }

    #[test]
    fn a_mid_table_refusal_reports_its_own_window_not_the_anchors() {
        // No shipped table has a refusal between two verified rows, but the
        // lookup must still describe one correctly: the window starts at the
        // refusing row and ends at the next row's start.
        let table = RegimeTable {
            charge: "test charge",
            exchange: Some(Exchange::Nse),
            anchor: RegimeRow::verified(TradeDay::MIN, 10, "cited"),
            later: [
                Some(RegimeRow::unverified(OCT_2024, "the gap")),
                Some(RegimeRow::verified(APR_2026, 30, "cited")),
            ],
        };
        assert_eq!(rate_of(table.rate_on(day(2024, 9, 30))), 10);
        let refusal = table
            .rate_on(day(2025, 1, 1))
            .expect_err("mid-table refusal");
        assert_eq!(refusal.window_start(), OCT_2024);
        assert_eq!(refusal.verified_from(), Some(APR_2026));
        assert_eq!(refusal.source(), "the gap");
        assert_eq!(rate_of(table.rate_on(APR_2026)), 30);

        // A trailing refusal is open-ended and must say "open", not guess.
        let trailing = RegimeTable {
            charge: "test charge",
            exchange: None,
            anchor: RegimeRow::verified(TradeDay::MIN, 10, "cited"),
            later: [Some(RegimeRow::unverified(OCT_2024, "the gap")), None],
        };
        let open = trailing
            .rate_on(TradeDay::MAX)
            .expect_err("trailing refusal");
        assert_eq!(open.verified_from(), None);
        assert!(open.to_string().contains("-> open)"));
        assert_eq!(
            trailing.refusal_windows(),
            [
                None,
                Some(RefusalWindow {
                    start: OCT_2024,
                    verified_from: None
                }),
                None
            ]
        );
    }

    #[test]
    fn the_lookup_agrees_with_a_naive_scan_on_every_representable_day() {
        // 40,542 days against every shipped table, differentially against a
        // reference that keeps the last row whose start has passed. Not a
        // sample and not a property with a seed: the entire domain.
        let tables = [
            &STT_OPTIONS,
            &STT_EXERCISE,
            &NSE_EXCHANGE_CHARGE_TABLE,
            &BSE_EXCHANGE_CHARGE_TABLE,
        ];
        let mut checked = 0u32;
        for today in every_representable_day() {
            for table in tables {
                let mut expected = &table.anchor;
                for row in table.later.iter().flatten() {
                    if row.start <= today {
                        expected = row;
                    }
                }
                let got = table.rate_on(today);
                match expected.rate {
                    Rate::Verified(bps) => assert_eq!(got, Ok(bps), "{today} {}", table.charge),
                    Rate::Unverified => {
                        let refusal = got.expect_err("an unverified row cannot price");
                        assert_eq!(refusal.window_start(), expected.start);
                        assert_eq!(refusal.source(), expected.source);
                    }
                }
                // Idempotent: the same question twice gives the same answer.
                assert_eq!(table.rate_on(today), got);
                checked += 1;
            }
        }
        assert_eq!(checked, 40_542 * 4, "the whole domain, four tables");
    }

    #[test]
    fn a_boundary_is_inclusive_on_its_own_day_across_the_whole_domain() {
        // A histogram rather than three counters: it pins WHICH rates the
        // whole domain produces as well as how many days each covers, so an
        // unshipped rate appearing anywhere is a failed comparison rather than
        // a value quietly folded into the nearest bucket.
        let mut days_at_rate: BTreeMap<i64, u32> = BTreeMap::new();
        for today in every_representable_day() {
            *days_at_rate
                .entry(rate_of(stt_options_rate(today)))
                .or_default() += 1;
        }
        assert_eq!(
            days_at_rate,
            BTreeMap::from([
                (6_250, 19_997 - 7_305),       // 1990-01-01 up to 2024-09-30
                (10_000, 20_544 - 19_997),     // 2024-10-01 up to 2026-03-31
                (15_000, 47_846 - 20_544 + 1), // 2026-04-01 onward
            ])
        );
        assert_eq!(days_at_rate.values().sum::<u32>(), 40_542);
    }

    #[test]
    fn the_exchange_charge_refuses_exactly_the_pre_boundary_days_and_no_others() {
        for exchange in [Exchange::Nse, Exchange::Bse] {
            let mut refused = 0u32;
            let mut priced = 0u32;
            for today in every_representable_day() {
                match exchange_charge_rate(exchange, today) {
                    Ok(_) => priced += 1,
                    Err(_) => refused += 1,
                }
            }
            assert_eq!(refused, 19_997 - 7_305, "{exchange:?} refuses pre-boundary");
            assert_eq!(refused, 12_692);
            assert_eq!(priced, 47_846 - 19_997 + 1, "{exchange:?} prices the rest");
            assert_eq!(refused + priced, 40_542);
        }
    }
}
