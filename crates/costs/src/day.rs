//! The trade date, as a day ordinal.
//!
//! A regime lookup compares dates and does nothing else with them, so the
//! representation that matters is the one that makes a comparison a single
//! machine instruction. [`TradeDay`] carries a proleptic-Gregorian day ordinal
//! (days since 1970-01-01) as its first field, so the derived [`Ord`] is one
//! `i32` compare and never walks year, month and day in turn.
//!
//! # Why this is not a second date type
//!
//! `crates/core` already owns the only calendar validator in this workspace:
//! [`brutex_core::instrument::Expiry`], which refuses 31 February rather than
//! normalising it and honours the Gregorian century rule. Writing a second
//! leap-year rule here would be a rule that can disagree with the one the
//! store files contracts under. [`TradeDay::new`] therefore **validates
//! through** `Expiry` and keeps only the ordinal arithmetic, which `Expiry`
//! has no reason to carry.
//!
//! That inherits `Expiry`'s year window, 1990..=2100, and the test
//! `the_year_window_is_cores_and_a_drift_would_be_caught` pins the two
//! together so a future widening of `core` cannot silently widen this crate.
//!
//! # Why the window is a refusal rather than a clamp
//!
//! A date outside 1990..=2100 is [`CostError::YearOutsideWindow`], not a
//! saturated `MIN`/`MAX`. Clamping would price a 1974 trade at the 1990 rate
//! and say nothing — a fall-back that hides a failure, which `CLAUDE.md` §4
//! bans. The rate tables anchor at [`TradeDay::MIN`], so **every representable
//! day** has a row; the dates that have no row are exactly the dates that
//! cannot be constructed.

use std::fmt;

use brutex_core::instrument::Expiry;

use crate::error::CostError;

/// The first year [`TradeDay`] can represent.
///
/// This is [`brutex_core::instrument::Expiry`]'s own lower bound, restated
/// here because the regime tables anchor on it. A test pins the two together.
pub const FIRST_YEAR: u16 = 1990;

/// The last year [`TradeDay`] can represent.
///
/// [`brutex_core::instrument::Expiry`]'s own upper bound. A test pins them.
pub const LAST_YEAR: u16 = 2100;

/// The shortest a month can be: February in a common year.
///
/// [`TradeDay::last_of_its_month`] walks upward from here. It matters that this
/// day exists in **every** month of every year — that is what makes the walk
/// total and lets it have no fallback arm.
const SHORTEST_MONTH: u8 = 28;

/// The longest a month can be.
///
/// The walk stops here. Between the two, `core` decides which lengths are real
/// — this crate holds **no second leap rule**, here or anywhere.
const LONGEST_MONTH: u8 = 31;

/// A day of the week, Monday first.
///
/// Monday-first is the order the expiry regime tables are written in, because
/// it is the order the source's tables are written in (the predecessor's
/// `date.weekday()`: `Mon = 0 .. Sun = 6`). Keeping the same origin means an
/// expiry weekday transcribed from a circular does not need a second mental
/// conversion, which is where an off-by-one lands a monthly expiry a day early.
///
/// All seven days exist even though every shipped expiry row is a weekday. A
/// five-variant type would make Saturday unrepresentable, which sounds like a
/// guarantee until [`TradeDay::weekday`] has to answer for a Saturday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    /// Monday, index 0.
    Monday,
    /// Tuesday, index 1.
    Tuesday,
    /// Wednesday, index 2.
    Wednesday,
    /// Thursday, index 3.
    Thursday,
    /// Friday, index 4.
    Friday,
    /// Saturday, index 5. No Indian exchange settles on one.
    Saturday,
    /// Sunday, index 6. No Indian exchange settles on one.
    Sunday,
}

impl Weekday {
    /// Every day of the week, in index order.
    pub const ALL: [Self; 7] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    /// The Monday-first index, 0..=6.
    ///
    /// `u8` rather than a signed type on purpose: every distance this crate
    /// takes between two weekdays is written `(a + 7 - b) % 7`, which stays
    /// inside 0..=13 and can therefore never underflow, never overflow and
    /// never need a cast. A signed index would invite `(a - b).rem_euclid(7)`,
    /// which is the same answer reached through a subtraction that can go
    /// negative.
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }

    /// How many days forward from `self` the next `target` is, 0..=6.
    ///
    /// Zero when they are the same day of the week — "the next Thursday on or
    /// after a Thursday is that Thursday", which is the expiry law's own
    /// inclusive reading.
    #[must_use]
    pub const fn days_until(self, target: Self) -> u8 {
        (target.index() + 7 - self.index()) % 7
    }

    /// How many days back from `self` the previous `target` is, 0..=6.
    #[must_use]
    pub const fn days_since(self, target: Self) -> u8 {
        (self.index() + 7 - target.index()) % 7
    }

    /// Whether an Indian exchange trades on this day.
    ///
    /// Weekends only. A trading **holiday** is a different question and this
    /// crate does not answer it — see [`crate::expiry`], which says so where a
    /// caller will read it.
    #[must_use]
    pub const fn is_trading_weekday(self) -> bool {
        !matches!(self, Self::Saturday | Self::Sunday)
    }

    /// The three-letter name, which is how every circular writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Monday => "Mon",
            Self::Tuesday => "Tue",
            Self::Wednesday => "Wed",
            Self::Thursday => "Thu",
            Self::Friday => "Fri",
            Self::Saturday => "Sat",
            Self::Sunday => "Sun",
        }
    }
}

impl fmt::Display for Weekday {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A calendar date on which a trade may be priced.
///
/// Ordering is chronological and costs one `i32` compare. Equality and hashing
/// are over every field, which is consistent with ordering because the ordinal
/// is a bijection with `(year, month, day)` over the representable window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TradeDay {
    /// Days since 1970-01-01. First field, so the derived `Ord` uses it alone
    /// in every case that matters.
    ordinal: i32,
    year: u16,
    month: u8,
    day: u8,
}

impl TradeDay {
    /// The earliest representable day, 1990-01-01.
    ///
    /// Every regime table in this crate anchors its first row here, which is
    /// asserted at compile time. That is what makes "the date precedes the
    /// table" unrepresentable rather than a branch nothing can cover.
    pub const MIN: Self = Self {
        ordinal: days_from_civil(FIRST_YEAR, 1, 1),
        year: FIRST_YEAR,
        month: 1,
        day: 1,
    };

    /// The latest representable day, 2100-12-31.
    pub const MAX: Self = Self {
        ordinal: days_from_civil(LAST_YEAR, 12, 31),
        year: LAST_YEAR,
        month: 12,
        day: 31,
    };

    /// Builds a trade day, refusing anything that is not a real date.
    ///
    /// # Errors
    ///
    /// * [`CostError::YearOutsideWindow`] if the year is outside
    ///   [`FIRST_YEAR`]..=[`LAST_YEAR`]. This is checked here, before
    ///   `core`, so the operator is told *which* of the two things was wrong
    ///   rather than getting one undifferentiated "malformed".
    /// * [`CostError::MalformedDate`] if the month or the day is not a real
    ///   one for that month, leap years included. The judgement is
    ///   [`brutex_core::instrument::Expiry`]'s, not a second copy of it.
    ///
    /// # Examples
    ///
    /// ```
    /// use costs::day::TradeDay;
    /// let boundary = TradeDay::new(2024, 10, 1)?;
    /// assert_eq!(boundary.to_string(), "2024-10-01");
    /// # Ok::<(), costs::error::CostError>(())
    /// ```
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, CostError> {
        if !(FIRST_YEAR..=LAST_YEAR).contains(&year) {
            return Err(CostError::YearOutsideWindow { year });
        }
        Self::new_const(year, month, day).ok_or(CostError::MalformedDate)
    }

    /// Builds a trade day in a `const` context, or `None` if it is not one.
    ///
    /// The regime tables are `const` items, so their boundary dates cannot go
    /// through [`Self::new`] — a `Result` carrying a [`CostError`] cannot be
    /// unwrapped in a `const` context without a panic, and `clippy::panic` is
    /// denied. This is the same construction with the reason collapsed to
    /// `None`, and [`Self::new`] is a thin wrapper over it so there is one
    /// construction rather than two that could disagree.
    ///
    /// `core` decides what a real date is — the month lengths, the leap rule
    /// **and the year window**. This crate does not hold a second opinion about
    /// February, and it does not hold a second copy of the year check either:
    /// [`Self::new`] tests the year only so it can name it in the error, and
    /// repeating that test here would be a branch no input can distinguish.
    /// `cargo-mutants` found exactly that — a mutant flipping the duplicate
    /// `||` to `&&` survived, because `Expiry` refuses the same years anyway.
    pub(crate) const fn new_const(year: u16, month: u8, day: u8) -> Option<Self> {
        match Expiry::new(year, month, day) {
            Ok(checked) => Some(Self {
                ordinal: days_from_civil(checked.year(), checked.month(), checked.day()),
                year: checked.year(),
                month: checked.month(),
                day: checked.day(),
            }),
            Err(_) => None,
        }
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub const fn ordinal(self) -> i32 {
        self.ordinal
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month, 1..=12.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of month.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// Whether this day is strictly earlier than `other`.
    ///
    /// A `const fn` because the regime tables assert their own ordering at
    /// compile time, and `PartialOrd` is not usable in a `const` context.
    #[must_use]
    pub const fn before(self, other: Self) -> bool {
        self.ordinal < other.ordinal
    }

    /// Whether this day is the same day as `other`.
    ///
    /// A `const fn` for the same reason as [`Self::before`].
    #[must_use]
    pub const fn same_day(self, other: Self) -> bool {
        self.ordinal == other.ordinal
    }

    /// The day of the week.
    ///
    /// One remainder. 1970-01-01 was a Thursday, so adding three shifts the
    /// epoch onto a Monday and the Monday-first index falls out of
    /// `(ordinal + 3) mod 7` with no table and no branch on the calendar.
    ///
    /// Every representable ordinal is at least [`TradeDay::MIN`]'s 7,305, so
    /// the sum is positive and `rem_euclid` and `%` agree; `rem_euclid` is used
    /// anyway because it is the operation the law is stated in.
    #[must_use]
    pub const fn weekday(self) -> Weekday {
        match (self.ordinal + 3).rem_euclid(7) {
            0 => Weekday::Monday,
            1 => Weekday::Tuesday,
            2 => Weekday::Wednesday,
            3 => Weekday::Thursday,
            4 => Weekday::Friday,
            5 => Weekday::Saturday,
            _ => Weekday::Sunday,
        }
    }

    /// The day `days` after this one, or a refusal naming the ordinal it would
    /// have needed.
    ///
    /// # Errors
    ///
    /// [`CostError::OrdinalOutsideWindow`] when the result falls outside
    /// [`Self::MIN`]..=[`Self::MAX`]. Refused rather than saturated: a
    /// saturated expiry files a contract on a day it never expired.
    pub fn plus_days(self, days: i32) -> Result<Self, CostError> {
        let ordinal = self.ordinal.checked_add(days).ok_or(CostError::Overflow {
            operation: "day + days",
        })?;
        Self::from_ordinal(ordinal)
    }

    /// The day with this ordinal, or a refusal.
    ///
    /// The inverse of [`Self::ordinal`], by Howard Hinnant's `civil_from_days`.
    /// Pure integer arithmetic: a fixed count of multiplications and divisions,
    /// no table, no loop, and no branch whose count depends on the input.
    ///
    /// # Errors
    ///
    /// [`CostError::OrdinalOutsideWindow`] for an ordinal outside
    /// [`Self::MIN`]..=[`Self::MAX`]. The check comes **first**, which is what
    /// makes the published algorithm's negative-era correction unreachable and
    /// therefore honestly absent — see [`days_from_civil`] for the same
    /// argument in the other direction.
    ///
    /// # Examples
    ///
    /// ```
    /// use costs::day::TradeDay;
    /// let day = TradeDay::new(2024, 10, 1)?;
    /// assert_eq!(TradeDay::from_ordinal(day.ordinal()), Ok(day));
    /// # Ok::<(), costs::error::CostError>(())
    /// ```
    pub fn from_ordinal(ordinal: i32) -> Result<Self, CostError> {
        if !(Self::MIN.ordinal..=Self::MAX.ordinal).contains(&ordinal) {
            return Err(CostError::OrdinalOutsideWindow { ordinal });
        }
        let (year, month, day) = civil_from_days(ordinal);
        // The ordinal is in range, so the civil triple it decodes to is a real
        // date inside the window and `new` cannot refuse it. `?` propagates
        // rather than asserting, because an assertion here would be a panic and
        // `clippy::panic` is denied; the round-trip test over all 40,542 days
        // is what proves the arm is never taken.
        Self::new(year, month, day)
    }

    /// The last day of the month this day falls in.
    ///
    /// Total: it cannot fail. `self` is already a real day, so its month is a
    /// real month, and exactly one of 28, 29, 30 and 31 is that month's length.
    /// The starting point is the 28th, which exists in **every** month of every
    /// year and therefore needs no probe of its own; the three longer lengths
    /// are asked of `core`'s calendar in turn and the last that answers wins.
    /// There is no fallback arm to hide anything in, and nothing to refuse.
    ///
    /// Three probes, and `core`'s leap rule rather than a second copy of it.
    ///
    /// They are **unrolled rather than looped**, deliberately. A loop needs a
    /// counter, and a counter is one mutation away from never advancing — a
    /// non-terminating mutant is detected by a hung test rather than by an
    /// assertion, and "the suite hung" is a worse signal than "this assertion
    /// failed". Three is a small enough number to write out.
    #[must_use]
    pub const fn last_of_its_month(self) -> Self {
        let mut found = self.with_day_in_same_month(SHORTEST_MONTH);
        if let Some(candidate) = Self::new_const(self.year, self.month, 29) {
            found = candidate;
        }
        if let Some(candidate) = Self::new_const(self.year, self.month, 30) {
            found = candidate;
        }
        if let Some(candidate) = Self::new_const(self.year, self.month, LONGEST_MONTH) {
            found = candidate;
        }
        found
    }

    /// This day's month and year, with the day-of-month replaced.
    ///
    /// Caller's obligation: `day` is a real day of `self`'s month. Under it the
    /// year and the month do not move — only the ordinal and the day-of-month
    /// do, and neither can become invalid — so this is total, needs no
    /// validation, and has **no fallback arm**. It is the one unchecked
    /// constructor in this type, and both its call sites meet the obligation by
    /// a fact rather than by a check:
    ///
    /// * [`Self::last_of_its_month`] passes 28, which is a real day of every
    ///   month of every year.
    /// * [`Self::earlier_in_same_month`] passes `self.day - back` with
    ///   `back <= 6` off a month's last day, which is at least 28.
    ///
    /// `earlier_in_same_month_agrees_with_a_day_by_day_walk` and
    /// `the_last_day_of_its_month_is_cores_answer_and_not_a_second_leap_rule`
    /// prove the results against independent walks over every month of the
    /// window.
    // The casts are widening ones from unsigned types: no truncation and no
    // sign loss are possible, so only the stylistic `cast_lossless` needs
    // excusing — and `i32::from` is not callable in a `const fn` on the pinned
    // toolchain, for the reason `days_from_civil` records.
    #[allow(clippy::cast_lossless)]
    pub(crate) const fn with_day_in_same_month(self, day: u8) -> Self {
        Self {
            ordinal: self.ordinal + day as i32 - self.day as i32,
            year: self.year,
            month: self.month,
            day,
        }
    }

    /// The day `back` days earlier, when that day is known to be in the same
    /// month.
    ///
    /// Caller's obligation: `back < self.day` — see
    /// [`Self::with_day_in_same_month`], which this is a thin wrapper over so
    /// there is one unchecked construction rather than two that could disagree.
    pub(crate) const fn earlier_in_same_month(self, back: u8) -> Self {
        self.with_day_in_same_month(self.day - back)
    }

    /// The `(year, month)` after this day's own.
    ///
    /// December rolls the year. The rolled year may leave the window, and that
    /// is not this function's problem to hide — the caller asks for a day in it
    /// and gets [`CostError::YearOutsideWindow`] by name.
    #[must_use]
    pub const fn next_month(self) -> (u16, u8) {
        if self.month == 12 {
            (self.year + 1, 1)
        } else {
            (self.year, self.month + 1)
        }
    }
}

impl fmt::Display for TradeDay {
    /// Renders `YYYY-MM-DD`, the one date format this repository accepts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Days since 1970-01-01, by Howard Hinnant's `days_from_civil`.
///
/// Pure integer arithmetic — five multiplications and four divisions, the same
/// count for every input, no table and no loop. That is the whole reason the
/// ordinal is stored rather than recomputed on every compare.
///
/// The published algorithm carries a correction for negative years
/// (`era = (y >= 0 ? y : y - 399) / 400`). It is **deliberately absent**:
/// [`TradeDay::new`] refuses any year below [`FIRST_YEAR`], so `y` is at worst
/// 1989 and the correction could never fire. Writing it would be a branch no
/// test could reach, and `CLAUDE.md` §9 requires 100% coverage — an
/// unreachable branch is not a safety net, it is an uncovered line that has to
/// be excused.
///
/// Caller's obligation: `year` is in [`FIRST_YEAR`]..=[`LAST_YEAR`] and
/// `(month, day)` is a real date. Both are checked by [`TradeDay::new`], which
/// is the only caller outside this module's own compile-time constants.
// `i32::from` is not callable in a `const fn` on the pinned toolchain -- `From`
// is not a const trait yet (rust-lang/rust#143874), and this function has to be
// const because the regime tables are `const` items whose boundary dates are
// built from it. Every cast below is a widening one from an unsigned type or a
// bool into `i32`: no truncation and no sign loss are possible, which is why
// the workspace's `cast_possible_truncation` and `cast_sign_loss` denials do
// not fire and only the stylistic `cast_lossless` needs excusing.
#[allow(clippy::cast_lossless)]
const fn days_from_civil(year: u16, month: u8, day: u8) -> i32 {
    let m = month as i32;
    let d = day as i32;
    // Shift the year so that March is month 1: the leap day then lands at the
    // END of the shifted year, where it needs no special case.
    let y = year as i32 - (m <= 2) as i32;
    let era = y / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146_096]
    era * 146_097 + doe - 719_468
}

/// The civil date of a day ordinal, by Howard Hinnant's `civil_from_days`.
///
/// The exact inverse of [`days_from_civil`], and pure integer arithmetic in the
/// same way: a fixed count of multiplications and divisions, no table, no loop,
/// and no branch whose count depends on the input.
///
/// The published algorithm carries a correction for ordinals before the era
/// epoch (`era = (z >= 0 ? z : z - 146096) / 146097`). It is **deliberately
/// absent** for the same reason its twin's is: [`TradeDay::from_ordinal`]
/// refuses any ordinal below [`TradeDay::MIN`]'s 7,305 **before** calling this,
/// so `z` is at worst `7_305 + 719_468` and the correction could never fire.
/// An unreachable branch is not a safety net, it is an uncovered line.
///
/// Caller's obligation: `ordinal` is in
/// [`TradeDay::MIN`]`.ordinal()..=`[`TradeDay::MAX`]`.ordinal()`. Checked by
/// [`TradeDay::from_ordinal`], its only caller.
// Every cast below is bounded by the caller's obligation and proven by
// `from_ordinal_is_the_exact_inverse_of_ordinal_over_the_whole_window`, which
// walks all 40,542 representable days: the year lands in 1990..=2100 (fits
// `u16`), the month in 1..=12 and the day in 1..=31 (both fit `u8`). Widening
// the return type to `(i32, i32, i32)` would only move the same three casts
// into the caller.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn civil_from_days(ordinal: i32) -> (u16, u8, u8) {
    // Shift onto the era epoch, 0000-03-01, where the leap day is last.
    let z = ordinal + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097; // [0, 146_096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year as u16, month as u8, day as u8)
}

/// Every representable day, in ascending order.
///
/// A test helper, shared with `crate::regime` so that its differential test
/// walks the whole domain rather than sampling it.
#[cfg(test)]
pub(crate) fn every_representable_day() -> impl Iterator<Item = TradeDay> {
    (FIRST_YEAR..=LAST_YEAR).flat_map(|y| {
        (1u8..=12).flat_map(move |m| (1u8..=31).filter_map(move |d| TradeDay::new(y, m, d).ok()))
    })
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn the_ordinals_are_the_hand_computed_ones() {
        // Derived by hand from 1970-01-01 = 0, via the two anchors anyone can
        // recheck: 2000-01-01 = 10_957 (30 years, 7 leap days) and
        // 2020-01-01 = 18_262 (20 more years, 5 more leap days).
        for (want, y, m, d) in [
            (7_305, 1990, 1, 1),    // 20 years + 5 leap days after the epoch
            (19_996, 2024, 9, 30),  // the day BEFORE the true-to-label boundary
            (19_997, 2024, 10, 1),  // the boundary itself
            (20_543, 2026, 3, 31),  // the day before the Finance Act 2026 hike
            (20_544, 2026, 4, 1),   // the hike
            (47_846, 2100, 12, 31), // the last representable day
        ] {
            let got = TradeDay::new(y, m, d).expect("a real date");
            assert_eq!(got.ordinal(), want, "{y:04}-{m:02}-{d:02}");
        }
    }

    #[test]
    fn min_and_max_are_the_ends_of_the_representable_window() {
        assert_eq!(TradeDay::new(1990, 1, 1).expect("real"), TradeDay::MIN);
        assert_eq!(TradeDay::new(2100, 12, 31).expect("real"), TradeDay::MAX);
        assert_eq!(TradeDay::MIN.ordinal(), 7_305);
        assert_eq!(TradeDay::MAX.ordinal(), 47_846);
        assert!(TradeDay::MIN.before(TradeDay::MAX));
        assert!(!TradeDay::MAX.before(TradeDay::MIN));
        assert!(TradeDay::MIN.same_day(TradeDay::MIN));
        assert!(!TradeDay::MIN.same_day(TradeDay::MAX));
    }

    #[test]
    fn every_representable_day_is_one_ordinal_after_the_one_before_it() {
        // The strongest statement available about the ordinal: it is a
        // bijection onto a contiguous integer range over the whole window.
        // If a leap rule were wrong anywhere in 111 years, this fails.
        let mut n = 0i32;
        let mut previous: Option<TradeDay> = None;
        for today in every_representable_day() {
            if let Some(yesterday) = previous {
                assert_eq!(
                    today.ordinal() - yesterday.ordinal(),
                    1,
                    "{yesterday} -> {today} is not one day"
                );
                assert!(yesterday < today, "ordering must be chronological");
            }
            previous = Some(today);
            n += 1;
        }
        assert_eq!(n, 47_846 - 7_305 + 1, "the window is contiguous");
        assert_eq!(previous, Some(TradeDay::MAX));
    }

    #[test]
    fn refuses_an_impossible_date_rather_than_normalising_it() {
        // 31 February would become 3 March under a normalising date type and
        // would price a trade in a month it never happened in.
        for (y, m, d) in [
            (2025, 2, 29),
            (2025, 2, 31),
            (2025, 13, 1),
            (2025, 0, 1),
            (2025, 1, 0),
            (2025, 4, 31),
            (2100, 2, 29),
            (2025, u8::MAX, u8::MAX),
        ] {
            assert_eq!(
                TradeDay::new(y, m, d),
                Err(CostError::MalformedDate),
                "{y}-{m}-{d} must be refused"
            );
        }
        // The leap day itself is a real date and is not collateral damage.
        assert_eq!(TradeDay::new(2024, 2, 29).expect("leap").day(), 29);
        assert_eq!(TradeDay::new(2000, 2, 29).expect("400-leap").month(), 2);
    }

    #[test]
    fn refuses_a_year_outside_the_window_by_name() {
        for year in [0, 1, 1989, 2101, u16::MAX] {
            assert_eq!(
                TradeDay::new(year, 1, 1),
                Err(CostError::YearOutsideWindow { year }),
                "{year} must be refused as a year, not as a malformed date"
            );
        }
        assert_eq!(TradeDay::new(1990, 1, 1).expect("in window").year(), 1990);
        assert_eq!(TradeDay::new(2100, 1, 1).expect("in window").year(), 2100);
    }

    #[test]
    fn the_year_window_is_cores_and_a_drift_would_be_caught() {
        // This crate restates core's year bounds so it can name them in an
        // error. Restating is duplication, and duplication drifts. If core
        // ever widens Expiry, this fails and the constants above get fixed
        // rather than quietly disagreeing with the store.
        assert!(
            Expiry::new(FIRST_YEAR - 1, 1, 1).is_err(),
            "core accepts a year this crate refuses"
        );
        assert!(
            Expiry::new(LAST_YEAR + 1, 1, 1).is_err(),
            "core accepts a year this crate refuses"
        );
        assert!(Expiry::new(FIRST_YEAR, 1, 1).is_ok());
        assert!(Expiry::new(LAST_YEAR, 12, 31).is_ok());
    }

    #[test]
    fn the_const_constructor_answers_exactly_as_the_checked_one_does() {
        // `new` is a wrapper over `new_const`, and the regime tables build
        // their boundary dates through the latter. If the two ever disagreed,
        // a table would anchor somewhere its documentation did not say.
        for (y, m, d) in [
            (1990, 1, 1),
            (2024, 9, 30),
            (2024, 10, 1),
            (2026, 4, 1),
            (2024, 2, 29),
            (2100, 12, 31),
        ] {
            assert_eq!(
                TradeDay::new_const(y, m, d),
                TradeDay::new(y, m, d).ok(),
                "{y:04}-{m:02}-{d:02}"
            );
            assert!(TradeDay::new_const(y, m, d).is_some());
        }
        for (y, m, d) in [
            (1989, 12, 31),
            (2101, 1, 1),
            (0, 1, 1),
            (2025, 2, 29),
            (2025, 13, 1),
            (2025, 4, 31),
        ] {
            assert_eq!(
                TradeDay::new_const(y, m, d),
                None,
                "{y:04}-{m:02}-{d:02} must not build"
            );
            assert!(TradeDay::new(y, m, d).is_err());
        }
    }

    #[test]
    fn renders_the_one_accepted_date_format() {
        assert_eq!(
            TradeDay::new(2024, 10, 1).expect("real").to_string(),
            "2024-10-01"
        );
        assert_eq!(TradeDay::MIN.to_string(), "1990-01-01");
        assert_eq!(TradeDay::MAX.to_string(), "2100-12-31");
        assert_eq!(
            format!("{:?}", TradeDay::MIN),
            "TradeDay { ordinal: 7305, year: 1990, month: 1, day: 1 }"
        );
    }

    #[test]
    fn the_accessors_return_what_was_put_in() {
        let d = TradeDay::new(2026, 4, 1).expect("real");
        assert_eq!((d.year(), d.month(), d.day()), (2026, 4, 1));
        assert_eq!(d.ordinal(), 20_544);
    }

    #[test]
    fn from_ordinal_is_the_exact_inverse_of_ordinal_over_the_whole_window() {
        // The bijection, both directions, on every one of the 40,542 days. An
        // off-by-one anywhere in the era arithmetic — the leap rule, the month
        // lengths, the March shift, the epoch offset — diverges here.
        let mut n = 0u32;
        for today in every_representable_day() {
            assert_eq!(
                TradeDay::from_ordinal(today.ordinal()),
                Ok(today),
                "{today} did not survive the round trip"
            );
            n += 1;
        }
        assert_eq!(n, 40_542);
        assert_eq!(TradeDay::from_ordinal(7_305), Ok(TradeDay::MIN));
        assert_eq!(TradeDay::from_ordinal(47_846), Ok(TradeDay::MAX));
    }

    #[test]
    fn an_ordinal_off_either_end_is_refused_by_name_and_never_saturated() {
        for ordinal in [7_304, 0, -1, i32::MIN, 47_847, i32::MAX] {
            assert_eq!(
                TradeDay::from_ordinal(ordinal),
                Err(CostError::OrdinalOutsideWindow { ordinal }),
                "ordinal {ordinal} must be refused, not clamped onto MIN or MAX"
            );
        }
    }

    #[test]
    fn the_weekday_walks_forward_one_day_at_a_time_across_the_whole_window() {
        // 1970-01-01 was a Thursday; 1990-01-01 is 7,305 days later, and
        // 7,305 + 3 = 7,308 = 7 * 1,044, so the window opens on a Monday.
        assert_eq!(TradeDay::MIN.weekday(), Weekday::Monday);
        // Independently checkable anchors.
        assert_eq!(
            TradeDay::new(1970 + 20, 1, 1).expect("real").weekday(),
            Weekday::Monday
        );
        assert_eq!(
            TradeDay::new(2024, 10, 1).expect("real").weekday(),
            Weekday::Tuesday,
            "the true-to-label boundary was a Tuesday"
        );
        assert_eq!(
            TradeDay::new(2025, 9, 2).expect("real").weekday(),
            Weekday::Tuesday,
            "the first day of the NIFTY Tuesday-expiry regime is itself a Tuesday"
        );
        assert_eq!(
            TradeDay::new(2024, 12, 25).expect("real").weekday(),
            Weekday::Wednesday
        );
        assert_eq!(
            TradeDay::new(2024, 2, 29).expect("real").weekday(),
            Weekday::Thursday,
            "the leap day of 2024 was a Thursday"
        );
        // The law: consecutive days are consecutive weekdays, all 40,542 of
        // them, and every weekday is reached.
        let mut previous: Option<Weekday> = None;
        let mut seen = [0u32; 7];
        for today in every_representable_day() {
            let weekday = today.weekday();
            if let Some(yesterday) = previous {
                assert_eq!(
                    weekday.index(),
                    (yesterday.index() + 1) % 7,
                    "{today} does not follow its predecessor's weekday"
                );
            }
            seen[usize::from(weekday.index())] += 1;
            previous = Some(weekday);
        }
        assert_eq!(seen.iter().sum::<u32>(), 40_542);
        for count in seen {
            assert!((5_791..=5_792).contains(&count), "{count} of one weekday");
        }
    }

    #[test]
    fn the_weekday_type_knows_its_index_its_name_and_whether_india_trades() {
        assert_eq!(Weekday::ALL.len(), 7);
        for (index, weekday) in Weekday::ALL.into_iter().enumerate() {
            assert_eq!(
                weekday.index(),
                u8::try_from(index).expect("0..=6"),
                "{weekday} is at the wrong index"
            );
        }
        assert_eq!(Weekday::Thursday.index(), 3);
        assert_eq!(Weekday::Thursday.as_str(), "Thu");
        assert_eq!(Weekday::Thursday.to_string(), "Thu");
        assert_eq!(Weekday::Sunday.to_string(), "Sun");
        assert_eq!(
            Weekday::ALL.map(Weekday::as_str),
            ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        );
        for weekday in Weekday::ALL {
            assert_eq!(
                weekday.is_trading_weekday(),
                weekday.index() <= 4,
                "{weekday} traded?"
            );
        }
        assert!(Weekday::Monday < Weekday::Sunday);
        assert_eq!(format!("{:?}", Weekday::Friday), "Friday");
    }

    #[test]
    fn the_distance_between_two_weekdays_is_inclusive_forward_and_backward() {
        // The two laws the expiry arithmetic is written in. Zero on the same
        // weekday is the inclusive reading: the next Thursday on or after a
        // Thursday is that Thursday.
        assert_eq!(Weekday::Thursday.days_until(Weekday::Thursday), 0);
        assert_eq!(Weekday::Monday.days_until(Weekday::Thursday), 3);
        assert_eq!(Weekday::Friday.days_until(Weekday::Thursday), 6);
        assert_eq!(Weekday::Thursday.days_since(Weekday::Thursday), 0);
        assert_eq!(Weekday::Sunday.days_since(Weekday::Thursday), 3);
        assert_eq!(Weekday::Wednesday.days_since(Weekday::Thursday), 6);
        // Over every ordered pair: both are in 0..=6, they sum to 0 or 7, and
        // walking forward then back returns to the start.
        for from in Weekday::ALL {
            for to in Weekday::ALL {
                let forward = from.days_until(to);
                let backward = to.days_since(from);
                assert_eq!(forward, backward, "{from} -> {to}");
                assert!(forward <= 6, "{from} -> {to} is {forward} days");
                let mirror = to.days_until(from);
                assert_eq!(
                    forward + mirror,
                    if from == to { 0 } else { 7 },
                    "{from} <-> {to}"
                );
            }
        }
    }

    #[test]
    fn the_last_day_of_its_month_is_cores_answer_and_not_a_second_leap_rule() {
        for (year, month, day, want) in [
            (2024, 1, 5, 31),
            (2024, 2, 1, 29),  // leap
            (2023, 2, 14, 28), // not
            (2000, 2, 1, 29),  // the 400 rule
            (2100, 2, 1, 28),  // the 100 rule — the case a naive %4 gets wrong
            (2024, 4, 30, 30),
            (2024, 12, 25, 31),
            (1990, 1, 1, 31),
        ] {
            let last = TradeDay::new(year, month, day)
                .expect("real")
                .last_of_its_month();
            assert_eq!((last.year(), last.month(), last.day()), (year, month, want));
        }
        // The sweep: every month of every representable year. The last day is
        // in the same month, and the day after it is not.
        let mut months = 0u32;
        for year in FIRST_YEAR..=LAST_YEAR {
            for month in 1u8..=12 {
                let first = TradeDay::new(year, month, 1).expect("the first exists");
                let last = first.last_of_its_month();
                assert_eq!((last.year(), last.month()), (year, month));
                assert!(last.day() >= SHORTEST_MONTH && last.day() <= LONGEST_MONTH);
                assert!(
                    TradeDay::new(year, month, last.day() + 1).is_err(),
                    "{last} has a day after it inside its own month"
                );
                assert_eq!(
                    last.last_of_its_month(),
                    last,
                    "the last day of a month is its own month's last"
                );
                months += 1;
            }
        }
        assert_eq!(months, 111 * 12);
        assert_eq!(SHORTEST_MONTH, 28);
        assert_eq!(LONGEST_MONTH, 31);
    }

    #[test]
    fn earlier_in_same_month_agrees_with_a_day_by_day_walk() {
        // The obligation is `back < self.day`; the call site never exceeds 6.
        // This checks every month's last day against a day-by-day walk back.
        let mut checked = 0u32;
        for year in FIRST_YEAR..=LAST_YEAR {
            for month in 1u8..=12 {
                let last = TradeDay::new(year, month, 1)
                    .expect("the first exists")
                    .last_of_its_month();
                for back in 0u8..=6 {
                    let got = last.earlier_in_same_month(back);
                    let want = TradeDay::new(year, month, last.day() - back).expect("same month");
                    assert_eq!(got, want, "{last} minus {back}");
                    assert_eq!(got.ordinal(), last.ordinal() - i32::from(back));
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 111 * 12 * 7);
    }

    #[test]
    fn plus_days_walks_the_calendar_and_refuses_to_walk_off_the_end() {
        let boundary = TradeDay::new(2024, 9, 30).expect("real");
        assert_eq!(
            boundary.plus_days(1),
            Ok(TradeDay::new(2024, 10, 1).expect("real"))
        );
        assert_eq!(boundary.plus_days(0), Ok(boundary));
        assert_eq!(
            boundary.plus_days(-1),
            Ok(TradeDay::new(2024, 9, 29).expect("real"))
        );
        // Across a leap February.
        let leap = TradeDay::new(2024, 2, 28).expect("real");
        assert_eq!(
            leap.plus_days(1),
            Ok(TradeDay::new(2024, 2, 29).expect("real"))
        );
        assert_eq!(
            leap.plus_days(2),
            Ok(TradeDay::new(2024, 3, 1).expect("real"))
        );
        // Off the end, by one and by a lot — named, never saturated.
        assert_eq!(
            TradeDay::MAX.plus_days(1),
            Err(CostError::OrdinalOutsideWindow { ordinal: 47_847 })
        );
        assert_eq!(
            TradeDay::MIN.plus_days(-1),
            Err(CostError::OrdinalOutsideWindow { ordinal: 7_304 })
        );
        // And past i32 itself, which is an overflow rather than a window miss.
        // The two are different errors on purpose: one says the day is real but
        // outside the table's reach, the other says the sum is not a day at all.
        assert_eq!(
            TradeDay::MAX.plus_days(i32::MAX),
            Err(CostError::Overflow {
                operation: "day + days"
            })
        );
        // Walking back from MIN by i32::MIN does NOT overflow — 7,305 plus
        // i32::MIN still fits — so it is the window miss, named with the
        // ordinal it would have needed.
        assert_eq!(
            TradeDay::MIN.plus_days(i32::MIN),
            Err(CostError::OrdinalOutsideWindow {
                ordinal: 7_305 + i32::MIN
            })
        );
        assert_eq!(
            TradeDay::MIN.plus_days(-7_306),
            Err(CostError::OrdinalOutsideWindow { ordinal: -1 })
        );
    }

    #[test]
    fn the_next_month_rolls_december_into_january_of_the_following_year() {
        assert_eq!(
            TradeDay::new(2024, 11, 20).expect("real").next_month(),
            (2024, 12)
        );
        assert_eq!(
            TradeDay::new(2024, 12, 25).expect("real").next_month(),
            (2025, 1)
        );
        assert_eq!(TradeDay::MAX.next_month(), (2101, 1));
        assert_eq!(TradeDay::MIN.next_month(), (1990, 2));
        // The rolled year leaving the window is not hidden here: it is refused
        // by name by the constructor that actually needs a day in it.
        let (year, month) = TradeDay::MAX.next_month();
        assert_eq!(
            TradeDay::new(year, month, 15),
            Err(CostError::YearOutsideWindow { year: 2101 })
        );
        // Every month of every year rolls to the month after it.
        for year in FIRST_YEAR..=LAST_YEAR {
            for month in 1u8..=12 {
                let (next_year, next_month) = TradeDay::new(year, month, 1)
                    .expect("the first exists")
                    .next_month();
                let want = if month == 12 {
                    (year + 1, 1)
                } else {
                    (year, month + 1)
                };
                assert_eq!((next_year, next_month), want, "{year}-{month}");
            }
        }
    }

    #[test]
    fn ordering_and_hashing_agree_with_the_calendar() {
        use std::collections::HashSet;

        let early = TradeDay::new(2024, 9, 30).expect("real");
        let late = TradeDay::new(2024, 10, 1).expect("real");
        let same = TradeDay::new(2024, 10, 1).expect("real");
        assert!(early < late);
        assert_eq!(late, same);
        assert_eq!(late.cmp(&same), std::cmp::Ordering::Equal);

        let mut set = HashSet::new();
        assert!(set.insert(late));
        assert!(!set.insert(same), "equal days must hash to one entry");
        assert!(set.insert(early));
        assert_eq!(set.len(), 2);
    }
}
