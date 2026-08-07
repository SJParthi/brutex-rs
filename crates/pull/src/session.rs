//! The trading session, as one set of numbers, and the calendar arithmetic a
//! vendor window needs.
//!
//! # The session, from the exchange
//!
//! `docs/00-charter.md` §3: the regular session is **09:15 inclusive to 15:30
//! exclusive IST**, the last one-minute bar opens at **15:29**, and that is
//! exactly **375** bars. Those three statements are one statement, and
//! [`SESSION_OPEN_MINUTE`], [`SESSION_CLOSE_MINUTE`] and
//! [`BARS_PER_REGULAR_SESSION`] are pinned to each other by a compile-time
//! assertion so they cannot drift apart.
//!
//! **An operator asserted 15:40. The exchange says 15:30**, and NSE's own
//! `marketStatus` endpoint was read to confirm it. Ten phantom minutes a day is
//! not a rounding difference: every VWAP, every session-close figure and every
//! forced-exit bar would be computed over a window the exchange never traded.
//! The number is written **once**, here, and every filter reads it from this
//! module — `CLAUDE.md` §3 rule 1, and the reason a constant repeated in three
//! filters is a constant that will one day disagree with itself.
//!
//! # A daily bar is exempt, and that is not a special case
//!
//! A daily bar has no intraday time. Vendors stamp it at midnight, or at the
//! open, or at the close, and none of those is inside 09:15–15:30 by
//! coincidence. Applying an intraday window to it would drop every daily bar
//! ever fetched, so [`Cadence`] is an argument to the filter rather than an
//! assumption inside it, and `pull::fetch::a_daily_bar_is_exempt_from_the_
//! session_filter` is what holds it up.
//!
//! # What this module deliberately does **not** know
//!
//! **There is no trading calendar here.** It does not know a holiday, a Muhurat
//! session or a Saturday budget session, and it therefore never claims a bar is
//! on a non-trading date. `docs/00-charter.md` records three special-session
//! shapes and no holiday list, so a weekend rule would be *wrong* — 2025-02-01
//! was a Saturday and a full 375-bar session. Invariant P-03 stays `—` for that
//! reason, and `docs/06-limits.md` §20 says so rather than letting a
//! session-window filter be mistaken for a calendar. What is filtered here is
//! the **window**: the operator's date range, and the intraday session bounds.
//!
//! # Cost
//!
//! Every function here is arithmetic on two integers. There is no table, no
//! search and no allocation: `days_from_civil` and `civil_from_days` are the
//! standard closed-form civil-calendar pair, so a timestamp in 1970 and one in
//! 9999 cost the same — `docs/07-o1-architecture.md` law 4, arithmetic beats
//! lookup.

use std::fmt;

use store::path::{PathError, YearMonth};

/// Seconds IST is ahead of UTC: 5 hours 30 minutes, fixed.
///
/// `docs/00-charter.md` §3: *"IST, fixed +05:30, no daylight saving."* India
/// has observed no daylight saving since 1945, so this is a constant rather
/// than a lookup — which is what lets a timestamp be converted with an add
/// instead of a timezone database.
pub const IST_OFFSET_SECS: i64 = 5 * 3_600 + 30 * 60;

/// Seconds in a day.
pub const SECS_PER_DAY: i64 = 86_400;

/// Seconds in a minute.
pub const SECS_PER_MINUTE: i64 = 60;

/// The first minute of the regular session, **inclusive**: 09:15 IST.
///
/// Minutes since IST midnight, so 9·60 + 15.
pub const SESSION_OPEN_MINUTE: u32 = 9 * 60 + 15;

/// The end of the regular session, **exclusive**: 15:30 IST.
///
/// Exclusive is the whole point. A bar is stamped at the **open** of its
/// interval (`docs/00-charter.md` §3), so the last one-minute bar of a session
/// opens at 15:29 and closes exactly at 15:30. A filter written `<= 15:30`
/// admits a 15:30 bar that the exchange never traded.
pub const SESSION_CLOSE_MINUTE: u32 = 15 * 60 + 30;

/// One-minute bars in one regular session.
///
/// `docs/00-charter.md` §3: 375. It is *derived* from the two bounds above by
/// the assertion below rather than merely written beside them, because the
/// arithmetic is the proof that the two bounds are the right pair: 09:15
/// through 15:29 inclusive is 375 minutes, and any other close makes this
/// number wrong.
pub const BARS_PER_REGULAR_SESSION: u32 = 375;

const _: () = assert!(SESSION_CLOSE_MINUTE - SESSION_OPEN_MINUTE == BARS_PER_REGULAR_SESSION);
const _: () = assert!(SESSION_OPEN_MINUTE == 555 && SESSION_CLOSE_MINUTE == 930);
const _: () = assert!(IST_OFFSET_SECS == 19_800);

/// The first year a bar can carry. Timestamps are seconds since the epoch.
pub const MIN_YEAR: u32 = 1_970;

/// The last year this build will name. Above it a four-digit rendering lies,
/// and `store::path::YearMonth` refuses it too.
pub const MAX_YEAR: u32 = 9_999;

/// Why a timestamp or a date is not one.
///
/// Every variant carries the value it refused. A refusal that says only
/// "invalid date" sends an operator to a hex dump of an epoch second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SessionError {
    /// The timestamp is before the Unix epoch.
    ///
    /// No bar predates 1970, and a negative epoch second is a vendor field that
    /// was read at the wrong offset or a sentinel that leaked. Refused rather
    /// than clamped: a clamped timestamp is a bar filed under 1970-01-01.
    BeforeEpoch {
        /// The value that was refused.
        secs: i64,
    },
    /// The timestamp does not name a date this build can render.
    ///
    /// Reached by a value so large that the day count leaves `u32`, or a year
    /// that leaves `u16`. Both are the same fault to an operator — the column
    /// is not epoch seconds — so they are one variant.
    TimestampOutOfRange {
        /// The value that was refused.
        secs: i64,
    },
    /// The year is outside `1970..=9999`.
    YearOutOfRange {
        /// The year that was refused.
        year: u32,
    },
    /// The month is outside `1..=12`.
    MonthOutOfRange {
        /// The month that was refused.
        month: u8,
    },
    /// The day is outside `1..=` the length of that month, in that year.
    ///
    /// Leap years are computed, not tabulated: 2024-02-29 exists and
    /// 2023-02-29 does not, and a table would be a second definition of the
    /// Gregorian rule.
    DayOutOfRange {
        /// The day that was refused.
        day: u8,
        /// How many days that month actually has.
        month_len: u8,
    },
    /// The day after this one is past [`MAX_YEAR`].
    ///
    /// Refused rather than wrapped, because the one caller is the vendor's
    /// non-inclusive `toDate` rule: a wrapped day would silently ask for a
    /// window ending in 1970.
    NoNextDay,
    /// A window whose end is before its start.
    ///
    /// Refused rather than silently swapped. An operator who typed the dates in
    /// the wrong order gets told; a pull that quietly reversed them would fetch
    /// a range nobody asked for and report success.
    WindowRunsBackwards {
        /// The start.
        from: Day,
        /// The end, which does not follow it.
        to: Day,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BeforeEpoch { secs } => {
                write!(f, "timestamp {secs} is before the Unix epoch")
            }
            Self::TimestampOutOfRange { secs } => {
                write!(f, "timestamp {secs} names no date this build can render")
            }
            Self::YearOutOfRange { year } => {
                write!(f, "year {year} is not {MIN_YEAR}..={MAX_YEAR}")
            }
            Self::MonthOutOfRange { month } => write!(f, "month {month} is not 1..=12"),
            Self::DayOutOfRange { day, month_len } => {
                write!(f, "day {day} is not 1..={month_len} in that month")
            }
            Self::NoNextDay => write!(f, "there is no day after {MAX_YEAR}-12-31"),
            Self::WindowRunsBackwards { from, to } => {
                write!(f, "window {from}..={to} runs backwards")
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// Whether a year is a leap year, by the Gregorian rule.
const fn is_leap(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// How many days a month holds, in that year.
///
/// A `match` rather than a table, so February's dependence on the year is in
/// the same expression as the answer and cannot be looked up without it.
const fn month_len(year: u32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // February, and every value outside `1..=12`. `Day::new` has already
        // refused those, so the arm is February's; folding them together avoids
        // an arm that no input can reach.
        _ => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
    }
}

/// One calendar date, in IST.
///
/// Fixed width, `Copy`, and validated on construction — `docs/07-o1-
/// architecture.md` layer 1. There is no constructor taking text: the vendor's
/// wire format is produced by [`std::fmt::Display`] and never parsed back, so a
/// date that was never validated is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[expect(
    clippy::struct_field_names,
    reason = "the lint fires because `day` repeats the type name `Day`. On a \
              calendar date that repetition is the correct naming: the field \
              *is* the day of the month, and `day_of_month` or `dom` would be \
              a longer or an obscurer spelling of the same thing. The three \
              fields are the three parts of an ISO date and are named after \
              them."
)]
pub struct Day {
    // Field order is the comparison order, and that is load-bearing: the
    // derived `Ord` compares year, then month, then day, which is calendar
    // order. A window comparison is therefore one derived comparison rather
    // than a hand-written one that could get the precedence wrong.
    year: u16,
    month: u8,
    day: u8,
}

impl Day {
    /// Builds a date, refusing one that does not exist.
    ///
    /// # Errors
    ///
    /// [`SessionError::YearOutOfRange`], [`SessionError::MonthOutOfRange`], or
    /// [`SessionError::DayOutOfRange`] naming how long that month actually is.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::session::{Day, SessionError};
    /// assert_eq!(Day::new(2024, 2, 29)?.to_string(), "2024-02-29");
    /// assert!(Day::new(2023, 2, 29).is_err(), "2023 is not a leap year");
    /// # Ok::<(), SessionError>(())
    /// ```
    pub const fn new(year: u16, month: u8, day: u8) -> Result<Self, SessionError> {
        // `year as u32` rather than `u32::from(year)`: `From` is not const-stable
        // on this toolchain, and this constructor must be callable from the
        // `const fn` calendar below. The cast is a widening one and cannot lose a
        // bit. `succ` widens the same way for the same reason.
        let wide = year as u32;
        if wide < MIN_YEAR || wide > MAX_YEAR {
            return Err(SessionError::YearOutOfRange { year: wide });
        }
        if month == 0 || month > 12 {
            return Err(SessionError::MonthOutOfRange { month });
        }
        let len = month_len(wide, month);
        if day == 0 || day > len {
            return Err(SessionError::DayOutOfRange {
                day,
                month_len: len,
            });
        }
        Ok(Self { year, month, day })
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month, `1..=12`.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of the month.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// The month this date falls in, as the store names months.
    ///
    /// Returns `store::path::YearMonth` rather than a pair, because that is the
    /// type a bar file's path and a manifest key are built from — a second
    /// spelling of "which month" would let a pull write into a month the store
    /// cannot address.
    ///
    /// # Errors
    ///
    /// Whatever `YearMonth::new` refuses. Unreachable through a validated
    /// [`Day`], whose bounds are the tighter of the two, and returned rather
    /// than swallowed because `crates/store` is not this crate's to watch:
    /// if that bound ever narrows, this is a refusal rather than a panic.
    pub const fn year_month(self) -> Result<YearMonth, PathError> {
        YearMonth::new(self.year, self.month)
    }

    /// The next calendar day.
    ///
    /// **This is what makes the vendor's non-inclusive `toDate` invisible to an
    /// operator.** See [`Window::wire_to`].
    ///
    /// # Errors
    ///
    /// [`SessionError::NoNextDay`] after 9999-12-31.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::session::{Day, SessionError};
    /// assert_eq!(Day::new(2024, 2, 28)?.succ()?, Day::new(2024, 2, 29)?);
    /// assert_eq!(Day::new(2024, 12, 31)?.succ()?, Day::new(2025, 1, 1)?);
    /// assert!(Day::new(9999, 12, 31)?.succ().is_err());
    /// # Ok::<(), SessionError>(())
    /// ```
    pub const fn succ(self) -> Result<Self, SessionError> {
        let wide = self.year as u32;
        if self.day < month_len(wide, self.month) {
            return Ok(Self {
                day: self.day + 1,
                ..self
            });
        }
        if self.month < 12 {
            return Ok(Self {
                month: self.month + 1,
                day: 1,
                ..self
            });
        }
        if wide >= MAX_YEAR {
            return Err(SessionError::NoNextDay);
        }
        Ok(Self {
            year: self.year + 1,
            month: 1,
            day: 1,
        })
    }

    /// Days since 1970-01-01, by the standard civil-calendar formula.
    ///
    /// Constant time and allocation-free. The era arithmetic is the published
    /// closed form; every intermediate is non-negative for the `1970..=9999`
    /// range a [`Day`] is validated into, which is why it is written in `u32`
    /// rather than in a signed type with shifts.
    #[must_use]
    pub const fn days_from_epoch(self) -> u32 {
        let y = self.year as u32;
        let m = self.month as u32;
        let d = self.day as u32;
        // March-based year: February's leap day becomes the last day of it, so
        // the month-length pattern is regular and needs no table.
        let y = if m <= 2 { y - 1 } else { y };
        let era = y / 400;
        let yoe = y - era * 400;
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// The date `days` days after 1970-01-01.
    ///
    /// The inverse of [`Day::days_from_epoch`], and
    /// `pull::fetch::the_calendar_round_trips_every_day_this_build_can_name`
    /// walks all 2,932,897 of them through both.
    ///
    /// # Errors
    ///
    /// [`SessionError::YearOutOfRange`] past 9999-12-31.
    pub const fn from_days(days: u32) -> Result<Self, SessionError> {
        let z = days + 719_468;
        let era = z / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        if y > MAX_YEAR {
            return Err(SessionError::YearOutOfRange { year: y });
        }
        // Every one of the three is inside its type by construction — the year
        // was just bounded, `mp` is at most 11 so `m` is `1..=12`, and `d` is
        // `1..=31`. The conversion is written as one refusal rather than three
        // because it has one cause: a day count this build cannot name. The
        // `y > MAX_YEAR` gate above is what makes it unreachable from here, and
        // the gate itself is reached by `from_days(4_000_000)`.
        // `TryFrom` is not const-stable on this toolchain and this is a
        // `const fn`, so the conversion is a cast guarded by the bound above
        // rather than by a `Result`. `Self::new` re-checks all three, so a cast
        // that was somehow wrong is refused there rather than believed.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "y was just refused above if it exceeds MAX_YEAR = 9,999, \
                      which fits u16; m is 1..=12 and d is 1..=31 by \
                      construction of the civil-date algorithm, both fitting \
                      u8. Self::new re-checks every one."
        )]
        let (year, month, day) = (y as u16, m as u8, d as u8);
        Self::new(year, month, day)
    }
}

impl fmt::Display for Day {
    /// `YYYY-MM-DD` — the shape both vendors take on the wire.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// One instant, as IST sees it: which day, and how far into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IstMoment {
    day: Day,
    minute_of_day: u32,
    second_of_minute: u32,
}

impl IstMoment {
    /// The IST moment of an epoch second.
    ///
    /// The vendor sends UTC epoch seconds; the exchange trades in IST. This is
    /// the one place the two are reconciled, and it is an add.
    ///
    /// # Errors
    ///
    /// [`SessionError::BeforeEpoch`] for a negative value, or
    /// [`SessionError::TimestampOutOfRange`] for one whose day count leaves
    /// `u32` — both of which are what a vendor column read at the wrong offset
    /// looks like.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::session::{IstMoment, SessionError};
    /// // 1326220200 is 18:30:00 UTC, which is 00:00:00 the next day in IST.
    /// // That is what a *daily* bar's timestamp looks like, and it is the
    /// // reason `Cadence::Daily` is exempt from the intraday window: midnight
    /// // is not inside 09:15..15:30, so an unconditional filter would drop
    /// // every daily bar ever fetched.
    /// let at = IstMoment::from_epoch_secs(1_326_220_200)?;
    /// assert_eq!(at.day().to_string(), "2012-01-11");
    /// assert_eq!(at.minute_of_day(), 0, "IST midnight — a daily stamp");
    ///
    /// // An intraday opening bar, for contrast: the same day at 09:15 IST is
    /// // 03:45 UTC, which is 33,300 seconds later.
    /// let open = IstMoment::from_epoch_secs(1_326_220_200 + 9 * 3_600 + 15 * 60)?;
    /// assert_eq!(open.minute_of_day(), 9 * 60 + 15);
    /// # Ok::<(), SessionError>(())
    /// ```
    pub const fn from_epoch_secs(epoch_secs: i64) -> Result<Self, SessionError> {
        let Some(local) = epoch_secs.checked_add(IST_OFFSET_SECS) else {
            return Err(SessionError::TimestampOutOfRange { secs: epoch_secs });
        };
        if local < 0 {
            return Err(SessionError::BeforeEpoch { secs: epoch_secs });
        }
        let day_number = local / SECS_PER_DAY;
        let into_day = local % SECS_PER_DAY;
        // Same const-fn constraint as `Day::from_days`. `local` was bounded
        // before the division, so `day_number` is below `u32::MAX`, and
        // `into_day` is a remainder modulo 86,400. `Day::from_days` refuses a
        // day count whose year exceeds 9,999, so an out-of-range day is caught
        // immediately below rather than trusted.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "local was range-checked before the division so \
                      day_number fits u32; into_day is a remainder mod 86,400. \
                      Day::from_days refuses anything past year 9,999."
        )]
        #[expect(
            clippy::cast_sign_loss,
            reason = "the `local < 0` guard above is what makes both \
                      non-negative: day_number is a truncating division of a \
                      non-negative value and into_day its remainder, so \
                      neither can be signed here. `a_negative_timestamp_is_\
                      refused_not_wrapped` is the test that holds the guard up."
        )]
        let (days, second_of_day) = (day_number as u32, into_day as u32);
        // A day count inside `u32` whose year is past 9999. Reported as an
        // out-of-range timestamp rather than as a year, because the caller
        // handed over a second and that is what it can act on.
        let Ok(day) = Day::from_days(days) else {
            return Err(SessionError::TimestampOutOfRange { secs: epoch_secs });
        };
        Ok(Self {
            day,
            minute_of_day: second_of_day / 60,
            second_of_minute: second_of_day % 60,
        })
    }

    /// The IST date.
    #[must_use]
    pub const fn day(self) -> Day {
        self.day
    }

    /// Minutes since IST midnight.
    #[must_use]
    pub const fn minute_of_day(self) -> u32 {
        self.minute_of_day
    }

    /// Seconds past that minute. Non-zero means the bar is not minute-aligned.
    #[must_use]
    pub const fn second_of_minute(self) -> u32 {
        self.second_of_minute
    }

    /// Whether this moment is inside the regular session, 09:15 inclusive to
    /// 15:30 exclusive.
    #[must_use]
    pub const fn in_regular_session(self) -> bool {
        self.minute_of_day >= SESSION_OPEN_MINUTE && self.minute_of_day < SESSION_CLOSE_MINUTE
    }
}

/// Which cadence a bar was fetched at.
///
/// An argument rather than an assumption: a daily bar carries no intraday time,
/// so the session bounds do not apply to it. See this module's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cadence {
    /// One bar per minute. The session filter applies.
    Minute,
    /// One bar per trading day. **Exempt** from the session filter.
    Daily,
}

/// Why one bar was not stored.
///
/// Each reason is counted separately because they say different things: a bar
/// before the session open is the vendor including the pre-open auction, and a
/// bar outside the requested window is the vendor ignoring the range — and an
/// operator who sees one total cannot tell which happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DropReason {
    /// Before 09:15 IST — the pre-open auction, which is not a bar.
    BeforeSessionOpen,
    /// At or after 15:30 IST. **At** is a drop: the close is exclusive.
    AtOrAfterSessionClose,
    /// The date is before the operator's window.
    BeforeWindow,
    /// The date is after the operator's window.
    ///
    /// This is the reason that fires when the `toDate + 1` sent on the wire
    /// brings back a bar from the extra day. The bar is dropped, counted, and
    /// visible — `CLAUDE.md` §4, degrade loudly and name the reason.
    AfterWindow,
}

impl DropReason {
    /// A short, stable label, for a report.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BeforeSessionOpen => "before the session open",
            Self::AtOrAfterSessionClose => "at or after the session close",
            Self::BeforeWindow => "before the requested window",
            Self::AfterWindow => "after the requested window",
        }
    }
}

impl fmt::Display for DropReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// How many bars were dropped, by reason.
///
/// Counters, not a log: `docs/07-o1-architecture.md` law 3. Answering "how many
/// did we discard" must be a read, and the reason must survive to the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct DropCensus {
    before_open: u32,
    after_close: u32,
    before_window: u32,
    after_window: u32,
}

impl DropCensus {
    /// A census of nothing dropped.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            before_open: 0,
            after_close: 0,
            before_window: 0,
            after_window: 0,
        }
    }

    /// Counts one drop. Saturating, because a census that wrapped would report
    /// a smaller number than the truth, which is the one direction a count must
    /// never be wrong in.
    pub const fn count(&mut self, reason: DropReason) {
        let slot = match reason {
            DropReason::BeforeSessionOpen => &mut self.before_open,
            DropReason::AtOrAfterSessionClose => &mut self.after_close,
            DropReason::BeforeWindow => &mut self.before_window,
            DropReason::AfterWindow => &mut self.after_window,
        };
        *slot = slot.saturating_add(1);
    }

    /// How many were dropped for one reason.
    #[must_use]
    pub const fn of(&self, reason: DropReason) -> u32 {
        match reason {
            DropReason::BeforeSessionOpen => self.before_open,
            DropReason::AtOrAfterSessionClose => self.after_close,
            DropReason::BeforeWindow => self.before_window,
            DropReason::AfterWindow => self.after_window,
        }
    }

    /// How many were dropped in total.
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.before_open
            .saturating_add(self.after_close)
            .saturating_add(self.before_window)
            .saturating_add(self.after_window)
    }

    /// Whether nothing was dropped.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// The operator's date range, **inclusive at both ends**.
///
/// Inclusive because that is what an operator means by "2022-01-08 to
/// 2022-02-08". The vendor's `toDate` is not inclusive, and reconciling the two
/// is [`Window::wire_to`] — the single most expensive silent bug available in
/// this crate, and the reason that method exists rather than a `+ 1` at a call
/// site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    from: Day,
    to: Day,
}

impl Window {
    /// A window from `from` to `to`, both included.
    ///
    /// # Errors
    ///
    /// [`SessionError::WindowRunsBackwards`] when `to` is before `from`. A
    /// one-day window (`from == to`) is legal and is the commonest resume
    /// shape.
    pub const fn new(from: Day, to: Day) -> Result<Self, SessionError> {
        // A hand-written comparison rather than `to < from`, because `Ord` is
        // not callable in a `const fn` — and the field order is the calendar
        // order, so this is the same comparison written out.
        let backwards = to.year < from.year
            || (to.year == from.year
                && (to.month < from.month || (to.month == from.month && to.day < from.day)));
        if backwards {
            return Err(SessionError::WindowRunsBackwards { from, to });
        }
        Ok(Self { from, to })
    }

    /// The first day the operator asked for.
    #[must_use]
    pub const fn from(self) -> Day {
        self.from
    }

    /// The last day the operator asked for.
    #[must_use]
    pub const fn to(self) -> Day {
        self.to
    }

    /// The `toDate` to put **on the wire**: the day after [`Window::to`].
    ///
    /// # The rule, and why it is a function
    ///
    /// **Dhan's `toDate` is not inclusive.** Its own documentation says so. A
    /// range an operator selects as inclusive must therefore be sent as
    /// `to + 1`, or every pull silently loses its last day — silently, because
    /// a short window looks exactly like a day the vendor had no data for, and
    /// the loss is one day per request rather than one day per pull.
    ///
    /// It is a method on the window rather than an addition at the call site
    /// for the reason `CLAUDE.md` §6 gives about a parameter: an adjustment
    /// that can be forgotten will be forgotten, and there is no symptom.
    /// `pull::fetch::the_wire_to_date_is_the_day_after_the_operators_last_day`
    /// and `pull::fetch::an_inclusive_window_survives_the_vendors_exclusive_
    /// to_date` are what hold it up.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoNextDay`] for a window ending on 9999-12-31.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::session::{Day, SessionError, Window};
    /// let window = Window::new(Day::new(2022, 1, 8)?, Day::new(2022, 2, 8)?)?;
    /// assert_eq!(window.to().to_string(), "2022-02-08");
    /// assert_eq!(window.wire_to()?.to_string(), "2022-02-09");
    /// # Ok::<(), SessionError>(())
    /// ```
    pub const fn wire_to(self) -> Result<Day, SessionError> {
        self.to.succ()
    }

    /// How many days the window spans, both ends included.
    ///
    /// Arithmetic on two day counts, so a one-day window and a ten-year one
    /// cost the same.
    #[must_use]
    pub const fn days(self) -> u32 {
        self.to.days_from_epoch() - self.from.days_from_epoch() + 1
    }

    /// Whether a date is inside the window.
    #[must_use]
    pub const fn contains(self, day: Day) -> bool {
        let after_start = day.days_from_epoch() >= self.from.days_from_epoch();
        after_start && day.days_from_epoch() <= self.to.days_from_epoch()
    }

    /// Whether one bar is kept, and if not, why.
    ///
    /// The order is the reason order: the window is checked before the session,
    /// so a bar from the extra day the wire `toDate` brings back is reported as
    /// *after the window* rather than as an out-of-session bar. Both are drops;
    /// only one of them tells an operator what actually happened.
    ///
    /// # Errors
    ///
    /// Whatever [`IstMoment::from_epoch_secs`] refuses. A timestamp that is not
    /// a timestamp is a refusal, never a drop: a drop is a bar this engine
    /// declined, and a bar it could not read is the vendor or the decoder being
    /// wrong.
    pub const fn verdict(
        self,
        epoch_secs: i64,
        cadence: Cadence,
    ) -> Result<Option<DropReason>, SessionError> {
        let at = match IstMoment::from_epoch_secs(epoch_secs) {
            Ok(at) => at,
            Err(e) => return Err(e),
        };
        let day = at.day().days_from_epoch();
        if day < self.from.days_from_epoch() {
            return Ok(Some(DropReason::BeforeWindow));
        }
        if day > self.to.days_from_epoch() {
            return Ok(Some(DropReason::AfterWindow));
        }
        // A DAILY BAR HAS NO INTRADAY TIME AND IS EXEMPT. See the module
        // header: vendors stamp it at midnight, at the open or at the close,
        // and an intraday window would drop every one of them.
        if matches!(cadence, Cadence::Daily) {
            return Ok(None);
        }
        if at.minute_of_day() < SESSION_OPEN_MINUTE {
            return Ok(Some(DropReason::BeforeSessionOpen));
        }
        if at.minute_of_day() >= SESSION_CLOSE_MINUTE {
            return Ok(Some(DropReason::AtOrAfterSessionClose));
        }
        Ok(None)
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..={}", self.from, self.to)
    }
}
