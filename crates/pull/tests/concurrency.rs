//! The rate governor with more than one caller: `pull::concurrency::*`.
//!
//! # Why this file is not a `loom` test
//!
//! `docs/04-invariants.md` P-01 named `pull::loom::governor_ceiling`, which
//! exists in no file, and `loom` appears in no `Cargo.toml` here. Taking that
//! dependency to satisfy a row would be backwards, and it would be measuring
//! something this type cannot do: [`Governor::admit`] takes `&mut self` and the
//! type holds no interior mutability, so **safe Rust admits exactly one shape
//! of sharing** — exclusive access, taken one caller at a time. `crates/pull`
//! is `#![forbid(unsafe_code)]`, so there is no second shape hiding behind a
//! raw pointer. An interleaving checker enumerates orderings of steps that can
//! interleave; no step of `admit` can interleave with another `admit`, because
//! the borrow checker will not compile the program in which they do.
//!
//! What is left to prove is therefore not "which interleaving" but **the
//! ceiling itself, under concurrent callers**: that eight threads racing for
//! the same budget are collectively held to the allowance one caller would
//! have had, rather than each seeing a budget of its own. That is what these
//! tests assert, with real threads, against exact counts.
//!
//! # No clock is read and nothing sleeps
//!
//! [`Governor::admit`] takes the instant as an argument, so every thread here
//! hands over the **same** microsecond and the result is deterministic: a test
//! that slept would be asserting on the scheduler. The only nondeterminism
//! left is which thread wins which permit, and no assertion here depends on
//! that — the permits are counted in total, which is what a vendor counts.
//!
//! # What this does not prove
//!
//! **Two governors are two budgets.** Nothing here — and nothing in the
//! crate — bounds the sum of two [`Governor`] values built for the same
//! vendor. One governor per task would publish one ceiling per task and the
//! vendor would see their sum. The type is `Copy`, so that mistake is one
//! accidental dereference away, and it is a real limit rather than a
//! hypothetical one.

// A test that asserts nothing is banned, and a test that cannot fail loudly is
// a test that asserts nothing.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use pull::rate::{Governor, MICROS_PER_SECOND, Verdict, WindowSpan};

/// The per-second allowance every test here is held to.
///
/// Five is `docs/00-charter.md` §4's figure for one of the two vendors, and it
/// is small enough that "exactly this many admits" is a sharp assertion: a
/// governor that leaked one permit per thread would report eight.
const CEILING: u32 = 5;

/// How many threads race for it.
const THREADS: usize = 8;

/// How many times each thread asks.
///
/// Far more than the ceiling, so every thread is refused many times and the
/// refusal path is exercised under contention rather than only the grant.
const ATTEMPTS: usize = 64;

/// Hammers `governor` from [`THREADS`] threads at one fixed instant.
///
/// Returns how many requests were admitted in total. Every thread asks
/// [`ATTEMPTS`] times, so the number of *calls* is fixed and known and the
/// number of *grants* is the thing under test.
fn hammer(governor: &Mutex<Governor>, now_micros: u64) -> u64 {
    let admitted = AtomicU64::new(0);
    let denied = AtomicU64::new(0);

    thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                for _ in 0..ATTEMPTS {
                    // The lock is what `&mut self` forces a caller to supply.
                    // It is taken here, in the test, precisely because the
                    // crate does not supply one — see the module header.
                    let mut held = governor.lock().expect("the governor lock");
                    match held.admit(now_micros) {
                        Verdict::Admit => admitted.fetch_add(1, Ordering::Relaxed),
                        Verdict::Deny { .. } => denied.fetch_add(1, Ordering::Relaxed),
                    };
                }
            });
        }
    });

    let calls = u64::try_from(THREADS * ATTEMPTS).expect("a call count that fits");
    assert_eq!(
        admitted.load(Ordering::Relaxed) + denied.load(Ordering::Relaxed),
        calls,
        "every call returned exactly one verdict"
    );
    admitted.load(Ordering::Relaxed)
}

// ===========================================================================
// P-01
// ===========================================================================

#[test]
fn the_ceiling_holds_however_many_threads_share_the_governor() {
    let governor = Mutex::new(
        Governor::new(Some(CEILING), None, None).expect("a governor with a per-second ceiling"),
    );

    // 512 requests arrive at one instant from eight threads. Exactly five are
    // issued, because the vendor published five per second and the bucket
    // starts full. A governor that kept per-caller state would issue forty.
    assert_eq!(
        hammer(&governor, 0),
        u64::from(CEILING),
        "the whole thread pool shares one budget"
    );

    // One second later the bucket has earned itself back — exactly once, not
    // once per thread.
    assert_eq!(hammer(&governor, MICROS_PER_SECOND), u64::from(CEILING));

    // Two seconds of credit is still one bucket: idle time does not
    // accumulate into a burst the vendor never agreed to.
    assert_eq!(hammer(&governor, 3 * MICROS_PER_SECOND), u64::from(CEILING));

    let held = governor.lock().expect("the governor lock");
    assert_eq!(
        held.ceiling(WindowSpan::Second),
        Some(CEILING),
        "the published ceiling is untouched by any of it"
    );
    assert_eq!(
        held.permitted(WindowSpan::Second),
        Some(CEILING),
        "and so is the allowance: nothing recorded a throttle"
    );
}

#[test]
fn a_throttle_recorded_by_one_thread_binds_every_other() {
    let governor = Mutex::new(
        Governor::new(Some(CEILING), None, None).expect("a governor with a per-second ceiling"),
    );

    // One caller learns the vendor is refusing. Multiplicative decrease: the
    // allowance halves and the bucket drains.
    governor
        .lock()
        .expect("the governor lock")
        .record_throttled();
    assert_eq!(
        governor
            .lock()
            .expect("the governor lock")
            .permitted(WindowSpan::Second),
        Some(2),
        "five halved, rounded down"
    );

    // The drained bucket means nothing at all is issued at that instant, from
    // any thread — a caller that had already read the old allowance does not
    // get to spend a budget that was just disproven.
    assert_eq!(hammer(&governor, 0), 0, "a drained bucket issues nothing");

    // A second later the halved allowance is what earns back, and it is what
    // bounds the pool: two, not five, however many threads are asking.
    assert_eq!(hammer(&governor, MICROS_PER_SECOND), 2);

    let held = governor.lock().expect("the governor lock");
    assert_eq!(
        held.ceiling(WindowSpan::Second),
        Some(CEILING),
        "the ceiling is the vendor's published figure and a throttle never edits it"
    );
}
