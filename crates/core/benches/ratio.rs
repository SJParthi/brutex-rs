//! Gate 8 for `crates/core` — decoding one vendor row costs the same whatever
//! the vendor sent.
//!
//! # The regression this exists to catch
//!
//! `decode_master_row` substring-searched two raw vendor `&str` for test
//! markers with nothing bounding their length, about a hundred lines above the
//! only width guard in the pipeline. Measured before D-0033, with `underlying`
//! pinned so only the scanned-but-unused `trading_symbol` grew: 8 B → 56.4 ns,
//! 1 KiB → 102.0 ns, 16 KiB → 997.8 ns, 4 MiB → 245,839.2 ns — 4,289× the cost
//! of an ordinary row, and the 4 MiB row was **accepted and stored**.
//!
//! No gate could see it. Gate 8 tested for a `benches` directory at the
//! repository root, found none, and exited zero. This file is what that step
//! now runs.
//!
//! See `crates/store/benches/ratio.rs` for why there is no benchmarking
//! framework and why every ratio here is integer arithmetic.

use std::hint::black_box;
use std::time::Instant;

use brutex_core::vendor::{MAX_FIELD_BYTES, MasterRow, Vendor, decode_master_row};

/// A ratio above this is a failure. Thousandths, so the comparison is integer.
///
/// 3.0× — `docs/04-invariants.md`, the shared-CI number.
const CEILING_PERMILLE: u128 = 3_000;

/// How many times each measurement is repeated before the minimum is taken.
const TRIALS: u32 = 60;

/// Times one closure, in picoseconds per call, as a minimum over [`TRIALS`].
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

/// The defect's row exactly: a legitimate `underlying`, and a `trading_symbol`
/// that is scanned by the marker search and then never becomes the identity.
fn row(trading_symbol: &str) -> MasterRow<'_> {
    MasterRow {
        exchange: "NSE",
        segment: "CASH",
        underlying: "RELIANCE",
        trading_symbol,
        instrument_type: "EQ",
        listing_class: "EQ",
        isin: "INE002A01018",
        expiry: "",
        strike_rupees: "",
        option_side: "",
    }
}

/// C-09 — per-row decode cost does not track the width of a vendor field.
///
/// The filler byte is `X`, which appears in neither `NSETEST` nor `BSETEST`, so
/// both needles scan to the end and fail. That is the worst case a bound exists
/// for, and it is the case the old code paid in full.
fn decode_is_flat_in_field_width() -> bool {
    let widest_real = "NIFTYNXT50-Aug2026-101500-CE"; // 28 B, measured, both masters
    let at_bound = "X".repeat(MAX_FIELD_BYTES);
    let over = "X".repeat(MAX_FIELD_BYTES * 100);
    let absurd = "X".repeat(4 * 1024 * 1024);

    let time = |s: &str| {
        cost_ps(2_000, || {
            decode_master_row(Vendor::Groww, black_box(row(s)))
        })
    };
    let base = time(widest_real);
    let mut ok = true;
    ok &= ratio("C-09 decode, field at the bound", base, time(&at_bound));
    ok &= ratio("C-09 decode, field 100x the bound", base, time(&over));
    ok &= ratio("C-09 decode, field 4 MiB", base, time(&absurd));
    ok
}

/// C-10 — an over-wide row is refused, not merely decoded quickly.
///
/// Speed is only half of it. The 4 MiB row used to come back
/// `Ok(Keep(RELIANCE))` — accepted and stored — because the width guard saw
/// only the field that became the identity. A cost ratio alone would pass a
/// future implementation that got fast by looking at even less.
fn an_over_wide_row_is_refused() -> bool {
    let absurd = "X".repeat(4 * 1024 * 1024);
    let refused = decode_master_row(Vendor::Groww, row(&absurd)).is_err();
    let kept = decode_master_row(Vendor::Groww, row("NIFTYNXT50-Aug2026-101500-CE")).is_ok();
    println!(
        "  {:<44} refused={refused} ordinary-row-kept={kept}  {}",
        "C-10 a 4 MiB field is refused",
        if refused && kept { "ok" } else { "BREACH" }
    );
    refused && kept
}

fn main() {
    println!("gate 8 — crates/core, ceiling {CEILING_PERMILLE} permille");
    let mut ok = true;
    ok &= decode_is_flat_in_field_width();
    ok &= an_over_wide_row_is_refused();
    if ok {
        println!("all ratios within the ceiling");
    } else {
        println!("A RATIO BREACHED ITS CEILING — see the lines marked BREACH.");
        std::process::exit(1);
    }
}
