//! Vendor ingest: where the credential path comes from, how the credential is
//! read, and what is counted so nothing ever has to be walked.
//!
//! # What is here
//!
//! | Module | Owns |
//! |---|---|
//! | [`config`] | the untracked path configuration, and the only thing that assembles a parameter path |
//! | [`secret`] | the one-method secret source, the SSM adapter, and the loud halts |
//! | [`manifest`] | the per-vendor counter file — layer 13 of `docs/07-o1-architecture.md` |
//!
//! # What is deliberately **not** here
//!
//! **No vendor HTTP call, and no `/pull` page.** A control panel for a
//! downloader that cannot download is a dashboard that reports on nothing, and
//! this repository already refuses that shape everywhere else. The page ships in
//! the change that ships the fetch.
//!
//! **No AWS SDK.** `src/secret.rs` defines the port an SDK plugs into and a
//! thin adapter over it; the dependency is taken by the change that first makes
//! a live call. See `crates/pull/Cargo.toml` and `docs/05-decisions.md` D-0035.
//!
//! **No rate governor, no window walk, no calendar filter.** `P-01` through
//! `P-04` in `docs/04-invariants.md` keep their `—` status; this change moves
//! `P-05` through `P-08` and adds the manifest rows.
//!
//! # The one rule that shapes every line of `config`
//!
//! `CLAUDE.md` §8: **no literal parameter path appears in any tracked file.**
//! This repository is public, and a Parameter Store path names an account, an
//! environment and a vendor relationship; once pushed it is in every fork's
//! history forever. So this crate holds the *shape* — `/<org>/<env>/<vendor>/
//! <field>` — and the field *names* come from a local file that is never
//! committed. There is no `org` literal, no `env` literal and no real vendor
//! path segment anywhere under `crates/pull`, including in its tests, which use
//! invented segments. CI gate 1c is the check that this held.
//!
//! # The one rule that shapes every line of `secret`
//!
//! `CLAUDE.md` §8: the credential **value** is never an environment variable,
//! never a file, never a prompt, and **this repository never mints a token.**
//! [`secret::SecretSource`] therefore has exactly one method and it reads. There
//! is no write, no put, no mint and no refresh-by-creation anywhere in the
//! crate's surface, so a token cannot be minted by code that does not exist.
//! `pull::unit::readonly_credentials` proves it against a double that panics if
//! a write is ever attempted.
//!
//! # The one rule that shapes every line of `manifest`
//!
//! `docs/07-o1-architecture.md` law 3: **never scan to answer a question.**
//! "How many expired option series do I hold" against a directory tree of
//! ~248,000 files is ~248,000 `stat` calls, and it gets slower with every
//! ingest. A counter maintained on write turns it into one read of one header.

#![forbid(unsafe_code)]

pub mod config;
pub mod manifest;
pub mod secret;
