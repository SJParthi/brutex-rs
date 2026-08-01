//! Every way this crate can refuse, and the reason attached to each.
//!
//! `CLAUDE.md` §4: *degrade loudly and name the reason, or refuse. Never both
//! silently.* Nothing here means "something was wrong, here is a default
//! anyway". A caller that gets an `Err` gets the quantity that broke and the
//! bound it broke.
//!
//! These are plain enums with a hand-written [`std::fmt::Display`]. A derive
//! macro would be a dependency, and this crate has none.

use std::fmt;

/// A greek, a price or an implied volatility could not be produced.
///
/// Every variant carries the numbers an operator needs to see. A refusal that
/// says only "bad input" is a refusal nobody can act on.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum GreeksError {
    /// A NaN or an infinity arrived in a named input.
    ///
    /// A vendor that sends this is malfunctioning. There is no sensible
    /// substitute, so the whole computation is refused rather than repaired.
    NotFinite {
        /// The field, by its name in the input struct.
        field: &'static str,
    },
    /// A quantity that must be strictly positive was zero or negative.
    ///
    /// Spot, strike and volatility. Zero is not a small number here: a zero
    /// strike has no logarithm and a zero volatility has no `d1`.
    NotPositive {
        /// The field, by its name in the input struct.
        field: &'static str,
        /// What was actually supplied.
        value: f64,
    },
    /// A finite, correctly signed quantity was outside the range this crate
    /// is willing to evaluate.
    ///
    /// The bound exists so that no arithmetic below it can overflow to an
    /// infinity — which is the alternative shape of this failure, arriving
    /// silently and several steps later. `docs/07-o1-architecture.md` law 5:
    /// bound every input at the boundary.
    OutOfRange {
        /// The field, by its name in the input struct.
        field: &'static str,
        /// What was actually supplied.
        value: f64,
        /// The largest magnitude this crate accepts for that field.
        bound: f64,
    },
    /// Time to expiry was zero or negative: the contract has expired.
    ///
    /// Separate from [`Self::NotPositive`] because it is not a malformed
    /// input. It is a perfectly well-formed statement about a contract that
    /// no longer has any optionality, and the operator needs to see that
    /// difference.
    Expired {
        /// The time to expiry supplied, in years.
        years_to_expiry: f64,
    },
    /// Every input was well-formed, and the model still produced a value that
    /// is not a finite number.
    ///
    /// Reachable at the extremes of the accepted ranges — a spot of `1e-300`
    /// with a maturity of `1e-300` divides one underflowed quantity by
    /// another. Refused rather than returned, because a `NaN` delta looks
    /// exactly like a real one until it is multiplied by something.
    NotRepresentable,
    /// The market price is at or below the option's discounted intrinsic
    /// value, so no positive volatility produces it.
    ///
    /// This is the arbitrage floor, not a numerical one: for a European
    /// option the price as volatility falls to zero is
    /// `max(S·e^-qT − K·e^-rT, 0)` for a call and `max(K·e^-rT − S·e^-qT, 0)`
    /// for a put. A quote at or under it has no implied volatility to find.
    /// A deep out-of-the-money option has an intrinsic value of zero, so a
    /// zero or negative quote lands here too, with `intrinsic` printed as `0`.
    PriceBelowIntrinsic {
        /// The market price supplied.
        price: f64,
        /// The discounted intrinsic value it failed to exceed.
        intrinsic: f64,
    },
    /// The market price is at or above the largest value the model can reach
    /// at any volatility.
    ///
    /// As volatility rises without bound a call tends to `S·e^-qT` and a put
    /// to `K·e^-rT`. A quote at or above that is outside the model.
    PriceAboveMaximum {
        /// The market price supplied.
        price: f64,
        /// The supremum of the model price over all volatilities.
        maximum: f64,
    },
    /// The price sits inside the arbitrage bounds but outside the band this
    /// crate searches, so the volatility that produces it — if one exists —
    /// is below [`crate::solver::MIN_VOLATILITY`] or above
    /// [`crate::solver::MAX_VOLATILITY`].
    ///
    /// This is a refusal to extrapolate. The search range is a stated bound,
    /// not a guess, and a price outside it is reported rather than clamped
    /// into it.
    OutsideVolatilityRange {
        /// The market price supplied.
        price: f64,
        /// The model price at [`crate::solver::MIN_VOLATILITY`].
        at_lowest: f64,
        /// The model price at [`crate::solver::MAX_VOLATILITY`].
        at_highest: f64,
    },
    /// A volatility was found, and the price does not determine it.
    ///
    /// Vega at the solution is so small that one unit in the last place of
    /// the market price moves the answer by more than
    /// [`crate::solver::MAX_RELATIVE_UNCERTAINTY`] of itself. Deep in the
    /// money this is the normal state of affairs: the price is intrinsic
    /// value plus a rounding error, and the rounding error is what the solver
    /// would be inverting.
    ///
    /// Returning the number anyway is the failure `CLAUDE.md` §4 bans — a
    /// fallback that hides a failure. It is refused instead, with the numbers
    /// that make the refusal checkable.
    Indeterminate {
        /// The volatility the solver reached, refused rather than returned.
        volatility: f64,
        /// Vega at that volatility.
        vega: f64,
        /// The relative uncertainty implied by one unit in the last place of
        /// the market price.
        uncertainty: f64,
        /// The largest relative uncertainty this crate will return.
        bound: f64,
    },
    /// A strike is not an whole number of strike steps from the money.
    ///
    /// The ladder is a grid. A strike between two rungs is either a different
    /// grid or a bad input, and this crate refuses to guess which.
    OffGrid {
        /// `(strike − at_the_money) / interval`, as computed.
        steps: f64,
        /// How far from a whole number the result is allowed to fall.
        tolerance: f64,
    },
}

impl fmt::Display for GreeksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinite { field } => {
                write!(f, "`{field}` is not a finite number")
            }
            Self::NotPositive { field, value } => {
                write!(f, "`{field}` must be strictly positive; got {value}")
            }
            Self::OutOfRange {
                field,
                value,
                bound,
            } => write!(
                f,
                "`{field}` is {value}; this crate evaluates magnitudes up to {bound}"
            ),
            Self::Expired { years_to_expiry } => write!(
                f,
                "the contract has expired: {years_to_expiry} years to expiry"
            ),
            Self::NotRepresentable => f.write_str(
                "the inputs are in range and the model still produced a value that is not finite",
            ),
            Self::PriceBelowIntrinsic { price, intrinsic } => write!(
                f,
                "price {price} is at or below the discounted intrinsic value {intrinsic}; \
                 no volatility produces it"
            ),
            Self::PriceAboveMaximum { price, maximum } => write!(
                f,
                "price {price} is at or above the model's supremum {maximum}; \
                 no volatility produces it"
            ),
            Self::OutsideVolatilityRange {
                price,
                at_lowest,
                at_highest,
            } => write!(
                f,
                "price {price} is outside the searched band, which spans \
                 {at_lowest} to {at_highest}"
            ),
            Self::Indeterminate {
                volatility,
                vega,
                uncertainty,
                bound,
            } => write!(
                f,
                "the price does not determine a volatility: at {volatility} vega is {vega}, \
                 so one unit in the last place of the price moves the answer by {uncertainty} \
                 of itself; the bound is {bound}"
            ),
            Self::OffGrid { steps, tolerance } => write!(
                f,
                "the strike is {steps} steps from the money, which is not a whole number \
                 within {tolerance}"
            ),
        }
    }
}

impl std::error::Error for GreeksError {}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::GreeksError;

    /// One of every variant, so a new variant with no message fails here.
    fn every_variant() -> Vec<GreeksError> {
        vec![
            GreeksError::NotFinite { field: "spot" },
            GreeksError::NotPositive {
                field: "strike",
                value: -1.0,
            },
            GreeksError::OutOfRange {
                field: "rate",
                value: 50.0,
                bound: 10.0,
            },
            GreeksError::Expired {
                years_to_expiry: -0.5,
            },
            GreeksError::NotRepresentable,
            GreeksError::PriceBelowIntrinsic {
                price: 1.0,
                intrinsic: 2.0,
            },
            GreeksError::PriceAboveMaximum {
                price: 3.0,
                maximum: 2.0,
            },
            GreeksError::OutsideVolatilityRange {
                price: 1.0,
                at_lowest: 2.0,
                at_highest: 3.0,
            },
            GreeksError::Indeterminate {
                volatility: 0.2,
                vega: 1e-30,
                uncertainty: 4.0,
                bound: 1e-3,
            },
            GreeksError::OffGrid {
                steps: 1.5,
                tolerance: 1e-6,
            },
        ]
    }

    #[test]
    fn every_error_renders_a_distinct_reason() {
        let rendered: Vec<String> = every_variant().iter().map(ToString::to_string).collect();
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
    fn the_reason_names_the_numbers_that_produced_it() {
        // A refusal an operator cannot check is a refusal nobody acts on.
        let rendered = GreeksError::PriceBelowIntrinsic {
            price: 12.5,
            intrinsic: 13.25,
        }
        .to_string();
        assert!(rendered.contains("12.5"), "{rendered}");
        assert!(rendered.contains("13.25"), "{rendered}");
    }

    #[test]
    fn errors_are_std_errors_and_are_copied_compared_and_printed() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        for error in every_variant() {
            assert_error(&error);
            let copied = error;
            assert_eq!(error, copied);
            assert!(!format!("{error:?}").is_empty());
        }
        assert_ne!(
            GreeksError::NotRepresentable,
            GreeksError::NotFinite { field: "spot" }
        );
    }
}
