//! Black-Scholes-Merton greeks and implied volatility for **European**
//! options, in `f64`.
//!
//! Index options on NSE are European, which is what makes a closed form
//! correct here rather than an approximation to an early-exercise value.
//!
//! # Why this is its own crate, and why it declares nothing
//!
//! Two reasons, and they point the same way.
//!
//! 1. **It is shared.** The `tickvault` repository consumes it by git URL.
//!    So it declares zero dependencies — the same rule `crates/core` follows —
//!    and **no type from this workspace appears in its public surface**. Every
//!    argument and every field is an `f64` or a plain enum this crate owns.
//!    A consumer takes this crate and gets this crate. Both properties are
//!    held by **CI gate 9b**, which reads the manifest and the sources; the
//!    integration test in `tests/standalone.rs` is a companion and not the
//!    proof, because a test can only fail on the items it names. Its
//!    `rust-version` is written literally rather than inherited from the
//!    workspace, for the same reason: a consumer on an older stable is
//!    otherwise refused before a line is compiled.
//!
//! 2. **It is the one place a float is the right type.** `CLAUDE.md` §7 fixes
//!    prices at `i64` paisa and says in the same breath that *statistical
//!    values keep full precision and are never rounded for storage*. A delta
//!    is a derivative, a gamma is a second derivative, an implied volatility
//!    is a solved parameter: they are statistical values, not money, and none
//!    of them is representable on a two-decimal tick grid. So this crate is
//!    `f64` throughout — **and it never sees a paisa.** There is no `i64` in
//!    its surface, no conversion from one, and no rounding to a tick. The
//!    caller converts at its own boundary; this crate never learns that money
//!    was involved.
//!
//!    That separation is what buys the lint exception. `clippy::float_arithmetic`
//!    is denied across the workspace because a float in a *price* path is a
//!    defect. Every module here that does arithmetic carries a narrowly scoped
//!    `#![allow(clippy::float_arithmetic)]` with that reason attached; the
//!    workspace level is untouched, so the lint still fires everywhere else.
//!    See `docs/05-decisions.md` D-0046.
//!
//! # What is here
//!
//! | Module | Owns |
//! |---|---|
//! | [`error`] | every refusal, each naming what was wrong |
//! | [`normal`] | the standard normal density and distribution |
//! | [`bsm`] | `d1`, `d2`, price, delta, gamma, vega, theta, rho |
//! | [`solver`] | implied volatility, and the bound on its cost |
//! | [`moneyness`] | a strike's position on the ladder, in strike steps |
//!
//! # Rho is here even though one vendor omits it
//!
//! Dhan's option chain publishes implied volatility, delta, gamma, theta and
//! vega, and **no rho field** — measured against a live response. Groww
//! publishes all five including rho. Rho is one more `N(d2)` on a quantity
//! already computed, so omitting it would save nothing and would make this
//! crate unable to reproduce one of the two vendors.
//!
//! # The solver is not O(1), and does not claim to be
//!
//! [`bsm`] is closed form: a fixed number of arithmetic operations, no loop,
//! no input-dependent cost. [`solver`] is a root find. `CLAUDE.md` §3 rule 4
//! is about per-operation cost, and inverting a transcendental function is not
//! one operation. What the solver has instead is a **hard bound that is
//! arithmetic rather than hopeful** — see [`solver`] and `docs/06-limits.md`
//! §18.
//!
//! # Every refusal is loud
//!
//! No function here returns a plausible-looking number it did not compute. A
//! non-finite input, an expired contract, a non-positive spot, strike or
//! volatility, a price below intrinsic or above the maximum, and a price that
//! does not determine a volatility at all are each a distinct [`GreeksError`]
//! that names the quantity and the bound it broke.
//!
//! # Reproducible within one target, and not across two
//!
//! Same inputs, same bits — on one machine and one libm. Every `d1`, every
//! discount factor and both branches of the CDF go through `exp` and `ln`,
//! which no mainstream libm rounds correctly and which IEEE-754 does not
//! require it to. Measured between this platform's libm and the pure-Rust
//! `libm` crate that `wasm32` links: 140 of 1,344 solved volatilities differ,
//! worst `4.22e-14` relative. **A greek or an implied volatility from this
//! crate must therefore never enter a content hash or be compared byte for
//! byte across machines.** `docs/06-limits.md` §18 carries the measurement.
//!
//! ```
//! use greeks::{Contract, OptionKind};
//!
//! let contract = Contract {
//!     spot: 25_851.19,
//!     strike: 25_850.0,
//!     years_to_expiry: 7.0 / 365.0,
//!     rate: 0.0946,
//!     carry: 0.0,
//! };
//! let call = contract.greeks(0.098, OptionKind::Call).expect("well-formed");
//! let put = contract.greeks(0.098, OptionKind::Put).expect("well-formed");
//!
//! // Gamma and vega do not know which side you are on.
//! assert_eq!(call.gamma.to_bits(), put.gamma.to_bits());
//! assert_eq!(call.vega.to_bits(), put.vega.to_bits());
//! ```

#![forbid(unsafe_code)]

pub mod bsm;
pub mod error;
pub mod moneyness;
pub mod normal;
pub mod solver;

pub use bsm::{Contract, Greeks, OptionKind};
pub use error::GreeksError;
pub use moneyness::{Moneyness, Position};
pub use normal::{standard_normal_cdf, standard_normal_pdf};
pub use solver::{ImpliedVolatility, Method};
