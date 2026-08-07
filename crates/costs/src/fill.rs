//! The fill law: which price each leg of a round trip is filled at.
//!
//! There is **one** fill model and it is the worst case. The predecessor's
//! `DEC-FILL-WORST-CASE-ONLY-001` deleted the alternative ("precise", anchored
//! on the bar open) along with the flag that selected it, and this port carries
//! that across: there is no mode, no parameter and no second body.
//!
//! # The law, in four lines
//!
//! Each leg anchors on the **adverse extreme** of its own fill bar — a buy on
//! the bar HIGH, a sell on the bar LOW — and then pays one further adverse
//! [`TICK`]:
//!
//! | Direction | Buy leg anchors on | Sell leg anchors on |
//! |---|---|---|
//! | [`Direction::Long`] | the **entry** bar's high | the **exit** bar's low |
//! | [`Direction::Short`] | the **exit** bar's high (the cover) | the **entry** bar's low (the opening sell) |
//!
//! Both legs adverse, on both directions. The short arm is the predecessor's
//! reconciled `CALCULATOR_SPEC` §8 rule, which superseded an earlier pin that
//! had the signs the other way round.
//!
//! # Why the buy fill deliberately prices outside the printed range
//!
//! On a flat bar (`high == low == open`) a bare buy at the HIGH would be one
//! tick *better* than the retired open-anchored fill. Keeping the tick on top
//! of the extreme makes the two exactly equal there, so the worst-case fill can
//! never flatter the retired one on any bar. It is an honest adverse floor, not
//! a price that printed.
//!
//! # The sell floor
//!
//! A sell fill is floored at one [`TICK`]: nothing trades below ₹0.05, and a
//! sell leg anchored on a sub-two-tick bar low would otherwise fill at ₹0.00 —
//! a free sale, which is not a conservative assumption but an impossible one.
//! The floor changes the **realized** slippage too: a leg that could not move
//! the full tick reports the movement it actually made, so the informational
//! slippage line never overstates what the notionals carry.
//!
//! The **buy** fill needs no floor, and does not have one. [`Bar::new`] refuses
//! a bar whose high is below one tick, so `high + TICK` is at least two ticks
//! on every bar that can be constructed. A floor there would be a branch no
//! input could take.
//!
//! # Constant per-operation cost
//!
//! One selection, two additions, one comparison, two differences and three
//! narrowings. No loop, and nothing that depends on how far apart the two bars
//! are.

use brutex_core::price::Paisa;

use crate::error::CostError;
use crate::money::narrow;
use crate::rate::TICK;

/// Which way round the trip goes.
///
/// It selects which bar each leg anchors on, and — for an option — the sign of
/// the gross profit and loss. A trip priced with the wrong direction is not
/// slightly wrong; it is filled off the wrong two bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    /// Buy to open, sell to close.
    Long,
    /// Sell to open, buy to close.
    Short,
}

impl Direction {
    /// The direction's name, for a refusal or a breakdown line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

impl core::fmt::Display for Direction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One minute bar's two adverse anchors.
///
/// Only the high and the low are carried, because only they are read. The
/// predecessor's input type also carries the open, and uses it **solely** to
/// validate that the extremes bracket it; that validation is expressed here as
/// `low <= high`, which is what it implies and all that the arithmetic needs. A
/// field that never enters a computation is a field that can drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bar {
    high: Paisa,
    low: Paisa,
}

impl Bar {
    /// A bar from its two extremes.
    ///
    /// # Errors
    ///
    /// * [`CostError::BelowTick`] when the high is under one [`TICK`]. A bar
    ///   whose highest print is below ₹0.05 never printed: the grid does not
    ///   have a rung there.
    /// * [`CostError::InvertedBar`] when the low is above the high. That is not
    ///   a wide bar or a thin one, it is two numbers in the wrong order, and
    ///   swapping them silently would fill both legs off the wrong anchors.
    ///
    /// A **low** below one tick, or below zero, is deliberately allowed. The
    /// predecessor is explicit that a degenerate low is absorbed by the sell
    /// floor rather than refused, so that a single malformed bar in a backtest
    /// is a conservative fill and not a crash.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::price::Paisa;
    /// use costs::fill::Bar;
    ///
    /// let bar = Bar::new(Paisa::from_raw(120_50), Paisa::from_raw(119_00))?;
    /// assert_eq!(bar.high().raw(), 120_50);
    /// assert_eq!(bar.low().raw(), 119_00);
    /// # Ok::<(), costs::error::CostError>(())
    /// ```
    pub fn new(high: Paisa, low: Paisa) -> Result<Self, CostError> {
        if high.raw() < TICK.raw() {
            return Err(CostError::BelowTick {
                quantity: "bar high",
                value: high.raw(),
            });
        }
        if low.raw() > high.raw() {
            return Err(CostError::InvertedBar {
                high: high.raw(),
                low: low.raw(),
            });
        }
        Ok(Self { high, low })
    }

    /// A bar that did not move: high, low and open all the same price.
    ///
    /// The worked examples in the predecessor's `COSTS_VERIFIED` §5 quote a
    /// single price per leg, so this is the shape that reproduces them — and on
    /// a flat bar the worst-case anchor coincides with the open exactly.
    ///
    /// # Errors
    ///
    /// [`CostError::BelowTick`] when the price is under one [`TICK`]. The
    /// inversion arm cannot fire: a price is never above itself.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::price::Paisa;
    /// use costs::fill::Bar;
    ///
    /// let bar = Bar::flat(Paisa::from_raw(100_00))?;
    /// assert_eq!(bar.high(), bar.low());
    /// # Ok::<(), costs::error::CostError>(())
    /// ```
    pub fn flat(price: Paisa) -> Result<Self, CostError> {
        Self::new(price, price)
    }

    /// The bar's highest print — the adverse anchor for a buy.
    #[must_use]
    pub const fn high(self) -> Paisa {
        self.high
    }

    /// The bar's lowest print — the adverse anchor for a sell.
    #[must_use]
    pub const fn low(self) -> Paisa {
        self.low
    }
}

/// The two fills of a round trip, and the slippage actually baked into them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fills {
    buy: Paisa,
    sell: Paisa,
    realized_slip_per_unit: Paisa,
}

impl Fills {
    /// The price the buy leg filled at.
    #[must_use]
    pub const fn buy(self) -> Paisa {
        self.buy
    }

    /// The price the sell leg filled at.
    #[must_use]
    pub const fn sell(self) -> Paisa {
        self.sell
    }

    /// The adverse movement **per unit** truly baked into the two fills.
    ///
    /// Two ticks on the ordinary path. Less when the sell floor bound, because
    /// a leg that could not move the full tick did not move it — and more when
    /// the sell anchor sat below the floor, because the floor moved the fill up
    /// past where the bar printed. It is an informational line: it is not a
    /// charge and it is not subtracted from anything, because it is already
    /// inside the fills and therefore inside the gross.
    #[must_use]
    pub const fn realized_slip_per_unit(self) -> Paisa {
        self.realized_slip_per_unit
    }
}

/// The worst-case fills for one round trip.
///
/// # Errors
///
/// [`CostError::Overflow`] when a fill or the realized slippage leaves `i64`.
/// Refused, never saturated: a saturated fill is a price nobody traded at.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::fill::{Bar, Direction, worst_case_fills};
///
/// // `COSTS_VERIFIED` §5 Ex.1: flat bars at ₹100.00 and ₹120.00.
/// let fills = worst_case_fills(
///     Bar::flat(Paisa::from_raw(100_00))?,
///     Bar::flat(Paisa::from_raw(120_00))?,
///     Direction::Long,
/// )?;
/// assert_eq!(fills.buy().raw(), 100_05);
/// assert_eq!(fills.sell().raw(), 119_95);
/// assert_eq!(fills.realized_slip_per_unit().raw(), 10);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn worst_case_fills(entry: Bar, exit: Bar, direction: Direction) -> Result<Fills, CostError> {
    let tick = i128::from(TICK.raw());
    let (buy_anchor, sell_anchor) = match direction {
        // The buy opens on the entry bar, the sell closes on the exit bar.
        Direction::Long => (entry.high.raw(), exit.low.raw()),
        // The opening SELL is on the entry bar and the covering BUY on the
        // exit bar — adverse on both legs, which is the reconciled §8 rule.
        Direction::Short => (exit.high.raw(), entry.low.raw()),
    };

    // No floor on the buy: `Bar::new` refuses a high below one tick, so this is
    // at least two ticks on every constructible bar. A `.max(tick)` here would
    // be a branch no input could take. The addition is done at `i128` width and
    // narrowed once, because an anchor at the `i64` edge plus a tick does not
    // fit and must be refused rather than wrapped.
    let buy = narrow(i128::from(buy_anchor) + tick, "the buy fill")?;

    // The sell fill is computed at `i64` width on purpose. Subtracting a tick
    // can only leave `i64` downward, and only from `i64::MIN` — which is far
    // below the floor, so the floor answers for it. There is therefore no
    // narrowing here that could fail, and no dead arm pretending there is.
    let sell_fill = match sell_anchor.checked_sub(TICK.raw()) {
        Some(slipped) => slipped.max(TICK.raw()),
        None => TICK.raw(),
    };

    // The buy leg's contribution is exactly one tick, because it has no floor
    // to shorten it. The sell leg's is whatever the floor left of its tick —
    // zero when the anchor sat exactly on the floor, more than a tick when the
    // anchor sat below it and the fill had to be pushed up to reach it.
    let realized = tick + (i128::from(sell_anchor) - i128::from(sell_fill)).abs();

    Ok(Fills {
        buy,
        sell: Paisa::from_raw(sell_fill),
        realized_slip_per_unit: narrow(realized, "the realized slippage per unit")?,
    })
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

    fn bar(high: i64, low: i64) -> Bar {
        Bar::new(Paisa::from_raw(high), Paisa::from_raw(low)).expect("a legal bar")
    }

    fn flat(price: i64) -> Bar {
        Bar::flat(Paisa::from_raw(price)).expect("a legal bar")
    }

    fn triple(entry: Bar, exit: Bar, direction: Direction) -> (i64, i64, i64) {
        let fills = worst_case_fills(entry, exit, direction).expect("in range");
        (
            fills.buy().raw(),
            fills.sell().raw(),
            fills.realized_slip_per_unit().raw(),
        )
    }

    #[test]
    fn a_long_fills_the_entry_high_and_the_exit_low_each_one_tick_adverse() {
        // The predecessor's pinned flat-bar family.
        assert_eq!(
            triple(flat(10_000), flat(12_000), Direction::Long),
            (10_005, 11_995, 10)
        );
        // A zero-move trade still pays both ticks.
        assert_eq!(
            triple(flat(10_000), flat(10_000), Direction::Long),
            (10_005, 9_995, 10)
        );
        // Ranged bars anchor the EXTREMES, not the midpoints: the buy takes the
        // entry high and the sell the exit low.
        assert_eq!(
            triple(bar(10_100, 9_900), bar(12_050, 11_950), Direction::Long),
            (10_105, 11_945, 10)
        );
        assert_eq!(
            triple(bar(12_100, 11_900), bar(15_050, 14_900), Direction::Long),
            (12_105, 14_895, 10)
        );
    }

    #[test]
    fn a_short_fills_the_exit_high_and_the_entry_low_and_is_adverse_on_both_legs() {
        // The cover BUYS on the exit bar's high; the opening SELL is on the
        // entry bar's low. Both adverse — the reconciled rule, not the mirror
        // of the long one.
        assert_eq!(
            triple(flat(10_000), flat(12_000), Direction::Short),
            (12_005, 9_995, 10)
        );
        assert_eq!(
            triple(bar(10_100, 9_900), bar(12_050, 11_950), Direction::Short),
            (12_055, 9_895, 10)
        );
        // The two directions read different anchors off the same two bars, so
        // swapping the direction is never a no-op.
        let entry = bar(10_100, 9_900);
        let exit = bar(12_050, 11_950);
        assert_ne!(
            triple(entry, exit, Direction::Long),
            triple(entry, exit, Direction::Short)
        );
    }

    #[test]
    fn the_sell_floor_binds_at_one_tick_and_the_realized_slippage_follows_it() {
        // A one-tick exit low would sell at nothing without the floor. With it
        // the sell leg truly moved zero, so the realized slippage is one tick,
        // not two.
        assert_eq!(triple(flat(100), flat(5), Direction::Long), (105, 5, 5));
        // The smallest legal bar on both sides.
        assert_eq!(triple(flat(5), flat(5), Direction::Long), (10, 5, 5));
        // A sub-two-tick sell anchor: the fill floors to 5 and the leg moved
        // 2, so the realized slippage is 7 — a band, not a two-point set.
        assert_eq!(
            triple(flat(10_000), bar(10_000, 7), Direction::Long),
            (10_005, 5, 7)
        );
        // A sell anchor BELOW the floor: the fill is pushed up to one tick and
        // the recorded movement is the distance it was pushed.
        assert_eq!(
            triple(flat(10_000), bar(10_000, -95), Direction::Long),
            (10_005, 5, 105)
        );
        // A short's opening sell floors the same way, off the ENTRY low.
        assert_eq!(triple(flat(5), flat(100), Direction::Short), (105, 5, 5));
    }

    #[test]
    fn the_buy_leg_needs_no_floor_because_no_legal_bar_can_reach_it() {
        // The smallest constructible high is one tick, so the smallest possible
        // buy fill is two ticks. There is no bar for which a floor at one tick
        // would change the answer, which is why there is no floor.
        let mut checked = 0_u32;
        for high in TICK.raw()..=TICK.raw() + 200 {
            for low in [i64::MIN, -1_000, 0, TICK.raw(), high] {
                let entry = bar(high, low.min(high));
                let (buy, _, _) = triple(entry, flat(10_000), Direction::Long);
                assert_eq!(buy, high + TICK.raw(), "the buy fill is anchor plus a tick");
                assert!(buy >= 2 * TICK.raw(), "and never below two ticks");
                checked += 1;
            }
        }
        assert_eq!(checked, 201 * 5);
    }

    #[test]
    fn the_worst_case_fill_never_flatters_an_open_anchored_one() {
        // For any open inside the bar, the extreme-anchored buy is never below
        // the open-anchored buy and the sell never above it. Checked at both
        // bracket ends of a grid of bars, which is where the inequality is
        // tightest.
        let mut checked = 0_u32;
        for entry_high in [5_i64, 100, 10_000] {
            for entry_low in [5_i64, 60, 9_000] {
                for exit_high in [5_i64, 100, 10_000] {
                    for exit_low in [5_i64, 60, 9_000] {
                        let entry = bar(entry_high.max(entry_low), entry_low.min(entry_high));
                        let exit = bar(exit_high.max(exit_low), exit_low.min(exit_high));
                        for direction in [Direction::Long, Direction::Short] {
                            let (buy, sell, _) = triple(entry, exit, direction);
                            for (entry_open, exit_open) in
                                [(entry.low(), exit.low()), (entry.high(), exit.high())]
                            {
                                let flat_entry = Bar::flat(entry_open).expect("legal");
                                let flat_exit = Bar::flat(exit_open).expect("legal");
                                let (open_buy, open_sell, _) =
                                    triple(flat_entry, flat_exit, direction);
                                assert!(buy >= open_buy, "the buy flattered");
                                assert!(sell <= open_sell, "the sell flattered");
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 3 * 3 * 3 * 3 * 2 * 2);
    }

    #[test]
    fn a_bar_whose_high_is_below_one_tick_is_refused_by_name() {
        assert_eq!(
            Bar::new(Paisa::from_raw(4), Paisa::from_raw(1)),
            Err(CostError::BelowTick {
                quantity: "bar high",
                value: 4
            })
        );
        assert_eq!(
            Bar::flat(Paisa::ZERO),
            Err(CostError::BelowTick {
                quantity: "bar high",
                value: 0
            })
        );
        assert_eq!(
            Bar::new(Paisa::from_raw(i64::MIN), Paisa::from_raw(i64::MIN)),
            Err(CostError::BelowTick {
                quantity: "bar high",
                value: i64::MIN
            })
        );
        // Exactly one tick is legal — the boundary is inclusive.
        assert_eq!(Bar::flat(TICK).expect("legal").high(), TICK);
    }

    #[test]
    fn a_bar_whose_low_is_above_its_high_is_refused_rather_than_swapped() {
        assert_eq!(
            Bar::new(Paisa::from_raw(100), Paisa::from_raw(101)),
            Err(CostError::InvertedBar {
                high: 100,
                low: 101
            })
        );
        // Equal is not inverted.
        let touching = Bar::new(Paisa::from_raw(100), Paisa::from_raw(100)).expect("legal");
        assert_eq!(touching.high(), touching.low());
        // A low below zero is NOT refused: the sell floor absorbs it.
        let degenerate = Bar::new(Paisa::from_raw(100), Paisa::from_raw(-5_000)).expect("legal");
        assert_eq!(degenerate.low().raw(), -5_000);
    }

    #[test]
    fn a_fill_or_a_slippage_past_i64_is_refused_by_name_and_never_wrapped() {
        // The buy anchor at the i64 edge: adding the tick leaves i64.
        assert_eq!(
            worst_case_fills(bar(i64::MAX, 10), flat(10), Direction::Long),
            Err(CostError::Overflow {
                operation: "the buy fill"
            })
        );
        // One tick below the edge still computes.
        assert_eq!(
            triple(bar(i64::MAX - TICK.raw(), 10), flat(10), Direction::Long).0,
            i64::MAX
        );
        // A deeply negative sell anchor: the fill floors fine, but the recorded
        // movement is the whole distance and that is what leaves i64.
        assert_eq!(
            worst_case_fills(flat(10), bar(10, i64::MIN), Direction::Long),
            Err(CostError::Overflow {
                operation: "the realized slippage per unit"
            })
        );
        // The short arm reaches the same two refusals off the other two bars.
        assert_eq!(
            worst_case_fills(flat(10), bar(i64::MAX, 10), Direction::Short),
            Err(CostError::Overflow {
                operation: "the buy fill"
            })
        );
        assert_eq!(
            worst_case_fills(bar(10, i64::MIN), flat(10), Direction::Short),
            Err(CostError::Overflow {
                operation: "the realized slippage per unit"
            })
        );
    }

    #[test]
    fn the_sell_fill_is_total_at_both_edges_of_the_domain() {
        // The claim the sell leg's `i64` arithmetic rests on, asserted rather
        // than assumed: an anchor at the top of `i64` slips a tick and stays in
        // range, and an anchor at the very bottom — where the subtraction
        // itself would leave `i64` — lands on the floor.
        assert_eq!(
            triple(flat(10), bar(i64::MAX, i64::MAX), Direction::Long).1,
            i64::MAX - 5
        );
        assert_eq!(
            triple(flat(10), bar(10, i64::MIN + 20), Direction::Long).1,
            5
        );
        // `i64::MIN` exactly: the `checked_sub` arm that has no answer, and the
        // floor stands in for it. The realized slippage is what leaves i64
        // there, not the fill.
        assert_eq!(
            worst_case_fills(flat(10), bar(10, i64::MIN), Direction::Long),
            Err(CostError::Overflow {
                operation: "the realized slippage per unit"
            })
        );
        // And with the entry high large enough that i64::MIN is a legal LOW on
        // the same bar the short reads its opening sell from.
        assert_eq!(
            worst_case_fills(bar(10, i64::MIN), flat(10), Direction::Short),
            Err(CostError::Overflow {
                operation: "the realized slippage per unit"
            })
        );
    }

    #[test]
    fn a_direction_names_itself_and_the_two_are_distinct() {
        assert_eq!(Direction::Long.as_str(), "long");
        assert_eq!(Direction::Short.as_str(), "short");
        assert_eq!(Direction::Long.to_string(), "long");
        assert_eq!(Direction::Short.to_string(), "short");
        assert_ne!(Direction::Long, Direction::Short);
        assert!(Direction::Long < Direction::Short);
        assert_eq!(format!("{:?}", Direction::Short), "Short");
    }

    #[test]
    fn a_bar_and_a_fill_set_are_plain_comparable_data() {
        use std::collections::HashSet;

        let one = bar(100, 90);
        assert_eq!(one, bar(100, 90));
        assert_ne!(one, bar(100, 89));
        assert!(bar(100, 89) < bar(100, 90));
        assert_eq!(
            format!("{:?}", flat(5)),
            "Bar { high: Paisa(5), low: Paisa(5) }"
        );

        let fills = worst_case_fills(one, flat(200), Direction::Long).expect("in range");
        assert_eq!(
            fills,
            worst_case_fills(one, flat(200), Direction::Long).expect("in range")
        );
        let mut set = HashSet::new();
        assert!(set.insert(fills));
        assert!(!set.insert(fills));
        assert_eq!(set.len(), 1);
        assert!(format!("{fills:?}").starts_with("Fills {"));
    }
}
