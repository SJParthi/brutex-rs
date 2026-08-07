//! Expiry resolution: the next weekly and the next monthly, by arithmetic.
//!
//! # It does not scan a calendar
//!
//! Both laws are closed-form remainders, and this is the module the operator's
//! "must not scan a calendar" applies to, so it is stated precisely.
//!
//! * **Weekly.** `next = on + (target − weekday(on)) mod 7`. One remainder and
//!   one addition. It does not step forward day by day looking for a Thursday.
//! * **Monthly** — "the last `<weekday>` of the calendar month". The month's
//!   last day comes from [`TradeDay::last_of_its_month`], which starts at the
//!   28th — a real day of every month — and asks `core`'s calendar about the
//!   three longer lengths in turn; the walk back to the
//!   weekday is `(weekday(last) − target) mod 7` days, subtracted directly.
//!   If that day has already passed, the month rolls **once** and the same
//!   arithmetic runs again.
//!
//! So the cost is bounded by two compile-time constants and by nothing else:
//! at most **two** month resolutions, each of at most **three** calendar probes,
//! plus at most **two** table lookups of `dated::MAX_LATER_ROWS`
//! iterations. No input can raise any of those numbers. There is no loop over
//! days, weeks or months anywhere on the path.
//!
//! # What this does not know
//!
//! **Trading holidays.** When an expiry day falls on an exchange holiday the
//! contract settles on the previous trading day, and this module does not
//! account for that — exactly as the source does not. It is stated here rather
//! than left to be discovered: an expiry returned by this module is the
//! *calendar* expiry, and a holiday calendar is a separate, unbuilt thing.
//! `docs/06-limits.md` carries it.
//!
//! # Why the weekday regime is dated, and where it refuses
//!
//! It moved five times across the two swept indices between 2023 and 2025, and
//! once it was **withdrawn** outright: SEBI ended BANKNIFTY weekly expiries
//! after 2024-11-13. That withdrawal is a *value*, not a refusal — the table
//! knows it, cites it, and [`WeeklyRegime::Withdrawn`] says so. A refusal means
//! something entirely different: no evidence was retrieved. The two are
//! different variants of different types on purpose, because collapsing them
//! would let "there is no weekly" and "we do not know" print the same.
//!
//! The source's tables begin 2020-01-01. Before that there is no row, and
//! [`TradeDay`] reaches back to 1990, so 1990-01-01..2019-12-31 refuses.

use brutex_core::instrument::{Exchange, InstrumentKey};

use crate::dated::{DatedRow, DatedTable, boundary};
use crate::day::{TradeDay, Weekday};
use crate::error::{CostError, Refusal};
use crate::venue::SweptSlot;

/// How to close an expiry-weekday refusal.
const EXPIRY_REMEDIATION: &str = "source the NSE circular fixing the expiry weekday for the window \
     and add ONE dated row (effective date, weekday, citation) to the table in \
     crates/costs/src/expiry.rs, with a docs/05-decisions.md ledger entry — zero mechanism change";

/// The first day the shipped expiry weekdays are citation-grounded.
///
/// The source's tables begin here. Its 2020 rows are **secondary**-corroborated
/// — it records that the primary NSE circulars were unreachable (HTTP 403) and
/// that two independent research rounds agreed the Thursday regime was
/// continuous through 2020 for both NIFTY and BANKNIFTY. That caveat is carried
/// into the rows' own citations and into `docs/06-limits.md`; it is not
/// upgraded to a fact here.
pub const EXPIRY_VERIFIED_FROM: TradeDay = boundary!(2020 - 1 - 1);

/// Which day of a month the rolled regime is read on.
///
/// Fifteen — the source's own choice (`expiry_calendar.py`). It matters that it
/// is mid-month: a regime that took effect on the 1st and one that took effect
/// on the 20th would be read differently by the 1st or the last day, and the
/// source's answer is the one being ported.
const MONTHLY_REGIME_REFERENCE_DAY: u8 = 15;

/// The prose carried by every expiry-weekday refusal row.
const EXPIRY_GAP: &str = "UNVERIFIED — the expiry weekday before 2020-01-01. The source's tables \
     (`brutex/options/expiry_calendar.py`, TRACK2_OPTIONS_SPEC §2, citation-grounded 2026-05-16) \
     begin 2020-01-01, which is its own data-pull floor, and it records no evidence at all for \
     anything earlier. `TradeDay` reaches back to 1990 because `core`'s expiry calendar does, so \
     this window has no row.";

/// Whether a weekly expiry exists, and on which weekday.
///
/// [`Self::Withdrawn`] is a **verified fact with a citation**: SEBI ended
/// BANKNIFTY weekly expiries after 2024-11-13, and the table says so. It is not
/// a refusal, not a missing value and not an absence of evidence — those are
/// [`Refusal`], which this type has no variant for and cannot represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeeklyRegime {
    /// A weekly contract expires on this weekday.
    Expires(Weekday),
    /// No weekly contract exists — withdrawn by the regulator, and cited.
    Withdrawn,
}

impl core::fmt::Display for WeeklyRegime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Expires(weekday) => write!(f, "weekly on {weekday}"),
            Self::Withdrawn => f.write_str("no weekly expiry"),
        }
    }
}

/// The weekly-expiry weekday of each swept underlying, keyed on `core`'s order.
const WEEKLY: [DatedTable<WeeklyRegime>; SweptSlot::COUNT] = [
    DatedTable {
        subject: "weekly expiry weekday (NIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: EXPIRY_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, EXPIRY_GAP),
        later: [
            Some(DatedRow::verified(
                EXPIRY_VERIFIED_FROM,
                WeeklyRegime::Expires(Weekday::Thursday),
                "Thursday — a coverage extension of the same regime rather than a transition. \
                 NIFTY weekly options launched 2019-02-11 on Thursday and stayed Thursday until \
                 2025-09-02. SECONDARY-corroborated across two independent research rounds; the \
                 primary NSE circulars were unreachable (HTTP 403). TRACK2_OPTIONS_SPEC §2.",
            )),
            Some(DatedRow::verified(
                boundary!(2025 - 9 - 2),
                WeeklyRegime::Expires(Weekday::Tuesday),
                "Tuesday — NSE `FAOP/68747` under the SEBI expiry-standardisation of May 2025. \
                 The withdrawn Monday regime (announced March 2025 for 2025-04-04, withdrawn \
                 2025-03-27 before any Monday expiry traded) is deliberately NOT a row: no \
                 contract ever expired under it.",
            )),
            None,
            None,
            None,
        ],
    },
    DatedTable {
        subject: "weekly expiry weekday (BANKNIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: EXPIRY_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, EXPIRY_GAP),
        later: [
            Some(DatedRow::verified(
                EXPIRY_VERIFIED_FROM,
                WeeklyRegime::Expires(Weekday::Thursday),
                "Thursday — a coverage extension of the same regime rather than a transition. \
                 BANKNIFTY weekly options launched May 2016 on Thursday and first changed weekday \
                 2023-09-04. SECONDARY-corroborated across two independent research rounds; the \
                 primary NSE circulars were unreachable (HTTP 403). TRACK2_OPTIONS_SPEC §2.",
            )),
            Some(DatedRow::verified(
                boundary!(2023 - 9 - 4),
                WeeklyRegime::Expires(Weekday::Wednesday),
                "Wednesday — NSE `FAOP/57540`; the first Wednesday weekly expired 2023-09-06.",
            )),
            Some(DatedRow::verified(
                boundary!(2024 - 11 - 14),
                WeeklyRegime::Withdrawn,
                "WITHDRAWN — SEBI ended the BANKNIFTY weekly contract at the close of \
                 2024-11-13. This is a cited fact and not a citation gap: there is no weekly \
                 expiry, rather than an unknown one.",
            )),
            None,
            None,
        ],
    },
];

/// The monthly-expiry weekday of each swept underlying, keyed on `core`'s order.
///
/// A monthly contract always exists, so there is no withdrawal variant here —
/// the value is a plain [`Weekday`].
const MONTHLY: [DatedTable<Weekday>; SweptSlot::COUNT] = [
    DatedTable {
        subject: "monthly expiry weekday (NIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: EXPIRY_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, EXPIRY_GAP),
        later: [
            Some(DatedRow::verified(
                EXPIRY_VERIFIED_FROM,
                Weekday::Thursday,
                "last Thursday — NSE index monthly expiry was last-Thursday from the early 2000s \
                 until the 2023-2025 change series, with no monthly-weekday change documented \
                 near 2020. SECONDARY-corroborated across two independent research rounds. \
                 TRACK2_OPTIONS_SPEC §2.",
            )),
            Some(DatedRow::verified(
                boundary!(2025 - 9 - 2),
                Weekday::Tuesday,
                "last Tuesday — the SEBI expiry-standardisation of May 2025.",
            )),
            None,
            None,
            None,
        ],
    },
    DatedTable {
        subject: "monthly expiry weekday (BANKNIFTY)",
        exchange: Some(Exchange::Nse),
        remediation: EXPIRY_REMEDIATION,
        anchor: DatedRow::unverified(TradeDay::MIN, EXPIRY_GAP),
        later: [
            Some(DatedRow::verified(
                EXPIRY_VERIFIED_FROM,
                Weekday::Thursday,
                "last Thursday — the same basis as NIFTY monthly: continuous through 2020, first \
                 change 2024-03-01. SECONDARY-corroborated across two independent research \
                 rounds. TRACK2_OPTIONS_SPEC §2.",
            )),
            Some(DatedRow::verified(
                boundary!(2024 - 3 - 1),
                Weekday::Wednesday,
                "last Wednesday — NSE `FAOP/60011`; the first Wednesday monthly expired \
                 2024-03-27.",
            )),
            Some(DatedRow::verified(
                boundary!(2025 - 1 - 1),
                Weekday::Thursday,
                "last Thursday again — NSE `FAOP/65336`.",
            )),
            Some(DatedRow::verified(
                boundary!(2025 - 9 - 2),
                Weekday::Tuesday,
                "last Tuesday — the SEBI expiry-standardisation of May 2025.",
            )),
            None,
        ],
    },
];

// ---------------------------------------------------------------------------
// COMPILE-TIME structural proof, and the slot binding.
//
// A reordered SWEPT would give BANKNIFTY NIFTY's expiry weekday, which after
// 2023-09-04 is a whole day wrong on every contract. It stops compiling.
// ---------------------------------------------------------------------------
const _: () = assert!(WEEKLY[0].is_shipping_shape());
const _: () = assert!(WEEKLY[1].is_shipping_shape());
const _: () = assert!(MONTHLY[0].is_shipping_shape());
const _: () = assert!(MONTHLY[1].is_shipping_shape());
const _: () = assert!(crate::dated::str_eq(InstrumentKey::SWEPT[0].1, "NIFTY"));
const _: () = assert!(crate::dated::str_eq(InstrumentKey::SWEPT[1].1, "BANKNIFTY"));

/// The weekly-expiry regime in force for `slot` on `day`.
///
/// # Errors
///
/// [`Refusal`] for a day before [`EXPIRY_VERIFIED_FROM`]. A withdrawn weekly is
/// **not** an error — it is [`WeeklyRegime::Withdrawn`], a cited value.
///
/// # Examples
///
/// ```
/// use brutex_core::symbol::Symbol;
/// use costs::day::{TradeDay, Weekday};
/// use costs::expiry::{weekly_regime, WeeklyRegime};
/// use costs::venue::swept_slot;
///
/// let banknifty = swept_slot(Symbol::new("BANKNIFTY")?)?;
///
/// // The last day a BANKNIFTY weekly existed, and the first day it did not.
/// assert_eq!(
///     weekly_regime(banknifty, TradeDay::new(2024, 11, 13)?)?,
///     WeeklyRegime::Expires(Weekday::Wednesday),
/// );
/// assert_eq!(
///     weekly_regime(banknifty, TradeDay::new(2024, 11, 14)?)?,
///     WeeklyRegime::Withdrawn,
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
// The slot's inner index is produced only by `venue::swept_slot`, which returns
// an index into `InstrumentKey::SWEPT`, and the array has exactly that length by
// its own type. The access is in bounds by construction.
#[allow(clippy::indexing_slicing)]
pub fn weekly_regime(slot: SweptSlot, day: TradeDay) -> Result<WeeklyRegime, Refusal> {
    WEEKLY[slot.index()].value_on(day)
}

/// The weekday a monthly contract expires on for `slot` on `day`.
///
/// The answer is "the last `<this weekday>` of the month", not a date — see
/// [`next_monthly_expiry`] for the date.
///
/// # Errors
///
/// [`Refusal`] for a day before [`EXPIRY_VERIFIED_FROM`].
#[allow(clippy::indexing_slicing)]
pub fn monthly_regime(slot: SweptSlot, day: TradeDay) -> Result<Weekday, Refusal> {
    MONTHLY[slot.index()].value_on(day)
}

/// The next weekly expiry on or after `on`, or `None` if the contract does not
/// exist.
///
/// `None` means **withdrawn and cited** — see [`WeeklyRegime::Withdrawn`]. It
/// never means "unknown"; that is the [`Refusal`] path.
///
/// The result is the *calendar* expiry: trading holidays are not accounted for,
/// as the module documentation states.
///
/// # Errors
///
/// * [`CostError::Unverified`] for a day before [`EXPIRY_VERIFIED_FROM`].
/// * [`CostError::OrdinalOutsideWindow`] when the expiry would fall past
///   [`TradeDay::MAX`] — the last week of 2100 has no Thursday after it.
///
/// # Examples
///
/// ```
/// use brutex_core::symbol::Symbol;
/// use costs::day::TradeDay;
/// use costs::expiry::next_weekly_expiry;
/// use costs::venue::swept_slot;
///
/// let nifty = swept_slot(Symbol::new("NIFTY")?)?;
///
/// // 2025-09-01 is a Monday, still in the Thursday regime.
/// assert_eq!(
///     next_weekly_expiry(nifty, TradeDay::new(2025, 9, 1)?)?,
///     Some(TradeDay::new(2025, 9, 4)?),
/// );
/// // 2025-09-02 is the first day of the Tuesday regime, and is a Tuesday.
/// assert_eq!(
///     next_weekly_expiry(nifty, TradeDay::new(2025, 9, 2)?)?,
///     Some(TradeDay::new(2025, 9, 2)?),
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[allow(clippy::indexing_slicing)]
pub fn next_weekly_expiry(slot: SweptSlot, on: TradeDay) -> Result<Option<TradeDay>, CostError> {
    next_weekly_on(&WEEKLY[slot.index()], on)
}

/// The next monthly expiry on or after `on` — the last `<weekday>` of the month.
///
/// If this month's has already passed, the month rolls once and the regime is
/// **re-read** for the rolled month. That re-reading is what makes the
/// BANKNIFTY December-2024 to January-2025 rollover come out right: December's
/// is the last Wednesday and January's is the last Thursday, and a
/// single-lookup implementation would answer with December's weekday applied to
/// January.
///
/// The result is the *calendar* expiry: trading holidays are not accounted for.
///
/// # Errors
///
/// * [`CostError::Unverified`] for a day before [`EXPIRY_VERIFIED_FROM`], or —
///   reachable only for a table with a later refusal row — for the rolled
///   month.
/// * [`CostError::YearOutsideWindow`] when the roll would leave the
///   representable window: December 2100 has no January after it.
///
/// # Examples
///
/// ```
/// use brutex_core::symbol::Symbol;
/// use costs::day::TradeDay;
/// use costs::expiry::next_monthly_expiry;
/// use costs::venue::swept_slot;
///
/// let banknifty = swept_slot(Symbol::new("BANKNIFTY")?)?;
///
/// // The last Wednesday of December 2024, asked for on the day itself.
/// assert_eq!(
///     next_monthly_expiry(banknifty, TradeDay::new(2024, 12, 25)?)?,
///     TradeDay::new(2024, 12, 25)?,
/// );
/// // One day later it has passed, so January is next — and January 2025 is
/// // back on the last THURSDAY, which is the regime re-read.
/// assert_eq!(
///     next_monthly_expiry(banknifty, TradeDay::new(2024, 12, 26)?)?,
///     TradeDay::new(2025, 1, 30)?,
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[allow(clippy::indexing_slicing)]
pub fn next_monthly_expiry(slot: SweptSlot, on: TradeDay) -> Result<TradeDay, CostError> {
    next_monthly_on(&MONTHLY[slot.index()], on)
}

/// The weekly law, against any table.
///
/// Separate from the public entry point so the tests can drive it with a table
/// whose refusal row is not the anchor — a shape the shipped tables do not have
/// and which would otherwise leave an arm unreachable and unproven.
fn next_weekly_on(
    table: &DatedTable<WeeklyRegime>,
    on: TradeDay,
) -> Result<Option<TradeDay>, CostError> {
    match table.value_on(on)? {
        WeeklyRegime::Withdrawn => Ok(None),
        // One remainder, one addition. `days_until` is inclusive, so a Thursday
        // asked on a Thursday answers with itself.
        WeeklyRegime::Expires(target) => on
            .plus_days(i32::from(on.weekday().days_until(target)))
            .map(Some),
    }
}

/// The monthly law, against any table. Separate for the same reason.
fn next_monthly_on(table: &DatedTable<Weekday>, on: TradeDay) -> Result<TradeDay, CostError> {
    let this_month = table.value_on(on)?;
    let candidate = last_weekday_of_month(on, this_month);
    if !candidate.before(on) {
        return Ok(candidate);
    }
    // It has passed. Roll once — and re-read the regime for the rolled month,
    // because a transition can sit between the two.
    let (year, month) = on.next_month();
    let reference = TradeDay::new(year, month, MONTHLY_REGIME_REFERENCE_DAY)?;
    let next_month = table.value_on(reference)?;
    Ok(last_weekday_of_month(reference, next_month))
}

/// The last `weekday` of the month `any_day` falls in.
///
/// Total. The month's last day is at least the 28th and the walk back is at
/// most six days, so the answer is always inside the same month and there is
/// nothing that could be invalid — which is why this returns a day rather than
/// a `Result` with an arm no input could reach.
fn last_weekday_of_month(any_day: TradeDay, weekday: Weekday) -> TradeDay {
    let last = any_day.last_of_its_month();
    last.earlier_in_same_month(last.weekday().days_since(weekday))
}

#[cfg(test)]
// A money literal is written `rupees_paisa` — `24_012_00` reads as the twelve
// rupees over twenty-four thousand a circular or a screen would show, where
// `2_401_200` reads as nothing at all. The grouping is deliberate.
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::inconsistent_digit_grouping
)]
mod tests {
    use super::*;

    use brutex_core::symbol::Symbol;

    use crate::dated::{Dated, MAX_LATER_ROWS};
    use crate::venue::swept_slot;

    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    fn slot(text: &str) -> SweptSlot {
        swept_slot(Symbol::new(text).expect("a valid symbol")).expect("a swept underlying")
    }

    #[test]
    fn the_weekly_regime_boundaries_are_the_source_boundaries() {
        // Every transition, on the day and the day before it. The boundary is
        // inclusive: 2023-09-04 is the first day of the Wednesday regime.
        for (underlying, (y, m, d), on, before) in [
            (
                "NIFTY",
                (2025, 9, 2),
                WeeklyRegime::Expires(Weekday::Tuesday),
                WeeklyRegime::Expires(Weekday::Thursday),
            ),
            (
                "BANKNIFTY",
                (2023, 9, 4),
                WeeklyRegime::Expires(Weekday::Wednesday),
                WeeklyRegime::Expires(Weekday::Thursday),
            ),
            (
                "BANKNIFTY",
                (2024, 11, 14),
                WeeklyRegime::Withdrawn,
                WeeklyRegime::Expires(Weekday::Wednesday),
            ),
        ] {
            let subject = slot(underlying);
            let boundary = day(y, m, d);
            assert_eq!(weekly_regime(subject, boundary), Ok(on), "{underlying}");
            assert_eq!(
                weekly_regime(subject, boundary.plus_days(-1).expect("in window")),
                Ok(before),
                "{underlying} the day before"
            );
        }
        // The withdrawal is permanent for the rest of the window, and it is a
        // value rather than an error at every one of those days.
        assert_eq!(
            weekly_regime(slot("BANKNIFTY"), TradeDay::MAX),
            Ok(WeeklyRegime::Withdrawn)
        );
        assert_eq!(
            weekly_regime(slot("NIFTY"), TradeDay::MAX),
            Ok(WeeklyRegime::Expires(Weekday::Tuesday))
        );
    }

    #[test]
    fn the_monthly_regime_boundaries_are_the_source_boundaries() {
        for (underlying, (y, m, d), on, before) in [
            ("NIFTY", (2025, 9, 2), Weekday::Tuesday, Weekday::Thursday),
            (
                "BANKNIFTY",
                (2024, 3, 1),
                Weekday::Wednesday,
                Weekday::Thursday,
            ),
            (
                "BANKNIFTY",
                (2025, 1, 1),
                Weekday::Thursday,
                Weekday::Wednesday,
            ),
            (
                "BANKNIFTY",
                (2025, 9, 2),
                Weekday::Tuesday,
                Weekday::Thursday,
            ),
        ] {
            let subject = slot(underlying);
            let boundary = day(y, m, d);
            assert_eq!(monthly_regime(subject, boundary), Ok(on), "{underlying}");
            assert_eq!(
                monthly_regime(subject, boundary.plus_days(-1).expect("in window")),
                Ok(before),
                "{underlying} the day before"
            );
        }
    }

    #[test]
    fn the_next_weekly_lands_on_the_source_answers() {
        for (underlying, (y, m, d), want) in [
            // The Thursday regime's last week, then the Tuesday regime's first
            // day, which is itself a Tuesday.
            ("NIFTY", (2025, 9, 1), Some((2025, 9, 4))),
            ("NIFTY", (2025, 9, 2), Some((2025, 9, 2))),
            ("NIFTY", (2025, 9, 3), Some((2025, 9, 9))),
            // BANKNIFTY's last weekly, then the withdrawal.
            ("BANKNIFTY", (2024, 11, 13), Some((2024, 11, 13))),
            ("BANKNIFTY", (2024, 11, 14), None),
            ("BANKNIFTY", (2025, 6, 1), None),
            // The Thursday-to-Wednesday flip, either side.
            ("BANKNIFTY", (2023, 9, 3), Some((2023, 9, 7))),
            ("BANKNIFTY", (2023, 9, 4), Some((2023, 9, 6))),
        ] {
            let got = next_weekly_expiry(slot(underlying), day(y, m, d))
                .expect("inside the verified window");
            assert_eq!(
                got,
                want.map(|(wy, wm, wd)| day(wy, wm, wd)),
                "{underlying} on {y}-{m}-{d}"
            );
        }
    }

    #[test]
    fn the_next_weekly_is_the_first_day_on_or_after_with_the_regimes_weekday() {
        // A brute-force differential across the whole verified window, on both
        // underlyings: scan forward day by day for the regime's weekday and
        // check the arithmetic found the same day. This is the assertion that
        // the closed form IS the scan, without being one.
        //
        // The domain is walked with `every_representable_day`, whose trip count
        // is 111 years x 12 months x 31 days by construction, and every inner
        // scan is bounded to a week by an assertion. Nothing here is driven by
        // an ordinal, so a broken ordinal fails an assertion rather than
        // spinning — a hung suite is a worse signal than a failed test.
        assert_eq!(
            TradeDay::MIN.ordinal(),
            7_305,
            "the domain must be the domain"
        );
        assert_eq!(TradeDay::MAX.ordinal(), 47_846);
        let last_scannable = TradeDay::MAX.ordinal() - 7;
        let mut checked = 0u32;
        let mut withdrawn = 0u32;
        for underlying in ["NIFTY", "BANKNIFTY"] {
            let subject = slot(underlying);
            for today in crate::day::every_representable_day() {
                // Start at the verified window, and stop a week short of MAX so
                // the reference scan itself never runs off the end.
                if today.before(EXPIRY_VERIFIED_FROM) || today.ordinal() > last_scannable {
                    continue;
                }
                match weekly_regime(subject, today).expect("inside the window") {
                    WeeklyRegime::Withdrawn => {
                        assert_eq!(next_weekly_expiry(subject, today), Ok(None));
                        withdrawn += 1;
                    }
                    WeeklyRegime::Expires(target) => {
                        let got = next_weekly_expiry(subject, today)
                            .expect("in window")
                            .expect("not withdrawn");
                        let mut want = today;
                        let mut walked = 0u8;
                        while want.weekday() != target {
                            want = want.plus_days(1).expect("within a week of today");
                            walked += 1;
                            assert!(walked <= 6, "no {target} within a week of {today}");
                        }
                        assert_eq!(got, want, "{underlying} on {today}");
                        assert_eq!(got.ordinal() - today.ordinal(), i32::from(walked));
                        assert!(!got.before(today), "an expiry cannot be in the past");
                    }
                }
                checked += 1;
            }
        }
        // 2020-01-01 .. 2100-12-24 inclusive, twice.
        assert_eq!(checked, 2 * (47_839 - 18_262 + 1));
        assert!(withdrawn > 0, "the withdrawal window must be exercised");
    }

    #[test]
    fn the_next_monthly_lands_on_the_source_answers() {
        for (underlying, (y, m, d), (wy, wm, wd)) in [
            // August 2025's last Thursday has passed, so it rolls into
            // September — which is the TUESDAY regime, not August's Thursday.
            ("NIFTY", (2025, 8, 29), (2025, 9, 30)),
            ("NIFTY", (2025, 9, 2), (2025, 9, 30)),
            // The BANKNIFTY year-boundary rollover: December 2024 is the last
            // Wednesday, January 2025 is back to the last Thursday.
            ("BANKNIFTY", (2024, 12, 25), (2024, 12, 25)),
            ("BANKNIFTY", (2024, 12, 26), (2025, 1, 30)),
            ("BANKNIFTY", (2024, 12, 31), (2025, 1, 30)),
            // A leap-day expiry, asked for on the day.
            ("BANKNIFTY", (2024, 2, 29), (2024, 2, 29)),
            // The first day of the Wednesday monthly regime.
            ("BANKNIFTY", (2024, 3, 1), (2024, 3, 27)),
            // The start of the verified window, and the roll out of January.
            ("NIFTY", (2020, 1, 1), (2020, 1, 30)),
            ("NIFTY", (2020, 1, 31), (2020, 2, 27)),
        ] {
            assert_eq!(
                next_monthly_expiry(slot(underlying), day(y, m, d)),
                Ok(day(wy, wm, wd)),
                "{underlying} on {y}-{m}-{d}"
            );
        }
    }

    #[test]
    fn the_next_monthly_is_the_last_regime_weekday_of_its_own_or_the_next_month() {
        // A differential over every day of the verified window: the answer is
        // on or after the day asked, it carries the weekday its own month's
        // regime names, and no LATER day with that weekday exists in its month.
        //
        // Walked with `every_representable_day`, whose trip count is fixed by
        // construction rather than by an ordinal, for the reason the weekly
        // differential above gives.
        assert_eq!(
            TradeDay::MIN.ordinal(),
            7_305,
            "the domain must be the domain"
        );
        assert_eq!(TradeDay::MAX.ordinal(), 47_846);
        let last_december = day(2100, 12, 1);
        let mut checked = 0u32;
        let mut rolled = 0u32;
        for underlying in ["NIFTY", "BANKNIFTY"] {
            let subject = slot(underlying);
            for today in crate::day::every_representable_day() {
                // Start at the verified window, and stop before December 2100,
                // whose roll leaves the window — that refusal has its own test.
                if today.before(EXPIRY_VERIFIED_FROM) || !today.before(last_december) {
                    continue;
                }
                let got = next_monthly_expiry(subject, today).expect("inside the window");
                assert!(!got.before(today), "{underlying} answered in the past");
                // The regime is read at the day asked, or — if the month
                // rolled — at the 15th of the rolled month. That reference rule
                // is re-derived here rather than taken from the implementation;
                // the arithmetic that follows from it is what is being checked.
                let reference = if got.month() == today.month() {
                    today
                } else {
                    assert_eq!(
                        (got.year(), got.month()),
                        today.next_month(),
                        "the roll must be exactly one month"
                    );
                    day(got.year(), got.month(), MONTHLY_REGIME_REFERENCE_DAY)
                };
                let regime = monthly_regime(subject, reference).expect("in window");
                assert_eq!(got.weekday(), regime, "{underlying} on {today}");
                // Nothing later in the answer's own month shares that weekday.
                let last = got.last_of_its_month();
                assert!(
                    last.ordinal() - got.ordinal() < 7,
                    "{got} is not the LAST {regime} of its month"
                );
                if got.month() != today.month() {
                    rolled += 1;
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 2 * (47_816 - 18_262));
        assert!(rolled > 0, "the rollover arm must be exercised");
    }

    #[test]
    fn the_regime_is_re_read_for_the_rolled_month_and_not_carried_across() {
        // The single assertion that separates a correct rollover from a naive
        // one. December 2024's BANKNIFTY monthly is the last WEDNESDAY; asked
        // on 2024-12-26 the answer must be January's last THURSDAY, not
        // January's last Wednesday (2025-01-29).
        let banknifty = slot("BANKNIFTY");
        assert_eq!(
            monthly_regime(banknifty, day(2024, 12, 26)),
            Ok(Weekday::Wednesday)
        );
        assert_eq!(
            monthly_regime(banknifty, day(2025, 1, 15)),
            Ok(Weekday::Thursday)
        );
        assert_eq!(
            next_monthly_expiry(banknifty, day(2024, 12, 26)),
            Ok(day(2025, 1, 30))
        );
        assert_ne!(
            next_monthly_expiry(banknifty, day(2024, 12, 26)),
            Ok(day(2025, 1, 29)),
            "carrying December's weekday into January is the bug this catches"
        );
        // And the same across the NIFTY August-to-September 2025 boundary,
        // where the flip is Thursday to Tuesday mid-week.
        let nifty = slot("NIFTY");
        assert_eq!(
            monthly_regime(nifty, day(2025, 8, 29)),
            Ok(Weekday::Thursday)
        );
        assert_eq!(
            monthly_regime(nifty, day(2025, 9, 15)),
            Ok(Weekday::Tuesday)
        );
        assert_eq!(
            next_monthly_expiry(nifty, day(2025, 8, 29)),
            Ok(day(2025, 9, 30))
        );
        assert_eq!(MONTHLY_REGIME_REFERENCE_DAY, 15);
    }

    #[test]
    fn the_last_weekday_of_a_month_is_the_last_one_in_it() {
        // Spot checks against dates anyone can look up.
        for ((y, m, d), weekday, (wy, wm, wd)) in [
            ((2020, 1, 15), Weekday::Thursday, (2020, 1, 30)),
            ((2024, 12, 1), Weekday::Wednesday, (2024, 12, 25)),
            ((2025, 1, 1), Weekday::Thursday, (2025, 1, 30)),
            ((2024, 2, 1), Weekday::Thursday, (2024, 2, 29)), // leap February
            ((2025, 9, 1), Weekday::Tuesday, (2025, 9, 30)),
        ] {
            assert_eq!(
                last_weekday_of_month(day(y, m, d), weekday),
                day(wy, wm, wd),
                "last {weekday} of {y}-{m}"
            );
        }
        // The law, over every month of the window and every weekday: the answer
        // is in the same month, carries that weekday, and nothing later in the
        // month does.
        let mut checked = 0u32;
        for year in 1990u16..=2100 {
            for month in 1u8..=12 {
                let any = day(year, month, 1);
                let last = any.last_of_its_month();
                for weekday in Weekday::ALL {
                    let got = last_weekday_of_month(any, weekday);
                    assert_eq!((got.year(), got.month()), (year, month));
                    assert_eq!(got.weekday(), weekday);
                    assert!(last.ordinal() - got.ordinal() < 7);
                    assert!(!got.before(any));
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 111 * 12 * 7);
    }

    #[test]
    fn the_pre_history_window_refuses_on_both_tables_and_every_day_of_it() {
        let mut refused = 0u32;
        for underlying in ["NIFTY", "BANKNIFTY"] {
            let subject = slot(underlying);
            for today in crate::day::every_representable_day() {
                if !today.before(EXPIRY_VERIFIED_FROM) {
                    assert!(weekly_regime(subject, today).is_ok());
                    assert!(monthly_regime(subject, today).is_ok());
                    continue;
                }
                for refusal in [
                    weekly_regime(subject, today).expect_err("pre-history"),
                    monthly_regime(subject, today).expect_err("pre-history"),
                ] {
                    assert_eq!(refusal.window_start(), TradeDay::MIN);
                    assert_eq!(refusal.verified_from(), Some(EXPIRY_VERIFIED_FROM));
                    assert_eq!(refusal.remediation(), EXPIRY_REMEDIATION);
                    assert!(refusal.to_string().contains(underlying));
                    refused += 1;
                }
                // And both date functions carry the SAME refusal out, word for
                // word — not a second, vaguer one composed on the way past.
                assert_eq!(
                    next_weekly_expiry(subject, today),
                    Err(CostError::Unverified(
                        weekly_regime(subject, today).expect_err("pre-history")
                    ))
                );
                assert_eq!(
                    next_monthly_expiry(subject, today),
                    Err(CostError::Unverified(
                        monthly_regime(subject, today).expect_err("pre-history")
                    ))
                );
            }
        }
        // 1990-01-01 .. 2019-12-31 is 10,957 days, twice over, on two tables.
        assert_eq!(refused, 2 * 2 * 10_957);
        assert_eq!(EXPIRY_VERIFIED_FROM, day(2020, 1, 1));
    }

    #[test]
    fn an_expiry_past_the_end_of_the_window_is_refused_rather_than_wrapped() {
        // 2100-12-31 is a Friday. NIFTY's weekly regime is Tuesday by then, so
        // the next one is four days later — outside the representable window.
        assert_eq!(TradeDay::MAX.weekday(), Weekday::Friday);
        assert_eq!(Weekday::Friday.days_until(Weekday::Tuesday), 4);
        assert_eq!(
            next_weekly_expiry(slot("NIFTY"), TradeDay::MAX),
            Err(CostError::OrdinalOutsideWindow {
                ordinal: TradeDay::MAX.ordinal() + 4
            })
        );
        // The monthly roll out of December 2100 needs January 2101.
        let after_december_expiry = day(2100, 12, 29);
        assert_eq!(
            next_monthly_expiry(slot("NIFTY"), after_december_expiry),
            Err(CostError::YearOutsideWindow { year: 2101 })
        );
        // December 2100's own last Tuesday is 2100-12-28, so asking on or
        // before it still answers — the refusal is the boundary, not the month.
        assert_eq!(
            next_monthly_expiry(slot("NIFTY"), day(2100, 12, 28)),
            Ok(day(2100, 12, 28))
        );
        // BANKNIFTY's weekly is withdrawn by then, so it answers None rather
        // than running off the end at all.
        assert_eq!(
            next_weekly_expiry(slot("BANKNIFTY"), TradeDay::MAX),
            Ok(None)
        );
    }

    #[test]
    fn a_refusal_in_the_rolled_month_is_carried_out_rather_than_guessed() {
        // The shipped tables have their only refusal at the anchor, so the
        // rolled-month lookup can never refuse on them. A table with a LATER
        // refusal row can, and a caller must get the refusal rather than
        // December's weekday applied to an unverified January.
        let table = DatedTable::<Weekday> {
            subject: "a monthly regime with a hole in it",
            exchange: Some(Exchange::Nse),
            remediation: "fill the hole",
            anchor: DatedRow::verified(TradeDay::MIN, Weekday::Thursday, "the anchor"),
            later: [
                Some(DatedRow::unverified(day(2050, 2, 1), "the hole")),
                None,
                None,
                None,
                None,
            ],
        };
        assert!(table.is_shipping_shape());
        // January 2050's last Thursday is the 27th, so the 28th rolls into
        // February — which has no verified weekday.
        assert_eq!(
            next_monthly_on(&table, day(2050, 1, 27)),
            Ok(day(2050, 1, 27))
        );
        let refusal = next_monthly_on(&table, day(2050, 1, 28)).expect_err("February is a hole");
        // The refusal is the table's own, unchanged, and it names the day the
        // rolled month was read on rather than the day the caller asked about.
        assert_eq!(
            refusal,
            CostError::Unverified(
                table
                    .value_on(day(2050, 2, MONTHLY_REGIME_REFERENCE_DAY))
                    .expect_err("February is a hole")
            )
        );
        let rendered = refusal.to_string();
        assert!(rendered.contains("the hole"), "{rendered}");
        assert!(rendered.contains("2050-02-15"), "{rendered}");
    }

    #[test]
    fn every_shipped_expiry_row_is_a_trading_weekday_and_keyed_on_cores_order() {
        assert_eq!(WEEKLY.len(), SweptSlot::COUNT);
        assert_eq!(MONTHLY.len(), SweptSlot::COUNT);
        let mut weekly_rows = 0u32;
        let mut monthly_rows = 0u32;
        let mut withdrawals = 0u32;
        for (index, table) in WEEKLY.iter().enumerate() {
            assert!(table.is_shipping_shape(), "weekly table {index}");
            assert!(table.subject.contains(InstrumentKey::SWEPT[index].1));
            assert!(table.rows().count() <= MAX_LATER_ROWS + 1);
            for row in table.rows() {
                match row.value {
                    Dated::Verified(WeeklyRegime::Expires(weekday)) => {
                        assert!(
                            weekday.is_trading_weekday(),
                            "no Indian exchange expires on a {weekday}"
                        );
                        weekly_rows += 1;
                    }
                    Dated::Verified(WeeklyRegime::Withdrawn) => withdrawals += 1,
                    Dated::Unverified => assert!(row.start.same_day(TradeDay::MIN)),
                }
            }
        }
        for (index, table) in MONTHLY.iter().enumerate() {
            assert!(table.is_shipping_shape(), "monthly table {index}");
            assert!(table.subject.contains(InstrumentKey::SWEPT[index].1));
            for row in table.rows() {
                match row.value {
                    Dated::Verified(weekday) => {
                        assert!(weekday.is_trading_weekday());
                        monthly_rows += 1;
                    }
                    Dated::Unverified => assert!(row.start.same_day(TradeDay::MIN)),
                }
            }
        }
        assert_eq!(weekly_rows, 4, "two NIFTY rows and two live BANKNIFTY rows");
        assert_eq!(withdrawals, 1, "exactly one contract was withdrawn");
        assert_eq!(monthly_rows, 6, "two NIFTY rows and four BANKNIFTY rows");
    }

    #[test]
    fn a_withdrawn_weekly_and_a_refusal_never_read_the_same() {
        // The distinction the whole type exists for: "there is no weekly" and
        // "we do not know" must not print the same, and must not be the same
        // shape in the type system either.
        let banknifty = slot("BANKNIFTY");
        let withdrawn = weekly_regime(banknifty, day(2025, 1, 1)).expect("a cited value");
        assert_eq!(withdrawn, WeeklyRegime::Withdrawn);
        assert_eq!(withdrawn.to_string(), "no weekly expiry");
        assert_eq!(
            WeeklyRegime::Expires(Weekday::Thursday).to_string(),
            "weekly on Thu"
        );
        let unknown = weekly_regime(banknifty, day(2019, 12, 31)).expect_err("no evidence");
        assert!(unknown.to_string().contains("UNVERIFIED"));
        assert!(!unknown.to_string().contains("no weekly expiry"));
        assert_ne!(
            WeeklyRegime::Expires(Weekday::Thursday),
            WeeklyRegime::Withdrawn
        );
        assert!(WeeklyRegime::Expires(Weekday::Sunday) < WeeklyRegime::Withdrawn);
        assert_eq!(format!("{:?}", WeeklyRegime::Withdrawn), "Withdrawn");
    }

    #[test]
    fn the_two_underlyings_expire_on_different_days_and_that_is_the_point() {
        // Between 2023-09-04 and 2024-11-13 the two swept indices had DIFFERENT
        // weekly expiry weekdays. One shared calendar would put every
        // BANKNIFTY weekly a day late for fourteen months.
        let on = day(2024, 6, 3); // a Monday
        assert_eq!(
            next_weekly_expiry(slot("NIFTY"), on),
            Ok(Some(day(2024, 6, 6))),
            "NIFTY expired on the Thursday"
        );
        assert_eq!(
            next_weekly_expiry(slot("BANKNIFTY"), on),
            Ok(Some(day(2024, 6, 5))),
            "BANKNIFTY expired on the Wednesday"
        );
        // And the monthlies differ too: March 2024 is BANKNIFTY's first
        // Wednesday monthly while NIFTY is still on Thursday.
        assert_eq!(
            next_monthly_expiry(slot("NIFTY"), day(2024, 3, 1)),
            Ok(day(2024, 3, 28))
        );
        assert_eq!(
            next_monthly_expiry(slot("BANKNIFTY"), day(2024, 3, 1)),
            Ok(day(2024, 3, 27))
        );
    }
}
