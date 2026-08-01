//! Implied volatility: the one thing in this crate that is not closed form.
//!
//! # This is not O(1) and it is not claimed to be
//!
//! `CLAUDE.md` §3 rule 4 fixes *per-operation* cost. Inverting the
//! Black-Scholes price in volatility is a root find, and a root find is not
//! one operation. What it has instead is a bound that is **arithmetic rather
//! than empirical**, which is the property rule 4 is actually protecting:
//!
//! ```text
//! MAX_ITERATIONS = NEWTON_STEPS + BISECTION_STEPS = 8 + 64 = 72
//! ```
//!
//! and 72 is not a hope. The bisection performs **exactly**
//! [`BISECTION_STEPS`] halvings of `[MIN_VOLATILITY, MAX_VOLATILITY]` with no
//! early exit and no data-dependent branch: `(5 − 1e-6) / 2^64 ≈ 2.7e-19`,
//! narrower than one unit in the last place of any volatility in that band.
//! It cannot take longer on a hard input, because it does not look at the
//! input to decide when to stop.
//!
//! # Why the bracketed method is the floor and Newton is the optimisation
//!
//! This is the inverse of how it is usually written, and the calibration is
//! why. Measured over an 819-point grid of moneyness × maturity × volatility:
//!
//! | Newton cap | solved by Newton | rescued | **worst total** | median |
//! |---|---|---|---|---|
//! | 0 — bracketed only | 0 | 523 | **55** | 17 |
//! | 8 | 232 | 291 | **63** | 19 |
//! | 500 | 432 | 100 | **444** | 11 |
//!
//! **That table was taken on the calibration harness, whose rescuer was Brent
//! and whose bisection exited early — not on the code in this file.** It is
//! why the shape below was chosen; it is not a measurement of the shape below.
//! What is measured on *this* code is a worst total of **72**, which is also
//! its arithmetic ceiling, by
//! `greeks::solver::the_iteration_count_never_exceeds_the_arithmetic_bound`.
//!
//! Newton alone reached **427 iterations** at `K/S = 1.10, T = 2h` with a
//! fixed tolerance and **444** with stagnation detection; median 7–8, p99
//! 351–394. A method whose worst case is fifty times its median is not one to
//! hang a cost rule on. Raising its cap to 500 buys a better median and a
//! seven-fold worse ceiling.
//!
//! Stated honestly, because it argues against the shape shipped here: at a cap
//! of 8 the Newton pre-pass did **not** beat the bracketed method alone on
//! that grid — 63 against 55 worst, 19 against 17 median. It is kept because
//! it is where the common case is cheap (an at-the-money strike converges in
//! 3–4 steps and never reaches the bisection at all) and because its cost is
//! bounded at 8 whatever happens. The number that matters is the ceiling, and
//! the ceiling is arithmetic.
//!
//! # And where Newton simply dies
//!
//! Constructed and measured: `S = 26900, K = 28110.5, T = 1 hour, r = 6.65%,
//! sigma = 0.35` prices at `2.44e-31` with `vega = 9.85e-29`. From a seed
//! below the root Newton **diverges outright** — vega `2.11e-162`, step
//! `−1.15e131`, and vega is exactly zero on the next evaluation, dead in two
//! iterations. From the Brenner-Subrahmanyam seed it converges *linearly*,
//! residual times `0.36` per step, still wrong in the ninth digit after 40
//! iterations. Newton's failure edge tightens from `K/S = 1.40` at one year to
//! `K/S = 1.005` at one hour.
//!
//! Every one of those states is a `break` here, into a bisection that has no
//! opinion about the shape of the surface.
//!
//! # What is refused rather than returned
//!
//! `CLAUDE.md` §4 bans a fallback that hides a failure. Twenty-one points on
//! the calibration grid are not recoverable at any cost — at `K/S = 0.75`
//! Newton "converges" to answers wrong by a relative `1.279`, `4.942` and
//! `28.9`. Those are refused here, by [`GreeksError::Indeterminate`], on a
//! criterion with a number in it: one unit in the last place of the market
//! price must not move the answer by more than
//! [`MAX_RELATIVE_UNCERTAINTY`] of itself.
//!
//! That criterion is **necessary and not sufficient**, and saying otherwise
//! would be the dishonesty rule 6 is about. `ulp(price)/vega` is the textbook
//! estimate and it is *optimistic*: measured against the smallest interval
//! around the true volatility on which the computed price is still strictly
//! ordered, it understates the real floor by two orders of magnitude at the
//! money — `1.60e-16` estimated against `1.67e-14` measured at seven days,
//! `1.29e-16` against `2.73e-13` at one hour. Passing this check does not
//! prove the answer is good to `MAX_RELATIVE_UNCERTAINTY`. Failing it proves
//! the answer is worthless. See `docs/06-limits.md` §18.

// Every expression below is a float expression. An implied volatility is a
// solved statistical parameter, not money; `CLAUDE.md` §7 keeps such values
// at full precision. No paisa reaches this file. See D-0036.
#![allow(clippy::float_arithmetic)]

use std::f64::consts::TAU;

use crate::bsm::{Checked, Contract, Greeks, OptionKind, finite};
use crate::error::GreeksError;

/// The lowest volatility the solver will look at.
pub const MIN_VOLATILITY: f64 = 1.0e-6;

/// The highest volatility the solver will look at.
///
/// Deliberately below [`crate::bsm::MAX_VOLATILITY`]: pricing needs no
/// bracket, and a root find needs one it can defend.
pub const MAX_VOLATILITY: f64 = 5.0;

/// The hard cap on Newton steps before the bisection takes over.
pub const NEWTON_STEPS: u32 = 8;

/// The number of halvings the bisection performs. Not a cap — a count. It
/// performs exactly this many, every time.
pub const BISECTION_STEPS: u32 = 64;

/// The largest total number of model evaluations one solve can cost.
pub const MAX_ITERATIONS: u32 = NEWTON_STEPS + BISECTION_STEPS;

/// A Newton step this small ends the search.
const NEWTON_STEP_TOLERANCE: f64 = 1.0e-12;

/// One unit in the last place of the market price may move the answer by this
/// fraction of itself, and no more. Past it the price does not determine a
/// volatility and [`GreeksError::Indeterminate`] is returned.
pub const MAX_RELATIVE_UNCERTAINTY: f64 = 1.0e-3;

/// Which method produced the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// Newton-Raphson converged inside its cap.
    Newton,
    /// Newton did not, and the bisection finished the job.
    Bisection,
}

/// A solved implied volatility, with the evidence that it means something.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpliedVolatility {
    /// The volatility, as a decimal — `0.0979`, not `9.79`.
    pub volatility: f64,
    /// Which method finished.
    pub method: Method,
    /// Model evaluations spent, Newton and bisection together. Never above
    /// [`MAX_ITERATIONS`].
    pub iterations: u32,
    /// Vega at the solution, which is what makes the next field checkable.
    pub vega: f64,
    /// How much of itself the answer moves for one unit in the last place of
    /// the market price. Below [`MAX_RELATIVE_UNCERTAINTY`], or this would be
    /// an error instead.
    pub uncertainty: f64,
}

impl Contract {
    /// Solves for the volatility that reproduces `market_price`.
    ///
    /// # Errors
    ///
    /// Every refusal of [`Contract::greeks`], plus:
    /// [`GreeksError::PriceBelowIntrinsic`] and
    /// [`GreeksError::PriceAboveMaximum`] for a quote outside the arbitrage
    /// bounds, [`GreeksError::OutsideVolatilityRange`] for one inside them but
    /// outside `[MIN_VOLATILITY, MAX_VOLATILITY]`, and
    /// [`GreeksError::Indeterminate`] for one that does not pin a volatility
    /// down at all.
    ///
    /// ```
    /// use greeks::{Contract, OptionKind};
    ///
    /// let contract = Contract {
    ///     spot: 25_851.19,
    ///     strike: 25_850.0,
    ///     years_to_expiry: 7.0 / 365.0,
    ///     rate: 0.0946,
    ///     carry: 0.0,
    /// };
    /// let quoted = contract.price(0.098, OptionKind::Call).expect("in range");
    /// let solved = contract
    ///     .implied_volatility(quoted, OptionKind::Call)
    ///     .expect("a quote the model can reach");
    ///
    /// assert!((solved.volatility - 0.098).abs() < 1e-12);
    /// assert!(solved.iterations <= greeks::solver::MAX_ITERATIONS);
    /// ```
    pub fn implied_volatility(
        &self,
        market_price: f64,
        kind: OptionKind,
    ) -> Result<ImpliedVolatility, GreeksError> {
        let checked = self.check()?;
        let price = finite(market_price, "market_price")?;

        let (intrinsic, maximum) = checked.no_arbitrage_bounds(kind);
        if price <= intrinsic {
            return Err(GreeksError::PriceBelowIntrinsic { price, intrinsic });
        }
        if price >= maximum {
            return Err(GreeksError::PriceAboveMaximum { price, maximum });
        }

        let lowest = checked.greeks(MIN_VOLATILITY, kind);
        let highest = checked.greeks(MAX_VOLATILITY, kind);
        if ![lowest, highest].iter().all(Greeks::is_finite) {
            return Err(GreeksError::NotRepresentable);
        }
        if price <= lowest.price {
            return Err(GreeksError::OutsideVolatilityRange {
                price,
                at_lowest: lowest.price,
                at_highest: highest.price,
            });
        }
        if price >= highest.price {
            return Err(GreeksError::OutsideVolatilityRange {
                price,
                at_lowest: lowest.price,
                at_highest: highest.price,
            });
        }

        let (volatility, method, iterations) = checked.search(price, kind);
        let solved = checked.greeks(volatility, kind);

        // One unit in the last place of the quote, expressed as a fraction of
        // the answer it would move.
        //
        // `>` and not `!(<=)`, and the difference is only safe because of what
        // is above it. `d1` is finite for every volatility in the band once
        // the two bracket ends were finite, so vega is finite and
        // non-negative, so this quotient is never a NaN -- it is `+inf` when
        // vega underflows to zero, and `+inf > bound` is true, so the
        // degenerate case is still refused. A NaN would slip through a `>`,
        // and there is no path to one here.
        let uncertainty = price.abs() * f64::EPSILON / solved.vega / volatility;
        if uncertainty > MAX_RELATIVE_UNCERTAINTY {
            return Err(GreeksError::Indeterminate {
                volatility,
                vega: solved.vega,
                uncertainty,
                bound: MAX_RELATIVE_UNCERTAINTY,
            });
        }

        Ok(ImpliedVolatility {
            volatility,
            method,
            iterations,
            vega: solved.vega,
            uncertainty,
        })
    }
}

/// The Brenner-Subrahmanyam starting guess, `sigma ~ sqrt(2 pi / T) · C / S`.
///
/// Near-exact at the money — at `S = K = 100, T = 0.25, sigma = 0.2` it lands
/// within 0.05% — and a rough guess everywhere else, which is all a seed has
/// to be. It is a **separate named function** so that a test can assert the
/// number it produces: folded into the search it was only ever exercised
/// through an answer that every starting point reaches, so any arithmetic in
/// it could be wrong without a single test noticing.
fn brenner_subrahmanyam(price: f64, spot: f64, years: f64) -> f64 {
    (TAU / years).sqrt() * price / spot
}

impl Checked {
    /// Newton first, with a hard cap; the bisection when Newton stops for any
    /// of the three reasons it can stop.
    fn search(&self, price: f64, kind: OptionKind) -> (f64, Method, u32) {
        // Clamped, because a seed is not an answer.
        let mut volatility = brenner_subrahmanyam(price, self.spot, self.years)
            .clamp(MIN_VOLATILITY, MAX_VOLATILITY);
        let mut spent = 0_u32;
        let mut converged = false;

        for _ in 0..NEWTON_STEPS {
            let at = self.greeks(volatility, kind);
            spent += 1;
            // Vega underflowed. The tangent is horizontal and Newton has
            // nothing to stand on. `<= 0.0` rather than `!(> 0.0)`: vega is
            // a product of non-negative finite factors here, so it is never a
            // NaN -- and a NaN would still leave on the next line, because
            // `contains` is false for one.
            if at.vega <= 0.0 {
                break;
            }
            let step = (at.price - price) / at.vega;
            let next = volatility - step;
            // The step left the band -- diverging, or walking off the top.
            // `contains` is false for a NaN or an infinity too.
            if !(MIN_VOLATILITY..=MAX_VOLATILITY).contains(&next) {
                break;
            }
            volatility = next;
            if step.abs() <= NEWTON_STEP_TOLERANCE {
                converged = true;
                break;
            }
        }

        if converged {
            return (volatility, Method::Newton, spent);
        }
        let bisected = self.bisect(price, kind);
        (bisected, Method::Bisection, spent + BISECTION_STEPS)
    }

    /// Exactly [`BISECTION_STEPS`] halvings. No early exit, no tolerance test,
    /// no stagnation detection: the cost of this function does not depend on
    /// any input value, and the final bracket is narrower than one unit in the
    /// last place of anything inside it.
    ///
    /// The caller has already established that the price is strictly between
    /// the model prices at the two ends, so the bracket is real before the
    /// first halving and stays real after every one of them.
    fn bisect(&self, price: f64, kind: OptionKind) -> f64 {
        let mut low = MIN_VOLATILITY;
        let mut high = MAX_VOLATILITY;
        for _ in 0..BISECTION_STEPS {
            let middle = 0.5 * (low + high);
            if self.greeks(middle, kind).price < price {
                low = middle;
            } else {
                high = middle;
            }
        }
        0.5 * (low + high)
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
mod tests {
    use super::{
        BISECTION_STEPS, Contract, GreeksError, MAX_ITERATIONS, MAX_RELATIVE_UNCERTAINTY,
        MAX_VOLATILITY, MIN_VOLATILITY, Method, NEWTON_STEPS, OptionKind,
    };
    use crate::bsm::tests::grid;
    use std::collections::HashSet;

    fn at_the_money() -> Contract {
        Contract {
            spot: 100.0,
            strike: 100.0,
            years_to_expiry: 1.0,
            rate: 0.0,
            carry: 0.0,
        }
    }

    #[test]
    fn a_price_round_trips_through_the_solver_and_back() {
        // price -> IV -> price. Every point either round-trips or is refused
        // for one of the two reasons this crate is allowed to refuse for; a
        // silent third outcome fails here.
        let mut solved = 0_u32;
        let mut refused = 0_u32;
        let mut below_intrinsic = 0_u32;
        let mut worst = 0.0_f64;
        for (contract, volatility) in grid() {
            for kind in [OptionKind::Call, OptionKind::Put] {
                let quoted = contract.price(volatility, kind).expect("priced");
                match contract.implied_volatility(quoted, kind) {
                    Ok(found) => {
                        solved += 1;
                        let back = contract.price(found.volatility, kind).expect("repriced");
                        let relative = (back - quoted).abs() / quoted.abs().max(1e-12);
                        worst = worst.max(relative);
                        assert!(
                            relative <= 1e-6,
                            "round trip {relative:e} at {contract:?} sigma {volatility} {kind:?}"
                        );
                    }
                    // The refusals are COUNTED, not matched against a
                    // catch-all that panics. A catch-all arm no input reaches
                    // is a region no run executes, and the coverage gate
                    // reports it -- the same trap as an assertion message that
                    // computes something. Requiring the counted kind to
                    // account for every refusal says the same thing, and every
                    // line of it runs.
                    //
                    // ONE kind is expected here, and it is asserted as one:
                    // deep in the money at five percent volatility the model
                    // price IS the discounted intrinsic value in f64, so no
                    // volatility recovers it. A refusal of any other kind
                    // arriving from this grid breaks the equality below.
                    Err(error) => {
                        refused += 1;
                        below_intrinsic += 1;
                        assert!(!error.to_string().is_empty());
                        let checked = contract.check().expect("checked");
                        let (intrinsic, _) = checked.no_arbitrage_bounds(kind);
                        assert_eq!(
                            error,
                            GreeksError::PriceBelowIntrinsic {
                                price: quoted,
                                intrinsic
                            },
                            "a refusal arrived from this grid that is not the arbitrage \
                             floor, at {contract:?} sigma {volatility} {kind:?}"
                        );
                    }
                }
            }
        }
        println!(
            "round trip: {solved} solved, {refused} refused \
             ({below_intrinsic} of them at the arbitrage floor), \
             worst relative {worst:e}"
        );
        assert_eq!(
            below_intrinsic, refused,
            "a refusal arrived from this grid that is not the arbitrage floor"
        );
        // The grid deliberately contains points no solver can recover. A run
        // that suddenly solves all of them, or none, is a change worth seeing.
        assert!(solved >= 500, "only {solved} solved");
        assert!(
            refused > 0,
            "the grid must still contain unrecoverable points"
        );
    }

    #[test]
    fn the_iteration_count_never_exceeds_the_arithmetic_bound() {
        let mut worst = 0_u32;
        let mut newton_only = 0_u32;
        let mut bisected = 0_u32;
        for (contract, volatility) in grid() {
            for kind in [OptionKind::Call, OptionKind::Put] {
                let quoted = contract.price(volatility, kind).expect("priced");
                if let Ok(found) = contract.implied_volatility(quoted, kind) {
                    worst = worst.max(found.iterations);
                    match found.method {
                        Method::Newton => newton_only += 1,
                        Method::Bisection => bisected += 1,
                    }
                }
            }
        }
        println!(
            "iterations: worst {worst} of a bound of {MAX_ITERATIONS}; \
             {newton_only} finished in Newton, {bisected} needed the bisection"
        );
        assert!(worst <= MAX_ITERATIONS, "worst {worst}");
        assert!(newton_only > 0, "Newton never finished a single point");
        assert!(bisected > 0, "the bisection was never exercised");
    }

    #[test]
    fn the_seed_lands_next_to_the_answer_at_the_money() {
        // The seed only ever affects how LONG the search takes, never what it
        // returns -- so nothing that asserts an answer can see it being wrong.
        // This asserts the seed itself.
        //
        // The maturity is deliberately not 1.0. At one year `TAU / years` and
        // `TAU * years` are the same number, and half the arithmetic in the
        // formula would be unfalsifiable.
        let spot = 100.0_f64;
        let years = 0.25_f64;
        let truth = 0.2_f64;
        let contract = Contract {
            spot,
            strike: spot,
            years_to_expiry: years,
            rate: 0.0,
            carry: 0.0,
        };
        let quoted = contract.price(truth, OptionKind::Call).expect("priced");
        let seed = super::brenner_subrahmanyam(quoted, spot, years);
        let relative = (seed / truth - 1.0).abs();
        println!("Brenner-Subrahmanyam seed {seed} against {truth}: {relative:e}");
        assert!(
            relative < 0.01,
            "seed {seed} is {relative} away from {truth}"
        );
        // And it earns its place: at the money the search finishes in Newton,
        // in a handful of steps, and never reaches the bisection.
        let found = contract
            .implied_volatility(quoted, OptionKind::Call)
            .expect("solved");
        assert_eq!(found.method, Method::Newton);
        assert!(found.iterations <= 4, "took {} steps", found.iterations);
    }

    #[test]
    fn the_iteration_count_is_the_work_actually_done() {
        // `iterations <= MAX_ITERATIONS` is satisfied by zero, so it cannot
        // tell a counter that counts from one that does not. Both floors are
        // asserted here: a Newton solve spends at least one evaluation, and a
        // bisection solve spends its 64 halvings PLUS the Newton steps that
        // preceded them.
        let contract = at_the_money();
        let quoted = contract.price(0.2, OptionKind::Call).expect("priced");
        let newton = contract
            .implied_volatility(quoted, OptionKind::Call)
            .expect("solved");
        assert_eq!(newton.method, Method::Newton);
        assert!(newton.iterations >= 1, "{} evaluations", newton.iterations);
        assert!(newton.iterations <= NEWTON_STEPS);

        // Deep in the money at a low volatility, where Newton crawls towards
        // the answer and is still crawling when its cap arrives: the full
        // 8 + 64.
        let stubborn = Contract {
            spot: 100.0,
            strike: 50.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        let quoted = stubborn.price(0.2, OptionKind::Call).expect("priced");
        let exhausted = stubborn
            .implied_volatility(quoted, OptionKind::Call)
            .expect("solved");
        assert_eq!(exhausted.method, Method::Bisection);
        assert_eq!(exhausted.iterations, MAX_ITERATIONS);

        // And the other end of the same arm: far out of the money at a low
        // volatility, vega underflows on the FIRST evaluation and Newton
        // leaves immediately. One Newton step plus the 64 halvings.
        let hopeless = Contract {
            spot: 100.0,
            strike: 125.0,
            years_to_expiry: 1.0,
            rate: 0.0,
            carry: 0.0,
        };
        let quoted = hopeless.price(0.05, OptionKind::Call).expect("priced");
        let short = hopeless
            .implied_volatility(quoted, OptionKind::Call)
            .expect("solved");
        assert_eq!(short.method, Method::Bisection);
        assert_eq!(short.iterations, BISECTION_STEPS + 1);
        assert!(short.iterations > BISECTION_STEPS);
    }

    #[test]
    fn the_solver_is_idempotent_to_the_bit() {
        // CLAUDE.md section 3 rule 5. Same inputs, same output, byte for byte.
        let contract = at_the_money();
        let quoted = contract.price(0.23, OptionKind::Call).expect("priced");
        let first = contract
            .implied_volatility(quoted, OptionKind::Call)
            .expect("solved");
        for _ in 0..8 {
            let again = contract
                .implied_volatility(quoted, OptionKind::Call)
                .expect("solved");
            assert_eq!(first.volatility.to_bits(), again.volatility.to_bits());
            assert_eq!(first.iterations, again.iterations);
            assert_eq!(first.method, again.method);
        }
    }

    #[test]
    fn a_price_at_or_below_the_discounted_intrinsic_is_refused_on_both_sides() {
        let contract = Contract {
            spot: 100.0,
            strike: 90.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        let checked = contract.check().expect("checked");
        let (call_floor, _) = checked.no_arbitrage_bounds(OptionKind::Call);
        // Asserted as an EQUALITY rather than through `matches!`. A bare
        // variant pattern compiles to a discriminant switch whose "no match"
        // arm no run reaches, and the coverage gate reports that arm. An
        // equality also checks the numbers the refusal carries, which is what
        // the operator reads.
        assert_eq!(
            contract
                .implied_volatility(call_floor, OptionKind::Call)
                .unwrap_err(),
            GreeksError::PriceBelowIntrinsic {
                price: call_floor,
                intrinsic: call_floor
            }
        );
        // A deep out-of-the-money put has an intrinsic value of zero, so a
        // zero or negative quote lands in the same refusal with `intrinsic`
        // printed as zero. That is the arbitrage case, named.
        let error = contract
            .implied_volatility(-1.0, OptionKind::Put)
            .unwrap_err();
        assert_eq!(
            error,
            GreeksError::PriceBelowIntrinsic {
                price: -1.0,
                intrinsic: 0.0
            }
        );
    }

    #[test]
    fn a_price_at_or_above_the_model_supremum_is_refused_on_both_sides() {
        let contract = at_the_money();
        let checked = contract.check().expect("checked");
        for kind in [OptionKind::Call, OptionKind::Put] {
            let (_, maximum) = checked.no_arbitrage_bounds(kind);
            // At the supremum, and well past it. The bound is exclusive: the
            // model reaches it only in the limit.
            assert_eq!(
                contract.implied_volatility(maximum, kind).unwrap_err(),
                GreeksError::PriceAboveMaximum {
                    price: maximum,
                    maximum
                }
            );
            assert_eq!(
                contract
                    .implied_volatility(maximum * 2.0, kind)
                    .unwrap_err(),
                GreeksError::PriceAboveMaximum {
                    price: maximum * 2.0,
                    maximum
                }
            );
        }
    }

    #[test]
    fn a_price_outside_the_searched_band_is_refused_rather_than_clamped_into_it() {
        let contract = at_the_money();
        let checked = contract.check().expect("checked");
        let at_lowest = checked.greeks(MIN_VOLATILITY, OptionKind::Call).price;
        let at_highest = checked.greeks(MAX_VOLATILITY, OptionKind::Call).price;
        // Below the band: a volatility of 1e-7, a tenth of MIN_VOLATILITY.
        let too_quiet = contract
            .price(MIN_VOLATILITY / 10.0, OptionKind::Call)
            .expect("priced");
        assert_eq!(
            contract
                .implied_volatility(too_quiet, OptionKind::Call)
                .unwrap_err(),
            GreeksError::OutsideVolatilityRange {
                price: too_quiet,
                at_lowest,
                at_highest
            }
        );
        // Above it: a volatility of 8, well past MAX_VOLATILITY and still
        // inside the arbitrage bounds.
        let too_wild = contract
            .price(MAX_VOLATILITY + 3.0, OptionKind::Call)
            .expect("priced");
        assert_eq!(
            contract
                .implied_volatility(too_wild, OptionKind::Call)
                .unwrap_err(),
            GreeksError::OutsideVolatilityRange {
                price: too_wild,
                at_lowest,
                at_highest
            }
        );
    }

    #[test]
    fn a_non_finite_market_price_is_refused_before_anything_is_computed() {
        let contract = at_the_money();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                contract
                    .implied_volatility(bad, OptionKind::Call)
                    .unwrap_err(),
                GreeksError::NotFinite {
                    field: "market_price"
                }
            );
        }
    }

    #[test]
    fn a_broken_contract_is_refused_before_the_search_starts() {
        let mut broken = at_the_money();
        broken.strike = 0.0;
        assert!(matches!(
            broken
                .implied_volatility(1.0, OptionKind::Call)
                .unwrap_err(),
            GreeksError::NotPositive {
                field: "strike",
                ..
            }
        ));
        // In range, and the model still cannot produce a finite bracket end.
        let degenerate = Contract {
            spot: 1e-300,
            strike: 1e-290,
            years_to_expiry: 1e-300,
            rate: 0.0,
            carry: 0.0,
        };
        // Strictly inside the arbitrage bounds -- the ceiling here is the
        // spot itself -- so the refusal comes from the model evaluation and
        // not from the cheaper check in front of it.
        assert_eq!(
            degenerate
                .implied_volatility(1e-305, OptionKind::Call)
                .unwrap_err(),
            GreeksError::NotRepresentable
        );
    }

    #[test]
    fn a_price_that_does_not_determine_a_volatility_is_refused_not_returned() {
        // Deep in the money, one year out. The price here is the discounted
        // intrinsic value plus a handful of units in the last place, and the
        // whole of the option's time value is those few units -- vega at the
        // solution is about 3e-11 against a price of 52.4. A solver WILL
        // return a number for this; the calibration measured Newton
        // "converging" on cases like it to answers wrong by a relative 1.279,
        // 4.942 and 28.9. Returning one is the fallback CLAUDE.md section 4
        // bans, so it is refused with the two numbers that make the refusal
        // checkable.
        let contract = Contract {
            spot: 100.0,
            strike: 50.0,
            years_to_expiry: 1.0,
            rate: 0.05,
            carry: 0.0,
        };
        // The model's own floor, then four units in the last place above it.
        // Built from the model rather than typed in, so it stays on the right
        // side of the arbitrage check whatever the last digit does.
        let floor = contract
            .price(MIN_VOLATILITY, OptionKind::Call)
            .expect("priced");
        let quote = floor + 4.0 * floor * f64::EPSILON;
        assert!(quote > floor, "the quote must sit above the floor");
        let error = contract
            .implied_volatility(quote, OptionKind::Call)
            .unwrap_err();
        println!("indeterminate case: {error}");
        // The `if let` is walked BOTH ways -- the degenerate quote matches and
        // an ordinary refusal does not -- so neither path is a region no run
        // executes. Exactly one of the two must match.
        let ordinary_refusal = contract
            .implied_volatility(-1.0, OptionKind::Call)
            .unwrap_err();
        let mut seen = 0_u32;
        for candidate in [error, ordinary_refusal] {
            if let GreeksError::Indeterminate {
                volatility,
                vega,
                uncertainty,
                bound,
            } = candidate
            {
                seen += 1;
                assert!(volatility > MIN_VOLATILITY, "volatility {volatility}");
                assert!(vega < 1e-6, "vega {vega} is not degenerate at all");
                assert!(uncertainty > bound, "{uncertainty} against {bound}");
                assert_eq!(bound, MAX_RELATIVE_UNCERTAINTY);
            }
        }
        assert_eq!(
            seen, 1,
            "expected a refusal about indeterminacy, got {error}"
        );
        // And the same contract at an ordinary volatility is answered, so the
        // refusal is about this quote and not about this contract.
        let ordinary = contract.price(0.35, OptionKind::Call).expect("priced");
        let found = contract
            .implied_volatility(ordinary, OptionKind::Call)
            .expect("an ordinary quote is answerable");
        assert!((found.volatility - 0.35).abs() < 1e-9);
    }

    #[test]
    fn the_result_is_an_ordinary_value() {
        let contract = at_the_money();
        let quoted = contract.price(0.2, OptionKind::Call).expect("priced");
        let found = contract
            .implied_volatility(quoted, OptionKind::Call)
            .expect("solved");
        assert_eq!(found, found.clone());
        assert!(!format!("{found:?}").is_empty());
        assert!(!format!("{:?}", Method::Bisection).is_empty());
        assert_ne!(Method::Newton, Method::Bisection);
        let methods: HashSet<Method> = [Method::Newton, Method::Bisection].into_iter().collect();
        assert_eq!(methods.len(), 2);
        assert!(found.iterations <= NEWTON_STEPS + BISECTION_STEPS);
        assert!(found.uncertainty <= MAX_RELATIVE_UNCERTAINTY);
        assert!(found.vega > 0.0);
    }
}
