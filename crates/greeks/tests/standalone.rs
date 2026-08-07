//! The structural proof that this crate is shareable.
//!
//! `crates/greeks` is consumed by the `tickvault` repository as well as this
//! one, by git URL. What this file proves, exactly:
//!
//! **Every public item listed below is reachable from outside the crate with
//! nothing else in scope, and every value crossing the boundary is an `f64`, a
//! `u32`, an `i32` or a plain enum this crate owns.** An integration test is a
//! separate crate that can only see what the manifest lets it see, so a
//! signature that had come to require a type from this workspace would make
//! this file stop compiling.
//!
//! **What it does NOT prove, stated because it was once claimed to.** An
//! integration test can only fail on the items it *names*. A public item this
//! file does not mention could take or return a workspace type and nothing
//! here would notice; a `pub use` of one adds no code region at all, so even
//! the coverage gate cannot see it. Both were measured, in a copy of this
//! workspace outside the repository: re-exporting `core`'s paisa type from
//! this crate's root left clippy, the whole suite and 100% coverage green.
//! **CI gate 9b is the enforcement** — it reads the manifest and refuses a
//! dependency, a dev-dependency, or any mention of a workspace type in this
//! crate's sources. This file is the ergonomic check that sits beside it.
//! `docs/05-decisions.md` D-0046.
//!
//! No paisa appears anywhere in this file, which is the other half of that
//! entry's argument: the crate is `f64` throughout *because it never sees
//! money*.

// The same exceptions every test module in this workspace takes: a test that
// cannot panic cannot fail, and the lints that forbid panicking exist to keep
// a panic out of the LIBRARY, not out of its tests. `float_arithmetic` and
// `float_cmp` are here for the reason the whole crate exists -- see the crate
// documentation and D-0046.
#![allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_arithmetic,
    clippy::float_cmp
)]

use greeks::{
    Contract, GreeksError, ImpliedVolatility, Method, Moneyness, OptionKind, Position,
    standard_normal_cdf, standard_normal_pdf,
};

/// Every public item, reached from outside the crate, with nothing else in
/// scope. This function is the test.
#[test]
fn the_whole_public_surface_is_reachable_with_nothing_else_in_scope() {
    let contract = Contract {
        spot: 25_851.19,
        strike: 25_850.0,
        years_to_expiry: 7.0 / 365.0,
        rate: 0.0946,
        carry: 0.0,
    };

    // Pricing and the five greeks, both sides.
    for kind in [OptionKind::Call, OptionKind::Put] {
        let out = contract.greeks(0.098, kind).expect("in range");
        assert!(out.is_finite());
        assert_eq!(
            out.price,
            contract.price(0.098, kind).expect("the same price")
        );
        assert!(out.gamma > 0.0);
        assert!(out.vega > 0.0);
        assert!(out.theta < 0.0);
        assert!(out.d1 > out.d2);
        // rho is here even though Dhan omits it; Groww publishes one.
        assert!(out.rho.abs() > 0.0);
    }

    // The solver, including the fields that make its answer checkable.
    let quoted = contract.price(0.098, OptionKind::Call).expect("priced");
    let solved: ImpliedVolatility = contract
        .implied_volatility(quoted, OptionKind::Call)
        .expect("a quote the model can reach");
    assert!((solved.volatility - 0.098).abs() < 1e-9);
    assert!(solved.iterations <= greeks::solver::MAX_ITERATIONS);
    assert!(matches!(solved.method, Method::Newton | Method::Bisection));
    assert!(solved.vega > 0.0);
    assert!(solved.uncertainty < greeks::solver::MAX_RELATIVE_UNCERTAINTY);

    // The ladder.
    let rung =
        Moneyness::from_ladder(25_950.0, 25_850.0, 50.0, OptionKind::Call).expect("on the grid");
    assert_eq!(rung.position, Position::OutOfTheMoney);
    assert_eq!(rung.steps, 2);
    assert_eq!(rung.distance, 2);
    assert_eq!(rung.to_string(), "OTM+2");

    // The distribution.
    assert_eq!(standard_normal_cdf(0.0), 0.5);
    assert!(standard_normal_pdf(0.0) > 0.39);

    // And a refusal, which is the part a consumer has to be able to match on.
    let refused = contract
        .implied_volatility(-1.0, OptionKind::Put)
        .expect_err("a negative quote has no implied volatility");
    assert!(matches!(refused, GreeksError::PriceBelowIntrinsic { .. }));
    assert!(!refused.to_string().is_empty());
    let as_std: &dyn std::error::Error = &refused;
    assert!(!as_std.to_string().is_empty());

    // A strike between two rungs is refused rather than rounded onto one, and
    // the refusal names the tolerance it broke.
    let off_grid = Moneyness::from_ladder(25_875.0, 25_850.0, 50.0, OptionKind::Call)
        .expect_err("a strike halfway between two rungs is not on the ladder");
    assert_eq!(
        off_grid,
        GreeksError::OffGrid {
            steps: 0.5,
            tolerance: greeks::moneyness::STEP_TOLERANCE
        }
    );

    // The bounds are public because a caller has to be able to check its own
    // input against them before it asks. Each is compared against a value this
    // test computed, never against another constant -- an assertion whose two
    // sides are both constants is evaluated by the compiler and proves nothing
    // at run time.
    assert!(contract.spot < greeks::bsm::MAX_UNDERLYING);
    assert!(contract.years_to_expiry < greeks::bsm::MAX_YEARS);
    assert!(contract.rate.abs() < greeks::bsm::MAX_RATE);
    assert!(solved.volatility < greeks::bsm::MAX_VOLATILITY);
    assert!(solved.volatility > greeks::solver::MIN_VOLATILITY);
    assert!(solved.volatility < greeks::solver::MAX_VOLATILITY);
    assert!(rung.steps < greeks::moneyness::MAX_STEPS);
    // Every evaluation is counted, including the two that establish the
    // bracket and the one at the answer. 2 + 8 + 64 + 1.
    assert_eq!(
        greeks::solver::MAX_ITERATIONS,
        greeks::solver::BRACKET_EVALUATIONS
            + greeks::solver::NEWTON_STEPS
            + greeks::solver::BISECTION_STEPS
            + greeks::solver::FINAL_EVALUATION
    );
    assert!(solved.iterations > greeks::solver::BRACKET_EVALUATIONS);
}
