//! Folding many snapshots into one bar, and the hard-gated ladder that decides
//! what may be pulled next.
//!
//! # Why folding must exist
//!
//! Both archive vendors ship **one-second snapshots**: two to four rows per
//! second, no sub-second field, no tiebreaker. Handed to the store as-is they
//! are refused, and correctly — a format addressed by
//! `base + header + index·stride` cannot hold two records claiming the same
//! instant. A real run against 194 contracts produced 354,675 rows and **zero
//! bars**, every member refused with *"does not follow"*.
//!
//! That refusal is the feature. What was missing is the step that makes a
//! one-second feed storable as one-minute bars at all: **fold every snapshot
//! inside a bucket into one OHLCV bar.**
//!
//! | Field | From the bucket's snapshots |
//! |---|---|
//! | open | the **first** |
//! | high | the **maximum** |
//! | low | the **minimum** |
//! | close | the **last** |
//! | volume | the **sum** |
//! | open interest | the **last** that carried one |
//!
//! First and last are **file order**, never sorted. Rows sharing a second have
//! no recoverable order, so a sort would invent one and quietly change which
//! price became the open.
//!
//! # The ladder
//!
//! The operator's rule, and it holds for every vendor without exception:
//!
//! | | Segment | | Granularity |
//! |---|---|---|---|
//! | 1 | **Spot** | 1 | **Daily** |
//! | 2 | Futures, expired | 2 | One minute |
//! | 3 | Options, expired | | |
//!
//! Nothing advances until the stage before it finished **completely clean** —
//! zero failures. [`Ladder::next`] is the only thing that says what may run,
//! and it refuses to skip.
//!
//! The order is not merely cautious, it is the cheapest place to fail. Spot is
//! two instruments; futures is ~213 underlyings; options is ~11,500 contracts
//! in a single day. Daily is one bar where minute is 375. Each stage is a
//! rehearsal for the next and costs a fraction of it, so a broken credential, a
//! wrong path or a bad session bound surfaces against two instruments rather
//! than against eleven thousand half-written files.

use store::format::Bar;

/// How wide a fold bucket is, in seconds.
///
/// Not a [`store::path::Timeframe`] because folding happens before a path
/// exists, and taking the width as a plain number keeps this module unable to
/// decide where anything is filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bucket(u32);

impl Bucket {
    /// One minute.
    pub const MINUTE: Self = Self(60);
    /// One trading day. Wide enough that a whole session lands in one bucket.
    pub const DAY: Self = Self(86_400);

    /// A bucket of `secs` seconds, or [`None`] for zero.
    ///
    /// Zero is refused rather than clamped: a zero-width bucket would divide by
    /// zero, and a bucket silently widened to one second is a different answer
    /// to the question that was asked.
    #[must_use]
    pub const fn of_secs(secs: u32) -> Option<Self> {
        if secs == 0 { None } else { Some(Self(secs)) }
    }

    /// The width in seconds.
    #[must_use]
    pub const fn secs(self) -> u32 {
        self.0
    }
}

/// Folds bars into buckets, one output bar per bucket that held anything.
///
/// Input must be in **non-decreasing** timestamp order, which is what every
/// vendor file is and what [`crate::archive`] preserves by never sorting rows.
/// An out-of-order row would open a bucket that was already closed; rather than
/// silently producing two bars for one minute, that is refused.
///
/// # Errors
///
/// [`FoldError::OutOfOrder`] naming the position and both timestamps.
///
/// # Examples
///
/// ```
/// # use pull::fold::{fold, Bucket};
/// # use store::format::Bar;
/// let at = |s: i64| s * 1_000_000;
/// let bar = |ts, p| Bar {
///     ts_micros: ts, open: p, high: p, low: p, close: p,
///     volume: 1, open_interest: i64::MIN,
/// };
/// // Three snapshots inside one minute, two sharing a second.
/// let snaps = vec![bar(at(0), 100), bar(at(0), 130), bar(at(30), 90)];
/// let bars = fold(&snaps, Bucket::MINUTE)?;
///
/// assert_eq!(bars.len(), 1, "one minute in, one bar out");
/// assert_eq!(bars[0].open, 100, "the FIRST in file order");
/// assert_eq!(bars[0].high, 130);
/// assert_eq!(bars[0].low, 90);
/// assert_eq!(bars[0].close, 90, "the LAST in file order");
/// assert_eq!(bars[0].volume, 3, "summed");
/// # Ok::<(), pull::fold::FoldError>(())
/// ```
pub fn fold(snapshots: &[Bar], bucket: Bucket) -> Result<Vec<Bar>, FoldError> {
    let width = i64::from(bucket.secs()) * 1_000_000;
    let mut out: Vec<Bar> = Vec::new();
    let mut open_at: Option<i64> = None;
    let mut previous: Option<i64> = None;

    for (i, snap) in snapshots.iter().enumerate() {
        if let Some(prev) = previous
            && snap.ts_micros < prev
        {
            return Err(FoldError::OutOfOrder {
                at: i,
                previous: prev,
                found: snap.ts_micros,
            });
        }
        previous = Some(snap.ts_micros);

        // The bucket a snapshot belongs to is arithmetic, not a search:
        // `div_euclid` rather than `/` so a pre-1970 instant floors downward
        // instead of toward zero, which would put it in the bucket after its own.
        let start = snap.ts_micros.div_euclid(width) * width;

        if open_at == Some(start) {
            let Some(bar) = out.last_mut() else {
                // Unreachable: `open_at` is only ever Some after a push.
                return Err(FoldError::OutOfOrder {
                    at: i,
                    previous: start,
                    found: snap.ts_micros,
                });
            };
            bar.high = bar.high.max(snap.high);
            bar.low = bar.low.min(snap.low);
            // CLOSE IS THE LAST IN FILE ORDER, not the highest timestamp. Rows
            // sharing a second have no tiebreaker, so file order is the only
            // order there is and sorting would invent one.
            bar.close = snap.close;
            bar.volume = bar.volume.saturating_add(snap.volume);
            if snap.open_interest != i64::MIN {
                bar.open_interest = snap.open_interest;
            }
        } else {
            open_at = Some(start);
            out.push(Bar {
                // The bar is stamped at the START of its bucket, which is what
                // `docs/00-charter.md` §3 means by a bar covering its interval.
                ts_micros: start,
                open: snap.open,
                high: snap.high,
                low: snap.low,
                close: snap.close,
                volume: snap.volume,
                open_interest: snap.open_interest,
            });
        }
    }

    Ok(out)
}

/// Why snapshots could not be folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FoldError {
    /// A width narrower than the bars it is being built from.
    ///
    /// **This is an information limit, not an arithmetic one, and it was found
    /// by measurement rather than by reading.** A folded one-minute bar is
    /// stamped at the *start* of its minute, so the whole minute travels with
    /// that single timestamp and *when inside the minute* things happened is
    /// gone. Fold again at a width whose edges do not land on minute
    /// boundaries and the entire minute is attributed to whichever bucket the
    /// stamp falls in — dragging in data from after that bucket closed.
    ///
    /// Measured on the operator's archive, all 12,132 contracts:
    ///
    /// | Width | Bars compared | Wrong | Worst error |
    /// |---|---|---|---|
    /// | 90 s | 2,513,114 | **513,605 (20.4%)** | ₹1,147.90 |
    /// | 100 s | 2,275,830 | **510,889 (22.4%)** | ₹1,147.90 |
    ///
    /// One concrete case: a 100-second bucket `[12:55:00, 12:56:40)` built from
    /// minute bars swallowed a trade stamped **12:56:58** — eighteen seconds of
    /// the future inside a closed bar, which is `CLAUDE.md` §3 rule 7 broken by
    /// arithmetic rather than by an accessor.
    ///
    /// **Nothing is forbidden by this.** Any width at all is exact when folded
    /// from the raw snapshots, which is what [`fold_from_snapshots`] is for.
    /// This refusal names the one combination that loses information.
    NarrowerThanSource {
        /// The width asked for, in seconds.
        want_secs: u32,
        /// The width the input bars already carry, in seconds.
        source_secs: u32,
    },
    /// A snapshot's timestamp precedes the one before it.
    ///
    /// Refused rather than sorted. Rows sharing a second carry no tiebreaker,
    /// so a sort would invent an order and quietly change which price became
    /// the open — and nothing downstream could tell.
    OutOfOrder {
        /// Zero-based position in the input.
        at: usize,
        /// The timestamp before it.
        previous: i64,
        /// What was found.
        found: i64,
    },
}

impl core::fmt::Display for FoldError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // A `match`, not a `let ... else`. The single-variant destructure that
        // stood here became `unreachable!()` the moment a second variant
        // existed, and the guard's own test is what fired it — a refusal that
        // panicked instead of printing.
        match *self {
            Self::NarrowerThanSource {
                want_secs,
                source_secs,
            } => write!(
                f,
                "a {want_secs}s bucket cannot be built from {source_secs}s \
                 bars: {want_secs} is not a whole multiple of {source_secs}, so \
                 a bucket edge falls INSIDE a source bar and that bar cannot be \
                 split — the information to split it was discarded when it was \
                 made. Measured cost of allowing it: 20.4% of bars wrong at \
                 90s, worst error Rs 1,147.90. Fold from the raw snapshots \
                 instead, where any width is exact."
            ),
            Self::OutOfOrder {
                at,
                previous,
                found,
            } => write!(
                f,
                "snapshot {at} is stamped {found}, before {previous}. Refused \
                 rather than sorted: rows sharing a second have no tiebreaker, \
                 so a sort invents an order and changes which price became the \
                 open."
            ),
        }
    }
}

impl core::error::Error for FoldError {}

// ───────────────────────────── the ladder ─────────────────────────────

/// Which segment a stage covers, in the order they must be pulled.
///
/// The discriminant **is** the order, so a new segment is inserted at its true
/// position and cannot accidentally sort elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Segment {
    /// Indices and cash. Two swept instruments — the cheapest rehearsal.
    Spot = 0,
    /// Expired futures. ~213 underlyings.
    Futures = 1,
    /// Expired option chains. ~11,500 contracts in one day.
    Options = 2,
}

/// Which granularity a stage covers, in the order they must be pulled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Grain {
    /// One bar per day. Proves the whole chain at 1/375th the volume.
    Daily = 0,
    /// One bar per minute.
    Minute = 1,
}

/// One rung: a segment at a granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stage {
    /// Which segment.
    pub segment: Segment,
    /// Which granularity.
    pub grain: Grain,
}

/// Every rung, in the only order they may be attempted.
///
/// Granularity is the **outer** loop: all three segments at daily, then all
/// three at minute. Daily is 1/375th the volume of minute, so finishing the
/// cheap pass across every segment before starting the expensive one surfaces a
/// structural fault — a wrong path, a bad session bound, a dead credential —
/// against the smallest possible amount of work.
pub const LADDER: [Stage; 6] = [
    Stage {
        segment: Segment::Spot,
        grain: Grain::Daily,
    },
    Stage {
        segment: Segment::Futures,
        grain: Grain::Daily,
    },
    Stage {
        segment: Segment::Options,
        grain: Grain::Daily,
    },
    Stage {
        segment: Segment::Spot,
        grain: Grain::Minute,
    },
    Stage {
        segment: Segment::Futures,
        grain: Grain::Minute,
    },
    Stage {
        segment: Segment::Options,
        grain: Grain::Minute,
    },
];

/// How far a vendor has got, and what it may attempt next.
///
/// One ladder per vendor. Every vendor climbs the same rungs in the same order
/// without exception — a vendor allowed to skip is a vendor whose failures
/// surface at the most expensive rung instead of the cheapest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ladder {
    done: u8,
}

impl Ladder {
    /// A vendor that has pulled nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { done: 0 }
    }

    /// The stage that may run now, or [`None`] when every rung is complete.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::fold::{Ladder, Segment, Grain};
    /// let mut l = Ladder::new();
    /// assert_eq!(l.next().map(|s| s.segment), Some(Segment::Spot));
    /// assert_eq!(l.next().map(|s| s.grain), Some(Grain::Daily));
    ///
    /// // A stage that failed does NOT advance the ladder.
    /// l.record(false);
    /// assert_eq!(l.next().map(|s| s.segment), Some(Segment::Spot), "still spot");
    ///
    /// l.record(true);
    /// assert_eq!(l.next().map(|s| s.segment), Some(Segment::Futures));
    /// ```
    #[must_use]
    pub fn next(self) -> Option<Stage> {
        LADDER.get(self.done as usize).copied()
    }

    /// Records the outcome of the stage [`Ladder::next`] returned.
    ///
    /// **Only a completely clean stage advances.** `clean` must mean zero
    /// failures — not "mostly worked". A partial success that advanced would
    /// carry its gap into a stage 375 times larger, where finding it costs 375
    /// times as much.
    pub const fn record(&mut self, clean: bool) {
        if clean && (self.done as usize) < LADDER.len() {
            self.done += 1;
        }
    }

    /// How many rungs are complete.
    #[must_use]
    pub const fn completed(self) -> usize {
        self.done as usize
    }

    /// Whether every rung is done.
    #[must_use]
    pub const fn finished(self) -> bool {
        self.done as usize >= LADDER.len()
    }
}

const _: () = assert!(LADDER.len() == 6);

/// Folds raw snapshots at **any** width. Always exact.
///
/// A snapshot carries its own instant, so no width can misattribute it. This
/// is the entry point a sub-minute timeframe must use, and the reason nothing
/// about the design is locked down: 1 second, 7 seconds, 90 seconds and one
/// day all go through here and all are correct.
///
/// # Errors
///
/// [`FoldError::OutOfOrder`] if the snapshots are not in non-decreasing
/// timestamp order.
pub fn fold_from_snapshots(snapshots: &[Bar], bucket: Bucket) -> Result<Vec<Bar>, FoldError> {
    fold(snapshots, bucket)
}

/// Folds bars that are already `source` wide into `bucket`-wide bars.
///
/// # Errors
///
/// [`FoldError::NarrowerThanSource`] when `bucket` is not a whole multiple of
/// `source`. That is the combination which loses information: see the variant's
/// own documentation for the measured cost of allowing it.
///
/// [`FoldError::OutOfOrder`] as for [`fold`].
///
/// # Examples
///
/// ```
/// # use pull::fold::{fold_from_bars, Bucket, FoldError};
/// let one_min = Bucket::MINUTE;
///
/// // Every whole-minute width is exact, so all of these are legal.
/// for secs in [60u32, 120, 180, 300, 900, 3_600, 86_400] {
///     let wider = Bucket::of_secs(secs).expect("non-zero");
///     assert!(fold_from_bars(&[], wider, one_min).is_ok(), "{secs}s");
/// }
///
/// // 90 seconds is one and a half minutes. Refused BY NAME, not silently
/// // mis-attributed — this is the case measured at 20.4% wrong bars.
/// let ninety = Bucket::of_secs(90).expect("non-zero");
/// assert_eq!(
///     fold_from_bars(&[], ninety, one_min),
///     Err(FoldError::NarrowerThanSource { want_secs: 90, source_secs: 60 }),
/// );
/// ```
pub fn fold_from_bars(bars: &[Bar], bucket: Bucket, source: Bucket) -> Result<Vec<Bar>, FoldError> {
    // WIDER IS ALWAYS SAFE, NARROWER NEVER IS, AND "NOT A WHOLE MULTIPLE" IS
    // NARROWER IN THE ONLY sense that matters: some bucket edge falls inside a
    // source bar, and that bar cannot be split because the information to split
    // it was discarded when it was made.
    if !bucket.secs().is_multiple_of(source.secs()) {
        return Err(FoldError::NarrowerThanSource {
            want_secs: bucket.secs(),
            source_secs: source.secs(),
        });
    }
    fold(bars, bucket)
}

#[cfg(test)]
mod guard {
    use super::{Bucket, FoldError, fold_from_bars, fold_from_snapshots};

    /// Every timeframe the operator named is a whole number of minutes, and
    /// every one of them is exact from stored one-minute bars.
    #[test]
    fn every_whole_minute_width_is_allowed_from_minute_bars() {
        for (label, secs) in [
            ("1 minute", 60u32),
            ("2 minute", 120),
            ("3 minute", 180),
            ("5 minute", 300),
            ("15 minute", 900),
            ("45 minute", 2_700),
            ("1 hour", 3_600),
            ("4 hour", 14_400),
            ("1 day", 86_400),
        ] {
            let want = Bucket::of_secs(secs).expect("non-zero");
            assert!(
                fold_from_bars(&[], want, Bucket::MINUTE).is_ok(),
                "{label} is {secs}s, a whole multiple of 60 — it must be allowed"
            );
        }
    }

    /// A width that is not a whole multiple is REFUSED, not mis-attributed.
    ///
    /// Measured on the operator's archive across all 12,132 contracts: a 90s
    /// bucket built from minute bars got **20.4% of bars wrong**, worst error
    /// Rs 1,147.90, and one case pulled a trade stamped 12:56:58 into a bucket
    /// that ended 12:56:40 — eighteen seconds of the future inside a closed
    /// bar. That is why this refuses rather than approximates.
    #[test]
    fn a_sub_minute_width_from_minute_bars_is_refused_by_name() {
        for secs in [1u32, 7, 30, 90, 100, 3_607] {
            let want = Bucket::of_secs(secs).expect("non-zero");
            assert_eq!(
                fold_from_bars(&[], want, Bucket::MINUTE),
                Err(FoldError::NarrowerThanSource {
                    want_secs: secs,
                    source_secs: 60,
                }),
                "{secs}s is not a whole multiple of 60"
            );
        }

        let text = fold_from_bars(&[], Bucket::of_secs(90).expect("nz"), Bucket::MINUTE)
            .expect_err("90 is not a multiple of 60")
            .to_string();
        assert!(
            text.contains("snapshots"),
            "the refusal must say where the width IS available — nothing is \
             forbidden, only this one lossy path. Got {text:?}"
        );
    }

    /// From snapshots, EVERY width is exact. Nothing is locked down.
    #[test]
    fn any_width_at_all_is_allowed_from_snapshots() {
        for secs in [1u32, 7, 30, 90, 100, 60, 300, 3_607, 86_400] {
            let want = Bucket::of_secs(secs).expect("non-zero");
            assert!(
                fold_from_snapshots(&[], want).is_ok(),
                "{secs}s from raw snapshots is exact — a snapshot carries its \
                 own instant, so no width can misattribute it"
            );
        }
    }

    /// Folding minute bars into wider ones, from a five-minute source too.
    #[test]
    fn the_guard_is_about_the_source_not_about_minutes() {
        let five = Bucket::of_secs(300).expect("nz");
        // 15m and 1h are whole multiples of 5m — allowed.
        assert!(fold_from_bars(&[], Bucket::of_secs(900).expect("nz"), five).is_ok());
        assert!(fold_from_bars(&[], Bucket::of_secs(3_600).expect("nz"), five).is_ok());
        // 1m is NARROWER than 5m — the information is gone.
        assert!(fold_from_bars(&[], Bucket::MINUTE, five).is_err());
        // 3m is not a multiple of 5m even though both are whole minutes.
        assert!(
            fold_from_bars(&[], Bucket::of_secs(180).expect("nz"), five).is_err(),
            "3 minutes is not a whole multiple of 5 minutes — a bucket edge \
             would fall inside a source bar"
        );
    }
}
