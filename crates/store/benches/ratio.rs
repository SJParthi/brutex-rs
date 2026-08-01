//! Gate 8 for `crates/store` — per-operation cost stays constant as the input
//! it sits in grows.
//!
//! # Why this file exists
//!
//! `CLAUDE.md` §3 rule 4 says "a change that makes one of them scan fails the
//! bench gate". Until D-0034 there was no bench gate: the workflow step tested
//! for a `benches` directory at the repository **root**, Cargo benches live at
//! `crates/<name>/benches`, and no such directory existed anywhere — so gate 8
//! took its skip arm and reported success without measuring anything. Both
//! defects this change repairs merged through it green.
//!
//! # Why no benchmarking framework
//!
//! `CLAUDE.md` §2 does not forbid a Rust dev-dependency, but a harness would
//! add a tree of them to measure four numbers with `Instant`. `harness = false`
//! makes this an ordinary binary: `cargo bench` runs it, it prints every number
//! it took, and it exits non-zero when a ratio breaches the ceiling. Nothing is
//! reported as passing that was not measured.
//!
//! # What "flat" is measured against
//!
//! `docs/04-invariants.md` sets the ceiling: **1.4×** on dedicated hardware,
//! **3.0×** on shared CI. This file asserts the CI number, because CI is what
//! actually runs, and prints the ratio so a local run can be read against the
//! tighter one.
//!
//! All arithmetic is integer. `clippy::float_arithmetic` is a workspace lint
//! and a ratio is the one place it would be tempting.

use std::hint::black_box;
use std::time::Instant;

use store::block;
use store::crc::crc32c;
use store::format::{BLOCK_LEN, FLAG_CHECKSUMS, HEADER_LEN, SLOT_LEN};
use store::header::Header;
use store::layout::Layout;

/// A ratio above this is a failure. Thousandths, so the comparison is integer.
///
/// 3.0× — `docs/04-invariants.md`, the shared-CI number. A ratio between 1.4×
/// and 3.0× on shared hardware is re-run on dedicated hardware before it is
/// believed; above 3.0× it is a regression on any hardware.
const CEILING_PERMILLE: u128 = 3_000;

/// How many times each measurement is repeated before the minimum is taken.
///
/// The minimum rather than the mean: a shared runner's scheduler can only ever
/// make a sample slower, so the smallest observation is the closest thing to
/// the cost of the work itself. Measured against a foreign process at 686% CPU
/// during this session, the minimum moved 0.2% across three runs while the
/// median moved 78%.
const TRIALS: u32 = 60;

/// Times one closure, in picoseconds per call, as a minimum over [`TRIALS`].
///
/// Picoseconds because the fastest operation here is a few nanoseconds and an
/// integer ratio of small nanosecond counts is mostly rounding.
fn cost_ps<T>(reps: u32, mut op: impl FnMut() -> T) -> u128 {
    let mut best = u128::MAX;
    for _ in 0..TRIALS {
        let start = Instant::now();
        for _ in 0..reps {
            black_box(op());
        }
        let ps = start.elapsed().as_nanos() * 1_000 / u128::from(reps);
        if ps < best {
            best = ps;
        }
    }
    best
}

/// Prints one measurement and returns whether it stayed under the ceiling.
///
/// `base` is the 1× cost. A base of zero means the clock could not resolve the
/// operation at all, which is reported as a failure rather than divided by:
/// an unmeasurable baseline makes every ratio meaningless, and reporting a
/// ratio nobody measured is what `CLAUDE.md` §3 rule 6 forbids.
fn ratio(label: &str, base_ps: u128, at_ps: u128) -> bool {
    if base_ps == 0 {
        println!("  {label:<44} UNMEASURABLE — the 1x baseline timed at zero");
        return false;
    }
    let permille = at_ps * 1_000 / base_ps;
    let ok = permille <= CEILING_PERMILLE;
    println!(
        "  {label:<44} {:>9} ps -> {:>9} ps   ratio {}.{:03}x  {}",
        base_ps,
        at_ps,
        permille / 1_000,
        permille % 1_000,
        if ok { "ok" } else { "BREACH" }
    );
    ok
}

/// A zeroed header region holding one genesis commit, `slots` slots long.
fn region(slots: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; slots * SLOT_LEN];
    let Ok(genesis) = Header::genesis(1, 60, FLAG_CHECKSUMS).commit() else {
        return bytes;
    };
    for (dst, src) in bytes.iter_mut().zip(genesis.bytes) {
        *dst = src;
    }
    bytes
}

/// C-01 — reading the header costs the same whatever region it is handed.
///
/// The region a caller passes may be a whole read-only mapping of the file, so
/// "the region grew" is "the file grew". [`Header::read_region`] is bounded by
/// `MAX_SLOTS` positions rather than by the region's length; before that bound
/// existed it auditioned every aligned run of record bytes as a header.
fn header_read_is_flat() -> bool {
    let file_len = HEADER_LEN;
    // 1x is the real header region; 10x and 100x are what a mapping looks like.
    let one = region(512);
    let ten = region(5_120);
    let hundred = region(51_200);
    let time = |r: &[u8]| cost_ps(200, || Header::read_region(black_box(r), file_len));
    let base = time(&one);
    let a = ratio("C-01 header read, 10x region", base, time(&ten));
    let b = ratio("C-01 header read, 100x region", base, time(&hundred));
    a && b
}

/// C-07 — sealing one block costs the same whatever the file holds.
///
/// This is the per-operation claim `docs/02-store-format.md` makes about
/// verification: one block's checksum, not a file scan. `n_valid` is the whole
/// file's record count and the block index is fixed, so a cost that tracked
/// `n_valid` would mean the verifier had started reading past its block.
fn block_seal_is_flat() -> bool {
    let bytes = vec![0x5Au8; usize::try_from(BLOCK_LEN).unwrap_or(0)];
    let records_per_block = BLOCK_LEN / 56;
    let time = |n_valid: u64| {
        cost_ps(200, || {
            block::seal(Layout::V2, n_valid, 0, black_box(&bytes))
        })
    };
    let base = time(records_per_block);
    let a = ratio(
        "C-07 block seal, 10x file",
        base,
        time(records_per_block * 10),
    );
    let b = ratio(
        "C-07 block seal, 100x file",
        base,
        time(records_per_block * 100),
    );
    a && b
}

/// The bit-by-bit kernel this store ran until D-0032.
///
/// Kept here so the comparison below is measured in the same process, on the
/// same machine, under the same load — the only shape of absolute-speed
/// assertion that means anything on a shared CI runner.
fn crc32c_bitwise(bytes: &[u8]) -> u32 {
    const POLYNOMIAL: u32 = 0x82F6_3B78;
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8u8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (POLYNOMIAL & mask);
        }
    }
    !crc
}

/// C-08 — the block checksum stays far away from the bit-by-bit kernel.
///
/// A ratio, not a nanosecond ceiling. An absolute threshold would have to be
/// guessed for hardware this repository has never measured on — CI is `x86_64`
/// and the operator's machine is `aarch64` — and a guessed threshold is either
/// red for no reason or green for every regression. Both kernels here scale
/// with the machine, so their ratio does not.
///
/// Measured on an Apple M4 Pro over one 4,088-byte block: 9.4×. The floor is
/// set at 3× so ordinary hardware variation cannot trip it, while a return to
/// walking bit by bit — which is what happened, and what this exists to catch —
/// cannot pass it.
fn checksum_beats_the_bit_loop() -> bool {
    const FLOOR_PERMILLE: u128 = 3_000;
    let bytes = vec![0x5Au8; usize::try_from(BLOCK_LEN).unwrap_or(0)];
    let fast = cost_ps(200, || crc32c(black_box(&bytes)));
    let slow = cost_ps(20, || crc32c_bitwise(black_box(&bytes)));
    if fast == 0 {
        println!("  C-08 checksum vs bit loop                    UNMEASURABLE");
        return false;
    }
    let permille = slow * 1_000 / fast;
    let ok = permille >= FLOOR_PERMILLE;
    println!(
        "  {:<44} {:>9} ps vs {:>9} ps  speedup {}.{:03}x  {}",
        "C-08 checksum vs the bit loop it replaced",
        fast,
        slow,
        permille / 1_000,
        permille % 1_000,
        if ok { "ok" } else { "BREACH" }
    );
    ok
}

fn main() {
    println!("gate 8 — crates/store, ceiling {CEILING_PERMILLE} permille");
    let mut ok = true;
    ok &= header_read_is_flat();
    ok &= block_seal_is_flat();
    ok &= checksum_beats_the_bit_loop();
    if ok {
        println!("all ratios within the ceiling");
    } else {
        println!("A RATIO BREACHED ITS CEILING — see the lines marked BREACH.");
        std::process::exit(1);
    }
}
