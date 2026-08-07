//! What to pull: chosen by hand, or computed from what is missing.
//!
//! # One code path, two ways in
//!
//! The page previously offered **three fixed buttons** — 2 swept indices, 35
//! reference indices, 750 equities — and nothing else. An operator could not
//! pull one instrument, or three, or the twelve that failed last night.
//!
//! [`Selection`] replaces that with a set of any size. **Manual** is a set the
//! operator names; **automatic** is a set [`gaps`] computes from what the store
//! is missing. Both produce the same [`Selection`], so everything downstream —
//! the ladder, the fold, the store write — cannot tell them apart and there is
//! no second path to get wrong.
//!
//! # Why the automatic side is the one that matters
//!
//! Gap = expected − held. The operator never chooses a count: re-running fetches
//! **nothing** when nothing is missing, fetches **only the new instrument** when
//! one is added, and **resumes exactly where it stopped** after an interruption
//! — because the memory is the store's own census, not a progress variable that
//! dies with the process.
//!
//! That is what makes a pull something a timer can run with nobody watching.
//!
//! # Cost
//!
//! [`gaps`] is one pass over the requested cells with one hash probe each
//! against the held set — `docs/07-o1-architecture.md` layer 3, a probe rather
//! than a walk. It is **O(cells requested)**, never O(store), and it never
//! lists a directory: a bulk question answered by walking ~248,000 files is the
//! exact cost layer 13 exists to remove.

use std::collections::HashSet;

use store::path::{Timeframe, YearMonth};

/// One instrument-month-timeframe cell: the unit a pull is measured in.
///
/// The same triple the store addresses by and the manifest counts, so a gap
/// here is directly a file there. A fourth spelling of "which slice of data"
/// would be a fourth thing to keep in step.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The instrument, as the store names it.
    pub instrument: String,
    /// Which month.
    pub month: YearMonth,
    /// Which granularity.
    pub timeframe: Timeframe,
}

/// Which instruments a run covers. **Any size, 1 to all of them.**
///
/// A set rather than an enum of three universes: an enum could not express
/// "these twelve", which is what an operator wants after twelve failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    names: Vec<String>,
}

impl Selection {
    /// A selection of exactly these instruments, in the order given.
    ///
    /// Duplicates are removed, because a duplicate would pull the same window
    /// twice and the second write would be refused as not following the first —
    /// a failure caused entirely by the caller's list.
    #[must_use]
    pub fn of<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut seen = HashSet::new();
        let names = names
            .into_iter()
            .map(Into::into)
            .filter(|n| !n.is_empty() && seen.insert(n.clone()))
            .collect();
        Self { names }
    }

    /// How many instruments. Zero is legal and means there is nothing to do.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the selection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The instruments, in order.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Every cell this selection covers across a span of months and one
    /// timeframe.
    ///
    /// The order is instrument-major: all of one instrument's months before the
    /// next instrument's. A month-major order would open and close every bar
    /// file once per month instead of once, which is the same bars through
    /// hundreds of times more file handles.
    #[must_use]
    pub fn cells(&self, months: &[YearMonth], timeframe: Timeframe) -> Vec<Cell> {
        let mut out = Vec::with_capacity(self.names.len().saturating_mul(months.len()));
        for name in &self.names {
            for month in months {
                out.push(Cell {
                    instrument: name.clone(),
                    month: *month,
                    timeframe,
                });
            }
        }
        out
    }
}

/// What is missing, and what is already held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// Cells that must be pulled, in the order they should be attempted.
    pub missing: Vec<Cell>,
    /// How many of the requested cells the store already holds.
    ///
    /// Reported rather than discarded: "held 1,248, missing 12" is the sentence
    /// that tells an operator a re-run is safe. "12 to do" alone does not say
    /// whether the other 1,248 were skipped or never existed.
    pub held: usize,
}

impl Work {
    /// Whether there is nothing to do.
    ///
    /// A re-run over a complete store lands here, which is the whole point of
    /// incremental: same inputs, no work, no vendor contacted.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// Requested cells, held plus missing.
    #[must_use]
    pub fn requested(&self) -> usize {
        self.held.saturating_add(self.missing.len())
    }
}

/// The cells a run must fetch: everything requested that the store lacks.
///
/// `held` is the store's own census — what the manifest already counts. This
/// takes it as a set rather than reaching into the manifest, so the arithmetic
/// is testable without a store on disk and so a second caller with a different
/// notion of "held" cannot appear.
///
/// # Cost
///
/// One pass with one hash probe per requested cell. **O(requested)**, never
/// O(store), and no directory is listed.
///
/// # Examples
///
/// ```
/// # use std::collections::HashSet;
/// # use pull::work::{gaps, Cell, Selection};
/// # use store::path::{Timeframe, YearMonth};
/// let july = YearMonth::new(2025, 7)?;
/// let pick = Selection::of(["NSE-NIFTY", "NSE-BANKNIFTY"]);
/// let want = pick.cells(&[july], Timeframe::MINUTE_1);
///
/// // Nothing held yet: everything is missing.
/// let empty = HashSet::new();
/// assert_eq!(gaps(&want, &empty).missing.len(), 2);
///
/// // One held: exactly one left, and the other is reported as held.
/// let mut held = HashSet::new();
/// held.insert(want[0].clone());
/// let work = gaps(&want, &held);
/// assert_eq!(work.missing.len(), 1);
/// assert_eq!(work.held, 1);
///
/// // Everything held: a re-run does nothing at all.
/// let all: HashSet<Cell> = want.iter().cloned().collect();
/// assert!(gaps(&want, &all).is_complete());
/// # Ok::<(), store::path::PathError>(())
/// ```
#[must_use]
pub fn gaps<S: core::hash::BuildHasher>(requested: &[Cell], held: &HashSet<Cell, S>) -> Work {
    let mut work = Work {
        // Reserved from a bound known before the loop — law 2, so the vector
        // never grows mid-pass.
        missing: Vec::with_capacity(requested.len()),
        held: 0,
    };
    for cell in requested {
        if held.contains(cell) {
            work.held += 1;
        } else {
            work.missing.push(cell.clone());
        }
    }
    work
}
