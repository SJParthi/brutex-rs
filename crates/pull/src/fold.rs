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
        let Self::OutOfOrder {
            at,
            previous,
            found,
        } = *self;
        write!(
            f,
            "snapshot {at} is stamped {found}, before {previous}. Refused \
             rather than sorted: rows sharing a second have no tiebreaker, so a \
             sort invents an order and changes which price became the open."
        )
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
