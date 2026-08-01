//! The standard normal density and distribution.
//!
//! # Which approximation, and why that one
//!
//! Three candidates were measured against a reference built from an
//! all-positive-term error-function series below `|x| = 1.5` and a backward
//! continued fraction for `erfc` above it — a reference that keeps *relative*
//! accuracy in the left tail, which is the only thing deep out-of-the-money
//! pricing cares about.
//!
//! | CDF | max abs err | max rel err | ns/call |
//! |---|---|---|---|
//! | reference (series + continued fraction) | agrees with published values to `3.6e-15` | — | 423–497 |
//! | **Hart 5666 rational — shipped here** | **2.220446049250313e-16** | **8.911872737504238e-9** (at `x = −7.78`) | **3.3** |
//! | Abramowitz & Stegun 7.1.26 | 6.97e-8 | 1.01 | — |
//!
//! The two Hart figures are what
//! `greeks::normal::hart_agrees_with_an_independent_reference` prints on this
//! machine, over 4,001 points from −20 to +20. The timings are the
//! calibration's, taken with a different harness, and are not re-measured
//! here.
//!
//! A&S 7.1.26 is the one everybody reaches for and it is unusable here: its
//! published `1.5e-7` bound on `erf` measures as `6.97e-8` on `N(x)`, which is
//! 7.2 absolute digits and **zero** relative digits in the tail. A CDF with no
//! relative accuracy in the tail cannot support an implied-volatility solve at
//! all — the price it produces for a far strike is noise.
//!
//! Hart is 145× faster than the reference and caps at roughly eight relative
//! digits. That is the trade this crate makes, stated rather than assumed:
//! `hart_agrees_with_an_independent_reference` measures it on every run.
//!
//! The reference itself is not shipped. It lives in this module's test
//! section, where it is the oracle rather than the implementation — a second
//! implementation of the same function is only worth its cost when the two
//! disagree loudly.

// Every expression below is a float expression. `CLAUDE.md` §7 makes a float
// wrong for a PRICE; this module computes a probability, which is a
// statistical value and has no representation on a tick grid. No paisa value
// reaches this file. See the crate documentation and D-0036.
#![allow(clippy::float_arithmetic)]

/// `1 / sqrt(2 * pi)`.
const INV_ROOT_TWO_PI: f64 = 0.398_942_280_401_432_7;

/// `sqrt(2 * pi)`.
const ROOT_TWO_PI: f64 = 2.506_628_274_631_000_7;

/// The cut past which the distribution saturates. **Exclusive**: at exactly
/// this value the distribution is still computed, and `N(-37)` measures
/// `5.725571222524049e-300`, which is not zero.
///
/// Past it, the density has underflowed and every product a caller could form
/// with the result has too, so `0` and `1` are the answers rather than
/// approximations to them.
const TAIL: f64 = 37.0;

/// Hart's split between the rational branch and the continued fraction, at
/// `5 * sqrt(2)`.
const SPLIT: f64 = 7.071_067_811_865_475;

// Hart 5666 numerator, lowest power first.
const A0: f64 = 220.206_867_912_376;
const A1: f64 = 221.213_596_169_931;
const A2: f64 = 112.079_291_497_871;
const A3: f64 = 33.912_866_078_383;
const A4: f64 = 6.373_962_203_531_65;
const A5: f64 = 0.700_383_064_443_688;
const A6: f64 = 0.035_262_496_599_891_1;

// Hart 5666 denominator, lowest power first.
const B0: f64 = 440.413_735_824_752;
const B1: f64 = 793.826_512_519_948;
const B2: f64 = 637.333_633_378_831;
const B3: f64 = 296.564_248_779_674;
const B4: f64 = 86.780_732_202_946_1;
const B5: f64 = 16.064_177_579_207;
const B6: f64 = 1.755_667_163_182_64;
const B7: f64 = 0.088_388_347_648_318_4;

/// The standard normal density, `n(x)`.
///
/// A non-finite argument yields a non-finite result. Every caller inside this
/// crate validates its inputs before reaching here; a caller outside it is
/// expected to do the same, or to check the answer.
#[must_use]
pub fn standard_normal_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() * INV_ROOT_TWO_PI
}

/// The standard normal distribution, `N(x)`, by Hart's 5666 rational
/// approximation.
///
/// Accurate to `2.22e-16` absolute everywhere and to roughly eight relative
/// digits in the far tail — see the module documentation for the measurement
/// and for what that rules out.
///
/// A non-finite argument yields a non-finite result, for the same reason as
/// [`standard_normal_pdf`].
#[must_use]
pub fn standard_normal_cdf(x: f64) -> f64 {
    let z = x.abs();
    if z > TAIL {
        // Not an approximation: past the cut the density has underflowed, so
        // the limits ARE the values.
        //
        // The sign, not an ordering. `x > 0.0` and `x >= 0.0` differ only at
        // zero, and zero cannot reach this line -- so the comparison would
        // have a boundary no test could ever stand on, which is a mutant
        // nobody can kill. Asking for the sign has no such boundary.
        return if x.is_sign_negative() { 0.0 } else { 1.0 };
    }
    let decay = (-0.5 * z * z).exp();
    // The upper tail `N(-z)`, always the small side, so it is computed
    // directly and never as `1 - something`.
    let upper = if z < SPLIT {
        let numerator = ((((((A6 * z + A5) * z + A4) * z + A3) * z + A2) * z + A1) * z) + A0;
        let denominator =
            (((((((B7 * z + B6) * z + B5) * z + B4) * z + B3) * z + B2) * z + B1) * z) + B0;
        decay * numerator / denominator
    } else {
        let fraction = z + 1.0 / (z + 2.0 / (z + 3.0 / (z + 4.0 / (z + 0.65))));
        decay / (fraction * ROOT_TWO_PI)
    };
    // The sign again, for the same reason as the tail above. At exactly zero
    // `upper` is exactly `0.5`, so `upper` and `1.0 - upper` are the same
    // number and no test could ever tell `>` from `>=` here.
    if x.is_sign_negative() {
        upper
    } else {
        1.0 - upper
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
    use super::{SPLIT, TAIL, standard_normal_cdf, standard_normal_pdf};

    const ROOT_TWO: f64 = std::f64::consts::SQRT_2;

    /// `sqrt(pi)`, derived rather than transcribed.
    ///
    /// Not a stylistic preference. A hand-typed constant is a hand-typed
    /// constant, and this module already caught one: `B6` was entered as
    /// `1.755_667_161_832_64` for `1.755_667_163_182_64`, two digits
    /// transposed, which left the tail exactly right and put `2.4e-13` of
    /// absolute error in the body — invisible to every check except a
    /// second implementation.
    fn root_pi() -> f64 {
        std::f64::consts::PI.sqrt()
    }

    /// `erf(x)` by the all-positive-term series
    /// `erf(x) = (2x/sqrt(pi)) e^-x^2 * sum (2x^2)^n / (2n+1)!!`.
    ///
    /// The textbook alternating Taylor series cancels catastrophically; this
    /// rearrangement has no subtraction in it at all. The iteration count is
    /// **fixed**, not conditional: with `|x| < 1.5/sqrt(2)` the terms fall
    /// beneath the rounding of the sum long before term 60, and a fixed count
    /// removes a branch whose boundary nobody could test.
    fn reference_erf(x: f64) -> f64 {
        let squared = x * x;
        let mut term = 1.0_f64;
        let mut sum = 1.0_f64;
        for n in 1..=60_u32 {
            term *= 2.0 * squared / (2.0 * f64::from(n) + 1.0);
            sum += term;
        }
        2.0 * x * (-squared).exp() * sum / root_pi()
    }

    /// `erfc(z)` by the backward continued fraction
    /// `erfc(z) = e^-z^2 / (sqrt(pi) (z + (1/2)/(z + 1/(z + (3/2)/(z + ...)))))`.
    ///
    /// Evaluated from the deepest level upward, which is the direction that
    /// converges. Fixed depth, for the same reason as the series.
    fn reference_erfc(z: f64) -> f64 {
        let mut fraction = 0.0_f64;
        for n in (1..=400_u32).rev() {
            fraction = 0.5 * f64::from(n) / (z + fraction);
        }
        (-z * z).exp() / (root_pi() * (z + fraction))
    }

    /// The oracle. Series where the series is stable, continued fraction where
    /// it is not, and the small tail computed directly so that relative
    /// accuracy survives.
    pub(crate) fn reference_cdf(x: f64) -> f64 {
        if x.abs() < 1.5 {
            0.5 * (1.0 + reference_erf(x / ROOT_TWO))
        } else if x < 0.0 {
            0.5 * reference_erfc(-x / ROOT_TWO)
        } else {
            1.0 - 0.5 * reference_erfc(x / ROOT_TWO)
        }
    }

    #[test]
    fn the_cdf_matches_known_values() {
        // Published values of the standard normal distribution. The tolerance
        // is RELATIVE, because an absolute tolerance says nothing at all
        // about a number near 1e-24.
        let known: [(f64, f64); 12] = [
            (0.0, 0.5),
            (0.5, 0.691_462_461_274_013_1),
            (-0.5, 0.308_537_538_725_986_9),
            (1.0, 0.841_344_746_068_542_9),
            (-1.0, 0.158_655_253_931_457_1),
            (1.96, 0.975_002_104_851_780_2),
            (2.0, 0.977_249_868_051_820_8),
            (-2.0, 0.022_750_131_948_179_2),
            (3.0, 0.998_650_101_968_369_9),
            (-3.0, 0.001_349_898_031_630_095),
            (-5.0, 2.866_515_718_791_939e-7),
            (-10.0, 7.619_853_024_160_526e-24),
        ];
        for (x, expected) in known {
            let got = standard_normal_cdf(x);
            let relative = (got - expected).abs() / expected;
            assert!(
                relative <= 1e-7,
                "N({x}) = {got}, published {expected}, relative error {relative}"
            );
        }
        // The one exact value: the distribution is symmetric about zero.
        assert_eq!(standard_normal_cdf(0.0), 0.5);
    }

    #[test]
    fn hart_agrees_with_an_independent_reference() {
        // 4001 points from -20 to +20, across both Hart branches and both
        // reference branches. The two implementations share no line of code.
        let mut worst_absolute = 0.0_f64;
        let mut worst_relative = 0.0_f64;
        let mut at = 0.0_f64;
        for i in 0..=4000_i32 {
            let x = f64::from(i) * 0.01 - 20.0;
            let got = standard_normal_cdf(x);
            let want = reference_cdf(x);
            worst_absolute = worst_absolute.max((got - want).abs());
            let relative = (got - want).abs() / want;
            if relative > worst_relative {
                worst_relative = relative;
                at = x;
            }
        }
        println!("worst absolute {worst_absolute:e}, worst relative {worst_relative:e} at {at}");
        assert!(worst_absolute <= 1e-15, "absolute {worst_absolute:e}");
        // Eight relative digits is what Hart buys. Asserting more would be
        // asserting something this approximation does not do.
        assert!(
            worst_relative <= 1e-8,
            "relative {worst_relative:e} at {at}"
        );
    }

    #[test]
    fn both_hart_branches_and_both_saturating_tails_are_exercised() {
        // The rational branch, the continued-fraction branch, and the two
        // saturating tails. Named so a reader can see the branch structure is
        // covered on purpose rather than by luck.
        assert!(standard_normal_cdf(SPLIT - 0.5) < 1.0);
        assert!(standard_normal_cdf(-(SPLIT + 0.5)) > 0.0);
        assert_eq!(standard_normal_cdf(TAIL + 3.0), 1.0);
        assert_eq!(standard_normal_cdf(-(TAIL + 3.0)), 0.0);

        // The tail cut is EXCLUSIVE, and this is where that is stated. At
        // exactly `TAIL` the distribution is still computed, and `N(-37)` is
        // about `5.7e-300` -- not zero. A cut one step wider would return zero
        // here and nothing else in this file would notice.
        let at_the_cut = standard_normal_cdf(-TAIL);
        println!("N(-{TAIL}) = {at_the_cut:e}");
        assert!(at_the_cut > 0.0, "N(-{TAIL}) saturated to zero");
        assert!(
            (at_the_cut - reference_cdf(-TAIL)).abs() / at_the_cut <= 1e-8,
            "N(-{TAIL}) = {at_the_cut:e} against the reference"
        );
        assert_eq!(standard_normal_cdf(TAIL), 1.0 - at_the_cut);

        // Continued fraction and rational branch must meet at the split, and
        // the split must be WHERE IT IS DOCUMENTED. The step across it is
        // bounded above by Hart's own tail accuracy -- eight relative digits,
        // and asserting less would be asserting something this approximation
        // does not do -- and bounded BELOW by the fact that the two branches
        // really are two branches. Measured step: 6.6e-9. A cut moved by one
        // comparison in either direction lands outside one of the two bounds.
        let below = standard_normal_cdf(-SPLIT * (1.0 - 1e-12));
        let at_split = standard_normal_cdf(-SPLIT);
        let step = (below - at_split).abs() / at_split;
        println!("step across the split: {step:e}");
        assert!(
            step < 1e-8,
            "the two branches disagree at the split: {below:e} vs {at_split:e}"
        );
        assert!(
            step > 1e-10,
            "the split is not at {SPLIT}: {below:e} and {at_split:e} came from one branch"
        );
    }

    #[test]
    fn the_distribution_is_symmetric_and_monotone() {
        let mut previous = 0.0_f64;
        for i in 0..=800_i32 {
            let x = f64::from(i) * 0.01 - 4.0;
            let value = standard_normal_cdf(x);
            assert!(value >= previous, "not monotone at {x}");
            previous = value;
            // `total` is computed OUTSIDE the assertion on purpose. An
            // expression that appears only in a failing assertion's message
            // is a region no passing run ever executes -- the trap
            // `docs/07-o1-architecture.md` records, arriving by a third
            // route. Hoisting it makes the arithmetic unconditional.
            let total = value + standard_normal_cdf(-x);
            assert!(
                (total - 1.0).abs() <= 4.0 * f64::EPSILON,
                "N({x}) + N(-{x}) = {total}"
            );
        }
    }

    #[test]
    fn the_density_is_the_derivative_of_the_distribution() {
        // A central difference on the CDF must reproduce the PDF. This is the
        // check that catches a normalising constant being wrong in one of
        // them but not the other.
        let step = 1e-5_f64;
        for i in 0..=60_i32 {
            let x = f64::from(i) * 0.1 - 3.0;
            let slope =
                (standard_normal_cdf(x + step) - standard_normal_cdf(x - step)) / (2.0 * step);
            let density = standard_normal_pdf(x);
            assert!(
                (slope - density).abs() <= 1e-9,
                "at {x}: slope {slope}, density {density}"
            );
        }
        assert_eq!(standard_normal_pdf(0.0), 0.398_942_280_401_432_7);
    }
}
