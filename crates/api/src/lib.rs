//! HTTP surface. Server-rendered HTML, no JavaScript anywhere.
//!
//! `docs/01-architecture.md` permits this crate `core`, `store`, `engine` and
//! — since D-0038 — `pull`. That is a maximum, not a requirement: `engine`
//! does not exist yet, so it is not taken.
//!
//! | Module | Owns |
//! |---|---|
//! | [`audit`] | what every pull did, on disk, one fixed-stride record each |
//! | [`master`] | reading one vendor's instrument master off disk |
//! | [`merge`] | one map from every vendor, and the ISIN cross-check on it |
//! | [`ingest`] | what the operator asked a pull to do, and every named refusal |
//! | [`census`] | what the store holds, read from the counter file, never from a directory |
//! | [`catalog`] | every ordering and every filter a page offers, decided once at load |
//! | [`render`] | turning instruments into HTML |
//! | [`server`] | the routes, the process, and everything `main` would hold |
//!
//! # Why `pull` is a dependency
//!
//! `/pull` and `/store` are the operator's window onto ingest, and everything
//! they render is a type `pull` already owns: [`pull::session::Day`] is the
//! validated calendar, [`pull::session::Window`] is the inclusive range and the
//! one place the vendor's non-inclusive `toDate` is reconciled,
//! [`pull::session::DropCensus`] is the drop tally, and
//! [`pull::manifest::Manifest`] is the counter file the `/store` page reads
//! instead of walking ~248,000 directory entries. Re-deriving any of them here
//! would be a second definition of a rule that already has one. D-0038.

#![forbid(unsafe_code)]

pub mod audit;
pub mod calendar;
pub mod catalog;
pub mod census;
pub mod ingest;
pub mod master;
pub mod merge;
pub mod render;
pub mod server;

/// Temporary fixture paths, unique per process. Compiled only under `cfg(test)`
/// — it exists so two concurrent test processes cannot delete each other's
/// fixtures, which is a property of the test suite and not of the server.
#[cfg(test)]
pub(crate) mod scratch;
