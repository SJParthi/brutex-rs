//! HTTP surface. Server-rendered HTML, no JavaScript anywhere.
//!
//! `docs/01-architecture.md` permits this crate `core`, `store` and `engine`.
//! That is a maximum, not a requirement: `store` and `engine` do not exist
//! yet, so today it depends on `core` alone and will gain the others when they
//! land.
//!
//! | Module | Owns |
//! |---|---|
//! | [`master`] | reading one vendor's instrument master off disk |
//! | [`merge`] | one map from every vendor, and the ISIN cross-check on it |
//! | [`render`] | turning instruments into HTML |
//! | [`server`] | the routes, the process, and everything `main` would hold |

#![forbid(unsafe_code)]

pub mod master;
pub mod merge;
pub mod render;
pub mod server;
