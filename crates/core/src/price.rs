//! Prices, as integers.
//!
//! Every price, cost and profit-and-loss figure in this system is an [`i64`]
//! count of paisa. There is no float on any path from the vendor wire to the
//! store to a ranked result. `docs/05-decisions.md` D-0010 records why: a
//! float price is how a rounding difference becomes a divergent result set six
//! months later, and integers are exact, comparable, hashable and free.
//!
//! A float appears in exactly one place — the vendor sends rupees as a
//! floating-point number, so the conversion has to accept one. That conversion
//! is [`Paisa::from_rupees_half_up`], it happens once at the ingest boundary,
//! and it is the only function in this crate that touches an `f64`.

use crate::error::PriceError;

/// The tick grid is two decimal places, so one rupee is one hundred paisa.
///
/// This is the only place the number 100 means "rupee to paisa". Anywhere else
/// it would be a magic constant that a reader has to infer.
pub const PAISA_PER_RUPEE: i64 = 100;

/// A price, as a count of paisa.
///
/// Ordering is numeric ordering, so a `Paisa` can be compared, sorted and used
/// as a map key without a comparator. Arithmetic is deliberately *not*
/// implemented through [`std::ops`]: a bare `+` on two prices silently
/// produces a meaningless value when one of them is a difference and the other
/// is a level, and the checked constructors below make the intent explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Paisa(i64);

impl Paisa {
    /// The zero price.
    pub const ZERO: Self = Self(0);

    /// Wraps a raw paisa count.
    ///
    /// Use this when the value is *already* a paisa integer — a value read
    /// back from the store, or a level computed from other paisa values. Use
    /// [`Paisa::from_rupees_half_up`] when converting from a vendor's rupee
    /// float, which is a different operation with a different failure mode.
    #[must_use]
    pub const fn from_raw(paisa: i64) -> Self {
        Self(paisa)
    }

    /// The raw paisa count.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Converts a vendor's rupee figure to paisa, snapping half-up.
    ///
    /// "Half-up" means toward positive infinity: a value exactly halfway
    /// between two paisa becomes the larger one. `0.125` rupees becomes `13`
    /// paisa, and `-0.125` rupees becomes `-12` paisa. This matches
    /// `CLAUDE.md` section 7, which fixes the snap at the write boundary and
    /// nowhere else.
    ///
    /// Note that this is *not* the rounding used for computed price levels —
    /// pivots, Fibonacci rungs and the central pivot range are a separate
    /// question with a separate answer. Snapping a vendor quote and rounding a
    /// derived level are different operations and are deliberately not sharing
    /// an implementation here.
    ///
    /// # Errors
    ///
    /// Returns [`PriceError::NotFinite`] for a NaN or an infinity, and
    /// [`PriceError::OutOfRange`] when the scaled value cannot be represented
    /// as an `i64`. Both are refusals: this function never returns a
    /// substitute value, because a substituted price is a wrong price that
    /// looks right.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::price::Paisa;
    /// assert_eq!(Paisa::from_rupees_half_up(23_109.55)?.raw(), 2_310_955);
    /// # Ok::<(), brutex_core::error::PriceError>(())
    /// ```
    // This is the ONLY function in the workspace permitted to do floating-point
    // arithmetic, and the allow is written here rather than relaxed at the
    // workspace level so that any second such function is a visible, reviewable
    // addition. The vendor sends rupees as an IEEE double; something has to
    // accept it, and the whole design is that this is the only thing that does.
    // Everything downstream of the `Ok` below is an integer forever.
    #[allow(clippy::float_arithmetic)]
    pub fn from_rupees_half_up(rupees: f64) -> Result<Self, PriceError> {
        /// The scale factor as a float, pinned to the integer constant by the
        /// compile-time assertion below so the two cannot drift apart.
        const SCALE: f64 = 100.0;
        const _: () = assert!(PAISA_PER_RUPEE == 100);

        // `as` on an out-of-range float is a *saturating* cast in Rust, which
        // would quietly clamp an absurd quote to i64::MAX and hand back a
        // plausible-looking extreme price. Reject the range explicitly so the
        // failure is a refusal. The bounds are floats because i64::MAX is not
        // exactly representable in f64 — 2^63 is, and is the first excluded
        // value on the positive side.
        const LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63
        const NEG_LIMIT: f64 = -9_223_372_036_854_775_808.0;

        if !rupees.is_finite() {
            return Err(PriceError::NotFinite);
        }

        // Scale, bias, floor. Adding 0.5 then flooring is half-up for every
        // sign; `f64::round` is half-away-from-zero and would disagree with
        // this function's own doc comment on a negative tie.
        let floored = (rupees * SCALE + 0.5).floor();

        if !(NEG_LIMIT..LIMIT).contains(&floored) {
            return Err(PriceError::OutOfRange);
        }

        // Safe: the range check above proves the value fits.
        #[allow(clippy::cast_possible_truncation)]
        let paisa = floored as i64;
        Ok(Self(paisa))
    }

    /// The whole-rupee part, truncated toward zero.
    #[must_use]
    pub const fn rupees_trunc(self) -> i64 {
        self.0 / PAISA_PER_RUPEE
    }

    /// The paisa remainder, carrying the sign of the price.
    #[must_use]
    pub const fn paisa_part(self) -> i64 {
        self.0 % PAISA_PER_RUPEE
    }

    /// Adds two paisa counts, refusing to wrap.
    ///
    /// # Errors
    ///
    /// Returns [`PriceError::Overflow`] on `i64` overflow.
    pub const fn checked_add(self, other: Self) -> Result<Self, PriceError> {
        match self.0.checked_add(other.0) {
            Some(v) => Ok(Self(v)),
            None => Err(PriceError::Overflow),
        }
    }

    /// Subtracts one paisa count from another, refusing to wrap.
    ///
    /// # Errors
    ///
    /// Returns [`PriceError::Overflow`] on `i64` overflow.
    pub const fn checked_sub(self, other: Self) -> Result<Self, PriceError> {
        match self.0.checked_sub(other.0) {
            Some(v) => Ok(Self(v)),
            None => Err(PriceError::Overflow),
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
    use super::*;

    #[test]
    fn raw_round_trips() {
        assert_eq!(Paisa::from_raw(2_310_955).raw(), 2_310_955);
        assert_eq!(Paisa::ZERO.raw(), 0);
        assert_eq!(Paisa::default(), Paisa::ZERO);
    }

    #[test]
    fn converts_a_real_nifty_quote() {
        // The exact first bar of 2024-06-03, as it sits in the lake.
        let got = Paisa::from_rupees_half_up(23_109.55).expect("finite, in range");
        assert_eq!(got.raw(), 2_310_955);
    }

    #[test]
    fn snaps_real_ties_toward_positive_infinity() {
        // 0.125 is exactly representable in binary, so 0.125 * 100 is exactly
        // 12.5 — a GENUINE tie. This is what "half-up" is actually about.
        let up = Paisa::from_rupees_half_up(0.125).expect("finite");
        assert_eq!(up.raw(), 13, "a true tie rounds up, not to even (12)");

        // The same tie on the negative side must go toward positive infinity
        // (-12), not away from zero (-13). This is the case that distinguishes
        // half-up from `f64::round`, and it is why this function does not use
        // `f64::round`.
        let down = Paisa::from_rupees_half_up(-0.125).expect("finite");
        assert_eq!(
            down.raw(),
            -12,
            "a negative tie rounds toward positive infinity, not away from zero"
        );
    }

    #[test]
    fn a_decimal_tie_is_usually_not_a_binary_tie() {
        // This test exists because the obvious version of the test above was
        // WRONG. `1.005` looks like a tie in decimal, but the nearest f64 is
        // 1.00499999999999989..., so scaled it is 100.4999... and there is no
        // tie to break. The function correctly follows the actual binary value
        // rather than the decimal spelling of it.
        //
        // This is not a defect and it is not fixable at this layer: the vendor
        // hands over an IEEE double, and by then the information distinguishing
        // 1.005 from 1.00499999999999989 is already gone. Recorded in
        // docs/06-limits.md.

        let got = Paisa::from_rupees_half_up(1.005).expect("finite");
        assert_eq!(
            got.raw(),
            100,
            "follows the binary value 1.00499..., not the decimal spelling"
        );
    }

    #[test]
    fn rejects_a_non_finite_price_rather_than_substituting() {
        assert_eq!(
            Paisa::from_rupees_half_up(f64::NAN),
            Err(PriceError::NotFinite)
        );
        assert_eq!(
            Paisa::from_rupees_half_up(f64::INFINITY),
            Err(PriceError::NotFinite)
        );
        assert_eq!(
            Paisa::from_rupees_half_up(f64::NEG_INFINITY),
            Err(PriceError::NotFinite)
        );
    }

    #[test]
    fn refuses_an_out_of_range_price_instead_of_saturating() {
        // `as` would clamp these to i64::MAX / i64::MIN without complaint,
        // turning an absurd quote into a plausible-looking extreme price.
        assert_eq!(
            Paisa::from_rupees_half_up(f64::MAX),
            Err(PriceError::OutOfRange)
        );
        assert_eq!(
            Paisa::from_rupees_half_up(f64::MIN),
            Err(PriceError::OutOfRange)
        );
    }

    #[test]
    fn splits_into_rupees_and_paisa() {
        let p = Paisa::from_raw(2_310_955);
        assert_eq!(p.rupees_trunc(), 23_109);
        assert_eq!(p.paisa_part(), 55);

        let neg = Paisa::from_raw(-2_310_955);
        assert_eq!(neg.rupees_trunc(), -23_109);
        assert_eq!(neg.paisa_part(), -55);
    }

    #[test]
    fn checked_arithmetic_refuses_to_wrap() {
        let a = Paisa::from_raw(100);
        let b = Paisa::from_raw(40);
        assert_eq!(a.checked_add(b), Ok(Paisa::from_raw(140)));
        assert_eq!(a.checked_sub(b), Ok(Paisa::from_raw(60)));

        let max = Paisa::from_raw(i64::MAX);
        let one = Paisa::from_raw(1);
        assert_eq!(max.checked_add(one), Err(PriceError::Overflow));

        let min = Paisa::from_raw(i64::MIN);
        assert_eq!(min.checked_sub(one), Err(PriceError::Overflow));
    }

    #[test]
    fn ordering_is_numeric() {
        let mut v = [
            Paisa::from_raw(300),
            Paisa::from_raw(-100),
            Paisa::from_raw(0),
        ];
        v.sort_unstable();
        assert_eq!(
            v,
            [Paisa::from_raw(-100), Paisa::ZERO, Paisa::from_raw(300)]
        );
    }

    #[test]
    fn every_lake_price_shape_converts_exactly() {
        // The lake survey measured every OHLC value in all four series onto
        // the 2-decimal grid, with a maximum deviation of 9.3e-10 paisa. These
        // are the observed extremes of each series; each must land on the
        // paisa its decimal string implies, with no off-by-one.
        for (rupees, want) in [
            (7_511.10_f64, 751_110_i64), // NIFTY low
            (26_372.85, 2_637_285),      // NIFTY high
            (16_116.25, 1_611_625),      // BANKNIFTY low
            (61_702.10, 6_170_210),      // BANKNIFTY high
            (56_147.23, 5_614_723),      // SENSEX low
            (86_109.40, 8_610_940),      // SENSEX high
            (8.18, 818),                 // INDIAVIX low
            (86.64, 8_664),              // INDIAVIX high
        ] {
            let got = Paisa::from_rupees_half_up(rupees).expect("finite, in range");
            assert_eq!(got.raw(), want, "{rupees} rupees");
        }
    }
}
