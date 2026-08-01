//! The operator entry point for the HTTP surface.
//!
//! This file holds one call and nothing else, on purpose. A `main` cannot be
//! entered by a unit test — `cargo test` builds the binary with its own
//! harness and never runs it — so every line of logic left here is a line no
//! test can reach. Everything therefore lives in [`api::server`], which is
//! ordinary library code, and `tests/binary.rs` runs this binary once so that
//! even this call is measured rather than assumed.
//!
//! Usage: `api [serve [ADDR] | report]`.

/// Serves, or reports, and returns the exit code the operator's shell reads.
///
/// The shutdown signal is passed IN rather than installed inside the server,
/// so that every arm of [`api::server::run`] is drivable from a test without a
/// signal and without a hard kill. Ctrl-C is what an operator has; an
/// already-resolved future is what a test has.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::ExitCode::from(api::server::run(&args, Box::pin(tokio::signal::ctrl_c())).await)
}
