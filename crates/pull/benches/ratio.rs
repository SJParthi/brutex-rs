//! Gate 8 for `crates/pull` — layer 13 asserted as a number.
//!
//! # Why this file exists
//!
//! `docs/07-o1-architecture.md`: *"A layer is not built because the code looks
//! right. It is built when a test asserts the bound as a number."* Layer 13 is
//! "counters, never scans", and the two numbers that make it real are here:
//! reading the census costs the same whatever the census holds, and it beats
//! re-deriving the same answer from the entries by a wide margin, measured in
//! the same process.
//!
//! # Why no benchmarking framework
//!
//! `harness = false` makes this an ordinary binary: `cargo bench` runs it, it
//! prints every number it took, and it exits non-zero when a ratio breaches its
//! ceiling. Nothing is reported as passing that was not measured. `test = false`
//! keeps `cargo test` and the coverage gate out of a timing loop. All
//! arithmetic is integer — `clippy::float_arithmetic` is a workspace lint and a
//! ratio is the one place it would be tempting.
//!
//! # What is measured, and what is NOT
//!
//! Measured: the cost of the answer, against the cost of deriving that same
//! answer from the manifest's own entries, on one machine, in one process,
//! under one load.
//!
//! **Not measured: the directory walk this file exists to replace.** The real
//! alternative to a manifest is a `stat` on roughly 248,000 files, and that
//! depends on a filesystem, a page cache and a device this harness does not
//! control — timing it would report the state of one machine's cache rather
//! than a property of the code. Any statement about that saving is an
//! **EXTRAPOLATION** from the entry scan below, and `docs/06-limits.md` §17
//! records it as one.
//!
//! Not measured either: residency. A probe into a 100,000-entry map that has
//! fallen out of cache costs more than one into a map that has not, and that is
//! layer 7's subject rather than layer 3's. These measurements probe one key
//! repeatedly, so what they report is the **probe count** — which is what "no
//! rehash, one probe" claims — and not the machine's memory hierarchy.

use std::hint::black_box;
use std::time::Instant;

use brutex_core::instrument::{Exchange, Segment};
use brutex_core::symbol::Symbol;
use brutex_core::vendor::Vendor;
use store::path::{Timeframe, YearMonth};

use pull::manifest::{Commit, Entry, EntryKey, HEADER_LEN, IMAGE_LEN, Manifest};

/// [`HEADER_LEN`] as a length, for building a header region.
const HEADER_LEN_LEN: usize = 32_768;
/// `store::format::SLOT_STRIDE` as a length, for placing the second slot.
const SLOT_STRIDE_LEN: usize = 16_384;

const _: () = assert!(HEADER_LEN == 32_768);

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
/// the cost of the work itself.
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

/// The smallest non-zero interval [`Instant`] reports on this host, in
/// nanoseconds.
///
/// Measured, printed, and asserted against nothing — it is context, not a
/// ratio. It exists because D-0035 explained the C-11 spread as "a field read
/// sits at the clock's resolution floor", and that explanation was never
/// measured. This number is what decides whether it is true: one tick spread
/// over `reps` repetitions is the smallest cost this harness can report, and if
/// an observation is a thousand times that, the clock is not what moved it.
/// `CLAUDE.md` §3 rule 6 — never claim a measurement you did not take. D-0036.
fn clock_tick_ns() -> u128 {
    let mut best = u128::MAX;
    for _ in 0..2_000 {
        let start = Instant::now();
        loop {
            let seen = start.elapsed().as_nanos();
            if seen > 0 {
                if seen < best {
                    best = seen;
                }
                break;
            }
        }
    }
    best
}

/// Prints one measurement and returns whether it stayed under the ceiling.
///
/// A base of zero means the clock could not resolve the operation at all, which
/// is reported as a failure rather than divided by: an unmeasurable baseline
/// makes every ratio meaningless, and reporting a ratio nobody measured is what
/// `CLAUDE.md` §3 rule 6 forbids.
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

/// The harness could not build its own input, and says so rather than
/// proceeding on a guess.
fn refuse(why: &str) -> ! {
    println!("THE HARNESS COULD NOT BUILD ITS OWN INPUT: {why}");
    println!("No ratio below was measured. This is a failure, not a skip.");
    std::process::exit(1)
}

/// Months this harness can name: `1970..=9999`, twelve each.
const MONTHS: u32 = (9_999 - 1_970 + 1) * 12;

/// The key of the *i*th month this harness records.
///
/// Distinct for every `index` below `3 · MONTHS`, which is 289,080 — the
/// calendar alone runs out at 96,360, so the segment cycles above it. Every key
/// is a key the *store* could address, because they are built from the same
/// `store::path` types a bar file's path is; a harness that fabricated a month
/// the store refuses would be measuring a shape that cannot exist.
fn key(index: u32) -> EntryKey {
    key_for("NIFTY", index)
}

/// [`key`], for a chosen symbol.
fn key_for(symbol: &str, index: u32) -> EntryKey {
    let Ok(year) = u16::try_from(1_970 + index / 12 % (MONTHS / 12)) else {
        refuse("a year outside u16")
    };
    let Ok(month) = u8::try_from(index % 12 + 1) else {
        refuse("a month outside u8")
    };
    let segment = match index / MONTHS % 3 {
        0 => Segment::Index,
        1 => Segment::Cash,
        _ => Segment::Fno,
    };
    let (Ok(symbol), Ok(month)) = (Symbol::new(symbol), YearMonth::new(year, month)) else {
        refuse("a symbol or a month this build refuses")
    };
    EntryKey {
        exchange: Exchange::Nse,
        segment,
        symbol,
        timeframe: Timeframe::MINUTE_1,
        month,
    }
}

/// The header region holding one commit in the slot it belongs to.
///
/// Built by extension rather than by writing into an index, because
/// `clippy::indexing_slicing` is denied across this workspace and a bench is a
/// target like any other.
fn header_region(commit: &Commit) -> Vec<u8> {
    let mut region: Vec<u8> = Vec::with_capacity(HEADER_LEN_LEN);
    if commit.slot != 0 {
        region.resize(SLOT_STRIDE_LEN, 0);
    }
    region.extend_from_slice(&commit.bytes);
    region.resize(HEADER_LEN_LEN, 0);
    region
}

/// A manifest holding `count` months, **as a reader sees it**, and the entry
/// bytes a reader would see.
///
/// It is written with `record` and then round-tripped through
/// [`Manifest::load`], and that round trip is the point of this function rather
/// than a flourish. C-12 is a claim about layer 3 — a map reserved from a known
/// bound, so no rehash — and only the loaded index is reserved;
/// `Manifest::genesis` reserves nothing and grows by rehashing. Measuring the
/// writer's map would have reported a number that had nothing to do with the
/// mechanism `docs/06-limits.md` §17 and `docs/07-o1-architecture.md` line 79
/// attribute it to, which is the shape `CLAUDE.md` §3 rule 6 forbids. D-0036.
fn census(count: u32) -> (Manifest, Vec<u8>) {
    let Ok(mut writer) = Manifest::open(Vendor::Groww, &[], &[]) else {
        refuse("a genesis manifest")
    };
    let mut data = Vec::with_capacity(count as usize * IMAGE_LEN);
    let mut published: Option<Commit> = None;
    for index in 0..count {
        let entry = Entry {
            key: key(index),
            rows: 7_312,
            first_ts_micros: 1_000,
            last_ts_micros: 2_000,
        };
        match writer.record(entry) {
            Ok(append) => {
                data.extend_from_slice(&append.bytes);
                published = Some(append.commit);
            }
            Err(e) => refuse(&e.to_string()),
        }
    }
    let Some(commit) = published else {
        refuse("a census of no months")
    };
    match Manifest::load(Vendor::Groww, &header_region(&commit), &data) {
        Ok(manifest) => (manifest, data),
        Err(e) => refuse(&e.to_string()),
    }
}

/// C-11 — the census is a counter read, not a walk of what it counts.
///
/// The scan side re-derives exactly the number the header already holds, by
/// decoding and checksum-verifying every entry — which is what any answer that
/// did not trust a counter would have to do. Both sides run in the same process
/// on the same machine, so the ratio is a property of the code rather than of
/// the hardware.
fn census_beats_the_scan_it_replaces() -> bool {
    /// The counter must beat the scan by at least this, in thousandths.
    ///
    /// A floor rather than a nanosecond ceiling: an absolute threshold would
    /// have to be guessed for hardware this repository has never measured on.
    /// 100× at ten thousand entries is far below what any correct
    /// implementation produces and far above what a counter that had quietly
    /// become a walk could reach.
    const FLOOR_PERMILLE: u128 = 100_000;

    /// Repetitions the counter side is averaged over.
    const REPS: u32 = 100_000;

    let (manifest, data) = census(10_000);
    let counter = cost_ps(REPS, || black_box(&manifest).total_rows());
    let scan = cost_ps(20, || {
        data.chunks_exact(IMAGE_LEN)
            .filter_map(|chunk| Entry::decode(chunk).ok())
            .fold(0u64, |sum, entry| sum.wrapping_add(entry.rows))
    });
    if counter == 0 {
        println!("  C-11 census vs the scan                      UNMEASURABLE");
        return false;
    }

    // How far the counter observation sits above the clock's own floor. The
    // whole point of printing it is that the answer is not "one".
    let tick = clock_tick_ns();
    let floor_fs = tick * 1_000_000 / u128::from(REPS);
    println!(
        "  {:<44} {tick} ns; one tick over {REPS} reps is {floor_fs} fs of reported cost, \
         so the counter observation is {} ticks",
        "clock granularity (context, not a ratio)",
        counter * u128::from(REPS) / (tick * 1_000)
    );
    let permille = scan * 1_000 / counter;
    let ok = permille >= FLOOR_PERMILLE;
    println!(
        "  {:<44} {:>9} ps vs {:>9} ps  speedup {}.{:03}x  {}",
        "C-11 census counter vs 10,000-entry scan",
        counter,
        scan,
        permille / 1_000,
        permille % 1_000,
        if ok { "ok" } else { "BREACH" }
    );
    ok
}

/// C-12 — one entry lookup costs the same at 1×, 10× and 100× the census.
///
/// The map is reserved from the entry count known before the load walk begins,
/// so it never rehashes: `docs/07-o1-architecture.md` layer 3, O(1) **worst
/// case** rather than average. A cost that tracked the census size would mean
/// the probe had started to depend on how much is held.
fn entry_lookup_is_flat() -> bool {
    let (one, _) = census(1_000);
    let (ten, _) = census(10_000);
    let (hundred, _) = census(100_000);

    let present = key(7);
    let absent = key_for("SENSEX", 7);

    let hit = |m: &Manifest| cost_ps(20_000, || black_box(m).entry(black_box(&present)));
    let miss = |m: &Manifest| cost_ps(20_000, || black_box(m).entry(black_box(&absent)));

    let base = hit(&one);
    let a = ratio("C-12 entry lookup, 10x census", base, hit(&ten));
    let b = ratio("C-12 entry lookup, 100x census", base, hit(&hundred));

    // A miss is the same probe as a hit. It is measured separately because a
    // structure that answered "no" by walking would look flat on hits alone.
    let base = miss(&one);
    let c = ratio("C-12 absent lookup, 10x census", base, miss(&ten));
    let d = ratio("C-12 absent lookup, 100x census", base, miss(&hundred));

    a && b && c && d
}

fn main() {
    println!("gate 8 — crates/pull, ceiling {CEILING_PERMILLE} permille");
    let mut ok = true;
    ok &= census_beats_the_scan_it_replaces();
    ok &= entry_lookup_is_flat();
    if ok {
        println!("all ratios within the ceiling");
    } else {
        println!("A RATIO BREACHED ITS CEILING — see the lines marked BREACH.");
        std::process::exit(1);
    }
}
