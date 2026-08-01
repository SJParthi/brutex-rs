//! HTTP surface. Server-rendered HTML, no JavaScript anywhere.
//!
//! `docs/01-architecture.md` permits this crate `core`, `store` and `engine`.
//! That is a maximum, not a requirement: `store` and `engine` do not exist
//! yet, so today it depends on `core` alone and will gain the others when they
//! land.

#![forbid(unsafe_code)]

pub mod render;
