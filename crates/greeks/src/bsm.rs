//! Black-Scholes-Merton, European exercise, closed form.
//!
//! ```text
//! d1 = [ln(S/K) + (r - q + sigma^2/2) T] / (sigma sqrt(T))
//! d2 = d1 - sigma sqrt(T)
//! ```
//!
//! with `S` the spot, `K` the strike, `T` the time to expiry in years, `r` the
//! continuously compounded risk-free rate, `q` the continuous carry (the
//! dividend or convenience yield) and `sigma` the volatility.
//!
//! # Units, because two of them are not what a vendor prints
//!
//! Everything here is in **natural** units. A vendor's option chain usually is
//! not, and the difference is not small:
//!
//! | Quantity | Here | Dhan's chain, measured |
//! |---|---|---|
//! | vega | per `1.00` of volatility | per **one percentage point** — divide by 100 |
//! | theta | per **year** | per a **calendar day**, not a trading day — divide by ≈365 |
//! | rho | per `1.00` of rate | not published at all |
//!
//! **What is measured about theta's divisor, and what is a convention.**
//! Solving the captured chain twice, once with 365 and once with 252, the two
//! sides of the same strike agree on `r` to **1.00 percentage point** under
//! 365 and to **23.26** under 252 — a factor of 23.3. That excludes a trading
//! day, and it is the part this table rests on. It does **not** select 365:
//! the same criterion has its root at **370.08**, where the two sides agree
//! exactly, and it prefers 375 to 365. So `365` here is the nearest ordinary
//! calendar convention to a root the sample puts elsewhere, and the day count
//! behind it is UNVERIFIED. Getting the divisor wrong by the trading-day
//! factor costs `365/252 = 1.448`, so a theta of `−15.15` a day becomes
//! `−10.46`: a 44.8% error on a single strike, invisible unless it is tested.
//! See `docs/05-decisions.md` D-0037 and `docs/06-limits.md` §18.
//!
//! # What is exact here and what is not
//!
//! Gamma and vega do not depend on which side of the contract you are on, and
//! in this module they are not merely equal, they are **bit-identical**: both
//! are computed once, above the branch that chooses call or put. There is no
//! tolerance in that test because there is no arithmetic difference to
//! tolerate.
//!
//! `delta_call − delta_put = e^-qT` is a different case. It is exact in
//! algebra and cannot be exact in `f64`, because the two deltas are computed
//! from `N(d1)` and `N(−d1)` separately — deliberately, so that each keeps
//! its own relative accuracy in its own tail rather than one being derived
//! from the other by a subtraction that destroys it. The identity is asserted
//! to a **measured** bound instead of a hoped-for zero.

// Every expression below is a float expression. A delta is a derivative and a
// vega is a sensitivity; `CLAUDE.md` §7 puts statistical values at full
// precision and reserves `i64` paisa for prices. No paisa reaches this file
// and no value here is ever snapped to a tick. See the crate documentation
// and D-0037.
#![allow(clippy::float_arithmetic)]

use crate::error::GreeksError;
use crate::normal::{standard_normal_cdf, standard_normal_pdf};

/// The largest spot or strike this crate evaluates.
///
/// Not a market view. It is the bound that keeps every product below from
/// reaching an infinity, which is the same failure arriving silently and
/// three steps later.
pub const MAX_UNDERLYING: f64 = 1.0e12;

/// The longest maturity this crate evaluates, in years.
pub const MAX_YEARS: f64 = 100.0;

/// The largest magnitude accepted for the rate or the carry.
///
/// `10.0` is 1000% continuously compounded. Nothing real is near it; the
/// bound exists so that `e^-rT` cannot overflow.
pub const MAX_RATE: f64 = 10.0;

/// The largest volatility this crate will price at.
///
/// The implied-volatility solver searches a narrower band — see
/// [`crate::solver::MAX_VOLATILITY`] — because a root find needs a bracket it
/// can defend, and pricing does not.
pub const MAX_VOLATILITY: f64 = 10.0;

/// Which side of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptionKind {
    /// The right to buy.
    Call,
    /// The right to sell.
    Put,
}

/// Everything about a European option except its volatility.
///
/// Volatility is not a field because it is the one quantity that is sometimes
/// an input and sometimes the answer. Keeping it out of the struct means
/// [`Contract::implied_volatility`] cannot be handed a stale one by accident.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contract {
    /// The spot level of the underlying.
    pub spot: f64,
    /// The strike.
    pub strike: f64,
    /// Time to expiry in years.
    pub years_to_expiry: f64,
    /// The continuously compounded risk-free rate, as a decimal — `0.0946`,
    /// not `9.46`.
    pub rate: f64,
    /// The continuous carry `q`, as a decimal. Zero for a spot index with no
    /// dividend adjustment.
    pub carry: f64,
}

/// The full set, in natural units. See the module documentation for how those
/// differ from a vendor's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Greeks {
    /// The model price.
    pub price: f64,
    /// `dPrice / dSpot`.
    pub delta: f64,
    /// `d2Price / dSpot2`. Identical for a call and a put at the same strike.
    pub gamma: f64,
    /// `dPrice / dVolatility`, per `1.00` of volatility. Identical for a call
    /// and a put at the same strike.
    pub vega: f64,
    /// `dPrice / dTime`, per **year**, negative for a long position that is
    /// losing time value.
    pub theta: f64,
    /// `dPrice / dRate`, per `1.00` of rate.
    pub rho: f64,
    /// The `d1` of the model, kept because a caller checking a vendor needs
    /// it and recomputing it is a second chance to disagree.
    pub d1: f64,
    /// The `d2` of the model.
    pub d2: f64,
}

impl Greeks {
    /// Whether every field is a finite number.
    ///
    /// The gate on the way out of [`Contract::greeks`]. A `NaN` delta looks
    /// exactly like a real one until it is multiplied by something.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        [
            self.price, self.delta, self.gamma, self.vega, self.theta, self.rho, self.d1, self.d2,
        ]
        .iter()
        .all(|value| value.is_finite())
    }
}

/// Refuses a non-finite value, naming the field it arrived in.
pub(crate) fn finite(value: f64, field: &'static str) -> Result<f64, GreeksError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(GreeksError::NotFinite { field })
    }
}

/// Refuses anything that is not finite, strictly positive and within `bound`.
pub(crate) fn positive_bounded(
    value: f64,
    field: &'static str,
    bound: f64,
) -> Result<f64, GreeksError> {
    let value = finite(value, field)?;
    if value <= 0.0 {
        return Err(GreeksError::NotPositive { field, value });
    }
    if value > bound {
        return Err(GreeksError::OutOfRange {
            field,
            value,
            bound,
        });
    }
    Ok(value)
}

/// Refuses anything that is not finite or whose magnitude exceeds `bound`.
/// The sign is free: a negative rate is a real thing.
fn signed_bounded(value: f64, field: &'static str, bound: f64) -> Result<f64, GreeksError> {
    let value = finite(value, field)?;
    if value.abs() > bound {
        return Err(GreeksError::OutOfRange {
            field,
            value,
            bound,
        });
    }
    Ok(value)
}

/// A contract that has passed every bound, with the discount factors already
/// taken so nothing below has to decide whether they were.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Checked {
    pub(crate) spot: f64,
    pub(crate) strike: f64,
    pub(crate) years: f64,
    pub(crate) rate: f64,
    pub(crate) carry: f64,
    pub(crate) root_years: f64,
    /// `S * e^-qT`, the forward value of the spot leg.
    pub(crate) forward: f64,
    /// `K * e^-rT`, the present value of the strike leg.
    pub(crate) discounted_strike: f64,
    /// `e^-qT`.
    pub(crate) carry_discount: f64,
}

impl Contract {
    /// Validates every bound once and takes the two discount factors.
    pub(crate) fn check(&self) -> Result<Checked, GreeksError> {
        let spot = positive_bounded(self.spot, "spot", MAX_UNDERLYING)?;
        let strike = positive_bounded(self.strike, "strike", MAX_UNDERLYING)?;
        let years = finite(self.years_to_expiry, "years_to_expiry")?;
        if years <= 0.0 {
            return Err(GreeksError::Expired {
                years_to_expiry: years,
            });
        }
        if years > MAX_YEARS {
            return Err(GreeksError::OutOfRange {
                field: "years_to_expiry",
                value: years,
                bound: MAX_YEARS,
            });
        }
        let rate = signed_bounded(self.rate, "rate", MAX_RATE)?;
        let carry = signed_bounded(self.carry, "carry", MAX_RATE)?;
        let carry_discount = (-carry * years).exp();
        Ok(Checked {
            spot,
            strike,
            years,
            rate,
            carry,
            root_years: years.sqrt(),
            forward: spot * carry_discount,
            discounted_strike: strike * (-rate * years).exp(),
            carry_discount,
        })
    }

    /// Price and all five greeks.
    ///
    /// # Errors
    ///
    /// [`GreeksError::NotFinite`], [`GreeksError::NotPositive`],
    /// [`GreeksError::OutOfRange`] or [`GreeksError::Expired`] when an input
    /// breaks its bound, and [`GreeksError::NotRepresentable`] when every
    /// input is in range and the model still produces something that is not a
    /// finite number.
    ///
    /// ```
    /// use greeks::{Contract, OptionKind};
    ///
    /// let contract = Contract {
    ///     spot: 100.0,
    ///     strike: 100.0,
    ///     years_to_expiry: 1.0,
    ///     rate: 0.0,
    ///     carry: 0.0,
    /// };
    /// let call = contract.greeks(0.2, OptionKind::Call).expect("in range");
    /// assert!((call.delta - 0.539_827_837_277_029).abs() < 1e-12);
    /// ```
    pub fn greeks(&self, volatility: f64, kind: OptionKind) -> Result<Greeks, GreeksError> {
        let checked = self.check()?;
        let volatility = positive_bounded(volatility, "volatility", MAX_VOLATILITY)?;
        let out = checked.greeks(volatility, kind);
        if out.is_finite() {
            Ok(out)
        } else {
            Err(GreeksError::NotRepresentable)
        }
    }

    /// The model price alone.
    ///
    /// # Errors
    ///
    /// Exactly those of [`Contract::greeks`].
    pub fn price(&self, volatility: f64, kind: OptionKind) -> Result<f64, GreeksError> {
        self.greeks(volatility, kind).map(|out| out.price)
    }
}

#[cfg(test)]
thread_local! {
    /// Every model evaluation this crate performs passes through
    /// [`Checked::greeks`], so counting there counts **all** of them —
    /// including the two bracket ends and the final re-evaluation, which the
    /// solver's own arithmetic used to leave out.
    ///
    /// This exists so that
    /// `greeks::solver::the_reported_cost_is_every_model_evaluation` can check
    /// [`crate::solver::ImpliedVolatility::iterations`] against a number the
    /// solver did not compute. A counter that only reads the field the code
    /// reports cannot see the field being wrong, which is exactly how a
    /// three-evaluation gap survived a test named for the bound.
    ///
    /// Thread-local because `cargo test` runs tests in parallel and a shared
    /// counter would measure other tests' work.
    pub(crate) static MODEL_EVALUATIONS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl Checked {
    /// The closed form. No branch except the one that chooses a side, so the
    /// cost does not depend on any input value.
    pub(crate) fn greeks(&self, volatility: f64, kind: OptionKind) -> Greeks {
        #[cfg(test)]
        MODEL_EVALUATIONS.with(|spent| spent.set(spent.get().saturating_add(1)));
        let vol_root_years = volatility * self.root_years;
        let d1 = ((self.spot / self.strike).ln()
            + (self.rate - self.carry + 0.5 * volatility * volatility) * self.years)
            / vol_root_years;
        let d2 = d1 - vol_root_years;
        let density = standard_normal_pdf(d1);

        // Computed once, above the branch. This is why the call and the put
        // agree bit for bit rather than merely closely.
        let gamma = self.carry_discount * density / (self.spot * vol_root_years);
        let vega = self.forward * density * self.root_years;
        let decay = -self.forward * density * volatility / (2.0 * self.root_years);

        let (price, delta, theta, rho) = match kind {
            OptionKind::Call => {
                let cdf_upper = standard_normal_cdf(d1);
                let cdf_lower = standard_normal_cdf(d2);
                (
                    self.forward * cdf_upper - self.discounted_strike * cdf_lower,
                    self.carry_discount * cdf_upper,
                    decay - self.rate * self.discounted_strike * cdf_lower
                        + self.carry * self.forward * cdf_upper,
                    self.years * self.discounted_strike * cdf_lower,
                )
            }
            OptionKind::Put => {
                let cdf_upper = standard_normal_cdf(-d1);
                let cdf_lower = standard_normal_cdf(-d2);
                (
                    self.discounted_strike * cdf_lower - self.forward * cdf_upper,
                    -self.carry_discount * cdf_upper,
                    decay + self.rate * self.discounted_strike * cdf_lower
                        - self.carry * self.forward * cdf_upper,
                    -self.years * self.discounted_strike * cdf_lower,
                )
            }
        };
        Greeks {
            price,
            delta,
            gamma,
            vega,
            theta,
            rho,
            d1,
            d2,
        }
    }

    /// The magnitude the model price is a **difference of**, which is the
    /// scale its absolute rounding error actually has.
    ///
    /// `price` is `forward · N(d1) − discounted_strike · N(d2)` for a call and
    /// the mirror for a put. Each product carries up to one unit in the last
    /// place of *its own* magnitude, and each `N` carries the CDF's `2.22e-16`
    /// absolute error multiplied by the leg in front of it. So the granularity
    /// of the answer is set by the two legs and **not** by the small residue
    /// they leave behind: one day out and two percent in the money, a price of
    /// `507.76` is what survives the subtraction of two terms near `25,600`,
    /// and `ulp(507.76)` is a measured **100.8 times** finer than the noise
    /// that price actually carries —
    /// `the_price_scale_is_the_two_legs_and_it_dwarfs_the_price_they_leave`
    /// takes that ratio on every run.
    ///
    /// Returned as the scale rather than as the error so the caller can divide
    /// before it multiplies by [`f64::EPSILON`]. Multiplying first underflows
    /// to exactly zero for a subnormal quantity, which reports the strongest
    /// possible certainty at the moment there is none. See D-0037.
    pub(crate) fn price_scale(&self) -> f64 {
        self.forward + self.discounted_strike
    }

    /// The two prices no volatility can reach past: the limit as volatility
    /// falls to zero, and the limit as it rises without bound.
    ///
    /// For a European option the low limit is the **discounted** intrinsic
    /// value, not `max(S − K, 0)`. Using the undiscounted one would reject
    /// perfectly ordinary in-the-money quotes.
    pub(crate) fn no_arbitrage_bounds(&self, kind: OptionKind) -> (f64, f64) {
        match kind {
            OptionKind::Call => (
                (self.forward - self.discounted_strike).max(0.0),
                self.forward,
            ),
            OptionKind::Put => (
                (self.discounted_strike - self.forward).max(0.0),
                self.discounted_strike,
            ),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp
)]
pub(crate) mod tests {
    use super::{
        Contract, GreeksError, MAX_RATE, MAX_UNDERLYING, MAX_VOLATILITY, MAX_YEARS, OptionKind,
    };
    use std::collections::HashSet;

    /// How one field of a well-formed contract is broken, for the tests that
    /// check each refusal by name. Named because the tuple type is otherwise
    /// dense enough that clippy refuses to read it, and so is a reader.
    type Break = fn(&mut Contract);

    /// A grid wide enough to have an opinion, and small enough to read.
    /// Moneyness, maturity, volatility, rate, carry.
    ///
    /// **The near-the-money rungs at two and five days are not decoration.**
    /// The first version of this grid stepped from 0.95 straight to 1.00 and
    /// from one day straight to seven, and the solver's worst answers live in
    /// exactly the gap that left: a strike a percent or two in the money, one
    /// to five days out, at a low volatility, where the price is a small
    /// residue of two terms near the spot. Every silently wrong implied
    /// volatility D-0037 records was found there, and no point of the old grid
    /// stood inside it. A grid that misses the regime a test exists to police
    /// makes the test decorative.
    pub(crate) fn grid() -> Vec<(Contract, f64)> {
        let mut out = Vec::new();
        for moneyness in [0.80_f64, 0.95, 0.98, 1.0, 1.02, 1.05, 1.25] {
            for years in [
                1.0_f64 / 8760.0,
                1.0 / 365.0,
                2.0 / 365.0,
                5.0 / 365.0,
                7.0 / 365.0,
                0.25,
                1.0,
                3.0,
            ] {
                for volatility in [0.05_f64, 0.12, 0.35, 1.0] {
                    for (rate, carry) in [(0.0_f64, 0.0_f64), (0.0946, 0.0), (0.07, 0.02)] {
                        out.push((
                            Contract {
                                spot: 25_851.19,
                                strike: 25_851.19 * moneyness,
                                years_to_expiry: years,
                                rate,
                                carry,
                            },
                            volatility,
                        ));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn gamma_and_vega_are_bit_identical_between_the_call_and_the_put() {
        // Pure mathematics; it needs no vendor to confirm it. Bit-identical,
        // not "within a tolerance": both are computed above the branch.
        for (contract, volatility) in grid() {
            let call = contract.greeks(volatility, OptionKind::Call).expect("call");
            let put = contract.greeks(volatility, OptionKind::Put).expect("put");
            assert_eq!(
                call.gamma.to_bits(),
                put.gamma.to_bits(),
                "gamma differs at {contract:?} sigma {volatility}"
            );
            assert_eq!(
                call.vega.to_bits(),
                put.vega.to_bits(),
                "vega differs at {contract:?} sigma {volatility}"
            );
            assert_eq!(call.d1.to_bits(), put.d1.to_bits());
            assert_eq!(call.d2.to_bits(), put.d2.to_bits());
        }
    }

    #[test]
    fn the_delta_difference_is_the_carry_discount_to_a_measured_bound() {
        // delta_call - delta_put = e^-qT exactly in algebra. In f64 the two
        // deltas come from N(d1) and N(-d1) computed separately, on purpose,
        // so the identity holds to a measured number of ulps and not to zero.
        let mut worst_ulps = 0.0_f64;
        for (contract, volatility) in grid() {
            let call = contract.greeks(volatility, OptionKind::Call).expect("call");
            let put = contract.greeks(volatility, OptionKind::Put).expect("put");
            let expected = (-contract.carry * contract.years_to_expiry).exp();
            let ulps = (call.delta - put.delta - expected).abs() / (f64::EPSILON * expected);
            worst_ulps = worst_ulps.max(ulps);
        }
        println!(
            "delta_call - delta_put - e^-qT: worst {worst_ulps} ulps over {} points",
            grid().len()
        );
        assert!(worst_ulps <= 2.0, "worst {worst_ulps} ulps");
    }

    #[test]
    fn put_call_parity_holds_across_the_grid() {
        // C - P = S e^-qT - K e^-rT. Model-free, so it catches a sign or a
        // discount factor applied to the wrong leg.
        let mut worst = 0.0_f64;
        for (contract, volatility) in grid() {
            let call = contract.price(volatility, OptionKind::Call).expect("call");
            let put = contract.price(volatility, OptionKind::Put).expect("put");
            let checked = contract.check().expect("checked");
            let expected = checked.forward - checked.discounted_strike;
            let relative = (call - put - expected).abs() / checked.spot;
            worst = worst.max(relative);
        }
        println!("put-call parity: worst {worst:e} of spot");
        assert!(worst <= 1e-14, "worst {worst:e}");
    }

    #[test]
    fn every_greek_reproduces_a_central_difference_wherever_one_can_be_taken() {
        // The derivative definition, applied to the shipped closed form. It
        // is what proves theta and rho, which no vendor field anchors here.
        //
        // A central difference is a MEASUREMENT and it has a noise floor: it
        // subtracts two nearly equal prices, so it can only see a derivative
        // bigger than `error(price) / (2h)`. Deep in the money at five percent
        // volatility, gamma is `4.5e-13` and the two deltas being subtracted
        // are both `0.99999999991` -- the stencil returns rounding, and
        // asserting against it would be asserting the behaviour of `f64`.
        //
        // So each greek is compared **only where the stencil can resolve it**,
        // by a factor of a million, and the count of comparisons actually made
        // is printed and asserted. A version of this test that quietly skipped
        // everything would pass; this one cannot.
        let mut compared = 0_u32;
        let mut unresolvable = 0_u32;
        let mut worst = 0.0_f64;
        for (contract, volatility) in grid() {
            // A one-hour maturity has a theta the size of the position; the
            // central difference in time needs room on both sides.
            if contract.years_to_expiry < 0.02 {
                continue;
            }
            for kind in [OptionKind::Call, OptionKind::Put] {
                let analytic = contract.greeks(volatility, kind).expect("greeks");
                let bumped = |change: fn(&mut Contract, f64), step: f64| -> f64 {
                    let mut up = contract;
                    let mut down = contract;
                    change(&mut up, step);
                    change(&mut down, -step);
                    (up.price(volatility, kind).expect("up")
                        - down.price(volatility, kind).expect("down"))
                        / (2.0 * step)
                };
                let spot_step = 1e-5 * contract.spot;
                let vol_step = 1e-5;
                let time_step = 1e-4;
                let rate_step = 1e-6;
                let delta = bumped(|c, h| c.spot += h, spot_step);
                let rho = bumped(|c, h| c.rate += h, rate_step);
                let theta = -bumped(|c, h| c.years_to_expiry += h, time_step);
                let vega = (contract.price(volatility + vol_step, kind).expect("up")
                    - contract.price(volatility - vol_step, kind).expect("down"))
                    / (2.0 * vol_step);
                let gamma = {
                    let mut up = contract;
                    let mut down = contract;
                    up.spot += spot_step;
                    down.spot -= spot_step;
                    (up.greeks(volatility, kind).expect("up").delta
                        - down.greeks(volatility, kind).expect("down").delta)
                        / (2.0 * spot_step)
                };
                // An absolute bound on the error of any price this contract
                // can produce: no price exceeds the spot, so no price carries
                // more than one unit in the last place of it.
                let price_noise = contract.spot * f64::EPSILON;
                // A delta never exceeds one, so the same bound for the
                // stencil that produces gamma is one ulp of one.
                let delta_noise = f64::EPSILON;
                let cases: [(&str, f64, f64, f64); 5] = [
                    (
                        "delta",
                        delta,
                        analytic.delta,
                        price_noise / (2.0 * spot_step),
                    ),
                    (
                        "gamma",
                        gamma,
                        analytic.gamma,
                        delta_noise / (2.0 * spot_step),
                    ),
                    ("vega", vega, analytic.vega, price_noise / (2.0 * vol_step)),
                    (
                        "theta",
                        theta,
                        analytic.theta,
                        price_noise / (2.0 * time_step),
                    ),
                    ("rho", rho, analytic.rho, price_noise / (2.0 * rate_step)),
                ];
                for (name, numeric, exact, noise) in cases {
                    // The skip is decided by the LARGER of the two, and that
                    // is not a detail. Deciding it on the analytic value alone
                    // lets any defect that makes a greek small escape by
                    // making itself unresolvable -- a mutation of rho's
                    // `years * discounted_strike` into a division survived
                    // exactly that way, and this is the line that now kills
                    // it. The numeric side is built from prices and knows
                    // nothing about the formula being checked.
                    if exact.abs().max(numeric.abs()) <= noise * 1e6 {
                        unresolvable += 1;
                        continue;
                    }
                    compared += 1;
                    let error = (numeric - exact).abs() / exact.abs();
                    worst = worst.max(error);
                    assert!(
                        error <= 1e-5,
                        "{name}: numeric {numeric}, analytic {exact}, \
                         relative {error} at {contract:?} sigma {volatility} {kind:?}"
                    );
                }
            }
        }
        println!(
            "central differences: {compared} compared, {unresolvable} below the \
             stencil's noise floor, worst relative {worst:e}"
        );
        assert!(compared >= 1_000, "only {compared} comparisons were made");
    }

    #[test]
    fn a_non_finite_input_is_refused_field_by_field() {
        let good = Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        let cases: [(&str, Break); 5] = [
            ("spot", |c| c.spot = f64::NAN),
            ("strike", |c| c.strike = f64::INFINITY),
            ("years_to_expiry", |c| c.years_to_expiry = f64::NAN),
            ("rate", |c| c.rate = f64::NEG_INFINITY),
            ("carry", |c| c.carry = f64::NAN),
        ];
        for (field, break_it) in cases {
            let mut broken = good;
            break_it(&mut broken);
            let error = broken.greeks(0.2, OptionKind::Call).unwrap_err();
            assert_eq!(error, GreeksError::NotFinite { field }, "for {field}");
        }
        // And the volatility, which is not a field of the contract.
        assert_eq!(
            good.greeks(f64::NAN, OptionKind::Call).unwrap_err(),
            GreeksError::NotFinite {
                field: "volatility"
            }
        );
    }

    #[test]
    fn a_non_positive_spot_strike_or_volatility_is_refused_by_name() {
        let good = Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        let mut zero_spot = good;
        zero_spot.spot = 0.0;
        assert_eq!(
            zero_spot.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::NotPositive {
                field: "spot",
                value: 0.0
            }
        );
        let mut negative_strike = good;
        negative_strike.strike = -1.0;
        assert_eq!(
            negative_strike.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::NotPositive {
                field: "strike",
                value: -1.0
            }
        );
        assert_eq!(
            good.greeks(0.0, OptionKind::Call).unwrap_err(),
            GreeksError::NotPositive {
                field: "volatility",
                value: 0.0
            }
        );
    }

    #[test]
    fn an_expired_contract_is_its_own_refusal_and_not_a_malformed_input() {
        let good = Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        for years in [0.0_f64, -1e-9, -3.0] {
            let mut expired = good;
            expired.years_to_expiry = years;
            assert_eq!(
                expired.greeks(0.2, OptionKind::Call).unwrap_err(),
                GreeksError::Expired {
                    years_to_expiry: years
                },
                "at {years}"
            );
        }
    }

    #[test]
    fn every_bound_refuses_the_value_just_past_it_and_names_it() {
        let good = Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        let mut wide_spot = good;
        wide_spot.spot = MAX_UNDERLYING * 2.0;
        assert_eq!(
            wide_spot.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::OutOfRange {
                field: "spot",
                value: MAX_UNDERLYING * 2.0,
                bound: MAX_UNDERLYING
            }
        );
        let mut wide_strike = good;
        wide_strike.strike = MAX_UNDERLYING * 2.0;
        assert!(matches!(
            wide_strike.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::OutOfRange {
                field: "strike",
                ..
            }
        ));
        let mut long = good;
        long.years_to_expiry = MAX_YEARS + 1.0;
        assert!(matches!(
            long.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::OutOfRange {
                field: "years_to_expiry",
                ..
            }
        ));
        let mut fast = good;
        fast.rate = -(MAX_RATE + 1.0);
        assert!(matches!(
            fast.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::OutOfRange { field: "rate", .. }
        ));
        let mut rich = good;
        rich.carry = MAX_RATE + 1.0;
        assert!(matches!(
            rich.greeks(0.2, OptionKind::Call).unwrap_err(),
            GreeksError::OutOfRange { field: "carry", .. }
        ));
        assert!(matches!(
            good.greeks(MAX_VOLATILITY + 1.0, OptionKind::Call)
                .unwrap_err(),
            GreeksError::OutOfRange {
                field: "volatility",
                ..
            }
        ));
    }

    #[test]
    fn every_bound_accepts_the_value_exactly_on_it() {
        // The companion to the test above, and it is not redundant with it.
        // Every bound here is written `value > bound`, so `>=` would also
        // refuse everything the tests above refuse -- the two tests only pin
        // the comparison together. Each bound is INCLUSIVE, and this is where
        // that is stated.
        let good = Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        let at_bound: [(&str, Break); 5] = [
            ("spot", |c| c.spot = MAX_UNDERLYING),
            ("strike", |c| c.strike = MAX_UNDERLYING),
            ("years_to_expiry", |c| c.years_to_expiry = MAX_YEARS),
            ("rate", |c| c.rate = -MAX_RATE),
            ("carry", |c| c.carry = MAX_RATE),
        ];
        for (field, set_to_bound) in at_bound {
            let mut edge = good;
            set_to_bound(&mut edge);
            // Not `unwrap_or_else(|e| panic!(..))`: that closure body is a
            // region no passing run enters. Asserting on the `Result` itself
            // says the same thing and every line of it runs.
            let answered = edge.greeks(0.2, OptionKind::Call);
            assert!(
                answered.is_ok(),
                "{field} at its bound was refused: {answered:?}"
            );
            let out = answered.expect("asserted on the line above");
            assert!(out.is_finite(), "{field} at its bound produced {out:?}");
        }
        let out = good
            .greeks(MAX_VOLATILITY, OptionKind::Call)
            .expect("volatility at its bound");
        assert!(out.is_finite());
    }

    #[test]
    fn an_in_range_input_that_the_model_cannot_represent_is_refused_not_returned() {
        // Every bound is satisfied and the arithmetic still dies: sigma
        // sqrt(T) underflows to zero and d1 is a division by it. Refusing is
        // the whole point -- the alternative is a NaN delta that looks real.
        let degenerate = Contract {
            spot: 1e-300,
            strike: 1e-290,
            years_to_expiry: 1e-300,
            rate: 0.0,
            carry: 0.0,
        };
        assert_eq!(
            degenerate.greeks(1e-300, OptionKind::Call).unwrap_err(),
            GreeksError::NotRepresentable
        );
    }

    #[test]
    fn the_kind_and_the_greeks_are_ordinary_values() {
        // Debug, Clone, Copy, PartialEq and Hash exist because callers use
        // them; a derive nothing exercises is a derive nobody has checked.
        let contract = Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.0,
            carry: 0.0,
        };
        let out = contract.greeks(0.2, OptionKind::Call).expect("greeks");
        assert_eq!(out, out.clone());
        assert!(out.is_finite());
        assert!(!format!("{out:?}").is_empty());
        assert_eq!(contract, contract.clone());
        let kinds: HashSet<OptionKind> = [OptionKind::Call, OptionKind::Put].into_iter().collect();
        assert_eq!(kinds.len(), 2);
        assert_ne!(OptionKind::Call, OptionKind::Put);
        assert!(!format!("{:?}", OptionKind::Put).is_empty());
    }

    #[test]
    fn the_no_arbitrage_floor_is_the_discounted_intrinsic_value_on_both_sides() {
        let contract = Contract {
            spot: 100.0,
            strike: 90.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.01,
        };
        let checked = contract.check().expect("checked");
        let (call_floor, call_ceiling) = checked.no_arbitrage_bounds(OptionKind::Call);
        let (put_floor, put_ceiling) = checked.no_arbitrage_bounds(OptionKind::Put);
        assert_eq!(call_floor, checked.forward - checked.discounted_strike);
        assert_eq!(call_ceiling, checked.forward);
        // The put is out of the money here, so its floor saturates at zero --
        // which is the `max(.., 0)` arm on the other side.
        assert_eq!(put_floor, 0.0);
        assert_eq!(put_ceiling, checked.discounted_strike);
        // And the mirror image, so both saturating arms are exercised.
        let mirrored = Contract {
            strike: 110.0,
            ..contract
        };
        let checked = mirrored.check().expect("checked");
        assert_eq!(checked.no_arbitrage_bounds(OptionKind::Call).0, 0.0);
        assert_eq!(
            checked.no_arbitrage_bounds(OptionKind::Put).0,
            checked.discounted_strike - checked.forward
        );
    }

    #[test]
    fn the_price_scale_is_the_two_legs_and_it_dwarfs_the_price_they_leave() {
        // The quantity `GreeksError::Indeterminate` screens on. It has to be
        // the size of the terms the price is a DIFFERENCE of, not the size of
        // the difference -- the whole defect D-0037 repairs is that those are
        // not the same number in the regime the guard exists for.
        let contract = Contract {
            spot: 25_851.19,
            strike: 25_350.0,
            years_to_expiry: 1.0 / 365.0,
            rate: 0.0946,
            carry: 0.0,
        };
        let checked = contract.check().expect("checked");
        assert_eq!(
            checked.price_scale(),
            checked.forward + checked.discounted_strike
        );
        // And the point of it, as a measured ratio rather than an assertion
        // about the source: one day out and two percent in the money at five
        // percent volatility, the granularity the price ACTUALLY has is nearly
        // a hundred times coarser than one unit in its own last place.
        let price = contract.price(0.05, OptionKind::Call).expect("priced");
        let by_the_price = price * f64::EPSILON;
        let by_the_legs = checked.price_scale() * f64::EPSILON;
        let ratio = by_the_legs / by_the_price;
        println!(
            "price {price}: ulp-of-price noise {by_the_price:e}, \
             ulp-of-legs noise {by_the_legs:e}, ratio {ratio:.1}"
        );
        assert!(ratio > 50.0, "the two estimates no longer differ: {ratio}");
    }
}
