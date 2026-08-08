//! The strike grid: the step, the at-the-money rung, and the rung a moneyness
//! names.
//!
//! # The one idea this module exists to preserve
//!
//! A strike grid is an **arithmetic progression**. The strikes of an index
//! option chain are `k × step` for integer `k`, so the rung nearest a spot is a
//! **rounding operation**, not a search through a list of listed strikes.
//! `docs/07-o1-architecture.md` law 4: if the address can be computed, never
//! search for it. Nothing in this module holds a chain, sorts a ladder or scans
//! a ledger — every answer is two multiplications and a division.
//!
//! # No floats, anywhere, ever
//!
//! The predecessor computed the at-the-money rung as
//! `int((spot + step / 2) // step) * step` over IEEE doubles, and its Rust twin
//! transcribed the predecessor runtime's floor-division operation-for-operation to stay
//! bit-identical. None of that crosses over. `CLAUDE.md` §7 fixes prices as
//! paisa integers, and the same rounding is exact in integers:
//!
//! ```text
//! floor((spot + step/2) / step) · step  =  floor((2·spot + step) / (2·step)) · step
//! ```
//!
//! The right-hand side is the identity multiplied through by two, so it needs
//! no halved step and is exact for an **odd** step as well as an even one — the
//! case the float chain could only approximate. It is evaluated in `i128` so
//! the doubling cannot overflow, and narrowed back to `i64` once, at the end,
//! with the narrowing refused rather than wrapped.
//!
//! # Which way a tie goes, and where that is written down
//!
//! A spot exactly halfway between two rungs rounds **up**, to the higher
//! strike. That is `floor`'s own behaviour on the shifted quotient and it is
//! the source's documented rule (`strike_grid.py`: "half-step ties up",
//! `round_to_atm(24_025.0, NIFTY) -> 24_050`). It is half-up toward **positive
//! infinity**, not half-away-from-zero; the distinction cannot be observed
//! here, because a non-positive spot is refused before the arithmetic runs.
//!
//! The snap happens **once**, in [`at_the_money`], and nothing downstream
//! re-rounds: [`strike_at`] moves a whole number of steps from a rung that is
//! already on the grid.

use brutex_core::price::Paisa;

use crate::dated::{DatedRow, DatedTable, boundary};
use crate::day::TradeDay;
use crate::error::{CostError, Refusal};
use crate::moneyness::MoneynessSteps;
use crate::venue::SweptSlot;

use brutex_core::instrument::{Exchange, InstrumentKey, OptionSide};

/// How to close a strike-step refusal.
const STEP_REMEDIATION: &str = "source the exchange F&O master (or the circular that changed the \
     step) for the window and add ONE dated row (effective date, step in paisa, citation) to the \
     table in crates/costs/src/strike.rs, with a docs/05-decisions.md ledger entry — zero \
     mechanism change";

/// The distance between two adjacent rungs of a strike grid, in paisa.
///
/// **Strictly positive by construction, and there is no constructor a caller
/// can reach.** A zero step would make the division below undefined and a
/// negative one would invert the ladder; neither is guarded at each use,
/// because neither can exist. Every value in circulation came out of the dated
/// table in this file, each entry is a named `const` asserted positive at
/// compile time, and [`Self::new_const`] is crate-private — the same
/// unrepresentability argument [`crate::rate::BpsX100`] makes for a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct StrikeStep(i64);

impl StrikeStep {
    /// Wraps a step. Crate-private, because a public one would let a caller
    /// substitute a grid the exchange does not publish, and every dated row in
    /// this crate would stop meaning anything.
    pub(crate) const fn new_const(paisa: i64) -> Self {
        Self(paisa)
    }

    /// The step in paisa.
    #[must_use]
    pub const fn paisa(self) -> Paisa {
        Paisa::from_raw(self.0)
    }

    /// The step as a raw paisa count, always strictly positive.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }
}

impl core::fmt::Display for StrikeStep {
    /// Renders the step in rupees, which is how the F&O master publishes it.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{:02}", self.0 / 100, (self.0 % 100).abs())
    }
}

/// The first day the shipped strike steps are citation-grounded.
///
/// The source states them as constants "for the 5-year backfill window", whose
/// floor is this day. [`TradeDay`] reaches back to 1990 because `core`'s expiry
/// calendar does, and the source says nothing at all about 1990..2020 — so that
/// window is a refusal, not an extrapolation of today's grid backwards.
pub const STEP_VERIFIED_FROM: TradeDay = boundary!(2021 - 1 - 1);

/// The prose carried by every strike-step refusal row.
const STEP_GAP: &str = "UNVERIFIED — the strike-grid step before 2021-01-01. The source \
     (`brutex/options/strike_grid.py`, TRACK2_OPTIONS_SPEC §5) states the steps as constants over \
     its own 2021-2026 backfill window and records no earlier evidence; no exchange F&O master or \
     circular fixing an earlier step was retrieved. `TradeDay` reaches back to 1990 because \
     `core`'s expiry calendar does, so this window has no row — and extrapolating today's grid \
     backwards would be exactly the fabrication this crate exists to refuse.";

/// NIFTY strikes step 50 rupees, which is 5,000 paisa.
const NIFTY_STEP: StrikeStep = StrikeStep::new_const(5_000);

/// BANKNIFTY strikes step 100 rupees, which is 10,000 paisa.
const BANKNIFTY_STEP: StrikeStep = StrikeStep::new_const(10_000);

// A step of zero would make the division in `at_the_money` undefined and a
// negative one would invert the ladder. Neither is guarded at the point of use,
// so both are refused here, by the compiler, on the only two values that exist.
const _: () = assert!(NIFTY_STEP.raw() > 0);
const _: () = assert!(BANKNIFTY_STEP.raw() > 0);
const _: () = assert!(NIFTY_STEP.raw() != BANKNIFTY_STEP.raw());

/// The strike-grid step of each swept underlying, keyed on `core`'s own order.
///
/// Both figures are the exchange-published grid: NIFTY 50 rupees, BANKNIFTY 100
/// rupees, from the NSE F&O master by way of the source's `STRIKE_STEP` table
/// (`TRACK2_OPTIONS_SPEC` §5, verified quarterly). They are stated in **paisa**
/// here because `CLAUDE.md` §7 fixes prices as paisa integers.
///
/// The array's length is [`SweptSlot::COUNT`], so widening the engine surface
/// stops this file compiling until it gains its row.
const STRIKE_STEPS: [DatedTable<StrikeStep>; SweptSlot::COUNT] = [
    DatedTable {
        subject: "strike grid step (NIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: STEP_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, STEP_GAP),
        later: [
            Some(DatedRow::verified(
                STEP_VERIFIED_FROM,
                NIFTY_STEP,
                "NIFTY 50 strikes step 50 rupees = 5,000 paisa — NSE F&O master by way of \
                 `brutex/options/strike_grid.py` `STRIKE_STEP`; TRACK2_OPTIONS_SPEC §5, verified \
                 quarterly, no SEBI step change recorded over 2021-2026.",
            )),
            None,
            None,
            None,
            None,
        ],
    },
    DatedTable {
        subject: "strike grid step (BANKNIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: STEP_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, STEP_GAP),
        later: [
            Some(DatedRow::verified(
                STEP_VERIFIED_FROM,
                BANKNIFTY_STEP,
                "BANKNIFTY strikes step 100 rupees = 10,000 paisa — NSE F&O master by way of \
                 `brutex/options/strike_grid.py` `STRIKE_STEP`; TRACK2_OPTIONS_SPEC §5, verified \
                 quarterly, no SEBI step change recorded over 2021-2026.",
            )),
            None,
            None,
            None,
            None,
        ],
    },
];

// ---------------------------------------------------------------------------
// COMPILE-TIME structural proof.
//
// A table that anchors later than TradeDay::MIN, does not ascend, has a hole,
// or carries a blank citation does not compile. And slot 0 must be the
// underlying `core` names first: if `InstrumentKey::SWEPT` is ever reordered,
// the two steps would swap silently, which is a 2x position-sizing error with
// nothing to see. It stops compiling instead.
// ---------------------------------------------------------------------------
const _: () = assert!(STRIKE_STEPS[0].is_shipping_shape());
const _: () = assert!(STRIKE_STEPS[1].is_shipping_shape());
const _: () = assert!(crate::dated::str_eq(InstrumentKey::SWEPT[0].1, "NIFTY"));
const _: () = assert!(crate::dated::str_eq(InstrumentKey::SWEPT[1].1, "BANKNIFTY"));

/// The strike-grid step in force for `slot` on `day`.
///
/// # Errors
///
/// [`Refusal`] for a day before [`STEP_VERIFIED_FROM`]. The refusal names the
/// window, the citation gap and the remedy. There is no default step and no way
/// to configure one — see `dated`.
///
/// # Examples
///
/// ```
/// use brutex_core::symbol::Symbol;
/// use costs::day::TradeDay;
/// use costs::strike::strike_step_on;
/// use costs::venue::swept_slot;
///
/// let nifty = swept_slot(Symbol::new("NIFTY")?)?;
/// let step = strike_step_on(nifty, TradeDay::new(2024, 10, 1)?)?;
/// assert_eq!(step.raw(), 5_000, "50 rupees, in paisa");
///
/// // Before the verified window there is a refusal, never a guess.
/// assert!(strike_step_on(nifty, TradeDay::new(2020, 12, 31)?).is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
// The slot's inner index is produced only by `venue::swept_slot`, which returns
// an index into `InstrumentKey::SWEPT`, and the array above has exactly that
// length by its own type. The access is in bounds by construction.
#[allow(clippy::indexing_slicing)]
pub fn strike_step_on(slot: SweptSlot, day: TradeDay) -> Result<StrikeStep, Refusal> {
    STRIKE_STEPS[slot.index()].value_on(day)
}

/// The rung of the grid nearest `spot` — the at-the-money strike.
///
/// Exact integer half-up rounding onto the progression, with ties going to the
/// **higher** rung. See the module documentation for the identity and for why
/// there is no float on the path.
///
/// # Errors
///
/// * [`CostError::NotPositive`] when `spot` is zero or negative. A spot is a
///   traded index level and cannot be either; refusing names which input was
///   wrong instead of returning a strike of nothing.
/// * [`CostError::NotPositive`] when the nearest rung is **rung zero** — a spot
///   below half a step, which is below the lowest strike the grid can carry. An
///   option struck at nothing is not a contract.
/// * [`CostError::Overflow`] when the rung is past `i64`. Refused, never
///   wrapped: a wrapped strike is a negative price.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::strike::{at_the_money, StrikeStep};
/// # use costs::day::TradeDay;
/// # use costs::venue::swept_slot;
/// # use brutex_core::symbol::Symbol;
/// let step = costs::strike::strike_step_on(
///     swept_slot(Symbol::new("NIFTY")?)?,
///     TradeDay::new(2024, 10, 1)?,
/// )?;
///
/// // 24,012.00 is nearer 24,000 than 24,050.
/// let low = at_the_money(Paisa::from_raw(24_012_00), step)?;
/// assert_eq!(low.raw(), 24_000_00);
///
/// // 24,025.00 is exactly halfway, and a tie goes up.
/// let tie = at_the_money(Paisa::from_raw(24_025_00), step)?;
/// assert_eq!(tie.raw(), 24_050_00);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn at_the_money(spot: Paisa, step: StrikeStep) -> Result<Paisa, CostError> {
    let spot = spot.raw();
    if spot <= 0 {
        return Err(CostError::NotPositive {
            quantity: "spot",
            value: spot,
        });
    }
    let (spot, step) = (i128::from(spot), i128::from(step.raw()));
    // floor((2·spot + step) / (2·step)) · step — the exact integer twin of
    // floor((spot + step/2) / step) · step, valid for an odd step too. Both
    // operands are positive, so `div_euclid` and floor division agree.
    let rung = (2 * spot + step).div_euclid(2 * step);
    let strike = rung * step;
    into_strike(strike, "at-the-money strike")
}

/// The strike a moneyness names, counted in steps from the at-the-money rung.
///
/// This is the source's `resolve_offset_strike` law
/// (`brutex/options/strike_rules.py`), which fixes the direction convention
/// once: **"plus" always means further out of the money in the trade
/// direction**. For a call, further out of the money is a higher strike; for a
/// put it is a lower one. So the sign flips with the side and never with the
/// caller's mood:
///
/// | Side | `OTM+N` | `ITM-N` |
/// |---|---|---|
/// | Call | `atm + N × step` | `atm − N × step` |
/// | Put | `atm − N × step` | `atm + N × step` |
///
/// `atm` is expected to be on the grid — it is what [`at_the_money`] returned.
/// Nothing is re-rounded here: the snap happens once, at that boundary.
///
/// # Errors
///
/// * [`CostError::NotPositive`] when `atm` is zero or negative, or when the
///   resolved strike is. Walking far enough into the money on a call runs off
///   the bottom of the grid, and a strike of nothing is not a contract.
/// * [`CostError::Overflow`] when the arithmetic leaves `i64`.
///
/// # Examples
///
/// ```
/// use brutex_core::instrument::OptionSide;
/// use brutex_core::price::Paisa;
/// use costs::moneyness::MoneynessSteps;
/// use costs::strike::strike_at;
/// # use costs::day::TradeDay;
/// # use costs::venue::swept_slot;
/// # use brutex_core::symbol::Symbol;
/// let step = costs::strike::strike_step_on(
///     swept_slot(Symbol::new("NIFTY")?)?,
///     TradeDay::new(2024, 10, 1)?,
/// )?;
/// let atm = Paisa::from_raw(24_000_00);
///
/// // Two steps out of the money is higher for a call and lower for a put.
/// let two_otm = MoneynessSteps::new(2);
/// assert_eq!(strike_at(atm, two_otm, step, OptionSide::Call)?.raw(), 24_100_00);
/// assert_eq!(strike_at(atm, two_otm, step, OptionSide::Put)?.raw(), 23_900_00);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn strike_at(
    atm: Paisa,
    moneyness: MoneynessSteps,
    step: StrikeStep,
    side: OptionSide,
) -> Result<Paisa, CostError> {
    if atm.raw() <= 0 {
        return Err(CostError::NotPositive {
            quantity: "at-the-money strike",
            value: atm.raw(),
        });
    }
    // i128 throughout: `-i32::MIN` has no i32, and `steps × step` has no i64 at
    // the extremes. Neither can overflow here, and the one narrowing is checked.
    let steps = i128::from(moneyness.steps());
    let rungs = match side {
        // A call gets more out of the money as the strike rises.
        OptionSide::Call => steps,
        // A put gets more out of the money as the strike falls.
        OptionSide::Put => -steps,
    };
    let strike = i128::from(atm.raw()) + rungs * i128::from(step.raw());
    into_strike(strike, "resolved strike")
}

/// Narrows a computed strike back to paisa, refusing rather than wrapping.
fn into_strike(strike: i128, what: &'static str) -> Result<Paisa, CostError> {
    let strike = i64::try_from(strike).map_err(|_| CostError::Overflow { operation: what })?;
    if strike <= 0 {
        return Err(CostError::NotPositive {
            quantity: what,
            value: strike,
        });
    }
    Ok(Paisa::from_raw(strike))
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

    use crate::venue::swept_slot;

    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    fn slot(text: &str) -> SweptSlot {
        swept_slot(Symbol::new(text).expect("a valid symbol")).expect("a swept underlying")
    }

    #[test]
    fn the_two_swept_underlyings_carry_the_two_published_steps() {
        // The source's STRIKE_STEP table, in paisa. NIFTY 50, BANKNIFTY 100 —
        // and they are DIFFERENT, which is the thing a single hardcoded step
        // would get wrong for one of the two on every trade.
        let on = day(2024, 10, 1);
        assert_eq!(strike_step_on(slot("NIFTY"), on), Ok(NIFTY_STEP));
        assert_eq!(
            strike_step_on(slot("NIFTY"), on).map(StrikeStep::raw),
            Ok(5_000)
        );
        assert_eq!(strike_step_on(slot("BANKNIFTY"), on), Ok(BANKNIFTY_STEP));
        assert_eq!(
            strike_step_on(slot("BANKNIFTY"), on).map(StrikeStep::raw),
            Ok(10_000)
        );
        assert_ne!(NIFTY_STEP, BANKNIFTY_STEP);
        assert_eq!(BANKNIFTY_STEP.raw(), 2 * NIFTY_STEP.raw());
        // The step is a price, so it reads as rupees.
        assert_eq!(NIFTY_STEP.to_string(), "50.00");
        assert_eq!(BANKNIFTY_STEP.to_string(), "100.00");
        assert_eq!(NIFTY_STEP.paisa(), Paisa::from_raw(5_000));
        assert_eq!(StrikeStep::new_const(5).to_string(), "0.05");
    }

    #[test]
    fn the_step_is_refused_before_the_window_the_source_verified() {
        let boundary = day(2021, 1, 1);
        assert_eq!(
            STEP_VERIFIED_FROM, boundary,
            "the boundary date is the date"
        );
        for slot_name in ["NIFTY", "BANKNIFTY"] {
            let subject = slot(slot_name);
            assert!(strike_step_on(subject, boundary).is_ok());
            let refusal = strike_step_on(subject, day(2020, 12, 31))
                .expect_err("the source verifies nothing earlier");
            assert_eq!(refusal.window_start(), TradeDay::MIN);
            assert_eq!(refusal.verified_from(), Some(boundary));
            assert_eq!(refusal.remediation(), STEP_REMEDIATION);
            assert!(refusal.to_string().contains("UNVERIFIED"));
            assert!(refusal.to_string().contains(slot_name));
            assert!(strike_step_on(subject, TradeDay::MIN).is_err());
            // And every day of the verified window answers.
            assert!(strike_step_on(subject, TradeDay::MAX).is_ok());
        }
    }

    #[test]
    fn the_step_tables_are_keyed_on_cores_order_and_every_row_is_positive() {
        assert_eq!(STRIKE_STEPS.len(), SweptSlot::COUNT);
        assert_eq!(STRIKE_STEPS.len(), 2);
        for (index, table) in STRIKE_STEPS.iter().enumerate() {
            assert!(table.is_shipping_shape(), "table {index}");
            assert!(
                table.subject.contains(InstrumentKey::SWEPT[index].1),
                "table {index} names the wrong underlying: {}",
                table.subject
            );
            for row in table.rows() {
                if let crate::dated::Dated::Verified(step) = row.value {
                    let raw = step.raw();
                    assert!(raw > 0, "a step of {raw} is not a grid");
                    assert_eq!(raw % 100, 0, "the published steps are whole rupees");
                }
            }
        }
    }

    #[test]
    fn a_spot_exactly_halfway_between_two_rungs_rounds_up() {
        // The source's own documented pin: round_to_atm(24_025.0, NIFTY) is
        // 24_050, not 24_000. This is the assertion that would fail if the
        // rounding were half-down, or banker's, or half-away-from-zero applied
        // to a shifted quotient.
        assert_eq!(
            at_the_money(Paisa::from_raw(24_025_00), NIFTY_STEP),
            Ok(Paisa::from_raw(24_050_00))
        );
        // One paisa either side of the tie, which kills an off-by-one.
        assert_eq!(
            at_the_money(Paisa::from_raw(24_025_00 - 1), NIFTY_STEP),
            Ok(Paisa::from_raw(24_000_00))
        );
        assert_eq!(
            at_the_money(Paisa::from_raw(24_025_00 + 1), NIFTY_STEP),
            Ok(Paisa::from_raw(24_050_00))
        );
        // The same law on the wider grid: 50,050 ties up to 50,100.
        assert_eq!(
            at_the_money(Paisa::from_raw(50_050_00), BANKNIFTY_STEP),
            Ok(Paisa::from_raw(50_100_00))
        );
        assert_eq!(
            at_the_money(Paisa::from_raw(50_050_00 - 1), BANKNIFTY_STEP),
            Ok(Paisa::from_raw(50_000_00))
        );
    }

    #[test]
    fn the_source_pins_land_on_the_source_answers() {
        // Every pin the predecessor's own test file hardcoded from its
        // float oracle, quantised to paisa. A float chain and an integer one must
        // agree here or the port changed the answer.
        for (spot_rupees, step, want_rupees) in [
            (24_012, NIFTY_STEP, 24_000),
            (24_025, NIFTY_STEP, 24_050), // half-tie UP
            (50_075, BANKNIFTY_STEP, 50_100),
            (24_975, NIFTY_STEP, 25_000),
            (81_250, BANKNIFTY_STEP, 81_300), // half-tie UP
            (81_249, BANKNIFTY_STEP, 81_200),
        ] {
            assert_eq!(
                at_the_money(Paisa::from_raw(spot_rupees * 100), step),
                Ok(Paisa::from_raw(want_rupees * 100)),
                "at_the_money({spot_rupees}, {step})"
            );
        }
    }

    #[test]
    fn the_rung_is_the_nearest_one_for_every_spot_across_two_whole_steps() {
        // A differential against an independently written nearest-multiple
        // search, over every paisa of two whole NIFTY steps: the answer is on
        // the grid, it is nearest, and a tie goes up.
        let step = NIFTY_STEP.raw();
        let base = 24_000_00i64;
        let mut checked = 0u32;
        for spot in (base - step)..=(base + step) {
            let got = at_the_money(Paisa::from_raw(spot), NIFTY_STEP)
                .expect("a positive spot on a positive grid")
                .raw();
            assert_eq!(got % step, 0, "{got} is not on the grid");
            let below = spot.div_euclid(step) * step;
            let above = below + step;
            let want = if spot - below >= above - spot {
                above
            } else {
                below
            };
            assert_eq!(got, want, "spot {spot}");
            checked += 1;
        }
        assert_eq!(checked, 2 * 5_000 + 1);
    }

    #[test]
    fn the_rung_never_moves_backwards_as_the_spot_rises() {
        // Monotonicity. A rounding that broke it would put a higher spot on a
        // lower strike, which no chain does. Starts above half a step, which is
        // where the first real rung begins.
        let mut previous = 0i64;
        for spot in ((BANKNIFTY_STEP.raw() / 2)..=200_000_00).step_by(7_919) {
            let got = at_the_money(Paisa::from_raw(spot), BANKNIFTY_STEP)
                .expect("positive")
                .raw();
            assert!(got >= previous, "spot {spot} moved the rung backwards");
            assert_eq!(got % BANKNIFTY_STEP.raw(), 0);
            previous = got;
        }
        assert!(previous > 0);
    }

    #[test]
    fn a_step_that_does_not_divide_the_spot_evenly_is_still_exact() {
        // An odd step in paisa: the float chain could not halve it exactly and
        // the integer identity can. 7 paisa steps, spot 50 paisa: the rungs are
        // 49 and 56, and 50 is nearer 49.
        let odd = StrikeStep::new_const(7);
        assert_eq!(
            at_the_money(Paisa::from_raw(50), odd),
            Ok(Paisa::from_raw(49))
        );
        // The tie for an odd step falls between paisa and so cannot occur: 52
        // is 3 above 49 and 4 below 56.
        assert_eq!(
            at_the_money(Paisa::from_raw(52), odd),
            Ok(Paisa::from_raw(49))
        );
        assert_eq!(
            at_the_money(Paisa::from_raw(53), odd),
            Ok(Paisa::from_raw(56))
        );
        // A one-paisa grid makes every spot its own strike.
        let unit = StrikeStep::new_const(1);
        for spot in 1i64..=25 {
            assert_eq!(
                at_the_money(Paisa::from_raw(spot), unit),
                Ok(Paisa::from_raw(spot))
            );
        }
        // And a spot that is not a whole rupee still lands on a whole rung.
        assert_eq!(
            at_the_money(Paisa::from_raw(24_012_37), NIFTY_STEP),
            Ok(Paisa::from_raw(24_000_00))
        );
    }

    #[test]
    fn a_spot_below_the_lowest_rung_is_refused_rather_than_struck_at_nothing() {
        // Below half a step — 25 rupees on the 50-rupee grid — the nearest rung
        // is zero, and an option struck at nothing is not a contract. This is
        // "a spot below the lowest representable strike".
        for spot in [1i64, 100, 24_99, 25_00 - 1] {
            assert_eq!(
                at_the_money(Paisa::from_raw(spot), NIFTY_STEP),
                Err(CostError::NotPositive {
                    quantity: "at-the-money strike",
                    value: 0
                }),
                "spot {spot} paisa is below the grid"
            );
        }
        // Exactly half a step ties UP onto the first real rung, so it is the
        // first spot that answers at all.
        assert_eq!(
            at_the_money(Paisa::from_raw(25_00), NIFTY_STEP),
            Ok(Paisa::from_raw(50_00))
        );
        // The wider grid's floor is twice as high, which is the same statement
        // about a different index.
        assert_eq!(
            at_the_money(Paisa::from_raw(50_00 - 1), BANKNIFTY_STEP),
            Err(CostError::NotPositive {
                quantity: "at-the-money strike",
                value: 0
            })
        );
        assert_eq!(
            at_the_money(Paisa::from_raw(50_00), BANKNIFTY_STEP),
            Ok(Paisa::from_raw(100_00))
        );
    }

    #[test]
    fn a_zero_or_negative_spot_is_refused_by_name() {
        for spot in [0i64, -1, -24_000_00, i64::MIN] {
            assert_eq!(
                at_the_money(Paisa::from_raw(spot), NIFTY_STEP),
                Err(CostError::NotPositive {
                    quantity: "spot",
                    value: spot
                }),
                "spot {spot} must be named, not rounded"
            );
        }
    }

    #[test]
    fn a_spot_above_the_highest_representable_rung_is_refused_rather_than_wrapped() {
        // `i64::MAX` is 807 paisa above a multiple of 5,000, so on the NIFTY
        // grid the top spot rounds DOWN and stays representable. That is the
        // answer, and it is asserted as a number rather than assumed.
        assert_eq!(
            at_the_money(Paisa::from_raw(i64::MAX), NIFTY_STEP),
            Ok(Paisa::from_raw(9_223_372_036_854_775_000))
        );
        // On a four-paisa grid it does not: `i64::MAX` is three above a
        // multiple of four, more than half a step, so the tie rounds UP past
        // the top of the domain. Refused — a wrapped strike is a negative
        // price, and a saturated one is a strike nobody listed.
        assert_eq!(
            at_the_money(Paisa::from_raw(i64::MAX), StrikeStep::new_const(4)),
            Err(CostError::Overflow {
                operation: "at-the-money strike"
            })
        );
        // One paisa lower is the exact half-step tie, which also rounds UP and
        // also leaves the domain — the tie rule does not soften at the edge.
        assert_eq!(
            at_the_money(Paisa::from_raw(i64::MAX - 1), StrikeStep::new_const(4)),
            Err(CostError::Overflow {
                operation: "at-the-money strike"
            })
        );
        // Two lower is below the tie and rounds down, so the refusal is the
        // boundary and not the neighbourhood.
        assert_eq!(
            at_the_money(Paisa::from_raw(i64::MAX - 2), StrikeStep::new_const(4)),
            Ok(Paisa::from_raw(i64::MAX - 3))
        );
        // A step wide enough to swallow the whole domain still answers.
        assert!(at_the_money(Paisa::from_raw(i64::MAX / 2), NIFTY_STEP).is_ok());
        assert!(at_the_money(Paisa::from_raw(i64::MAX), StrikeStep::new_const(i64::MAX)).is_ok());
    }

    #[test]
    fn plus_is_further_out_of_the_money_in_the_trade_direction() {
        let atm = Paisa::from_raw(24_000_00);
        // The source's own table, both directions.
        for (steps, call_rupees, put_rupees) in [
            (0i32, 24_000, 24_000),
            (1, 24_050, 23_950),
            (2, 24_100, 23_900),
            (5, 24_250, 23_750),
            (-1, 23_950, 24_050),
            (-2, 23_900, 24_100),
            (-5, 23_750, 24_250),
        ] {
            let moneyness = MoneynessSteps::new(steps);
            assert_eq!(
                strike_at(atm, moneyness, NIFTY_STEP, OptionSide::Call),
                Ok(Paisa::from_raw(call_rupees * 100)),
                "call at {moneyness}"
            );
            assert_eq!(
                strike_at(atm, moneyness, NIFTY_STEP, OptionSide::Put),
                Ok(Paisa::from_raw(put_rupees * 100)),
                "put at {moneyness}"
            );
        }
        // The predecessor's own resolve_offset_strike pins, on the wider grid.
        assert_eq!(
            strike_at(
                Paisa::from_raw(50_000_00),
                MoneynessSteps::new(-5),
                BANKNIFTY_STEP,
                OptionSide::Call
            ),
            Ok(Paisa::from_raw(49_500_00))
        );
        assert_eq!(
            strike_at(
                Paisa::from_raw(50_000_00),
                MoneynessSteps::new(-5),
                BANKNIFTY_STEP,
                OptionSide::Put
            ),
            Ok(Paisa::from_raw(50_500_00))
        );
    }

    #[test]
    fn the_two_sides_are_exact_mirrors_of_each_other_about_the_rung() {
        // For every moneyness the call and the put sit the same distance from
        // ATM on opposite sides. This is the whole content of the direction
        // law, stated as an equation rather than a table.
        let atm = 24_000_00i64;
        for steps in -400i32..=400 {
            let moneyness = MoneynessSteps::new(steps);
            let call = strike_at(
                Paisa::from_raw(atm),
                moneyness,
                NIFTY_STEP,
                OptionSide::Call,
            )
            .expect("within the grid")
            .raw();
            let put = strike_at(Paisa::from_raw(atm), moneyness, NIFTY_STEP, OptionSide::Put)
                .expect("within the grid")
                .raw();
            assert_eq!(call - atm, atm - put, "{moneyness} is not mirrored");
            assert_eq!(call % NIFTY_STEP.raw(), 0);
            assert_eq!(put % NIFTY_STEP.raw(), 0);
        }
    }

    #[test]
    fn a_moneyness_that_walks_off_the_grid_is_refused_rather_than_wrapped() {
        let atm = Paisa::from_raw(24_000_00);
        // Deep enough into the money on a call and the strike goes to zero and
        // then negative. 24,000 / 50 = 480 rungs to reach zero.
        assert_eq!(
            strike_at(atm, MoneynessSteps::new(-480), NIFTY_STEP, OptionSide::Call),
            Err(CostError::NotPositive {
                quantity: "resolved strike",
                value: 0
            })
        );
        assert_eq!(
            strike_at(atm, MoneynessSteps::new(-481), NIFTY_STEP, OptionSide::Call),
            Err(CostError::NotPositive {
                quantity: "resolved strike",
                value: -5_000
            })
        );
        assert!(strike_at(atm, MoneynessSteps::new(-479), NIFTY_STEP, OptionSide::Call).is_ok());
        // The put mirror walks off the same edge in the other direction.
        assert_eq!(
            strike_at(atm, MoneynessSteps::new(480), NIFTY_STEP, OptionSide::Put),
            Err(CostError::NotPositive {
                quantity: "resolved strike",
                value: 0
            })
        );
        // And past i64 in the other direction, on both sides, including the
        // i32::MIN step count whose negation has no i32.
        let huge = StrikeStep::new_const(i64::MAX / 2);
        for side in [OptionSide::Call, OptionSide::Put] {
            assert_eq!(
                strike_at(atm, MoneynessSteps::new(i32::MAX), huge, side),
                Err(CostError::Overflow {
                    operation: "resolved strike"
                }),
                "{side:?} at the top"
            );
            assert_eq!(
                strike_at(atm, MoneynessSteps::new(i32::MIN), huge, side),
                Err(CostError::Overflow {
                    operation: "resolved strike"
                }),
                "{side:?} at the bottom, where -i32::MIN has no i32"
            );
        }
    }

    #[test]
    fn a_non_positive_at_the_money_rung_is_refused_before_any_arithmetic() {
        for atm in [0i64, -1, i64::MIN] {
            assert_eq!(
                strike_at(
                    Paisa::from_raw(atm),
                    MoneynessSteps::new(0),
                    NIFTY_STEP,
                    OptionSide::Call
                ),
                Err(CostError::NotPositive {
                    quantity: "at-the-money strike",
                    value: atm
                })
            );
        }
    }

    #[test]
    fn the_snap_happens_once_and_a_second_pass_changes_nothing() {
        // Idempotence: a rung is already on the grid, so rounding it again is
        // the identity. If the snap were applied twice with different rules,
        // this is where it would show.
        for spot in (5_000_00i64..=30_000_00).step_by(1_237) {
            let once = at_the_money(Paisa::from_raw(spot), NIFTY_STEP).expect("positive");
            let twice = at_the_money(once, NIFTY_STEP).expect("a rung is positive");
            assert_eq!(once, twice, "spot {spot} moved on the second snap");
            // And a resolved strike is on the grid too, so it is also fixed.
            let moved = strike_at(once, MoneynessSteps::new(3), NIFTY_STEP, OptionSide::Call)
                .expect("within the grid");
            assert_eq!(at_the_money(moved, NIFTY_STEP), Ok(moved));
        }
    }

    #[test]
    fn a_strike_step_is_ordered_hashable_and_debuggable() {
        use std::collections::HashSet;

        assert!(NIFTY_STEP < BANKNIFTY_STEP);
        assert_eq!(format!("{NIFTY_STEP:?}"), "StrikeStep(5000)");
        let mut set = HashSet::new();
        assert!(set.insert(NIFTY_STEP));
        assert!(!set.insert(StrikeStep::new_const(5_000)));
        assert!(set.insert(BANKNIFTY_STEP));
        assert_eq!(set.len(), 2);
    }
}
