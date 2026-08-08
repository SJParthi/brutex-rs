//! Gate 8 for `crates/api` — rendering one page costs the same whatever the
//! size of the instrument set behind it.
//!
//! # The regression this exists to catch
//!
//! `docs/07-o1-architecture.md` layer 12 was marked BUILT with the words
//! "a fixed row cap with paging. NEVER O(universe)". Only the *rendered rows*
//! were capped. Every request re-folded the whole `by_key` map to compute the
//! universe pill counts, re-filtered it into a fresh `Vec`, and re-sorted that
//! vector — so the cost of drawing 200 rows tracked the size of the map behind
//! them. An audit of a running server measured 3.569 ms at 2,787 instruments
//! and 124.916 ms at 50,000, **rendering exactly 200 rows both times**, a
//! marginal 62.5 ns per instrument per request.
//!
//! # What is asserted, and why it is not one number
//!
//! A page at 2 instruments draws 2 rows and a page at 2,787 draws 200. Their
//! costs are *supposed* to differ — by rows, which is the bound the page is
//! allowed to have. Comparing them raw would assert the wrong thing and pass
//! the wrong code. So three gates, and each one isolates a different way the
//! defect could come back:
//!
//! | Gate | Compares | Catches |
//! |---|---|---|
//! | [`RATIO_CEILING_PERMILLE`] | 2,787 vs 50,000, both drawing 200 rows | any per-request whole-universe pass |
//! | [`MARGINAL_CEILING_PS`] | the slope between those two points | the audit's 62.5 ns per instrument, directly |
//! | [`PER_ROW_CEILING_PERMILLE`] | cost **per rendered row** at 2 vs at 50,000 | a page that got flat by drawing less |
//!
//! The absolute numbers move between machines; the shape does not, and the
//! shape is what these assert.
//!
//! See `crates/store/benches/ratio.rs` for why there is no benchmarking
//! framework and why every ratio here is integer arithmetic.

use std::hint::black_box;
use std::time::Instant;

use api::merge::{Entry, Merged};
use api::server::{Read, dashboard_html, instruments_html_from};
use brutex_core::instrument::{Exchange, InstrumentKey, Kind, Segment};
use brutex_core::isin::Isin;
use brutex_core::symbol::Symbol;
use brutex_core::universe::Universe;
use brutex_core::vendor::{Vendor, VendorSet};

/// A ratio above this is a failure, between two sizes that draw the same
/// number of rows. Thousandths, so the comparison is integer.
///
/// 3.0× — `docs/04-invariants.md`, the shared-CI number.
const RATIO_CEILING_PERMILLE: u128 = 3_000;

/// The most a request may cost per extra instrument in the universe, in
/// picoseconds.
///
/// 1,000 ps = 1 ns. The audit measured **62,500 ps** per instrument per
/// request against the pre-D-0042 code, so this is 62× below the defect and
/// still far above measurement noise on a flat implementation.
const MARGINAL_CEILING_PS: u128 = 1_000;

/// The most the cost **per rendered row** may grow between the smallest and
/// the largest universe. Thousandths.
///
/// Higher than the plain ratio because a 2-row page pays the page's fixed
/// chrome — nav, hero, pills, notes, table head — across 2 rows instead of
/// 200, so its per-row figure is legitimately the larger one. This gate is
/// here to catch the opposite: a large universe costing more per row.
const PER_ROW_CEILING_PERMILLE: u128 = 3_000;

/// How many times each measurement is repeated before the minimum is taken.
///
/// Lower than the 60 `crates/core` uses because one call here renders 200 rows
/// of HTML rather than decoding one row.
const TRIALS: u32 = 8;

/// The three sizes the audit measured, so the printed table is comparable.
///
/// `SMALL` draws fewer than a full page; `BASE` and `BIG` both draw a full 200
/// rows under every pill, which is what makes their ratio mean something.
const SMALL: usize = 2;
/// The real universe on the day of the audit.
const BASE: usize = 2_787;
/// Eighteen times the real universe.
const BIG: usize = 50_000;

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

/// Prints one ratio and returns whether it stayed under its ceiling.
fn ratio(label: &str, base_ps: u128, at_ps: u128, ceiling: u128) -> bool {
    if base_ps == 0 {
        println!("  {label:<46} UNMEASURABLE — the 1x baseline timed at zero");
        return false;
    }
    let permille = at_ps * 1_000 / base_ps;
    let ok = permille <= ceiling;
    println!(
        "  {label:<46} {:>10} ps -> {:>10} ps  ratio {}.{:03}x  {}",
        base_ps,
        at_ps,
        permille / 1_000,
        permille % 1_000,
        if ok { "ok" } else { "BREACH" }
    );
    ok
}

/// A bench may not `unwrap`, and `S0000000` is not a symbol that can fail.
/// If one ever is, say so and stop rather than measure nothing.
fn refuse(what: &str) -> ! {
    println!("  {what} was refused — the bench measured nothing");
    std::process::exit(1);
}

/// One synthetic instrument key.
fn key(n: usize) -> InstrumentKey {
    InstrumentKey {
        exchange: Exchange::Nse,
        segment: Segment::Cash,
        // A symbol holds 24 bytes; `S` plus seven digits is 8.
        underlying: Symbol::new(&format!("S{n:07}")).unwrap_or_else(|_| refuse("a symbol")),
        kind: Kind::Equity,
    }
}

/// A universe of `n` instruments, every one of them tracked so the page
/// actually renders rows.
///
/// Membership is set on the [`Entry`] directly rather than derived from
/// `universe::of_instrument`, because a synthetic symbol is in no real index
/// and a page that renders zero rows measures nothing.
///
/// Every fourth instrument is an F&O underlying and every tenth is an index,
/// so **every pill draws a full 200-row page** at [`BASE`] and at [`BIG`].
/// That is deliberate: a pill that ran out of rows at one size and not the
/// other would produce a ratio that measured the row count, not the universe.
fn read_of(n: usize) -> Read {
    let mut by_key = std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let mut u = Universe::TOTAL_MARKET;
        if i % 4 == 0 {
            u = u.union(Universe::FNO);
        }
        if i % 10 == 0 {
            u = u.union(Universe::INDEX);
        }
        by_key.insert(
            key(i),
            Entry {
                ids: [None; brutex_core::vendor::Vendor::ALL.len()],
                vendors: VendorSet::EMPTY.with(Vendor::Groww).with(Vendor::Dhan),
                // A body that fails the check digit is `None`, which is a real
                // state a row renders. Either way the ISIN column has content.
                isin: Isin::new(&format!("INE{:09}", i % 1_000_000_000))
                    .ok()
                    .map(|x| (Vendor::Groww, x)),
                conflict: None,
                universe: u,
            },
        );
    }
    Read::new(
        Merged {
            by_key,
            conflicts: Vec::new(),
            eligibility: Vec::new(),
        },
        Vec::new(),
        false,
        0,
        // unreadable: a synthetic bench universe decodes cleanly by construction.
        0,
    )
}

/// The three universes, built once. Rebuilding per measurement would time the
/// build, which is exactly the work D-0042 moved *out* of the request.
struct Fixtures {
    small: Read,
    base: Read,
    big: Read,
}

/// One page measured at all three sizes, with the row count it drew.
struct Measured {
    small_ps: u128,
    base_ps: u128,
    big_ps: u128,
    small_rows: usize,
    big_rows: usize,
}

/// How many `<tr` a page carries, less the header row.
fn drawn(html: &str) -> usize {
    html.matches("<tr").count().saturating_sub(1)
}

/// Measures one page at all three sizes.
fn measure(f: &Fixtures, reps: u32, page: impl Fn(&Read) -> String) -> Measured {
    Measured {
        small_ps: cost_ps(reps, || page(black_box(&f.small))),
        base_ps: cost_ps(reps, || page(black_box(&f.base))),
        big_ps: cost_ps(reps, || page(black_box(&f.big))),
        small_rows: drawn(&page(&f.small)),
        big_rows: drawn(&page(&f.big)),
    }
}

/// Applies all three gates to one measurement and prints every line.
fn gate(label: &str, m: &Measured) -> bool {
    let mut ok = ratio(
        &format!("C-14 {label} {BASE}->{BIG}"),
        m.base_ps,
        m.big_ps,
        RATIO_CEILING_PERMILLE,
    );

    // THE AUDIT'S NUMBER, DIRECTLY. Slope between two points that draw the
    // same rows, so every picosecond of it is universe size and nothing else.
    // Saturating: an implementation that got *faster* at the larger size has a
    // slope of zero, not a negative one.
    let marginal = m.big_ps.saturating_sub(m.base_ps) / (BIG - BASE) as u128;
    let slope_ok = marginal <= MARGINAL_CEILING_PS;
    println!(
        "  {:<46} {marginal:>10} ps per instrument per request       {}",
        format!("C-15 {label} marginal"),
        if slope_ok { "ok" } else { "BREACH" }
    );
    ok &= slope_ok;

    // AND THE PAGE STILL DREW ITS ROWS. Speed alone would pass an
    // implementation that got flat by rendering nothing, which is the same
    // class of mistake as capping the rows and calling the page constant.
    if m.small_rows == 0 || m.big_rows == 0 {
        println!(
            "  {:<46} drew {} and {} rows — MEASURED NOTHING",
            format!("C-17 {label} rows"),
            m.small_rows,
            m.big_rows
        );
        return false;
    }
    let per_row_small = m.small_ps / m.small_rows as u128;
    let per_row_big = m.big_ps / m.big_rows as u128;
    ok &= ratio(
        &format!("C-17 {label} per row {SMALL}->{BIG}"),
        per_row_small,
        per_row_big,
        PER_ROW_CEILING_PERMILLE,
    );
    println!(
        "      rows drawn: {} at n={SMALL}, {} at n={BIG}; raw n={SMALL} {} ps",
        m.small_rows, m.big_rows, m.small_ps
    );
    ok
}

/// C-16 — the dashboard costs the same at 2 and at 50,000 instruments.
///
/// The dashboard draws no rows at all, so the raw ratio between the smallest
/// and the largest universe is the whole story here and no per-row correction
/// applies.
fn the_dashboard_is_flat(f: &Fixtures) -> bool {
    let small = cost_ps(50, || dashboard_html(black_box(&f.small)));
    let base = cost_ps(50, || dashboard_html(black_box(&f.base)));
    let big = cost_ps(50, || dashboard_html(black_box(&f.big)));
    let mut ok = ratio(
        &format!("C-16 dashboard {SMALL}->{BASE}"),
        small,
        base,
        RATIO_CEILING_PERMILLE,
    );
    ok &= ratio(
        &format!("C-16 dashboard {SMALL}->{BIG}"),
        small,
        big,
        RATIO_CEILING_PERMILLE,
    );
    ok
}

/// C-14 — every sort order and every pill the UI offers is flat.
///
/// No combination is left unmeasured, because the defect was invisible
/// precisely where nobody looked.
fn every_order_and_pill_is_flat(f: &Fixtures) -> bool {
    let mut ok = true;
    for sort in [
        "", "symbol", "isin", "universe", "kind", "vendors", "-symbol",
    ] {
        for pill in ["", "fno", "ntm", "idx"] {
            let m = measure(f, 20, |r| {
                instruments_html_from(r, "", sort, false, pill, 0)
            });
            ok &= gate(&format!("sort={sort:?} pill={pill:?}"), &m);
        }
    }
    ok
}

/// C-14 — the escape hatch and a deep page are flat too.
///
/// `all=true` lifts the universe filter and `?page=9999` clamps to the last
/// page. Both were a separate `Vec` build and a separate sort before D-0042,
/// and the deep page additionally paid a `skip()` over everything ahead of it.
fn the_hatch_and_the_last_page_are_flat(f: &Fixtures) -> bool {
    let mut ok = true;
    for (label, all, page) in [("hatch all=true", true, 0), ("last page", false, 9_999)] {
        let m = measure(f, 20, |r| instruments_html_from(r, "", "", all, "", page));
        ok &= gate(label, &m);
    }
    ok
}

/// Search is the one thing that is **not** constant per rendered row, and it
/// is measured here rather than left silent. `docs/06-limits.md` §24.
///
/// Printed, never asserted: a substring that matches most of the universe must
/// look at most of the universe, and no index makes that untrue.
///
/// These synthetic names are the **worst possible** input for the trigram
/// index — every name is `NSE-S` followed by seven digits, so almost every
/// name carries almost every trigram and the index narrows nothing. Real
/// symbols share few trigrams. The figures below are therefore an upper bound
/// on search cost, not a typical one, and they are reported that way.
fn search_is_measured_and_named(f: &Fixtures) {
    for (needle, what) in [
        ("S0000001", "8 bytes, trigram-narrowed"),
        ("S0", "2 bytes — no trigram exists to narrow with"),
        ("S00", "a trigram nearly every synthetic name carries"),
    ] {
        for (n, read) in [(SMALL, &f.small), (BASE, &f.base), (BIG, &f.big)] {
            let ps = cost_ps(5, || {
                instruments_html_from(black_box(read), needle, "", false, "", 0)
            });
            println!(
                "  {:<46} {ps:>10} ps   ({what})",
                format!("§24 search {needle:?} n={n}")
            );
        }
    }
}

fn main() {
    println!("gate 8 — crates/api");
    println!(
        "  ratio ceiling {RATIO_CEILING_PERMILLE} permille · \
         marginal ceiling {MARGINAL_CEILING_PS} ps/instrument"
    );
    let started = Instant::now();
    let f = Fixtures {
        small: read_of(SMALL),
        base: read_of(BASE),
        big: read_of(BIG),
    };
    println!(
        "  built universes of {SMALL}, {BASE} and {BIG} once, in {} ms — \
         every figure below is per REQUEST",
        started.elapsed().as_millis()
    );
    let mut ok = true;
    ok &= the_dashboard_is_flat(&f);
    ok &= every_order_and_pill_is_flat(&f);
    ok &= the_hatch_and_the_last_page_are_flat(&f);
    search_is_measured_and_named(&f);
    if ok {
        println!("all ratios within the ceiling");
    } else {
        println!("A RATIO BREACHED ITS CEILING — see the lines marked BREACH.");
        std::process::exit(1);
    }
}
