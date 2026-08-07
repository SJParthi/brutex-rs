//! The external anchor: one real option chain, captured live from Dhan.
//!
//! Everything above this file is self-consistent mathematics. This file is the
//! only place where the crate is held against numbers it did not produce — and
//! the first thing it has to say is **how little that is worth on one strike.**
//!
//! # What was captured, and what it did not include
//!
//! One strike from a live Dhan option-chain response
//! (`dhanhq.co/docs/v2/option-chain/`), both sides:
//!
//! ```text
//! implied_volatility  11.939337251984934   and  9.789193798280868   (PERCENT)
//! delta                0.53871  call            -0.46732  put
//! gamma                0.00132                   0.00109
//! theta              -15.1539                  -10.61131
//! vega                12.2025                   12.18593
//! ```
//!
//! **No rho.** No spot, no strike, no timestamp, no expiry, and no statement
//! of what any unit means. Groww publishes all five greeks including rho, in a
//! dedicated Greeks section of `groww.in/trade-api/docs/curl/live-data`.
//!
//! # What is fitted, and why almost nothing here is a prediction
//!
//! Dhan publishes no spot, no strike and no maturity, so they are solved out
//! of the sample: the two deltas and the two volatilities give `T` in closed
//! form, vega gives the spot, theta gives the rate. **Six of the eight
//! published numbers are inputs to that fit** — delta twice, vega twice, theta
//! twice — **and cannot be evidence for it.** They land at `1e-15` to `1e-14`
//! against a `1e-9` tolerance because a bisection inverted its own equation,
//! which is not a measurement of anything outside this file.
//!
//! **The two gammas are the other two, and they are not an independent
//! prediction either.** That claim was made by the first version of this file,
//! and D-0046 withdraws it. With `q = 0` the fitted spot is
//! `S = vega·100 / (n(d1)·sqrt(T))`, so the fitted gamma collapses to
//!
//! ```text
//! gamma = n(d1)^2 / (100 · vega · sigma)
//! ```
//!
//! — no `S`, no `K`, no `T`, no `r` survives. That is *exactly* the scale-free
//! identity `vega·gamma·sigma == n(d1)^2` which
//! [`the_two_published_volatilities_are_transposed_and_the_identity_says_so`]
//! uses, **with the published gammas as inputs**, to choose which volatility
//! belongs to which side. Reproducing the gammas is the assignment criterion
//! restated, not a confirmation of it. Measured: the two residuals this file
//! prints (`1.561e-7` and `3.423e-6`) are the same two numbers that test
//! produces, and forcing `r` from −50% to +200% or scaling `T` by 0.25× to 10×
//! leaves both gammas identical to all fifteen printed digits —
//! [`the_two_gammas_are_invariant_to_the_rate_and_the_maturity`] pins that so
//! the limitation cannot be re-forgotten.
//!
//! **What the sample does prove** is narrower and worth stating on its own:
//! the published `delta / gamma / vega / IV` block is internally consistent
//! with Black-Scholes-Merton once the two volatilities are transposed. A
//! genuinely independent anchor needs a field the fit does not consume — the
//! premium `C − P`, or a second strike from the same chain at the same
//! instant. Neither was captured.
//!
//! # Three things measured out of the sample, and what each is worth
//!
//! 1. **Vega is per one percentage point**, not per unit of volatility. On the
//!    raw scaling the implied index level comes out at **258**; on the
//!    per-percent scaling it comes out at **25,851**. A factor of a hundred is
//!    not a judgement call.
//! 2. **The two implied volatilities are transposed** relative to the
//!    delta/gamma/vega block, by the scale-free identity above: satisfied to
//!    within gamma's own printed precision on the swapped assignment and out
//!    by a factor of **1.22** on the published one.
//! 3. **The day divisor is not 365, as far as this sample can tell.** 252 is
//!    excluded by a factor of 23. But the criterion that excludes it — the two
//!    sides of one strike must agree on `r` — has its **root at
//!    `D* = 370.0757`**, where the two sides agree to `2.8e-15` percentage
//!    points, and it prefers `D = 375` (spread 0.9700) to `D = 365` (spread
//!    0.9999). The first version of this file claimed 365 was *measured*. It
//!    is not: it is the nearest ordinary calendar convention to a root the
//!    criterion actually puts at 370.08, and the `0.9999`-point "rate
//!    residual" this crate used to pin as a property of the sample is an
//!    artifact of assuming 365. D-0046.
//!
//! # And the carry is assumed, not measured
//!
//! `q = 0` is *consistent* with the sample. It is not implied by it:
//! [`the_carry_is_consistent_with_zero_and_the_sample_cannot_pin_it`] refits
//! the same eight published fields at `q = 1%` and `q = 2%` and reproduces
//! every one of them, gammas included, at `T = 4.09` and `3.43` calendar days
//! instead of 5.16. Only the maturity and the rate move. Nothing in the sample
//! separates them.
//!
//! # And what this sample cannot settle, stated rather than guessed
//!
//! `r` is **not** pinned by it. Over the rounding box of the six printed
//! fields — 200,000 draws, uniform half-ulp on each — the 95% interval on `r`
//! is 1.09 percentage points wide on the call side and 1.14 on the put side.
//! `T` survives the same box to ±0.079 calendar days **conditional on `q = 0`
//! and on the transposition**; that is a rounding-box width, not an
//! identification result. The problem is well-conditioned in `T` and
//! ill-conditioned in `r`, because `r` enters theta only through
//! `r·K·e^-rT·N(d2)` while `T` enters through `S·n(d1)·sigma/(2·sqrt(T))`,
//! which is most of it.
//!
//! **UNVERIFIED, and not guessed at here:** whether Dhan's `r` is a market
//! rate or a hardcoded constant near 10%; which day count and which divisor
//! generate `T`; whether the carry is zero; whether the model is spot BSM or
//! forward Black-76; and the NIFTY strike interval, which no source in
//! `docs/00-charter.md` states. See `docs/06-limits.md` §18.

// The same exceptions every test module in this workspace takes, plus the two
// this crate exists for. See the crate documentation and D-0046.
#![allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_arithmetic,
    clippy::float_cmp
)]

// This file names nothing but `greeks::`. That is the structural proof that
// the public surface mentions no type from this workspace -- it would fail to
// compile if that stopped being true.
use greeks::{Contract, OptionKind, standard_normal_cdf, standard_normal_pdf};

// ---------------------------------------------------------------------------
// The capture. Nothing below is derived; these are the bytes the vendor sent.
// ---------------------------------------------------------------------------

/// Published as the call's implied volatility, in percent.
const IV_PUBLISHED_CALL: f64 = 11.939_337_251_984_934;
/// Published as the put's implied volatility, in percent.
const IV_PUBLISHED_PUT: f64 = 9.789_193_798_280_868;
const DELTA_CALL: f64 = 0.53871;
const DELTA_PUT: f64 = -0.46732;
const GAMMA_CALL: f64 = 0.00132;
const GAMMA_PUT: f64 = 0.00109;
const THETA_CALL_PER_DAY: f64 = -15.1539;
const THETA_PUT_PER_DAY: f64 = -10.61131;
const VEGA_CALL_PER_PERCENT: f64 = 12.2025;
const VEGA_PUT_PER_PERCENT: f64 = 12.18593;

/// Every published field is printed to at most five decimals, so half of the
/// last displayed place is `5e-6`. That is the box every residual below is
/// measured against, and it is the vendor's number, not a chosen tolerance.
const DISPLAY_HALF_ULP: f64 = 5e-6;

/// The divisor the fit is *reported* under. It is a convention, not a
/// measurement — see the module documentation and
/// [`the_rate_criterion_has_its_root_at_370_and_365_is_a_convention_near_it`].
const CALENDAR_DAYS: f64 = 365.0;

/// The assignment that survives the scale-free identity: the call takes the
/// smaller volatility and the put the larger, which is also the ordinary index
/// skew for a strike just below the spot.
const SIGMA_CALL: f64 = IV_PUBLISHED_PUT / 100.0;
const SIGMA_PUT: f64 = IV_PUBLISHED_CALL / 100.0;

/// The inverse distribution, by bisection on the shipped one.
///
/// A fixed 200 halvings of `[-40, 40]`, no early exit and no tolerance test:
/// the bracket is exhausted in `f64` long before the count runs out, and a
/// fixed count has no branch whose boundary a test would have to reach.
fn inverse_cdf(probability: f64) -> f64 {
    let mut low = -40.0_f64;
    let mut high = 40.0_f64;
    for _ in 0..200 {
        let middle = 0.5 * (low + high);
        if standard_normal_cdf(middle) < probability {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

/// `d1` for each side and `sqrt(T)`, solved together because with a non-zero
/// carry they depend on each other.
///
/// `delta_call = e^-qT N(d1)` and `delta_put = e^-qT (N(d1) − 1)`, so the two
/// `d1` need `T`; and `sqrt(T) = 2 (d1c sc − d1p sp) / (sc^2 − sp^2)` — from
/// eliminating `ln(S/K) + (r−q)T`, which is the same quantity on both sides of
/// one strike — needs the two `d1`. A fixed 200 passes, for the same reason
/// [`inverse_cdf`] takes a fixed count. At `q = 0` the first pass is already
/// the answer and the rest change nothing.
fn fit_maturity(carry: f64) -> (f64, f64, f64) {
    let mut root_t = 0.0_f64;
    let mut d1_call = 0.0_f64;
    let mut d1_put = 0.0_f64;
    for _ in 0..200 {
        let discount = (-carry * root_t * root_t).exp();
        d1_call = inverse_cdf(DELTA_CALL / discount);
        d1_put = -inverse_cdf(-DELTA_PUT / discount);
        root_t = 2.0 * (d1_call * SIGMA_CALL - d1_put * SIGMA_PUT)
            / (SIGMA_CALL * SIGMA_CALL - SIGMA_PUT * SIGMA_PUT);
    }
    (root_t, d1_call, d1_put)
}

/// The two `d1` under the assumption the rest of this file reports under.
fn published_d1() -> (f64, f64) {
    let (_, d1_call, d1_put) = fit_maturity(0.0);
    (d1_call, d1_put)
}

/// Closes one side: vega fixes the spot, theta fixes the rate, and `d1` then
/// fixes the strike. Returns the contract and the volatility that produced it.
#[allow(clippy::too_many_arguments)]
fn close_one_side(
    kind: OptionKind,
    sigma: f64,
    d1: f64,
    vega_per_percent: f64,
    theta_per_day: f64,
    day_divisor: f64,
    carry: f64,
    root_t: f64,
) -> (Contract, f64) {
    let years = root_t * root_t;
    let carry_discount = (-carry * years).exp();
    // vega = S e^-qT n(d1) sqrt(T), published per one percentage point.
    let spot = vega_per_percent * 100.0 / (carry_discount * standard_normal_pdf(d1) * root_t);
    let d2 = d1 - sigma * root_t;
    let strike_at = |rate: f64| {
        spot * (-(d1 * sigma * root_t - (rate - carry + 0.5 * sigma * sigma) * years)).exp()
    };
    let residual = |rate: f64| {
        let strike = strike_at(rate);
        let forward = spot * carry_discount;
        let decay = -forward * standard_normal_pdf(d1) * sigma / (2.0 * root_t);
        let discounted = rate * strike * (-rate * years).exp();
        let theta = match kind {
            OptionKind::Call => {
                decay - discounted * standard_normal_cdf(d2)
                    + carry * forward * standard_normal_cdf(d1)
            }
            OptionKind::Put => {
                decay + discounted * standard_normal_cdf(-d2)
                    - carry * forward * standard_normal_cdf(-d1)
            }
        };
        theta / day_divisor - theta_per_day
    };
    // 300 fixed halvings on a bracket wide enough to hold any rate anyone has
    // ever quoted. Same reasoning as `inverse_cdf`.
    let mut low = -5.0_f64;
    let mut high = 5.0_f64;
    for _ in 0..300 {
        let middle = 0.5 * (low + high);
        if residual(low) * residual(middle) <= 0.0 {
            high = middle;
        } else {
            low = middle;
        }
    }
    let rate = 0.5 * (low + high);
    (
        Contract {
            spot,
            strike: strike_at(rate),
            years_to_expiry: years,
            rate,
            carry,
        },
        sigma,
    )
}

/// Both sides, under a stated day divisor and a stated carry.
fn fitted_sides_at(day_divisor: f64, carry: f64) -> ((Contract, f64), (Contract, f64)) {
    let (root_t, d1_call, d1_put) = fit_maturity(carry);
    (
        close_one_side(
            OptionKind::Call,
            SIGMA_CALL,
            d1_call,
            VEGA_CALL_PER_PERCENT,
            THETA_CALL_PER_DAY,
            day_divisor,
            carry,
            root_t,
        ),
        close_one_side(
            OptionKind::Put,
            SIGMA_PUT,
            d1_put,
            VEGA_PUT_PER_PERCENT,
            THETA_PUT_PER_DAY,
            day_divisor,
            carry,
            root_t,
        ),
    )
}

/// Both sides under the conventions this file reports under: calendar days and
/// no carry. **Both are conventions.** See the module documentation.
fn fitted_sides() -> ((Contract, f64), (Contract, f64)) {
    fitted_sides_at(CALENDAR_DAYS, 0.0)
}

/// The side-to-side disagreement on `r`, in percentage points, as a function
/// of the day divisor. Signed, because its **root** is the quantity of
/// interest and an absolute value hides where it is.
fn rate_disagreement(day_divisor: f64) -> f64 {
    let ((call, _), (put, _)) = fitted_sides_at(day_divisor, 0.0);
    (call.rate - put.rate) * 100.0
}

#[test]
fn the_two_published_volatilities_are_transposed_and_the_identity_says_so() {
    // vega * gamma * sigma == n(d1)^2. No spot, no strike, no maturity and no
    // rate appear in it, so it tests the published fields against nothing but
    // themselves. It is the whole reason the assignment below is a
    // measurement rather than a preference -- and, as the module
    // documentation says, it is also the whole content of the "gamma
    // prediction" further down this file. One equation, asserted twice.
    let (d1_call, d1_put) = published_d1();
    let ratio = |gamma: f64, vega_per_percent: f64, sigma: f64, d1: f64| {
        let density = standard_normal_pdf(d1);
        vega_per_percent * 100.0 * gamma * sigma / (density * density)
    };

    let as_published_call = ratio(
        GAMMA_CALL,
        VEGA_CALL_PER_PERCENT,
        IV_PUBLISHED_CALL / 100.0,
        d1_call,
    );
    let as_published_put = ratio(
        GAMMA_PUT,
        VEGA_PUT_PER_PERCENT,
        IV_PUBLISHED_PUT / 100.0,
        d1_put,
    );
    let transposed_call = ratio(GAMMA_CALL, VEGA_CALL_PER_PERCENT, SIGMA_CALL, d1_call);
    let transposed_put = ratio(GAMMA_PUT, VEGA_PUT_PER_PERCENT, SIGMA_PUT, d1_put);
    println!(
        "identity ratio -- as published: call {as_published_call:.5} put {as_published_put:.5}; \
         transposed: call {transposed_call:.5} put {transposed_put:.5}"
    );

    // gamma is printed to five decimals, so 0.00132 carries about 0.38% and
    // 0.00109 about 0.46%. The transposed assignment sits inside that; the
    // published one sits fifty times outside it.
    let call_slack = DISPLAY_HALF_ULP / GAMMA_CALL;
    let put_slack = DISPLAY_HALF_ULP / GAMMA_PUT;
    assert!(
        (transposed_call - 1.0).abs() <= call_slack,
        "transposed call ratio {transposed_call} is outside gamma's own rounding {call_slack}"
    );
    assert!(
        (transposed_put - 1.0).abs() <= put_slack,
        "transposed put ratio {transposed_put} is outside gamma's own rounding {put_slack}"
    );
    assert!(
        (as_published_call - 1.0).abs() > 50.0 * call_slack,
        "the published assignment is no longer refuted: {as_published_call}"
    );
}

#[test]
fn the_delta_difference_is_reproduced_by_the_two_volatilities_and_says_nothing_about_the_carry() {
    // `delta_call - delta_put = 1.00603`, which is not 1. Under ONE volatility
    // that difference is e^-qT and would need a carry of -42.54% at this
    // maturity, which is not a rate. Under the two volatilities the sample
    // carries it is `N(d1c) + N(-d1p)` with q = 0.
    //
    // -42.54% and not the -46% the first version of this crate printed in its
    // prose: nothing computed that number, and this test now does. D-0046.
    //
    // WHAT THIS TEST IS NOT. It is not evidence that q = 0. `published_d1`
    // INVERTS the two deltas, so the second assertion below is
    // `N(N^-1(x)) + N(N^-1(y)) == x + y` and cannot fail for any deltas, any
    // volatilities or any carry. It is a consistency check on the inverse
    // distribution, and the first version of this file named it as if it were
    // a measurement of the carry. It is not. D-0046, and
    // `the_carry_is_consistent_with_zero_and_the_sample_cannot_pin_it` is what
    // actually probes q.
    let (d1_call, d1_put) = published_d1();
    let published_difference = DELTA_CALL - DELTA_PUT;
    let reconstructed = standard_normal_cdf(d1_call) + standard_normal_cdf(-d1_put);
    println!("delta difference: published {published_difference}, rebuilt {reconstructed}");
    assert!(
        (reconstructed - published_difference).abs() <= 1e-12,
        "rebuilt {reconstructed} against published {published_difference}"
    );
    assert!(
        (published_difference - 1.0).abs() > 1e-3,
        "the sample no longer shows the gap this test exists to explain"
    );
    // The gap a SINGLE volatility would have to explain, as a carry. It is
    // computed here so the figure in the prose is a number this file took.
    let years = {
        let (root_t, _, _) = fit_maturity(0.0);
        root_t * root_t
    };
    let single_volatility_carry = -published_difference.ln() / years;
    println!(
        "one volatility would need q = {:.2}%",
        single_volatility_carry * 100.0
    );
    assert!(
        (-0.43..=-0.42).contains(&single_volatility_carry),
        "a single volatility no longer needs an absurd carry: {single_volatility_carry}"
    );
}

#[test]
fn the_carry_is_consistent_with_zero_and_the_sample_cannot_pin_it() {
    // The test the one above was mistaken for. Refitting the SAME eight
    // published fields at a non-zero carry reproduces every one of them,
    // gammas included -- at a different maturity and a different rate. So
    // `q = 0` is a choice the sample tolerates, not one it makes.
    let mut reproduced = 0_u32;
    for carry in [0.0_f64, 0.01, 0.02] {
        let ((call, sigma_call), (put, sigma_put)) = fitted_sides_at(CALENDAR_DAYS, carry);
        let ours_call = call.greeks(sigma_call, OptionKind::Call).expect("call");
        let ours_put = put.greeks(sigma_put, OptionKind::Put).expect("put");
        let worst = [
            (ours_call.delta - DELTA_CALL).abs(),
            (ours_put.delta - DELTA_PUT).abs(),
            (ours_call.vega / 100.0 - VEGA_CALL_PER_PERCENT).abs(),
            (ours_put.vega / 100.0 - VEGA_PUT_PER_PERCENT).abs(),
            (ours_call.gamma - GAMMA_CALL).abs(),
            (ours_put.gamma - GAMMA_PUT).abs(),
        ]
        .into_iter()
        .fold(0.0_f64, f64::max);
        println!(
            "q {:.0}%: T {:.6} cal days, r {:.4}%/{:.4}%, worst published residual {worst:.3e}",
            carry * 100.0,
            call.years_to_expiry * CALENDAR_DAYS,
            call.rate * 100.0,
            put.rate * 100.0
        );
        assert!(
            worst <= DISPLAY_HALF_ULP,
            "q = {carry} no longer reproduces the chain: worst {worst}"
        );
        reproduced += 1;
    }
    assert_eq!(reproduced, 3, "a carry stopped reproducing the sample");
    // And the cost of the choice: two percent of carry moves the fitted
    // maturity by a third. The sample says nothing about which is right.
    let zero = fit_maturity(0.0).0.powi(2) * CALENDAR_DAYS;
    let two = fit_maturity(0.02).0.powi(2) * CALENDAR_DAYS;
    println!("T at q = 0: {zero:.6} days; at q = 2%: {two:.6} days");
    assert!(
        (zero / two - 1.0) > 0.4,
        "the carry no longer moves T: {zero} against {two}"
    );
}

#[test]
fn the_trading_day_divisor_is_excluded_and_the_calendar_one_is_not_selected() {
    // What survives of the old claim, which is the useful half: a 252-day
    // divisor is out by a factor of 23, so theta is NOT per trading day. That
    // is the fact `crates/greeks`'s unit table rests on, and it is robust.
    let calendar = rate_disagreement(365.0).abs();
    let trading = rate_disagreement(252.0).abs();
    println!(
        "rate disagreement between the two sides: {calendar:.4} points under 365, \
         {trading:.4} under 252 -- a factor of {:.1}",
        trading / calendar
    );
    assert!(
        trading > 10.0 * calendar,
        "252 is no longer excluded: {calendar} against {trading}"
    );
    // And the cost of getting it wrong, as a number: 365/252.
    assert!(
        ((365.0_f64 / 252.0) - 1.448_412_698_412_698_4).abs() < 1e-12,
        "the divisor ratio moved"
    );
    // What does NOT survive: that this criterion picks 365. It prefers 375 --
    // which is also, exactly, the number of one-minute bars in an NSE session,
    // so it is a realistic wrong choice a future editor could make. Asserted
    // here so the file can never again be read as pinning 365.
    let session_bars = rate_disagreement(375.0).abs();
    println!("...and {session_bars:.4} under 375, which is BETTER than 365");
    assert!(
        session_bars < calendar,
        "375 no longer beats 365: {session_bars} against {calendar}"
    );
}

#[test]
fn the_rate_criterion_has_its_root_at_370_and_365_is_a_convention_near_it() {
    // The discriminating, monotone quantity: `r` is affine in the divisor, so
    // the side-to-side rate spread is a straight line in it and has exactly
    // one root. THAT is what the criterion actually selects, and asserting its
    // location is the honest version of the assertion this file used to make
    // on the spread's value at an assumed 365.
    let low = 366.0_f64;
    let high = 380.0_f64;
    let at_low = rate_disagreement(low);
    let mut lo = low;
    let mut hi = high;
    for _ in 0..200 {
        let middle = 0.5 * (lo + hi);
        if rate_disagreement(middle) * at_low > 0.0 {
            lo = middle;
        } else {
            hi = middle;
        }
    }
    let root = 0.5 * (lo + hi);
    let ((call, _), (put, _)) = fitted_sides_at(root, 0.0);
    println!(
        "the rate spread has its root at D* = {root:.6}, where r = {:.6}% on both sides \
         (residual {:.3e} points)",
        call.rate * 100.0,
        rate_disagreement(root)
    );
    assert!(
        (370.0..=370.2).contains(&root),
        "the root moved: D* = {root}"
    );
    assert!(
        rate_disagreement(root).abs() < 1e-9,
        "the root is not a root: {}",
        rate_disagreement(root)
    );
    assert!(
        (call.rate - put.rate).abs() < 1e-12,
        "the two sides do not agree at the root: {} against {}",
        call.rate,
        put.rate
    );
    // The line is a line: the spread is monotone in the divisor across the
    // whole plausible range, so there is no second root hiding at 365.
    let mut previous = f64::NEG_INFINITY;
    for step in 0..=40_i32 {
        let divisor = 250.0 + f64::from(step) * 5.0;
        let spread = rate_disagreement(divisor);
        assert!(
            spread > previous,
            "the rate spread is not monotone in the divisor at {divisor}"
        );
        previous = spread;
    }
}

#[test]
fn vega_is_published_per_percentage_point_and_the_index_level_proves_it() {
    // On the raw scaling the implied index level is 258. On the per-percent
    // scaling it is 25,851, which is where NIFTY actually trades. This is the
    // whole evidence for the unit, and it is one line of arithmetic.
    let (d1_call, _) = published_d1();
    let (root_t, _, _) = fit_maturity(0.0);
    let raw = VEGA_CALL_PER_PERCENT / (standard_normal_pdf(d1_call) * root_t);
    let per_percent = raw * 100.0;
    println!("implied spot: {raw:.2} on raw vega, {per_percent:.2} on per-percent vega");
    assert!((raw - 258.51).abs() < 0.5, "raw scaling moved: {raw}");
    assert!(
        (25_000.0..27_000.0).contains(&per_percent),
        "per-percent scaling moved: {per_percent}"
    );
}

#[test]
fn the_maturity_is_recovered_in_closed_form_from_the_deltas_and_the_volatilities() {
    // Conditional on q = 0 AND on the transposition. The interval below is the
    // width of the rounding box of the six printed fields; it is NOT an
    // identification result, and `the_carry_is_consistent_with_zero_and_the_
    // sample_cannot_pin_it` measures how far outside it a different carry
    // moves the answer.
    let (root_t, _, _) = fit_maturity(0.0);
    let years = root_t * root_t;
    let calendar_days = years * CALENDAR_DAYS;
    println!("T = {years:.10} years = {calendar_days:.5} calendar days, GIVEN q = 0");
    assert!(
        (5.0796..=5.2376).contains(&calendar_days),
        "T = {calendar_days} calendar days, outside the measured interval"
    );
}

#[test]
fn our_greeks_reproduce_the_captured_dhan_chain() {
    // Delta, vega and theta went into the fit; the two gammas are the scale-
    // free identity restated. NONE of the eight is an independent prediction,
    // and the module documentation says why at length. What this test is for
    // is narrower and still worth having: it holds the SHIPPED closed form
    // against the eight numbers, so a sign error, a missing discount factor or
    // a wrong unit anywhere in `crates/greeks::bsm` shows up here as a failure
    // rather than as a plausible number.
    let ((call, sigma_call), (put, sigma_put)) = fitted_sides();
    println!(
        "fitted call side: S {:.4} K {:.4} T {:.10} r {:.4}%",
        call.spot,
        call.strike,
        call.years_to_expiry,
        call.rate * 100.0
    );
    println!(
        "fitted put  side: S {:.4} K {:.4} T {:.10} r {:.4}%",
        put.spot,
        put.strike,
        put.years_to_expiry,
        put.rate * 100.0
    );

    let ours_call = call.greeks(sigma_call, OptionKind::Call).expect("call");
    let ours_put = put.greeks(sigma_put, OptionKind::Put).expect("put");

    let cases: [(&str, f64, f64, f64); 8] = [
        ("delta call", ours_call.delta, DELTA_CALL, 1e-9),
        ("delta put", ours_put.delta, DELTA_PUT, 1e-9),
        (
            "vega call",
            ours_call.vega / 100.0,
            VEGA_CALL_PER_PERCENT,
            1e-9,
        ),
        (
            "vega put",
            ours_put.vega / 100.0,
            VEGA_PUT_PER_PERCENT,
            1e-9,
        ),
        (
            "theta call",
            ours_call.theta / CALENDAR_DAYS,
            THETA_CALL_PER_DAY,
            1e-9,
        ),
        (
            "theta put",
            ours_put.theta / CALENDAR_DAYS,
            THETA_PUT_PER_DAY,
            1e-9,
        ),
        // Not predictions. See the module documentation and D-0046.
        ("gamma call", ours_call.gamma, GAMMA_CALL, DISPLAY_HALF_ULP),
        ("gamma put", ours_put.gamma, GAMMA_PUT, DISPLAY_HALF_ULP),
    ];
    for (name, ours, published, tolerance) in cases {
        let deviation = (ours - published).abs();
        println!("  {name:<11} ours {ours:>18.12}  Dhan {published:>12}  |diff| {deviation:.3e}");
        assert!(
            deviation <= tolerance,
            "{name}: ours {ours}, Dhan {published}, off by {deviation} against {tolerance}"
        );
    }

    // Dhan publishes no rho, so there is nothing to anchor it against. What
    // can still be asserted is that it exists, has the sign the model
    // requires, and is not accidentally zero -- Groww publishes one and a
    // consumer of this crate will compare against it.
    assert!(ours_call.rho > 0.0, "call rho {}", ours_call.rho);
    assert!(ours_put.rho < 0.0, "put rho {}", ours_put.rho);
}

#[test]
fn the_two_gammas_are_invariant_to_the_rate_and_the_maturity() {
    // The limitation, pinned as a positive assertion rather than left implied.
    // The fitted gamma is `n(d1)^2 / (100 vega sigma)` once the spot has been
    // taken from vega, so neither `r` nor `T` nor `S` nor `K` can reach it. A
    // fitted gamma that agrees with the vendor therefore says nothing
    // whatsoever about any of them, and no future reader should have to
    // rediscover that.
    let (root_t, d1_call, d1_put) = fit_maturity(0.0);
    let rebuild = |rate: f64, scale: f64, d1: f64, sigma: f64, vega_per_percent: f64| {
        let years = root_t * root_t * scale;
        let root_years = years.sqrt();
        let spot = vega_per_percent * 100.0 / (standard_normal_pdf(d1) * root_years);
        let strike =
            spot * (-(d1 * sigma * root_years - (rate + 0.5 * sigma * sigma) * years)).exp();
        Contract {
            spot,
            strike,
            years_to_expiry: years,
            rate,
            carry: 0.0,
        }
    };

    let reference = rebuild(0.094_619, 1.0, d1_call, SIGMA_CALL, VEGA_CALL_PER_PERCENT)
        .greeks(SIGMA_CALL, OptionKind::Call)
        .expect("reference")
        .gamma;
    let mut compared = 0_u32;
    for rate in [-0.50_f64, 0.0, 0.094_619, 0.50, 2.0] {
        let contract = rebuild(rate, 1.0, d1_call, SIGMA_CALL, VEGA_CALL_PER_PERCENT);
        let gamma = contract
            .greeks(SIGMA_CALL, OptionKind::Call)
            .expect("greeks")
            .gamma;
        println!(
            "  r {:>7.2}%  K {:>12.4}  gamma {gamma:.15e}",
            rate * 100.0,
            contract.strike
        );
        assert!(
            (gamma / reference - 1.0).abs() <= 1e-14,
            "gamma moved with the rate: {gamma} against {reference}"
        );
        compared += 1;
    }
    for scale in [0.25_f64, 0.5, 2.0, 10.0] {
        let contract = rebuild(0.094_619, scale, d1_call, SIGMA_CALL, VEGA_CALL_PER_PERCENT);
        let gamma = contract
            .greeks(SIGMA_CALL, OptionKind::Call)
            .expect("greeks")
            .gamma;
        println!(
            "  T x{scale:<5}  S {:>12.4}  gamma {gamma:.15e}",
            contract.spot
        );
        assert!(
            (gamma / reference - 1.0).abs() <= 1e-14,
            "gamma moved with the maturity: {gamma} against {reference}"
        );
        compared += 1;
    }
    assert_eq!(compared, 9, "the invariance sweep shrank");

    // And the closed form it collapses to, on both sides, against the fitted
    // contracts the anchor above actually uses.
    let ((call, _), (put, _)) = fitted_sides();
    for (name, contract, sigma, kind, d1, vega_per_percent) in [
        (
            "call",
            call,
            SIGMA_CALL,
            OptionKind::Call,
            d1_call,
            VEGA_CALL_PER_PERCENT,
        ),
        (
            "put",
            put,
            SIGMA_PUT,
            OptionKind::Put,
            d1_put,
            VEGA_PUT_PER_PERCENT,
        ),
    ] {
        let fitted = contract.greeks(sigma, kind).expect("greeks").gamma;
        let density = standard_normal_pdf(d1);
        let closed = density * density / (100.0 * vega_per_percent * sigma);
        println!("  {name}: fitted gamma {fitted:.18e}, closed form {closed:.18e}");
        assert!(
            (fitted / closed - 1.0).abs() <= 1e-14,
            "{name}: the fitted gamma is not n(d1)^2/(100 vega sigma): {fitted} against {closed}"
        );
    }
}

#[test]
fn no_single_contract_reproduces_the_chain_and_the_shortfall_is_a_number() {
    // The headline the two-contract fit hides. Under q = 0 the four
    // scale-fixing fields depend on only three quantities -- `d1` on each side
    // and the product `S sqrt(T)` -- because delta is `N(d1)` and vega is
    // `S n(d1) sqrt(T) / 100`. Matching both deltas exactly fixes both `d1`,
    // and that alone fixes the model's vega RATIO, which contains no S, no K,
    // no T and no r. It does not equal the vendor's.
    let (d1_call, d1_put) = published_d1();
    let published_ratio = VEGA_CALL_PER_PERCENT / VEGA_PUT_PER_PERCENT;
    let model_ratio = standard_normal_pdf(d1_call) / standard_normal_pdf(d1_put);
    let gap = (model_ratio / published_ratio - 1.0).abs();
    println!(
        "vega ratio: published {published_ratio:.12}, forced by the deltas {model_ratio:.12}, \
         gap {:.6}%",
        gap * 100.0
    );
    assert!(
        gap > 1e-3,
        "the vega ratio no longer disagrees, which would make the two-contract fit \
         unnecessary: {gap}"
    );

    // How far outside the vendor's own printed precision the best possible
    // single contract sits. For a fixed pair of `d1` the scale that minimises
    // the worse of the two vega residuals balances them, in closed form, so
    // this is a two-dimensional minimax and a descent on it is exact enough to
    // quote. Measured: 884x, with all four residuals on the bound, which is
    // what a minimax optimum looks like.
    let worst = |u: f64, v: f64| {
        let density_u = standard_normal_pdf(u);
        let density_v = standard_normal_pdf(v);
        let delta_call = (standard_normal_cdf(u) - DELTA_CALL).abs();
        let delta_put = (standard_normal_cdf(v) - 1.0 - DELTA_PUT).abs();
        let vega = (VEGA_PUT_PER_PERCENT * density_u - VEGA_CALL_PER_PERCENT * density_v).abs()
            / (density_u + density_v);
        delta_call.max(delta_put).max(vega) / DISPLAY_HALF_ULP
    };
    let (mut u, mut v) = (d1_call, d1_put);
    let mut best = worst(u, v);
    let mut step = 1e-2_f64;
    while step > 1e-15 {
        let mut moved = false;
        for (du, dv) in [
            (step, 0.0),
            (-step, 0.0),
            (0.0, step),
            (0.0, -step),
            (step, step),
            (step, -step),
            (-step, step),
            (-step, -step),
        ] {
            if worst(u + du, v + dv) < best {
                best = worst(u + du, v + dv);
                u += du;
                v += dv;
                moved = true;
            }
        }
        if !moved {
            step *= 0.5;
        }
    }
    println!(
        "best single contract: every one of the four fields off by {best:.1}x the vendor's \
         own display half-ulp of {DISPLAY_HALF_ULP:e}"
    );
    assert!(
        (800.0..=1_000.0).contains(&best),
        "the single-contract shortfall moved: {best}"
    );
}

#[test]
fn the_two_sides_disagree_on_the_rate_and_the_disagreement_is_reported_not_hidden() {
    // CLAUDE.md section 3 rule 6. Under the reported conventions the fit does
    // not close: the two sides of one strike give spots 0.27% apart and rates
    // 1.00 points apart. The SPOT residual is the load-bearing one -- it is
    // invariant to the day divisor, measured identical to nine decimals for
    // every divisor from 252 to 500, so it is a property of the sample and not
    // of a convention. The RATE residual is not: it is a straight line in the
    // divisor with a root at 370.08, so its value here is a property of
    // choosing 365. Both are pinned, and which is which is stated.
    let ((call, _), (put, _)) = fitted_sides();
    let spot_spread = (call.spot / put.spot - 1.0).abs() * 100.0;
    let rate_spread = (call.rate - put.rate).abs() * 100.0;
    println!("residuals: spot spread {spot_spread:.4}%, rate spread {rate_spread:.4} points");
    assert!(
        (0.20..=0.35).contains(&spot_spread),
        "spot spread {spot_spread}% moved"
    );
    for divisor in [252.0_f64, 300.0, 365.0, 400.0, 500.0] {
        let ((call, _), (put, _)) = fitted_sides_at(divisor, 0.0);
        let at_divisor = (call.spot / put.spot - 1.0).abs() * 100.0;
        assert!(
            (at_divisor - spot_spread).abs() < 1e-9,
            "the spot residual moved with the divisor at {divisor}: {at_divisor}"
        );
    }
    assert!(
        (0.85..=1.15).contains(&rate_spread),
        "rate spread {rate_spread} points moved -- note this is a property of D = 365, \
         not of the sample"
    );
    // The honest summary: r is somewhere near ten percent and this sample
    // cannot say where, so nothing in this crate hardcodes one.
    let midpoint = (call.rate + put.rate) * 50.0;
    assert!(
        (9.0..=11.0).contains(&midpoint),
        "rate midpoint {midpoint}% moved"
    );
}
