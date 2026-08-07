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
//! **An operator asserted 15:40. The exchange says 15:30.** The source is
//! `docs/00-charter.md` §3, which records the close, the 15:29 last bar and the
//! 375-bar count together, and records the weekend budget sessions as 375 bars
//! each *confirmed in the lake*. It is cited that way and no other way:
//! `CLAUDE.md` §3 rule 1 wants a claim about an exchange traceable to the
//! charter, and an earlier draft of this header instead claimed that NSE's
//! `marketStatus` endpoint had been read to confirm it — a live read recorded
//! in no document, which nobody reading this file can check. Ten phantom
//! minutes a day is not a rounding difference: every VWAP, every session-close
//! figure and every forced-exit bar would be computed over a window the
//! exchange never traded. The number is written **once**, here, and every
//! filter reads it from this module — the reason a constant repeated in three
//! filters is a constant that will one day disagree with itself.
//!
//! # A daily bar is exempt, and that is not a special case
//!
//! A daily bar has no intraday time. Vendors stamp it at midnight, or at the
//! open, or at the close, and none of those is inside 09:15–15:30 by
//! coincidence. Applying an intraday window to it would drop every daily bar
//! ever fetched, so [`Cadence`] is an argument to the filter rather than an
//! assumption inside it, and `pull::unit::a_daily_bar_is_exempt_from_the_
//! session_filter` is what holds it up.
//!
//! # What this module does **not** check, and says so
//!
//! **Minute alignment.** [`IstMoment::second_of_minute`] is computed and
//! exposed, and [`Window::verdict`] does not look at it: a bar stamped
//! 09:15:30 is inside the 09:15 minute and is kept. Whether a vendor stamping
//! a one-minute bar off the minute boundary is a fault is a *decoder*
//! question, not a window question, and inventing a [`DropReason`] for it here
//! would put a data-quality judgement inside a clock. `pull::unit::a_bar_that_
//! is_not_minute_aligned_is_kept_and_its_offset_is_visible` pins the current
//! behaviour so that changing it has to be deliberate.
//!
//! # What this module deliberately does **not** know
//!
//! **There is no trading calendar here.** It does not know a holiday, a Muhurat
//! session or a Saturday budget session, and it therefore never claims a bar is
//! on a non-trading date. `docs/00-charter.md` §3 records special-session shapes
//! and **no holiday list**, so a weekend rule would be *wrong* — it records
//! 2025-02-01 as a Saturday and a full 375-bar session, alongside 2020-02-01
//! (Sat) and 2026-02-01 (Sun). Nothing in this module computes a day of the
//! week; there is no `% 7` in it, and
//! `pull::unit::a_saturday_is_a_full_session_because_there_is_no_weekend_rule`
//! walks all seven days of one week and asserts the same 375 for each.
//!
//! Invariant `P-03` — "a bar on a non-trading date is dropped and counted" —
//! therefore stays `—` in `docs/04-invariants.md`, and is **not** advanced by
//! this module. That status was checked against the table rather than asserted.
//! An earlier draft of this header pointed at `docs/06-limits.md` §20 for the
//! explanation; that section does not exist — `docs/06-limits.md` runs §19 then
//! §21 — so the reason is written out here instead of cited to nothing.
//!
//! What is filtered here is the **window**: the operator's date range, and the
//! intraday session bounds.
//!
//! # Cost
//!
//! Every function here is arithmetic on two integers. There is no table, no
//! search and no allocation: [`Day::days_from_epoch`] and [`Day::from_days`]
//! are the standard closed-form civil-calendar pair (published as
//! `days_from_civil` and `civil_from_days`), so a timestamp in 1970 and one in
//! 9999 cost the same — `docs/07-o1-architecture.md` law 4, arithmetic beats
//! lookup. `pull::unit::the_calendar_round_trips_every_day_this_build_can_name`
//! drives every one of the 2,932,897 day counts through both directions and
//! through `succ`, so the pair is exhaustively checked rather than spot-checked.

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
const _: () = assert!(SECS_PER_DAY == 86_400);
// `IstMoment::from_epoch_secs` narrows this one to `u32`. The cast is exact
// only while it is 60, and this is what says so at compile time.
const _: () = assert!(SECS_PER_MINUTE == 60);
const _: () = assert!(SECS_PER_DAY % SECS_PER_MINUTE == 0);

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
    /// The timestamp names an instant before 1970-01-01 00:00 IST.
    ///
    /// **The bound is on the IST instant, not on the sign of the input**, and
    /// the two differ by exactly [`IST_OFFSET_SECS`]: `-19_800` is 1970-01-01
    /// 00:00:00 IST and is accepted, `-19_801` is the second before it and is
    /// refused. A negative epoch second is therefore *not* by itself a refusal
    /// — `-1` is 1970-01-01 05:29:59 IST, a date a [`Day`] can hold. Anything
    /// this variant does carry is genuinely before the Unix epoch as well, so
    /// the message it prints is true whenever it is printed.
    ///
    /// Refused rather than clamped: a clamped timestamp is a bar filed under
    /// 1970-01-01, which is a fault that looks like data.
    /// `pull::unit::the_before_epoch_boundary_is_the_ist_instant_not_the_sign`
    /// is what holds the boundary up on both sides.
    BeforeEpoch {
        /// The value that was refused.
        secs: i64,
    },
    /// The timestamp does not name a date this build can render.
    ///
    /// Reached three ways, all of which are the same fault to an operator —
    /// the column is not epoch seconds — so they are one variant: a value whose
    /// IST day count leaves `u32`, one whose day count overflows the calendar's
    /// epoch shift, and one whose year leaves `1970..=9999`.
    TimestampOutOfRange {
        /// The value that was refused.
        secs: i64,
    },
    /// A day count [`Day::from_days`] cannot name, because adding the civil
    /// calendar's epoch shift to it would leave `u32`.
    ///
    /// Its own variant rather than a [`SessionError::YearOutOfRange`] carrying
    /// a computed year, because there is no year to compute: the addition that
    /// would produce one is the addition that overflows. Before this refusal
    /// existed the add wrapped, and a wrapped day count is the worst shape a
    /// fault can take — `Day::from_days(u32::MAX)` **panicked in a debug build
    /// and returned `YearOutOfRange { year: 1969 }` in a release build**, which
    /// is a build-dependent answer to a pure function. `CLAUDE.md` §3 rule 5.
    DayCountOutOfRange {
        /// The day count that was refused.
        days: u32,
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
            Self::DayCountOutOfRange { days } => {
                write!(f, "day count {days} names no date this build can render")
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
    /// `pull::unit::the_calendar_round_trips_every_day_this_build_can_name`
    /// walks all 2,932,897 of them through both.
    ///
    /// # Errors
    ///
    /// [`SessionError::YearOutOfRange`] past 9999-12-31, and
    /// [`SessionError::DayCountOutOfRange`] for a count so large that the
    /// epoch shift below cannot even be applied to it.
    pub const fn from_days(days: u32) -> Result<Self, SessionError> {
        // CHECKED, NOT PLAIN. `days` is a `u32` and every one of them is a
        // legal argument, so `days + 719_468` overflows for the top 719,468 of
        // them — which used to panic in a debug build and wrap in a release
        // build, giving a pure function two different answers depending on how
        // it was compiled. See [`SessionError::DayCountOutOfRange`].
        let Some(z) = days.checked_add(719_468) else {
            return Err(SessionError::DayCountOutOfRange { days });
        };
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

/// The largest day count a [`Day`] can carry: 9999-12-31, the last date this
/// build will name.
///
/// **Derived, not written down.** The assertion below computes it from the
/// calendar itself at compile time, so the number and the two bounds it comes
/// from cannot drift apart — the same argument
/// [`BARS_PER_REGULAR_SESSION`] is pinned by. The day *count* is one less than
/// the number of nameable days, because 1970-01-01 is day zero: 2,932,896 here
/// and 2,932,897 dates in `0..=MAX_DAY_NUMBER`.
pub const MAX_DAY_NUMBER: u32 = 2_932_896;

const _: () = assert!(match Day::new(9999, 12, 31) {
    Ok(last) => last.days_from_epoch() == MAX_DAY_NUMBER,
    Err(_) => false,
});
const _: () = assert!(match Day::new(1970, 1, 1) {
    Ok(first) => first.days_from_epoch() == 0,
    Err(_) => false,
});

/// The largest day count `u32` can hold at all, as an `i64`.
///
/// Spelled as a literal because `i64::from` is not const-stable on this
/// toolchain and this is used from a `const fn`.
/// `pull::unit::the_day_count_ceiling_is_exactly_u32_max` is what proves the
/// literal is `u32::MAX` rather than a typo, since a compile-time assertion
/// would need the very cast the literal exists to avoid.
const U32_DAYS_MAX: i64 = 4_294_967_295;

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
    /// [`SessionError::BeforeEpoch`] for a value below `-19_800` — that is,
    /// one whose *IST* instant precedes 1970-01-01 00:00 IST, which is not the
    /// same set as "negative"; see that variant. Or
    /// [`SessionError::TimestampOutOfRange`] for one above `253_402_280_999`,
    /// whose IST day is past 9999-12-31 or whose day count does not fit `u32`
    /// at all. Both are what a vendor column read at the wrong offset looks
    /// like.
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
        // THE CAST BELOW IS ONLY SAFE BECAUSE OF THIS GUARD, AND IT WAS NOT
        // HERE. `local` is bounded only by `i64`, so `day_number` reaches
        // 106,751,991,167,300 and `as u32` silently kept its low 32 bits:
        // epoch second 371,086,500,594,600 is day 4,294,982,646, which is
        // 11761233-01-30, and it came back as `Ok(2012-01-11 00:00:00)`. Not a
        // refusal, not a panic, a *plausible date* — the one outcome
        // `CLAUDE.md` §4 forbids outright. The bound is `u32::MAX` rather than
        // [`MAX_DAY_NUMBER`] so that the calendar keeps its own refusal below
        // rather than having it pre-empted here into an arm no input reaches.
        //
        // THE ONE EQUIVALENT MUTANT IN THIS MODULE IS ON THIS LINE, and it is
        // recorded rather than papered over. `cargo mutants` replaces `>` with
        // `>=`; the whole suite still passes, and it always will, because the
        // single day count that distinguishes them — exactly `u32::MAX` — is
        // refused either way and with the identical error: by this guard under
        // `>=`, and by `Day::from_days`'s own `DayCountOutOfRange` (remapped
        // just below) under `>`. It is not a missing test. Every constant this
        // guard could take lies in `MAX_DAY_NUMBER + 1 ..= u32::MAX`, and both
        // sides of every one of them refuse, so no choice of bound removes it;
        // the only structure that would is guarding at `MAX_DAY_NUMBER`, which
        // makes the refusal below unreachable and costs the coverage gate a
        // line instead. `pull::unit::the_guard_and_the_calendar_refuse_the_
        // same_boundary_identically` asserts the equivalence as a fact rather
        // than leaving it as this comment's assertion.
        if day_number > U32_DAYS_MAX {
            return Err(SessionError::TimestampOutOfRange { secs: epoch_secs });
        }
        // Same const-fn constraint as `Day::from_days`.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "day_number was just compared against U32_DAYS_MAX and \
                      refused above it, so the cast is exact; into_day is a \
                      remainder mod 86,400. `the_day_count_that_does_not_fit_\
                      u32_is_refused_not_truncated` is the test that holds the \
                      guard up."
        )]
        #[expect(
            clippy::cast_sign_loss,
            reason = "the `local < 0` guard above is what makes both \
                      non-negative: day_number is a truncating division of a \
                      non-negative value and into_day its remainder, so \
                      neither can be signed here. `the_before_epoch_boundary_\
                      is_the_ist_instant_not_the_sign` is the test that holds \
                      the guard up."
        )]
        let (days, second_of_day) = (day_number as u32, into_day as u32);
        // A day count that fits `u32` but that the calendar still refuses —
        // either its year is past 9999, or the epoch shift will not fit. Both
        // are reported as an out-of-range *timestamp* rather than passed
        // through, because the caller handed over a second and a second is
        // what it can act on. Both arms are reachable from here: the first at
        // `epoch_secs = 253_402_281_000`, the second above
        // `4_294_247_827` days, which is inside the `u32` the guard admits.
        let Ok(day) = Day::from_days(days) else {
            return Err(SessionError::TimestampOutOfRange { secs: epoch_secs });
        };
        // `SECS_PER_MINUTE`, not a bare `60`. The module header's own argument
        // about a constant repeated in three filters applies to this one too,
        // and it was declared here and then not used.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "SECS_PER_MINUTE is the literal 60 widened to i64; the \
                      cast back is exact and the compile-time assertion below \
                      is what keeps it exact if the constant ever moves."
        )]
        let per_minute = SECS_PER_MINUTE as u32;
        Ok(Self {
            day,
            minute_of_day: second_of_day / per_minute,
            second_of_minute: second_of_day % per_minute,
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
    ///
    /// # The one place this is not the sum of its reasons
    ///
    /// Exactly the sum of the four counters while that sum fits `u32`, and
    /// **saturated at `u32::MAX` above it**, which is the one arithmetic in
    /// this file whose answer can be smaller than the truth. It is stated
    /// rather than hidden: reaching it needs more than 4.29 billion drops in
    /// one census, four hundred times a full day of index bars across every
    /// symbol NSE lists, so it is an honest bound rather than a live risk.
    /// `pull::unit::the_census_total_is_the_sum_of_its_reasons` proves the
    /// equality at zero, at one of each and at a mixed count;
    /// `session::tests::the_census_saturates_rather_than_wrapping` proves the
    /// saturation itself, and lives inside this module because the counters
    /// are private and no public call can reach `u32::MAX` in finite time.
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
    /// `pull::unit::the_wire_to_date_is_the_day_after_the_operators_last_day`
    /// and `pull::unit::an_inclusive_window_survives_the_vendors_exclusive_
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

/// The two facts about [`DropCensus`] that no external test can reach.
///
/// Everything else about this module is proved from outside, in
/// `crates/pull/tests/unit.rs`, because that is where a caller stands. These
/// two are here because the counters are private and the only public way to
/// raise one is [`DropCensus::count`], which would need 4,294,967,295 calls.
/// A saturating add nobody ever saturates is a claim, not a behaviour.
#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "the same exception every test module in this workspace takes: a \
              test that cannot panic cannot fail."
)]
mod tests {
    use super::{DropCensus, DropReason};

    /// A counter at the ceiling stops there. It never wraps to a number
    /// smaller than the truth, which is the one direction a census must never
    /// be wrong in.
    #[test]
    fn the_census_saturates_rather_than_wrapping() {
        let mut census = DropCensus {
            before_open: u32::MAX,
            after_close: 0,
            before_window: 0,
            after_window: 0,
        };
        census.count(DropReason::BeforeSessionOpen);
        assert_eq!(
            census.of(DropReason::BeforeSessionOpen),
            u32::MAX,
            "the counter wrapped to zero instead of standing still"
        );
        assert_eq!(census.total(), u32::MAX);
        assert!(!census.is_empty());

        // Every slot saturates, not just the first one.
        for reason in [
            DropReason::BeforeSessionOpen,
            DropReason::AtOrAfterSessionClose,
            DropReason::BeforeWindow,
            DropReason::AfterWindow,
        ] {
            let mut one = DropCensus::new();
            let slot = match reason {
                DropReason::BeforeSessionOpen => &mut one.before_open,
                DropReason::AtOrAfterSessionClose => &mut one.after_close,
                DropReason::BeforeWindow => &mut one.before_window,
                DropReason::AfterWindow => &mut one.after_window,
            };
            *slot = u32::MAX;
            one.count(reason);
            assert_eq!(one.of(reason), u32::MAX, "{reason} wrapped");
        }
    }

    /// `total` is the sum until it cannot be, and then it is the ceiling —
    /// never a wrapped, smaller number. This is the documented limit on
    /// [`DropCensus::total`], stated here as an executed fact.
    #[test]
    fn the_census_total_saturates_instead_of_wrapping_past_u32() {
        let census = DropCensus {
            before_open: u32::MAX,
            after_close: 1,
            before_window: 1,
            after_window: 1,
        };
        // The true sum is 4,294,967,298. It does not fit, so the answer is the
        // ceiling — and emphatically not the wrapped 2.
        assert_eq!(census.total(), u32::MAX);
        assert_eq!(census.of(DropReason::AtOrAfterSessionClose), 1);

        // Two large counters, neither of them at the ceiling alone.
        let half = DropCensus {
            before_open: 3_000_000_000,
            after_close: 2_000_000_000,
            before_window: 0,
            after_window: 0,
        };
        assert_eq!(half.total(), u32::MAX, "5e9 does not fit u32");

        // And the largest census whose total is still exact.
        let exact = DropCensus {
            before_open: u32::MAX - 3,
            after_close: 1,
            before_window: 1,
            after_window: 1,
        };
        assert_eq!(exact.total(), u32::MAX);
        assert_eq!(
            u64::from(exact.before_open)
                + u64::from(exact.after_close)
                + u64::from(exact.before_window)
                + u64::from(exact.after_window),
            u64::from(u32::MAX),
            "this case is the boundary: the true sum is exactly u32::MAX"
        );
    }
}
