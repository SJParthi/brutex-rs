//! The trading session as a **dated regime**, because it stopped being a
//! constant on 2026-08-03.
//!
//! # What happened, and why one number can no longer serve
//!
//! NSE introduced a Closing Auction Session effective **2026-08-03**, and with
//! it there are now **three different closing times on the same afternoon**:
//!
//! | Segment | Continuous close | One-minute bars |
//! |---|---|---|
//! | Spot index — `NSE-NIFTY`, `NSE-BANKNIFTY` | **15:15** | **360** |
//! | Cash shares with no derivatives | 15:30 | 375 |
//! | Equity derivatives | **15:40** | **385** |
//!
//! [`session::SESSION_CLOSE_MINUTE`](crate::session::SESSION_CLOSE_MINUTE) is
//! `930`, which is now correct for exactly one of those three, and only for
//! dates up to 2026-07-31.
//!
//! # Why the swept indices close *earliest*, which is the counter-intuitive bit
//!
//! It is indirect. CAS eligibility is *"stocks on which derivative contracts
//! are available in any of the Exchanges"* (NSE/CMTR/74466 §A). Every NIFTY 50
//! and every NIFTY BANK constituent meets that test, so at 15:15 every share
//! the index is computed from stops trading continuously — and an index
//! computed from frozen inputs is itself frozen. NSE's own FAQ (v1.0, May 2026,
//! Q7) states the *actual* index during CAS is computed from the **"LTP of
//! CTS"**, the last price from before 15:15.
//!
//! So the instrument this engine sweeps has the *shortest* session of the
//! three, not the longest, and a filter written for the derivatives close would
//! admit twenty-five minutes of bars the index never printed.
//!
//! # A refusal row is not a gap in the table — it *is* the table
//!
//! No cash-segment circular establishing the pre-2010 open was retrieved. The
//! widely repeated 09:55 comes from an **F&O** circular and says nothing about
//! the cash segment or the index. So the earliest row carries no times at all
//! and [`session_at`] **refuses** any date that lands on it.
//!
//! This is the same shape [`crate::rate`] uses for an unverified vendor cap and
//! the same shape `crates/costs` uses for an unverified statutory rate: the
//! charge-an-unverified-value state is *unrepresentable*, because the field
//! that would hold the value is [`Option::None`] and there is no default
//! anywhere to fall back to. `CLAUDE.md` §4 — refuse, or degrade loudly and
//! name the reason; never both silently.
//!
//! # Cost
//!
//! [`session_at`] walks a `const` array whose length is fixed at compile time —
//! four rows for the swept index, three for derivatives. The row count is not
//! an input, so the walk is **O(1) by construction**, and it stays O(1) when
//! the table grows to forty rows because forty is also not an input. This is
//! deliberately not a binary search: `docs/07-o1-architecture.md` layer 4 bans
//! `binary_search` behind an O(1) label, and over four rows a linear walk of a
//! `const` array is fewer instructions than the branch it would replace.
//! `pull::unit::the_regime_walk_is_flat_across_every_row` asserts the flatness.

use core::fmt;

use crate::session::Day;

/// Which market clock applies.
///
/// Not "which instrument" — three different instruments can share a clock, and
/// the same underlying appears under two of these on the same afternoon. The
/// segment *is* the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Segment {
    /// A spot index: `NSE-NIFTY`, `NSE-BANKNIFTY`, and the reference indices.
    ///
    /// The only segment this engine sweeps, and — since 2026-08-03 — the one
    /// that closes earliest. See the module header for why.
    SpotIndex,
    /// A cash equity with no derivative contract, and therefore not CAS
    /// eligible.
    CashNonDerivative,
    /// A futures or options contract.
    EquityDerivative,
}

impl Segment {
    /// Every segment, in the order the tables are written.
    pub const ALL: [Self; 3] = [
        Self::SpotIndex,
        Self::CashNonDerivative,
        Self::EquityDerivative,
    ];

    /// What an operator calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SpotIndex => "spot index",
            Self::CashNonDerivative => "cash equity, no derivative",
            Self::EquityDerivative => "equity derivative",
        }
    }
}

/// One dated row: the session in force from `from` until the next row starts.
///
/// `open` and `close` are minutes since IST midnight, and `close` is
/// **exclusive** — a bar is stamped at the start of its minute, so the last
/// one-minute bar of a 15:15 close opens at 15:14. That single convention is
/// the whole arithmetic of this module: the bar count is `close - open`, with
/// no adjustment anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The first day this row governs, inclusive.
    pub from: Day,
    /// Minutes since IST midnight when continuous trading opens, inclusive.
    ///
    /// [`None`] marks a **refusal row**: no source was retrieved for this era,
    /// so no value exists to be used. There is no default.
    pub open: Option<u32>,
    /// Minutes since IST midnight when continuous trading closes, exclusive.
    ///
    /// [`None`] for the same reason as [`Row::open`], and always [`None`]
    /// together with it — a half-known row would be worse than an unknown one.
    pub close: Option<u32>,
    /// The citation, or the honest citation gap. Never blank.
    ///
    /// A verified row names its circular; a refusal row names what was looked
    /// for and why it was not found. `CLAUDE.md` §3 rule 1 — every external
    /// fact traceable to a recorded source.
    pub source: &'static str,
}

impl Row {
    /// One-minute bars in this session, or [`None`] on a refusal row.
    ///
    /// Derived rather than stored, so it cannot disagree with the two bounds it
    /// is computed from. A stored count is a third number that will one day
    /// contradict the other two.
    #[must_use]
    pub const fn bars(self) -> Option<u32> {
        match (self.open, self.close) {
            (Some(o), Some(c)) if c > o => Some(c - o),
            _ => None,
        }
    }

    /// Whether this row carries times. Derived, so it can never drift.
    #[must_use]
    pub const fn verified(self) -> bool {
        self.open.is_some() && self.close.is_some()
    }
}

/// Why a session could not be named for a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegimeError {
    /// The date lands on a row for which no source was ever retrieved.
    ///
    /// Refused rather than served from the nearest known row, because the
    /// nearest known row is a *different era's* clock and a bar filtered by it
    /// would be silently mis-windowed — the bars would simply not be there, and
    /// nothing downstream could tell that from a quiet market.
    Unverified {
        /// The segment asked about.
        segment: Segment,
        /// The day asked about.
        day: Day,
        /// The first day for which a verified row does exist, if any.
        verified_from: Option<Day>,
        /// The refusal row's own account of what is missing.
        source: &'static str,
    },
}

impl fmt::Display for RegimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Unverified {
                segment,
                day,
                verified_from,
                source,
            } => {
                write!(
                    f,
                    "no verified session for the {} on {day}",
                    segment.label()
                )?;
                if let Some(start) = verified_from {
                    write!(f, "; the record begins {start}")?;
                }
                write!(f, " — {source}")
            }
        }
    }
}

impl core::error::Error for RegimeError {}

/// Builds a [`Day`] in a `const` context, or fails the build.
///
/// A table row with an impossible date must not compile. `Day::new` returns a
/// `Result` and `?` is not available in a `const` context, so the panic arm is
/// the compile-time refusal — it can only fire while the compiler is evaluating
/// the constant, never at run time.
#[expect(
    clippy::panic,
    reason = "this runs ONLY while the compiler evaluates the const tables — \
              `?` is not available in a const context, so the panic arm is how \
              a bad row is refused. It cannot fire at run time: if it were \
              reachable the build would already have failed. A row naming \
              2026-02-30 stops the build rather than shipping."
)]
const fn d(y: u16, m: u8, day: u8) -> Day {
    match Day::new(y, m, day) {
        Ok(v) => v,
        Err(_) => panic!("a regime row names a date that does not exist"),
    }
}

/// 09:15 IST, in minutes since midnight.
const OPEN_0915: u32 = 9 * 60 + 15;

/// The swept spot indices — `NSE-NIFTY` and `NSE-BANKNIFTY`.
///
/// Rows ascend strictly by `from`; [`session_at`] returns the last row whose
/// `from` is not after the day asked about.
pub const SPOT_INDEX: [Row; 4] = [
    Row {
        from: d(1970, 1, 1),
        open: None,
        close: None,
        source: "UNVERIFIED — no cash-segment circular establishing the \
                 pre-2010 open was retrieved. The widely repeated 09:55 comes \
                 from an F&O circular and says nothing about the cash segment \
                 or the index. Refusal row per CLAUDE.md §3 rule 1.",
    },
    Row {
        from: d(2010, 1, 4),
        open: Some(9 * 60),
        close: Some(15 * 60 + 30),
        source: "NSE/CMTR/13705 (16-Dec-2009) as postponed by NSE/CMTR/13708 \
                 (17-Dec-2009). The 2009-12-18 date in the first circular \
                 never took effect.",
    },
    Row {
        from: d(2010, 10, 18),
        open: Some(OPEN_0915),
        close: Some(15 * 60 + 30),
        source: "NSE CM Circular 117 / Download 15981, 12-Oct-2010, \
                 'Introduction of Call auction in Pre-open session'. This is \
                 the regime this repository encoded as if it were timeless.",
    },
    Row {
        from: d(2026, 8, 3),
        open: Some(OPEN_0915),
        close: Some(15 * 60 + 15),
        source: "NSE/CMTR/74466 §E — 'Unexecuted limit orders of the CTS \
                 (Continuous Trading Session till 03:15PM)'. Every NIFTY 50 \
                 and NIFTY BANK constituent is CAS eligible (§A), so the index \
                 freezes with them; FAQ v1.0 Q7 confirms the actual index then \
                 uses the LTP of CTS.",
    },
];

/// Cash equities with no derivative contract, and therefore not CAS eligible.
pub const CASH_NON_DERIVATIVE: [Row; 3] = [
    Row {
        from: d(1970, 1, 1),
        open: None,
        close: None,
        source: "UNVERIFIED — same gap as the spot index refusal row.",
    },
    Row {
        from: d(2010, 1, 4),
        open: Some(9 * 60),
        close: Some(15 * 60 + 30),
        source: "NSE/CMTR/13705 as postponed by NSE/CMTR/13708.",
    },
    Row {
        from: d(2010, 10, 18),
        open: Some(OPEN_0915),
        close: Some(15 * 60 + 30),
        source: "NSE CM Circular 117 / Download 15981. Unchanged by the 2026 \
                 CAS introduction: a share with no derivative is not CAS \
                 eligible and keeps its 15:30 continuous close.",
    },
];

/// Futures and options. Stored, never swept, and filtered by this clock.
pub const EQUITY_DERIVATIVE: [Row; 3] = [
    Row {
        from: d(1970, 1, 1),
        open: None,
        close: None,
        source: "UNVERIFIED — the 09:55 open appears in NSE/F&O/049/2009 but \
                 the date from which it applied was not established.",
    },
    Row {
        from: d(2010, 10, 18),
        open: Some(OPEN_0915),
        close: Some(15 * 60 + 30),
        source: "Aligned with the cash segment from the pre-open introduction.",
    },
    Row {
        from: d(2026, 8, 3),
        open: Some(OPEN_0915),
        close: Some(15 * 60 + 40),
        source: "NSE/FAOP/74467 — equity derivatives extended ten minutes to \
                 15:40, so positions can be adjusted against the closing \
                 auction prices discovered in the cash segment.",
    },
];

/// The table for a segment.
#[must_use]
pub const fn table(segment: Segment) -> &'static [Row] {
    match segment {
        Segment::SpotIndex => &SPOT_INDEX,
        Segment::CashNonDerivative => &CASH_NON_DERIVATIVE,
        Segment::EquityDerivative => &EQUITY_DERIVATIVE,
    }
}

/// The session in force for a segment on a day.
///
/// # Errors
///
/// [`RegimeError::Unverified`] when the day lands on a refusal row. There is no
/// fallback: a session served from a neighbouring era's clock would mis-window
/// every bar of that day, and the result is indistinguishable from a quiet
/// market.
///
/// # Cost
///
/// A walk of a `const` array whose length is fixed at compile time. The row
/// count is not an input, so this is O(1) by construction and stays O(1) as the
/// table grows.
///
/// # Examples
///
/// ```
/// # use pull::regime::{session_at, Segment};
/// # use pull::session::Day;
/// // What this engine sweeps, before and after the auction landed.
/// let before = session_at(Segment::SpotIndex, Day::new(2026, 7, 31)?)?;
/// assert_eq!(before.bars(), Some(375), "09:15 to 15:30");
///
/// let after = session_at(Segment::SpotIndex, Day::new(2026, 8, 3)?)?;
/// assert_eq!(after.bars(), Some(360), "09:15 to 15:15 — the index freezes");
///
/// // The derivatives clock on the same afternoon is twenty-five minutes longer.
/// let fno = session_at(Segment::EquityDerivative, Day::new(2026, 8, 3)?)?;
/// assert_eq!(fno.bars(), Some(385), "09:15 to 15:40");
/// # Ok::<(), Box<dyn core::error::Error>>(())
/// ```
pub fn session_at(segment: Segment, day: Day) -> Result<Row, RegimeError> {
    // No indexing. `rows[0]` would be correct — every table opens at the epoch
    // and `Day` cannot precede it — but "correct because of a fact stated three
    // hundred lines away" is how an index becomes a panic during a later edit.
    // The fold carries the same information and cannot.
    let mut chosen: Option<Row> = None;
    let mut verified_from: Option<Day> = None;
    for row in table(segment) {
        if row.from <= day && chosen.is_none_or(|c| row.from >= c.from) {
            chosen = Some(*row);
        }
        if verified_from.is_none() && row.verified() {
            verified_from = Some(row.from);
        }
    }
    match chosen {
        Some(row) if row.verified() => Ok(row),
        // Either the day landed on a refusal row, or — unreachable while every
        // table opens at the epoch — on no row at all. Both are the same answer
        // to the caller: this build cannot name a session for that date, and it
        // will not invent one.
        Some(row) => Err(RegimeError::Unverified {
            segment,
            day,
            verified_from,
            source: row.source,
        }),
        None => Err(RegimeError::Unverified {
            segment,
            day,
            verified_from,
            source: "no regime row covers this date — the table does not reach \
                     back this far",
        }),
    }
}

// Rows must ascend strictly, or `session_at` picks the wrong one. Checked at
// compile time so a mis-ordered row cannot ship.
//
// Indexing rather than iteration because this is a `const fn`: slice iterators
// are not const-stable on this toolchain. The `while i < rows.len()` bound is
// what makes both accesses in range, and this runs only in the compiler.
#[expect(
    clippy::indexing_slicing,
    reason = "const fn — no const-stable slice iterator exists on this \
              toolchain. `i` starts at 1 and the loop condition bounds it \
              below `len`, so `rows[i]` and `rows[i - 1]` are both in range. \
              Evaluated by the compiler; an out-of-range access would be a \
              build failure, never a runtime panic."
)]
const fn ascends(rows: &[Row]) -> bool {
    let mut i = 1;
    while i < rows.len() {
        if rows[i].from.days_from_epoch() <= rows[i - 1].from.days_from_epoch() {
            return false;
        }
        i += 1;
    }
    true
}

const _: () = assert!(ascends(&SPOT_INDEX));
const _: () = assert!(ascends(&CASH_NON_DERIVATIVE));
const _: () = assert!(ascends(&EQUITY_DERIVATIVE));

// The three closes that now coexist on one afternoon. Written as an assertion
// rather than a comment so that changing one without the others fails the build.
const _: () = assert!(matches!(SPOT_INDEX[3].close, Some(915)));
const _: () = assert!(matches!(CASH_NON_DERIVATIVE[2].close, Some(930)));
const _: () = assert!(matches!(EQUITY_DERIVATIVE[2].close, Some(940)));

// The regime `crate::session`'s constants encode, so the two cannot drift.
const _: () = assert!(matches!(
    SPOT_INDEX[2].open,
    Some(crate::session::SESSION_OPEN_MINUTE)
));
const _: () = assert!(matches!(
    SPOT_INDEX[2].close,
    Some(crate::session::SESSION_CLOSE_MINUTE)
));
