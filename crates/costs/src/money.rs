//! The two rounding laws a charge obeys, and the trap between them.
//!
//! A charge is `notional × rate / RATE_SCALE`, and the interesting part is what
//! happens to the remainder. There are **two** answers in an Indian F&O cost
//! stack and they are not interchangeable:
//!
//! | Law | Applies to | Shape |
//! |---|---|---|
//! | [`levy_ceiling`] | exchange transaction charge, SEBI fee, IPFT | ceil to the **paisa**, once, per leg |
//! | [`statutory_levy`] | STT, stamp duty, GST | floor to the paisa **first**, then ceil to the whole **rupee** |
//!
//! Substituting one for the other moves a real charge. The predecessor's
//! `money.rs` records the reason it gave them different *function names* rather
//! than different *constants*: a half-up in the statutory raw stage drifts the
//! raw by one paisa and occasionally flips the rupee rounding, and nothing
//! downstream can tell that happened.
//!
//! # Why everything ceils
//!
//! Adverse rounding, toward `+∞`, on every rate-scaled levy. The predecessor's
//! `DEC-FILL-WORST-CASE-ONLY-001` fixes it as permanent, and the argument is
//! that a levy is a *charge*: rounding it up can only make the backtest's net
//! smaller than reality's. A real exchange rounds statutorily half-up, so the
//! ceiled backtest charge is **≥** the real one and the simulated result can
//! only understate the truth. A backtest that flatters is worse than one that
//! is pessimistic by a paisa.
//!
//! **Stated as a limit, because it is one:** this is not the exchange's own
//! rounding. It is deliberately more expensive than the exchange's, in a known
//! direction, by at most one rupee per statutory levy and one paisa per leg per
//! non-statutory one. `docs/06-limits.md` §27.
//!
//! # Constant per-operation cost, counted
//!
//! Every **primitive** here is a fixed sequence: at most one multiplication,
//! two divisions (`div_euclid` and `rem_euclid`), one comparison, one
//! conditional `+ 1`, and one narrowing. [`levy_ceiling`] is `scaled` then
//! [`ceil_to_paisa`]; [`statutory_levy`] is `scaled`, [`floor_to_paisa`] and
//! [`ceil_to_rupee`], so it costs two multiplications, three divisions and two
//! narrowings. There is no loop, no allocation and no branch whose count
//! depends on the magnitude of an input.
//!
//! # No floats, and no wrapping
//!
//! The product is formed in `i128`, where an `i64 × i64` cannot overflow, and
//! narrowed back to `i64` **once**, with the narrowing refused rather than
//! saturated. A saturated charge is a charge nobody was billed.

use brutex_core::price::Paisa;

use crate::error::CostError;
use crate::rate::{BpsX100, RATE_SCALE};

/// [`RATE_SCALE`] at `i128` width, for the one multiplication that needs it.
///
/// Written as its own literal and pinned to [`RATE_SCALE`] by the compile-time
/// assertion below, so the two cannot drift and no cast appears on any path.
const RATE_SCALE_WIDE: i128 = 10_000_000;
const _: () = assert!(RATE_SCALE == 10_000_000);

/// One rupee in paisa, at `i128` width. Pinned to `core`'s constant the same
/// way, so "a rupee" means one thing in this workspace.
const PAISA_PER_RUPEE_WIDE: i128 = 100;
const _: () = assert!(brutex_core::price::PAISA_PER_RUPEE == 100);

/// The least integer at or above `value / divisor`, for a **positive**
/// divisor.
///
/// `div_euclid` is true floor division for either sign of `value`, and
/// `rem_euclid` is the non-negative remainder, so this is the mathematical
/// ceiling and not an approximation of one. Both calls are total here: the only
/// ways `div_euclid` can panic are a zero divisor and `i128::MIN / -1`, and the
/// two divisors in this module are positive compile-time constants.
///
/// `quotient + 1` cannot overflow: the quotient's magnitude is at most
/// `i128::MAX / 100`.
fn ceil_div(value: i128, divisor: i128) -> i128 {
    let quotient = value.div_euclid(divisor);
    if value.rem_euclid(divisor) == 0 {
        quotient
    } else {
        quotient + 1
    }
}

/// `amount × rate`, exactly, at `i128` width.
///
/// This is the raw scaled product every charge starts from. It is **total**:
/// both factors are `i64`, so the product's magnitude is at most
/// `2^126`, which is inside `i128`. There is no overflow branch here because
/// there is no overflow to branch on.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::money::scaled;
/// use costs::rate::SEBI_TURNOVER_FEE;
///
/// // ₹1 crore at ₹10 per crore, before any rounding: 10^9 × 10 scaled units.
/// assert_eq!(scaled(Paisa::from_raw(10_000_000_00), SEBI_TURNOVER_FEE), 10_000_000_000);
/// ```
#[must_use]
pub fn scaled(amount: Paisa, rate: BpsX100) -> i128 {
    i128::from(amount.raw()) * i128::from(rate.get())
}

/// The **non-statutory** law: ceil a scaled product to the next whole paisa.
///
/// The exchange transaction charge, the SEBI turnover fee and the IPFT round
/// this way, **per leg**, and the per-leg results are summed afterwards. Never
/// summed first and rounded once — the worked example in `COSTS_VERIFIED` §5
/// Ex.1 is 502 paisa because it is `228 + 274`, and one rounding of the sum
/// gives 501.
///
/// # Errors
///
/// [`CostError::Overflow`] when the ceiled result leaves `i64`. Refused, never
/// saturated.
///
/// # Examples
///
/// ```
/// use costs::money::ceil_to_paisa;
///
/// // Any fraction of a paisa becomes a whole one; an exact multiple is fixed.
/// assert_eq!(ceil_to_paisa(1)?.raw(), 1);
/// assert_eq!(ceil_to_paisa(10_000_000)?.raw(), 1);
/// assert_eq!(ceil_to_paisa(10_000_001)?.raw(), 2);
/// // Toward +infinity for either sign: a sub-paisa negative reads zero.
/// assert_eq!(ceil_to_paisa(-9_999_999)?.raw(), 0);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn ceil_to_paisa(scaled_product: i128) -> Result<Paisa, CostError> {
    narrow(
        ceil_div(scaled_product, RATE_SCALE_WIDE),
        "ceiling a levy to the paisa",
    )
}

/// The **statutory raw** stage: floor a scaled product to the paisa, with no
/// rounding adjustment at all.
///
/// STT, stamp duty and GST take this first and [`ceil_to_rupee`] second. The
/// order is the law, not a preference: flooring then ceiling to the rupee is
/// not the same function as ceiling then ceiling, and the two disagree on every
/// raw whose paisa part is a fraction below a rupee boundary.
///
/// # Errors
///
/// [`CostError::Overflow`] when the floored result leaves `i64`.
///
/// # Examples
///
/// ```
/// use costs::money::floor_to_paisa;
///
/// // The `COSTS_VERIFIED` Ex.1 STT raw: 779,675 paisa at 0.15%.
/// assert_eq!(floor_to_paisa(779_675 * 15_000)?.raw(), 1_169);
/// // A fraction of a paisa floors away, where the levy law would have ceiled.
/// assert_eq!(floor_to_paisa(9_999_999)?.raw(), 0);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn floor_to_paisa(scaled_product: i128) -> Result<Paisa, CostError> {
    narrow(
        scaled_product.div_euclid(RATE_SCALE_WIDE),
        "flooring a statutory levy to the paisa",
    )
}

/// The **statutory rupee** stage: ceil an amount to the next whole rupee.
///
/// ₹11.01 becomes ₹12; ₹11.00 stays ₹11.
///
/// # Errors
///
/// [`CostError::Overflow`] when the ceiled multiple leaves `i64` — which
/// happens within 100 paisa of `i64::MAX`, because the ceiling of `i64::MAX`
/// is larger than `i64::MAX`.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::money::ceil_to_rupee;
///
/// assert_eq!(ceil_to_rupee(Paisa::from_raw(1_101))?.raw(), 1_200);
/// assert_eq!(ceil_to_rupee(Paisa::from_raw(1_100))?.raw(), 1_100);
/// assert_eq!(ceil_to_rupee(Paisa::from_raw(1))?.raw(), 100);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn ceil_to_rupee(amount: Paisa) -> Result<Paisa, CostError> {
    let rupees = ceil_div(i128::from(amount.raw()), PAISA_PER_RUPEE_WIDE);
    // `rupees` has magnitude at most i128::MAX / 100, so scaling it back by 100
    // is exact at i128 width. The narrowing below is the only fallible step.
    narrow(
        rupees * PAISA_PER_RUPEE_WIDE,
        "ceiling a statutory levy to the rupee",
    )
}

/// A non-statutory levy on one leg: [`scaled`], then [`ceil_to_paisa`].
///
/// # Errors
///
/// [`CostError::Overflow`], from [`ceil_to_paisa`].
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::money::levy_ceiling;
/// use costs::regime::NSE_EXCHANGE_CHARGE;
///
/// // `COSTS_VERIFIED` §5 Ex.1: the buy leg's exchange charge, 227.81 paisa.
/// let buy = levy_ceiling(Paisa::from_raw(650_325), NSE_EXCHANGE_CHARGE)?;
/// assert_eq!(buy.raw(), 228);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn levy_ceiling(notional: Paisa, rate: BpsX100) -> Result<Paisa, CostError> {
    ceil_to_paisa(scaled(notional, rate))
}

/// A statutory levy: [`scaled`], then [`floor_to_paisa`], then
/// [`ceil_to_rupee`].
///
/// The two stages in that order, which is what makes this a different function
/// from [`levy_ceiling`] rather than the same one with a different divisor.
///
/// # Errors
///
/// [`CostError::Overflow`], from either stage.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use costs::money::statutory_levy;
/// use costs::rate::STAMP_DUTY_BUY_SIDE;
///
/// // `COSTS_VERIFIED` §5 Ex.1 stamp duty: 19.5 paisa raw, floored to 19,
/// // then ceiled to one whole rupee.
/// let stamp = statutory_levy(Paisa::from_raw(650_325), STAMP_DUTY_BUY_SIDE)?;
/// assert_eq!(stamp.raw(), 100);
/// # Ok::<(), costs::error::CostError>(())
/// ```
pub fn statutory_levy(notional: Paisa, rate: BpsX100) -> Result<Paisa, CostError> {
    ceil_to_rupee(floor_to_paisa(scaled(notional, rate))?)
}

/// Narrows an `i128` paisa value back to [`Paisa`], refusing rather than
/// saturating.
///
/// Crate-visible because it is the **one** narrowing in this crate: the fill
/// law and the charge stack widen to `i128` for the same reason this module
/// does, and two narrowings could disagree about whether a saturated answer is
/// acceptable. It is not public, because a caller that could name the operation
/// could put misleading words on a refusal.
pub(crate) fn narrow(value: i128, operation: &'static str) -> Result<Paisa, CostError> {
    i64::try_from(value)
        .map(Paisa::from_raw)
        .map_err(|_| CostError::Overflow { operation })
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

    fn raw(result: Result<Paisa, CostError>) -> i64 {
        result.expect("in range").raw()
    }

    #[test]
    fn the_paisa_ceiling_walks_a_pinned_table_on_both_signs() {
        // Positive: ANY fraction ceils, an exact multiple is a fixed point.
        assert_eq!(raw(ceil_to_paisa(0)), 0);
        assert_eq!(raw(ceil_to_paisa(1)), 1);
        assert_eq!(raw(ceil_to_paisa(4_999_999)), 1);
        assert_eq!(raw(ceil_to_paisa(5_000_000)), 1);
        assert_eq!(raw(ceil_to_paisa(10_000_000)), 1);
        assert_eq!(raw(ceil_to_paisa(10_000_001)), 2);
        assert_eq!(raw(ceil_to_paisa(15_000_000)), 2);
        // Negative: toward +infinity, so a sub-paisa negative reads zero.
        assert_eq!(raw(ceil_to_paisa(-1)), 0);
        assert_eq!(raw(ceil_to_paisa(-9_999_999)), 0);
        assert_eq!(raw(ceil_to_paisa(-10_000_000)), -1);
        assert_eq!(raw(ceil_to_paisa(-15_000_000)), -1);
    }

    #[test]
    fn the_rupee_ceiling_walks_a_pinned_table_on_both_signs() {
        let ceil = |paisa: i64| raw(ceil_to_rupee(Paisa::from_raw(paisa)));
        assert_eq!(ceil(0), 0);
        assert_eq!(ceil(1), 100);
        assert_eq!(ceil(49), 100);
        assert_eq!(ceil(50), 100);
        assert_eq!(ceil(100), 100);
        assert_eq!(ceil(101), 200);
        assert_eq!(ceil(1_100), 1_100);
        assert_eq!(ceil(1_101), 1_200);
        assert_eq!(ceil(1_170), 1_200);
        assert_eq!(ceil(-1), 0);
        assert_eq!(ceil(-99), 0);
        assert_eq!(ceil(-100), -100);
        assert_eq!(ceil(-101), -100);
        assert_eq!(ceil(-200), -200);
    }

    #[test]
    fn the_statutory_raw_stage_floors_where_the_levy_stage_ceils() {
        // The same scaled product, through the two laws. They disagree on
        // every remainder, and that disagreement is the trap this module
        // exists to make unexpressible by accident.
        for product in [1_i128, 9_999_999, 10_000_001, -1, -9_999_999] {
            let floored = raw(floor_to_paisa(product));
            let ceiled = raw(ceil_to_paisa(product));
            assert_eq!(ceiled - floored, 1, "product {product} has a remainder");
        }
        // An exact multiple is where the two agree, and only there.
        assert_eq!(raw(floor_to_paisa(20_000_000)), 2);
        assert_eq!(raw(ceil_to_paisa(20_000_000)), 2);
        // The pinned source raws.
        assert_eq!(raw(floor_to_paisa(779_675 * 15_000)), 1_169);
        assert_eq!(raw(floor_to_paisa(650_325 * 300)), 19);
    }

    #[test]
    fn the_ceiling_is_the_least_integer_at_or_above_the_quotient() {
        // The defining property, checked two ways: densely across a whole
        // paisa either side of zero, and at every remainder that can flip the
        // answer for forty-one whole paisa either side of it. `r` must be at
        // or above the quotient, and `r - 1` strictly below it.
        let mut checked = 0_u32;
        let mut check = |product: i128| {
            let r = i128::from(raw(ceil_to_paisa(product)));
            assert!(
                r * RATE_SCALE_WIDE >= product,
                "{product}: not an upper bound"
            );
            assert!(
                (r - 1) * RATE_SCALE_WIDE < product,
                "{product}: not the LEAST upper bound"
            );
            checked += 1;
        };
        for product in -100_000_i128..=100_000 {
            check(product);
        }
        for whole in -20_i128..=20 {
            for remainder in [0, 1, 2, 4_999_999, 5_000_000, 9_999_998, 9_999_999] {
                check(whole * RATE_SCALE_WIDE + remainder);
            }
        }
        assert_eq!(checked, 200_001 + 41 * 7);
    }

    #[test]
    fn the_rupee_ceiling_is_the_least_whole_rupee_at_or_above_the_amount() {
        let mut checked = 0_u32;
        for paisa in -5_000_i64..=5_000 {
            let r = raw(ceil_to_rupee(Paisa::from_raw(paisa)));
            assert_eq!(r % 100, 0, "not a whole rupee");
            assert!(r >= paisa, "not at or above");
            assert!(r - paisa < 100, "not the LEAST whole rupee at or above");
            checked += 1;
        }
        assert_eq!(checked, 10_001);
    }

    #[test]
    fn the_scaled_product_is_exact_at_the_widest_inputs_and_never_wraps() {
        // i64::MIN x i64::MIN is the largest magnitude two i64s can produce.
        // It fits i128 with room to spare, which is why `scaled` has no
        // overflow branch: there is no overflow to branch on.
        let widest = scaled(Paisa::from_raw(i64::MIN), BpsX100::new(i64::MIN));
        assert_eq!(widest, i128::from(i64::MIN) * i128::from(i64::MIN));
        assert!(widest > 0 && widest < i128::MAX);
        assert_eq!(scaled(Paisa::ZERO, BpsX100::new(i64::MAX)), 0);
        assert_eq!(scaled(Paisa::from_raw(100), BpsX100::new(-3)), -300);
    }

    #[test]
    fn a_result_past_i64_is_refused_by_name_and_never_saturated() {
        // A scaled product whose quotient leaves i64.
        let past = i128::from(i64::MAX) * RATE_SCALE_WIDE + RATE_SCALE_WIDE;
        assert_eq!(
            ceil_to_paisa(past),
            Err(CostError::Overflow {
                operation: "ceiling a levy to the paisa"
            })
        );
        assert_eq!(
            floor_to_paisa(past),
            Err(CostError::Overflow {
                operation: "flooring a statutory levy to the paisa"
            })
        );
        // The rupee ceiling of i64::MAX is ...900, which is past i64::MAX.
        assert_eq!(
            ceil_to_rupee(Paisa::from_raw(i64::MAX)),
            Err(CostError::Overflow {
                operation: "ceiling a statutory levy to the rupee"
            })
        );
        // One rupee below the edge still computes, so the refusal is the
        // edge itself and not a blanket rejection of large amounts.
        assert_eq!(
            raw(ceil_to_rupee(Paisa::from_raw(i64::MAX - 807))),
            i64::MAX - 807
        );
        // i64::MIN ceils toward +infinity to ...800, which is in range.
        assert_eq!(raw(ceil_to_rupee(Paisa::from_raw(i64::MIN))), i64::MIN + 8);
    }

    #[test]
    fn the_two_composed_levies_reproduce_the_source_worked_example() {
        use crate::rate::{NSE_IPFT, SEBI_TURNOVER_FEE, STAMP_DUTY_BUY_SIDE};
        use crate::regime::NSE_EXCHANGE_CHARGE;

        let buy = Paisa::from_raw(650_325);
        let sell = Paisa::from_raw(779_675);
        // `COSTS_VERIFIED` §5 Ex.1, component by component.
        assert_eq!(raw(levy_ceiling(buy, NSE_EXCHANGE_CHARGE)), 228);
        assert_eq!(raw(levy_ceiling(sell, NSE_EXCHANGE_CHARGE)), 274);
        assert_eq!(raw(levy_ceiling(buy, SEBI_TURNOVER_FEE)), 1);
        assert_eq!(raw(levy_ceiling(sell, SEBI_TURNOVER_FEE)), 1);
        assert_eq!(raw(levy_ceiling(buy, NSE_IPFT)), 4);
        assert_eq!(raw(levy_ceiling(sell, NSE_IPFT)), 4);
        assert_eq!(raw(statutory_levy(buy, STAMP_DUTY_BUY_SIDE)), 100);
        // And the per-leg sum is NOT the rounded sum: 228 + 274 = 502, where
        // one rounding of the combined notional gives 501.
        let per_leg = raw(levy_ceiling(buy, NSE_EXCHANGE_CHARGE))
            + raw(levy_ceiling(sell, NSE_EXCHANGE_CHARGE));
        let summed_first = raw(levy_ceiling(
            Paisa::from_raw(buy.raw() + sell.raw()),
            NSE_EXCHANGE_CHARGE,
        ));
        assert_eq!(per_leg, 502);
        assert_eq!(summed_first, 501);
        assert_ne!(per_leg, summed_first);
    }

    #[test]
    fn a_composed_levy_carries_the_overflow_refusal_out_of_whichever_stage_failed() {
        let huge = Paisa::from_raw(i64::MAX);
        // The levy stage: the quotient leaves i64.
        assert_eq!(
            levy_ceiling(huge, BpsX100::new(1_000_000_000)),
            Err(CostError::Overflow {
                operation: "ceiling a levy to the paisa"
            })
        );
        // The statutory pair: the FLOOR stage is the one that fails here, and
        // its own words come out rather than the rupee stage's.
        assert_eq!(
            statutory_levy(huge, BpsX100::new(1_000_000_000)),
            Err(CostError::Overflow {
                operation: "flooring a statutory levy to the paisa"
            })
        );
        // A raw that fits but whose RUPEE ceiling does not: the second stage
        // is the one that refuses, with its own words.
        let at_the_edge = Paisa::from_raw(i64::MAX);
        assert_eq!(
            statutory_levy(at_the_edge, BpsX100::new(RATE_SCALE)),
            Err(CostError::Overflow {
                operation: "ceiling a statutory levy to the rupee"
            })
        );
    }
}
