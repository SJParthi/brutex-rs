//! Types, errors, the frozen condition table, and the trading calendar.
//!
//! This crate depends on nothing. That is a structural rule, not a
//! preference: `crates/web` compiles to `wasm32-unknown-unknown` and is
//! allowed exactly one dependency, this one. Every rule that both the server
//! and the browser must agree on — how a price renders, what counts as a
//! trading day, which bit means which condition — lives here and is compiled
//! twice from one source, so the two cannot drift.
//!
//! See `docs/01-architecture.md` §2 and `docs/05-decisions.md` D-0009.
//!
//! # What is here
//!
//! | Module | Owns |
//! |---|---|
//! | [`error`] | every error this crate can return |
//! | [`price`] | paisa integers and the one float boundary |
//! | [`symbol`] | fixed-width symbols, so hashing one is constant time |

#![forbid(unsafe_code)]

pub mod error;
pub mod price;
pub mod symbol;
