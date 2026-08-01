//! The external anchor: one real option chain, captured live from Dhan.
//!
//! Everything above this file is self-consistent mathematics. This file is the
//! only place where the crate is held against numbers it did not produce.
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
//! Four things had to be *measured* out of that sample before it could anchor
//! anything, and each one is a test below:
//!
//! 1. **Vega is per one percentage point**, not per unit of volatility. On the
//!    raw scaling the implied index level comes out at **258**; on the
//!    per-percent scaling it comes out at **25,851**.
//! 2. **Theta is per calendar day, divided by 365** and not by 252. The two
//!    sides of the same strike must agree on the rate, and they agree to one
//!    percentage point under 365 against twenty-three under 252.
//! 3. **The two implied volatilities are transposed** relative to the
//!    delta/gamma/vega block. The scale-free identity
//!    `vega * gamma * sigma == n(d1)^2` — which contains no spot, no strike,
//!    no maturity and no rate, so it tests the published fields against
//!    nothing but themselves — is satisfied to within gamma's own printed
//!    precision on the swapped assignment and is out by a factor of **1.22**
//!    on the published one.
//! 4. **The carry is zero.** `delta_call − delta_put = 1.00603`, which is not
//!    `1`. Under one volatility that difference would be `e^-qT` and would
//!    imply a carry of **−46%** at this maturity, which is not a rate. Under
//!    the two volatilities the sample actually carries, `N(d1_call) +
//!    N(−d1_put)` reproduces `1.00603` with `q = 0` exactly.
//!
//! # What is fitted and what is predicted
//!
//! Dhan publishes no spot, no strike and no maturity, so they are solved out
//! of the sample: the two deltas and the two volatilities give `T` in closed
//! form, vega gives the spot, and theta gives the rate. **Four of the eight
//! published numbers are inputs to that fit and cannot be evidence for it.**
//!
//! The two **gammas are not used anywhere in the fit**. They are the
//! prediction, and reproducing them is the thing this file actually proves.
//!
//! # And what this sample cannot settle, stated rather than guessed
//!
//! `r` is **not** pinned by it. Over the rounding box of the six printed
//! fields — 200,000 draws, uniform half-ulp on each — the 95% interval on `r`
//! is 1.09 percentage points wide on the call side and 1.14 on the put side,
//! and the two sides differ by 1.00 points, which is outside both intervals.
//! `T` survives the same box to ±0.079 calendar days. The problem is
//! well-conditioned in `T` and ill-conditioned in `r`, because `r` enters
//! theta only through `r·K·e^-rT·N(d2)` while `T` enters through
//! `S·n(d1)·sigma/(2·sqrt(T))`, which is most of it.
//!
//! **UNVERIFIED, and not guessed at here:** whether Dhan's `r` is a market
//! rate or a hardcoded constant near 10%; which day count generates
//! `T = 0.01413` years; whether the model is spot BSM or forward Black-76; and
//! the NIFTY strike interval, which no source in `docs/00-charter.md` states.
//! See `docs/06-limits.md` §18.

// The same exceptions every test module in this workspace takes, plus the two
// this crate exists for. See the crate documentation and D-0036.
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

/// `d1` for each side, straight out of the published deltas with `q = 0`:
/// `delta_call = N(d1)` and `delta_put = N(d1) − 1`.
fn published_d1() -> (f64, f64) {
    (inverse_cdf(DELTA_CALL), -inverse_cdf(-DELTA_PUT))
}

/// `sqrt(T)`, in closed form, with no spot, no strike and no rate in it.
///
/// `d1 * sigma * sqrt(T) − sigma^2 T / 2` is `ln(S/K) + rT`, which is the same
/// quantity on both sides of one strike. Eliminating it between the two sides
/// leaves `sqrt(T) = 2 (d1c sc − d1p sp) / (sc^2 − sp^2)`.
fn root_years() -> f64 {
    let (d1_call, d1_put) = published_d1();
    2.0 * (d1_call * SIGMA_CALL - d1_put * SIGMA_PUT)
        / (SIGMA_CALL * SIGMA_CALL - SIGMA_PUT * SIGMA_PUT)
}

/// Closes one side: vega fixes the spot, theta fixes the rate, and `d1` then
/// fixes the strike. Returns the contract and the volatility that produced it.
fn close_one_side(
    kind: OptionKind,
    sigma: f64,
    d1: f64,
    vega_per_percent: f64,
    theta_per_day: f64,
    day_divisor: f64,
) -> (Contract, f64) {
    let root_t = root_years();
    let years = root_t * root_t;
    // vega = S n(d1) sqrt(T), published per one percentage point.
    let spot = vega_per_percent * 100.0 / (standard_normal_pdf(d1) * root_t);
    let d2 = d1 - sigma * root_t;
    let strike_at =
        |rate: f64| spot * (-(d1 * sigma * root_t - (rate + 0.5 * sigma * sigma) * years)).exp();
    let residual = |rate: f64| {
        let strike = strike_at(rate);
        let decay = -spot * standard_normal_pdf(d1) * sigma / (2.0 * root_t);
        let discounted = rate * strike * (-rate * years).exp();
        let theta = match kind {
            OptionKind::Call => decay - discounted * standard_normal_cdf(d2),
            OptionKind::Put => decay + discounted * standard_normal_cdf(-d2),
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
            carry: 0.0,
        },
        sigma,
    )
}

/// Both sides, under the calendar-day divisor that the measurement chose.
fn fitted_sides() -> ((Contract, f64), (Contract, f64)) {
    let (d1_call, d1_put) = published_d1();
    (
        close_one_side(
            OptionKind::Call,
            SIGMA_CALL,
            d1_call,
            VEGA_CALL_PER_PERCENT,
            THETA_CALL_PER_DAY,
            365.0,
        ),
        close_one_side(
            OptionKind::Put,
            SIGMA_PUT,
            d1_put,
            VEGA_PUT_PER_PERCENT,
            THETA_PUT_PER_DAY,
            365.0,
        ),
    )
}

#[test]
fn the_two_published_volatilities_are_transposed_and_the_identity_says_so() {
    // vega * gamma * sigma == n(d1)^2. No spot, no strike, no maturity and no
    // rate appear in it, so it tests the published fields against nothing but
    // themselves. It is the whole reason the assignment below is a
    // measurement rather than a preference.
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
fn the_delta_difference_needs_no_carry_once_the_two_volatilities_are_right() {
    // delta_call - delta_put = 1.00603, which is not 1. Under ONE volatility
    // that difference is e^-qT and would need a carry of about -46% at this
    // maturity, which is not a rate. Under the two the sample carries, it is
    // N(d1_call) + N(-d1_put) with q = 0 exactly.
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
}

#[test]
fn the_day_divisor_is_365_because_the_two_sides_have_to_agree_on_the_rate() {
    let (d1_call, d1_put) = published_d1();
    let disagreement = |divisor: f64| {
        let (call, _) = close_one_side(
            OptionKind::Call,
            SIGMA_CALL,
            d1_call,
            VEGA_CALL_PER_PERCENT,
            THETA_CALL_PER_DAY,
            divisor,
        );
        let (put, _) = close_one_side(
            OptionKind::Put,
            SIGMA_PUT,
            d1_put,
            VEGA_PUT_PER_PERCENT,
            THETA_PUT_PER_DAY,
            divisor,
        );
        (call.rate - put.rate).abs() * 100.0
    };
    let calendar = disagreement(365.0);
    let trading = disagreement(252.0);
    println!(
        "rate disagreement between the two sides: {calendar:.4} points under 365, \
         {trading:.4} under 252 -- a factor of {:.1}",
        trading / calendar
    );
    assert!(
        trading > 10.0 * calendar,
        "365 no longer wins: {calendar} against {trading}"
    );
    // And the cost of getting it wrong, as a number: 365/252.
    assert!(
        ((365.0_f64 / 252.0) - 1.448_412_698_412_698_4).abs() < 1e-12,
        "the divisor ratio moved"
    );
}

#[test]
fn vega_is_published_per_percentage_point_and_the_index_level_proves_it() {
    // On the raw scaling the implied index level is 258. On the per-percent
    // scaling it is 25,851, which is where NIFTY actually trades. This is the
    // whole evidence for the unit, and it is one line of arithmetic.
    let (d1_call, _) = published_d1();
    let root_t = root_years();
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
    let years = root_years() * root_years();
    let calendar_days = years * 365.0;
    println!("T = {years:.10} years = {calendar_days:.5} calendar days");
    // 5.158 +- 0.079 calendar days at 95%, from the printed precision of the
    // six rounded fields alone. The assertion is the interval, not the point.
    assert!(
        (5.0796..=5.2376).contains(&calendar_days),
        "T = {calendar_days} calendar days, outside the measured interval"
    );
}

#[test]
fn our_greeks_reproduce_the_captured_dhan_chain() {
    // THE ANCHOR. Delta, vega and theta went into the fit and are checked
    // only to show the fit closed. GAMMA DID NOT, on either side: it is the
    // prediction, and its tolerance is the vendor's own display precision.
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
            ours_call.theta / 365.0,
            THETA_CALL_PER_DAY,
            1e-9,
        ),
        ("theta put", ours_put.theta / 365.0, THETA_PUT_PER_DAY, 1e-9),
        // The two predictions. Nothing above touched either of them.
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
fn the_two_sides_disagree_on_the_rate_and_the_disagreement_is_reported_not_hidden() {
    // CLAUDE.md section 3 rule 6. The fit does not close: the two sides of one
    // strike give spots 0.27% apart and rates 1.00 points apart, and both
    // residuals are outside the rounding box of the printed fields. Pinning
    // that here means a future change cannot quietly claim the sample is
    // cleaner than it is.
    let ((call, _), (put, _)) = fitted_sides();
    let spot_spread = (call.spot / put.spot - 1.0).abs() * 100.0;
    let rate_spread = (call.rate - put.rate).abs() * 100.0;
    println!("residuals: spot spread {spot_spread:.4}%, rate spread {rate_spread:.4} points");
    assert!(
        (0.20..=0.35).contains(&spot_spread),
        "spot spread {spot_spread}% moved"
    );
    assert!(
        (0.85..=1.15).contains(&rate_spread),
        "rate spread {rate_spread} points moved"
    );
    // The honest summary: r is somewhere near ten percent and this sample
    // cannot say where, so nothing in this crate hardcodes one.
    let midpoint = (call.rate + put.rate) * 50.0;
    assert!(
        (9.0..=11.0).contains(&midpoint),
        "rate midpoint {midpoint}% moved"
    );
}
