//! Where a strike sits on the ladder, counted in strike steps rather than in
//! rupees.
//!
//! ```text
//! steps = (strike - at_the_money) / interval
//! ```
//!
//! A step count is the only description of a strike that survives the
//! underlying moving. "Fifty points out" means one thing at 18,000 and another
//! at 26,000; "one step out" means the same thing at both.
//!
//! The ladder is a grid, and a strike that is not on a rung is **refused**
//! rather than rounded onto the nearest one. A strike between two rungs is
//! either a different grid or a bad input, and guessing which is how a
//! mislabelled series enters a result set.
//!
//! # The interval is a caller's fact, not this crate's
//!
//! No strike interval is hardcoded here and none is assumed. The NIFTY and
//! BANKNIFTY strike steps are **UNVERIFIED** in this repository — no source
//! in `docs/00-charter.md` states them — so this crate takes the interval as
//! an argument and refuses a strike that does not sit on the grid it was
//! given. `docs/05-decisions.md` D-0037.

// A step count is a ratio, and a ratio is a statistical value under
// `CLAUDE.md` §7. No paisa reaches this file: the strike arrives as an `f64`
// and leaves as a signed count. See D-0037.
#![allow(clippy::float_arithmetic)]

use std::fmt;

use crate::bsm::{MAX_UNDERLYING, OptionKind, positive_bounded};
use crate::error::GreeksError;

/// The furthest rung this crate will label, in either direction.
pub const MAX_STEPS: i32 = 1_000_000;

/// How far from a whole number the step count may fall before the strike is
/// declared off the grid.
pub const STEP_TOLERANCE: f64 = 1.0e-6;

/// Where the strike stands relative to the money, for the side being priced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Position {
    /// The strike is the at-the-money rung.
    AtTheMoney,
    /// The strike has intrinsic value: below the money for a call, above it
    /// for a put.
    InTheMoney,
    /// The strike has none: above the money for a call, below it for a put.
    OutOfTheMoney,
}

/// A strike's place on the ladder.
///
/// Renders as `ATM`, `ITM-3` or `OTM+2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Moneyness {
    /// `(strike − at_the_money) / interval`, signed, in strike steps. The
    /// sign is about the *strike*, not about the side — it is positive above
    /// the money for a call and for a put alike.
    pub steps: i32,
    /// Which side of the money that is, for the side being priced.
    pub position: Position,
    /// `|steps|`, the number that renders after the `ITM-` or the `OTM+`.
    pub distance: u32,
}

impl Moneyness {
    /// Places `strike` on the ladder whose at-the-money rung is
    /// `at_the_money` and whose rungs are `interval` apart.
    ///
    /// # Errors
    ///
    /// [`GreeksError::NotFinite`], [`GreeksError::NotPositive`] or
    /// [`GreeksError::OutOfRange`] for a malformed strike, at-the-money level
    /// or interval; [`GreeksError::OffGrid`] for a strike that is not a whole
    /// number of steps from the money; and [`GreeksError::OutOfRange`] on
    /// `steps` for one further away than [`MAX_STEPS`].
    ///
    /// ```
    /// use greeks::{Moneyness, OptionKind, Position};
    ///
    /// let two_out = Moneyness::from_ladder(25_950.0, 25_850.0, 50.0, OptionKind::Call)
    ///     .expect("on the grid");
    /// assert_eq!(two_out.position, Position::OutOfTheMoney);
    /// assert_eq!(two_out.to_string(), "OTM+2");
    ///
    /// // The same strike, the other side of the contract.
    /// let two_in = Moneyness::from_ladder(25_950.0, 25_850.0, 50.0, OptionKind::Put)
    ///     .expect("on the grid");
    /// assert_eq!(two_in.to_string(), "ITM-2");
    /// ```
    pub fn from_ladder(
        strike: f64,
        at_the_money: f64,
        interval: f64,
        kind: OptionKind,
    ) -> Result<Self, GreeksError> {
        let strike = positive_bounded(strike, "strike", MAX_UNDERLYING)?;
        let at_the_money = positive_bounded(at_the_money, "at_the_money", MAX_UNDERLYING)?;
        let interval = positive_bounded(interval, "interval", MAX_UNDERLYING)?;

        let exact = (strike - at_the_money) / interval;
        let rounded = exact.round();
        if (exact - rounded).abs() > STEP_TOLERANCE {
            return Err(GreeksError::OffGrid {
                steps: exact,
                tolerance: STEP_TOLERANCE,
            });
        }
        if rounded.abs() > f64::from(MAX_STEPS) {
            return Err(GreeksError::OutOfRange {
                field: "steps",
                value: rounded,
                bound: f64::from(MAX_STEPS),
            });
        }
        // The two checks above are the whole justification for this cast, and
        // they are immediately above it on purpose: `rounded` is a whole
        // number and its magnitude is at most 1e6, so the conversion is exact
        // and cannot truncate. There is no safe `f64 -> i32` in `std` to use
        // instead. `cast_possible_truncation` is denied workspace-wide and
        // stays denied everywhere else.
        #[allow(clippy::cast_possible_truncation)]
        let steps = rounded as i32;

        let position = match (kind, steps.signum()) {
            (_, 0) => Position::AtTheMoney,
            (OptionKind::Call, 1) | (OptionKind::Put, -1) => Position::OutOfTheMoney,
            _ => Position::InTheMoney,
        };
        Ok(Self {
            steps,
            position,
            distance: steps.unsigned_abs(),
        })
    }
}

impl fmt::Display for Moneyness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Position::AtTheMoney => f.write_str("ATM"),
            Position::InTheMoney => write!(f, "ITM-{}", self.distance),
            Position::OutOfTheMoney => write!(f, "OTM+{}", self.distance),
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
    use super::{MAX_STEPS, Moneyness, OptionKind, Position, STEP_TOLERANCE};
    use crate::error::GreeksError;
    use std::collections::HashSet;

    #[test]
    fn a_call_and_a_put_read_the_same_rung_from_opposite_sides() {
        // The step count is a fact about the strike; the position is a fact
        // about the contract. Confusing the two is the defect this pins.
        let cases: [(f64, i32, Position, Position, &str, &str); 5] = [
            (
                25_750.0,
                -2,
                Position::InTheMoney,
                Position::OutOfTheMoney,
                "ITM-2",
                "OTM+2",
            ),
            (
                25_800.0,
                -1,
                Position::InTheMoney,
                Position::OutOfTheMoney,
                "ITM-1",
                "OTM+1",
            ),
            (
                25_850.0,
                0,
                Position::AtTheMoney,
                Position::AtTheMoney,
                "ATM",
                "ATM",
            ),
            (
                25_900.0,
                1,
                Position::OutOfTheMoney,
                Position::InTheMoney,
                "OTM+1",
                "ITM-1",
            ),
            (
                25_950.0,
                2,
                Position::OutOfTheMoney,
                Position::InTheMoney,
                "OTM+2",
                "ITM-2",
            ),
        ];
        for (strike, steps, call_side, put_side, call_text, put_text) in cases {
            let call = Moneyness::from_ladder(strike, 25_850.0, 50.0, OptionKind::Call)
                .expect("on the grid");
            let put = Moneyness::from_ladder(strike, 25_850.0, 50.0, OptionKind::Put)
                .expect("on the grid");
            assert_eq!(call.steps, steps, "at {strike}");
            assert_eq!(put.steps, steps, "at {strike}");
            assert_eq!(call.distance, steps.unsigned_abs());
            assert_eq!(call.position, call_side, "at {strike}");
            assert_eq!(put.position, put_side, "at {strike}");
            assert_eq!(call.to_string(), call_text);
            assert_eq!(put.to_string(), put_text);
        }
    }

    #[test]
    fn a_strike_between_two_rungs_is_refused_and_never_rounded_onto_one() {
        let error = Moneyness::from_ladder(25_875.0, 25_850.0, 50.0, OptionKind::Call).unwrap_err();
        assert_eq!(
            error,
            GreeksError::OffGrid {
                steps: 0.5,
                tolerance: STEP_TOLERANCE
            }
        );
        // And the tolerance is a real width, not a synonym for exactness: a
        // strike a nanopoint off the rung is still on it.
        let nudged = Moneyness::from_ladder(25_900.0 + 1e-9, 25_850.0, 50.0, OptionKind::Call)
            .expect("within the tolerance");
        assert_eq!(nudged.steps, 1);
        // The tolerance is INCLUSIVE, at exactly its own width. Both operands
        // are the same double and the interval is one, so the subtraction and
        // the division are exact and the step count comes out at
        // STEP_TOLERANCE to the bit -- this pins the comparison rather than
        // something near it.
        let step = STEP_TOLERANCE;
        let on_the_edge = Moneyness::from_ladder(step + step, step, 1.0, OptionKind::Call)
            .expect("exactly at the tolerance is still on the grid");
        assert_eq!(on_the_edge.steps, 0);
        assert_eq!(on_the_edge.position, Position::AtTheMoney);
    }

    #[test]
    fn a_rung_beyond_the_bound_is_refused_rather_than_truncated_into_range() {
        let far = 25_850.0 + f64::from(MAX_STEPS) * 50.0 + 50.0;
        let error = Moneyness::from_ladder(far, 25_850.0, 50.0, OptionKind::Call).unwrap_err();
        assert!(
            matches!(error, GreeksError::OutOfRange { field: "steps", .. }),
            "{error}"
        );
        // The last rung inside the bound is still answered.
        let edge = 25_850.0 + f64::from(MAX_STEPS) * 50.0;
        let ok =
            Moneyness::from_ladder(edge, 25_850.0, 50.0, OptionKind::Call).expect("at the bound");
        assert_eq!(ok.steps, MAX_STEPS);
    }

    #[test]
    fn each_malformed_argument_is_refused_by_its_own_name() {
        assert_eq!(
            Moneyness::from_ladder(f64::NAN, 25_850.0, 50.0, OptionKind::Call).unwrap_err(),
            GreeksError::NotFinite { field: "strike" }
        );
        assert_eq!(
            Moneyness::from_ladder(25_850.0, -1.0, 50.0, OptionKind::Call).unwrap_err(),
            GreeksError::NotPositive {
                field: "at_the_money",
                value: -1.0
            }
        );
        assert_eq!(
            Moneyness::from_ladder(25_850.0, 25_850.0, 0.0, OptionKind::Call).unwrap_err(),
            GreeksError::NotPositive {
                field: "interval",
                value: 0.0
            }
        );
    }

    #[test]
    fn the_rung_is_an_ordinary_value() {
        let rung = Moneyness::from_ladder(25_900.0, 25_850.0, 50.0, OptionKind::Call)
            .expect("on the grid");
        assert_eq!(rung, rung.clone());
        assert!(!format!("{rung:?}").is_empty());
        assert!(!format!("{:?}", Position::AtTheMoney).is_empty());
        assert_ne!(Position::InTheMoney, Position::OutOfTheMoney);
        let seen: HashSet<Moneyness> = [rung, rung].into_iter().collect();
        assert_eq!(seen.len(), 1);
        let positions: HashSet<Position> = [
            Position::AtTheMoney,
            Position::InTheMoney,
            Position::OutOfTheMoney,
        ]
        .into_iter()
        .collect();
        assert_eq!(positions.len(), 3);
    }
}
