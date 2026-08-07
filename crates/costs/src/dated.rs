//! One dated table, four subjects, and the same refusal contract as a rate.
//!
//! [`crate::regime`] holds a dated table specialised to a statutory rate. Stage
//! two needs three more dated things — the options lot size, the expiry
//! weekday, and the strike-grid step — and every one of them has the same
//! shape: a first day, a value or the honest absence of one, and a citation.
//!
//! So the mechanism is written once, here, generic over the value.
//!
//! # The refusal contract, unchanged
//!
//! [`Dated::Unverified`] **has no field**. There is nowhere in the type for a
//! fabricated lot size, weekday or step to live, no `Option` to unwrap and no
//! default to fall back on, exactly as [`crate::regime::Rate`] arranges it for
//! a rate. A [`DatedTable`] is crate-private and every instance is a `const`
//! item this crate owns, so a caller cannot supply a substitute table either.
//!
//! # Why [`crate::regime`] was not migrated onto this
//!
//! It could have been, and it was deliberately not. `regime::RegimeTable`'s
//! compile-time proof is an **exhaustive match on a two-slot array shape** —
//! `[None, None]`, `[Some(_), None]`, `[Some(_), Some(_)]`, `[None, Some(_)]` —
//! which is what makes the "table has a hole" case a pattern the compiler
//! checks rather than a loop condition a test has to reach. That shape does not
//! survive being widened to five slots, and rewriting the mechanism the whole
//! refusal contract rests on is not something a port of the option arithmetic
//! has the authority to do. This table proves the same four properties with a
//! `while` loop instead, and its false arms are reached by tests that build
//! broken tables on purpose.
//!
//! # Constant per-operation cost
//!
//! A lookup runs over `[Option<DatedRow<V>>; MAX_LATER_ROWS]`, a fixed-size
//! array whose length is a compile-time constant. The trip count is
//! [`MAX_LATER_ROWS`] for **every** table and **every** date; no argument can
//! change it, because the tables are `const` items fixed before any input
//! exists. It is not a bisect and there is no `N` to grow.

use brutex_core::instrument::Exchange;

use crate::day::TradeDay;
use crate::error::Refusal;

/// How many rows a dated table may carry **after** its anchor row.
///
/// Five, which is the length of the longest shipped table (the lot-size
/// history of either swept underlying: one refusal anchor and five dated
/// rows). It is the length of a fixed-size array, so it bounds every lookup's
/// loop at compile time and no input can raise it.
pub(crate) const MAX_LATER_ROWS: usize = 5;

/// How many rows a dated table may carry in total, anchor included.
///
/// Test-only: the shipped code never counts rows, it walks a fixed-size array.
#[cfg(test)]
pub(crate) const MAX_ROWS: usize = MAX_LATER_ROWS + 1;

/// What a dated row knows about its value.
///
/// The two variants are not symmetric, and that asymmetry is the whole point:
/// [`Self::Verified`] carries a value and [`Self::Unverified`] carries
/// **nothing**. The citation gap lives in the row's `source` string, as prose,
/// and never as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Dated<V> {
    /// A citation-grounded value.
    Verified(V),
    /// No value exists for this window, and none will be invented for it.
    Unverified,
}

/// One dated row: the day it takes effect, its value or its absence, and its
/// citation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DatedRow<V> {
    /// The first day this row is in force, inclusive.
    pub(crate) start: TradeDay,
    /// The value, or the refusal.
    pub(crate) value: Dated<V>,
    /// The citation for a verified row; for a refusal row, what was identified
    /// and what was never retrieved. Never blank — asserted at compile time.
    pub(crate) source: &'static str,
}

impl<V> DatedRow<V> {
    /// A row that carries a value and the citation it came from.
    pub(crate) const fn verified(start: TradeDay, value: V, source: &'static str) -> Self {
        Self {
            start,
            value: Dated::Verified(value),
            source,
        }
    }

    /// A row that carries no value, and the honest account of why.
    pub(crate) const fn unverified(start: TradeDay, source: &'static str) -> Self {
        Self {
            start,
            value: Dated::Unverified,
            source,
        }
    }
}

/// A dated table for one subject.
///
/// Crate-private, and deliberately: this type is the mechanism the refusal
/// contract rests on. If a caller could build one, a caller could build one
/// that dates a 2019 trade at the 2026 lot size, and the guarantee would be a
/// comment again.
pub(crate) struct DatedTable<V> {
    /// The human name of the subject, carried into a refusal.
    pub(crate) subject: &'static str,
    /// The venue, when the subject is exchange-scoped.
    pub(crate) exchange: Option<Exchange>,
    /// The one action that would replace a refusal from this table with a
    /// citation. Per table, because each table lives in its own file.
    pub(crate) remediation: &'static str,
    /// The row in force from [`TradeDay::MIN`]. A field rather than an array
    /// element, which is what makes "empty table" and "before the table"
    /// unrepresentable rather than branches nothing can cover.
    pub(crate) anchor: DatedRow<V>,
    /// Later rows in strictly ascending order, `None`-padded at the end.
    pub(crate) later: [Option<DatedRow<V>>; MAX_LATER_ROWS],
}

impl<V: Copy> DatedTable<V> {
    /// The value in force on `day`, or the refusal that stands in its place.
    ///
    /// Exactly [`MAX_LATER_ROWS`] iterations, for every table and every date.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the row in force carries no verified value. The refusal
    /// names the window, the citation gap and the remedy.
    pub(crate) fn value_on(&self, day: TradeDay) -> Result<V, Refusal> {
        // Start on the anchor: it begins at TradeDay::MIN, so it is in force
        // unless a later row displaces it. There is no "not found" case.
        let mut selected = &self.anchor;
        let mut verified_from = None;
        for row in self.later.iter().flatten() {
            if row.start <= day {
                selected = row;
                // A later row has displaced the one whose successor this was.
                verified_from = None;
            } else if verified_from.is_none() {
                // Rows ascend, so the first row that has not started yet is the
                // successor of whichever row is in force.
                verified_from = Some(row.start);
            }
        }
        match selected.value {
            Dated::Verified(value) => Ok(value),
            Dated::Unverified => Err(Refusal::with_remediation(
                self.subject,
                self.exchange,
                day,
                selected.start,
                verified_from,
                selected.source,
                self.remediation,
            )),
        }
    }
}

impl<V> DatedTable<V> {
    /// Every row, anchor first, skipping the unused slots.
    ///
    /// Test-only. The lookup never iterates rows as a sequence — it runs a
    /// fixed-trip-count loop over the array — so this exists purely so a test
    /// can assert something about every shipped row.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> impl Iterator<Item = &DatedRow<V>> {
        std::iter::once(&self.anchor).chain(self.later.iter().flatten())
    }

    /// Whether the table is structurally sound, checked by the compiler.
    ///
    /// Four properties, and each one is a way a table could lie:
    ///
    /// 1. **The anchor covers every representable day.** If it started later
    ///    than [`TradeDay::MIN`], every earlier date would silently take the
    ///    anchor's value.
    /// 2. **The rows ascend strictly.** A mis-ordered table selects the wrong
    ///    row and says nothing.
    /// 3. **No populated slot follows an empty one.** [`Self::rows`] uses
    ///    `flatten`, which would silently close such a hole.
    /// 4. **Every citation says something.** A blank `source` is a refusal
    ///    that explains nothing, or a rate with no provenance.
    ///
    /// Asserted in a `const` block beside every shipped table, so a table that
    /// breaks any of the four does **not compile**.
    // The walk consumes the slice rather than indexing it with a counter, for
    // the reason `has_content` gives: a counter is one mutation away from never
    // advancing. It also means no `indexing_slicing` exemption is needed.
    pub(crate) const fn is_shipping_shape(&self) -> bool {
        if !self.anchor.start.same_day(TradeDay::MIN) {
            return false;
        }
        if !has_content(self.anchor.source) {
            return false;
        }
        if !has_content(self.remediation) {
            return false;
        }
        if !has_content(self.subject) {
            return false;
        }
        let mut previous = self.anchor.start;
        let mut ended = false;
        let mut remaining: &[Option<DatedRow<V>>] = &self.later;
        while let [slot, rest @ ..] = remaining {
            match slot {
                Some(row) => {
                    if ended {
                        // A populated slot after an empty one: a hole.
                        return false;
                    }
                    if !previous.before(row.start) {
                        return false;
                    }
                    if !has_content(row.source) {
                        return false;
                    }
                    previous = row.start;
                }
                None => ended = true,
            }
            remaining = rest;
        }
        true
    }
}

/// Whether a citation string says anything at all.
///
/// [`str::trim`] cannot be called in a `const` context, and this check has to
/// be const because it runs on every shipped row at compile time. So it walks
/// the bytes looking for one that is not ASCII whitespace.
///
/// The indexing is bounded by the slice's own length, and for every shipped row
/// this executes during const evaluation, where an out-of-bounds index is a
/// compile error and not a panic.
///
/// Known limit, stated rather than papered over: it tests ASCII whitespace
/// only, so a citation made entirely of non-breaking spaces would pass. The
/// check exists to catch `""` and `"   "`, which is what a hurried edit
/// actually produces.
///
/// The walk consumes the slice rather than indexing it with a counter. That is
/// not a style preference: a counter is one mutation away from never advancing,
/// and a non-terminating mutant is detected by a hung test rather than by an
/// assertion. Consuming the tail cannot fail to advance, and it needs no
/// `indexing_slicing` exemption either.
pub(crate) const fn has_content(text: &str) -> bool {
    let mut remaining = text.as_bytes();
    while let [first, rest @ ..] = remaining {
        if !first.is_ascii_whitespace() {
            return true;
        }
        remaining = rest;
    }
    false
}

/// Whether two strings are byte-for-byte equal, in a `const` context.
///
/// `str::eq` is not a `const fn` on the pinned toolchain. This exists so that
/// "table slot 0 is the underlying `core` lists first" can be a **compile-time
/// assertion** rather than a comment: if `InstrumentKey::SWEPT` is ever
/// reordered, the tables keyed on its order stop compiling instead of silently
/// pricing one index with the other's lot size.
/// Consumes both slices in step rather than indexing with a counter — the same
/// reason [`has_content`] does, and it makes the length check fall out of the
/// walk instead of being a separate statement that could disagree with it.
pub(crate) const fn str_eq(left: &str, right: &str) -> bool {
    let (mut left, mut right) = (left.as_bytes(), right.as_bytes());
    while let ([first, left_rest @ ..], [second, right_rest @ ..]) = (left, right) {
        if *first != *second {
            return false;
        }
        left = left_rest;
        right = right_rest;
    }
    // Both ran out together, or one was a prefix of the other.
    left.is_empty() && right.is_empty()
}

/// A day named at compile time, or [`TradeDay::MIN`] if it were not a real
/// date.
///
/// Every table in this crate is a `const` item, so its boundary dates cannot go
/// through [`TradeDay::new`], which returns a `Result` that cannot be unwrapped
/// in a `const` context without a panic — and `clippy::panic` is denied.
///
/// The fallback arm hides nothing. A later row that silently collapsed to
/// [`TradeDay::MIN`] would fail `anchor.start.before(row.start)` in
/// [`DatedTable::is_shipping_shape`], which is a **compile-time** assertion, so
/// a typo'd boundary is a build failure rather than a mispriced day. The anchor
/// is `TradeDay::MIN` on purpose, and every boundary is additionally pinned
/// against [`TradeDay::new`] by a test.
macro_rules! boundary {
    ($year:literal - $month:literal - $day:literal) => {
        match $crate::day::TradeDay::new_const($year, $month, $day) {
            Some(day) => day,
            None => $crate::day::TradeDay::MIN,
        }
    };
}

pub(crate) use boundary;

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::*;

    /// A day, or a test that fails at the point of the mistake.
    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    /// A table with `later` rows supplied, everything else sound.
    fn table(later: [Option<DatedRow<u8>>; MAX_LATER_ROWS]) -> DatedTable<u8> {
        DatedTable {
            subject: "a subject",
            exchange: Some(Exchange::Nse),
            remediation: "a remedy",
            anchor: DatedRow::verified(TradeDay::MIN, 1, "an anchor citation"),
            later,
        }
    }

    #[test]
    fn the_anchor_answers_every_day_no_later_row_claims() {
        let subject = table([
            Some(DatedRow::verified(day(2000, 1, 1), 2, "second")),
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(subject.value_on(TradeDay::MIN), Ok(1));
        assert_eq!(subject.value_on(day(1999, 12, 31)), Ok(1));
        assert_eq!(
            subject.value_on(day(2000, 1, 1)),
            Ok(2),
            "inclusive boundary"
        );
        assert_eq!(subject.value_on(TradeDay::MAX), Ok(2));
    }

    #[test]
    fn the_lookup_agrees_with_a_naive_scan_on_every_representable_day() {
        // The strongest available statement: over the whole 40,542-day domain,
        // the fixed-trip-count loop selects exactly the row a scan would.
        let subject = table([
            Some(DatedRow::verified(day(1995, 6, 15), 2, "b")),
            Some(DatedRow::unverified(day(2005, 2, 28), "gap")),
            Some(DatedRow::verified(day(2020, 12, 31), 4, "d")),
            None,
            None,
        ]);
        let mut checked = 0u32;
        let mut refused = 0u32;
        for today in crate::day::every_representable_day() {
            let mut want = &subject.anchor;
            for row in subject.rows() {
                if row.start <= today {
                    want = row;
                }
            }
            match want.value {
                Dated::Verified(value) => assert_eq!(subject.value_on(today), Ok(value)),
                Dated::Unverified => {
                    let refusal = subject.value_on(today).expect_err("an unverified row");
                    assert_eq!(refusal.window_start(), day(2005, 2, 28));
                    assert_eq!(refusal.verified_from(), Some(day(2020, 12, 31)));
                    refused += 1;
                }
            }
            checked += 1;
        }
        assert_eq!(checked, 40_542, "the whole representable domain");
        assert_eq!(refused, 5_785, "the days inside the seeded refusal window");
    }

    #[test]
    fn a_refusal_carries_the_tables_own_subject_venue_and_remedy() {
        let subject = DatedTable::<u8> {
            subject: "options lot size",
            exchange: Some(Exchange::Nse),
            remediation: "add one dated row",
            anchor: DatedRow::unverified(TradeDay::MIN, "no circular retrieved"),
            later: [
                Some(DatedRow::verified(day(2021, 1, 1), 75, "the first row")),
                None,
                None,
                None,
                None,
            ],
        };
        let refusal = subject
            .value_on(day(2020, 12, 31))
            .expect_err("the anchor refuses");
        assert_eq!(refusal.charge(), "options lot size");
        assert_eq!(refusal.exchange(), Some(Exchange::Nse));
        assert_eq!(refusal.on(), day(2020, 12, 31));
        assert_eq!(refusal.window_start(), TradeDay::MIN);
        assert_eq!(refusal.verified_from(), Some(day(2021, 1, 1)));
        assert_eq!(refusal.source(), "no circular retrieved");
        assert_eq!(refusal.remediation(), "add one dated row");
        assert_eq!(subject.value_on(day(2021, 1, 1)), Ok(75));
    }

    #[test]
    fn a_trailing_refusal_window_has_no_end_date() {
        let subject = table([
            Some(DatedRow::unverified(day(2050, 1, 1), "a future gap")),
            None,
            None,
            None,
            None,
        ]);
        let refusal = subject.value_on(TradeDay::MAX).expect_err("refuses");
        assert_eq!(refusal.verified_from(), None);
        assert_eq!(refusal.window_start(), day(2050, 1, 1));
    }

    #[test]
    fn every_shape_violation_is_caught_and_a_sound_table_is_not() {
        assert!(table([None, None, None, None, None]).is_shipping_shape());
        assert!(
            table([
                Some(DatedRow::verified(day(2000, 1, 1), 2, "b")),
                Some(DatedRow::verified(day(2001, 1, 1), 3, "c")),
                Some(DatedRow::verified(day(2002, 1, 1), 4, "d")),
                Some(DatedRow::verified(day(2003, 1, 1), 5, "e")),
                Some(DatedRow::verified(day(2004, 1, 1), 6, "f")),
            ])
            .is_shipping_shape(),
            "a full table is a legal table"
        );
        // 1 — an anchor that does not start at the beginning of time.
        let mut late = table([None, None, None, None, None]);
        late.anchor = DatedRow::verified(day(1990, 1, 2), 1, "a");
        assert!(!late.is_shipping_shape());
        // 2 — rows that do not ascend, and rows that repeat a day.
        assert!(
            !table([
                Some(DatedRow::verified(day(2001, 1, 1), 2, "b")),
                Some(DatedRow::verified(day(2000, 1, 1), 3, "c")),
                None,
                None,
                None,
            ])
            .is_shipping_shape()
        );
        assert!(
            !table([
                Some(DatedRow::verified(day(2000, 1, 1), 2, "b")),
                Some(DatedRow::verified(day(2000, 1, 1), 3, "c")),
                None,
                None,
                None,
            ])
            .is_shipping_shape()
        );
        assert!(
            !table([
                Some(DatedRow::verified(TradeDay::MIN, 2, "b")),
                None,
                None,
                None,
                None,
            ])
            .is_shipping_shape(),
            "a later row on the anchor's own day is not ascending"
        );
        // 3 — a hole.
        assert!(
            !table([
                None,
                Some(DatedRow::verified(day(2000, 1, 1), 2, "b")),
                None,
                None,
                None,
            ])
            .is_shipping_shape()
        );
        // 4 — blank citations, everywhere one can be blank.
        let mut blank_anchor = table([None, None, None, None, None]);
        blank_anchor.anchor = DatedRow::verified(TradeDay::MIN, 1, "  \t ");
        assert!(!blank_anchor.is_shipping_shape());
        assert!(
            !table([
                Some(DatedRow::verified(day(2000, 1, 1), 2, "")),
                None,
                None,
                None,
                None,
            ])
            .is_shipping_shape()
        );
        let mut blank_remedy = table([None, None, None, None, None]);
        blank_remedy.remediation = "";
        assert!(!blank_remedy.is_shipping_shape());
        let mut blank_subject = table([None, None, None, None, None]);
        blank_subject.subject = " ";
        assert!(!blank_subject.is_shipping_shape());
    }

    #[test]
    fn rows_walks_the_anchor_first_and_stops_at_the_padding() {
        let subject = table([
            Some(DatedRow::verified(day(2000, 1, 1), 2, "b")),
            Some(DatedRow::verified(day(2001, 1, 1), 3, "c")),
            None,
            None,
            None,
        ]);
        let starts: Vec<TradeDay> = subject.rows().map(|row| row.start).collect();
        assert_eq!(
            starts,
            vec![TradeDay::MIN, day(2000, 1, 1), day(2001, 1, 1)]
        );
        assert_eq!(subject.rows().count(), 3);
        assert!(subject.rows().count() <= MAX_ROWS);
        assert_eq!(MAX_ROWS, 6);
        assert_eq!(MAX_LATER_ROWS, 5);
    }

    #[test]
    fn has_content_sees_a_blank_citation_and_nothing_else() {
        assert!(has_content("x"));
        assert!(has_content("  x  "));
        assert!(has_content("0.05%"));
        assert!(!has_content(""));
        assert!(!has_content(" "));
        assert!(!has_content(" \t\r\n"));
    }

    #[test]
    fn str_eq_answers_exactly_as_the_operator_would() {
        assert!(str_eq("NIFTY", "NIFTY"));
        assert!(str_eq("", ""));
        assert!(!str_eq("NIFTY", "BANKNIFTY"), "different lengths");
        assert!(!str_eq("NIFTY", "NIFTZ"), "same length, one byte apart");
        assert!(!str_eq("NIFTY", "NIFTYY"));
        // The property the compile-time asserts rest on, over every pair of
        // swept symbols: equal iff the same slot.
        for (i, &(_, left)) in brutex_core::instrument::InstrumentKey::SWEPT
            .iter()
            .enumerate()
        {
            for (j, &(_, right)) in brutex_core::instrument::InstrumentKey::SWEPT
                .iter()
                .enumerate()
            {
                assert_eq!(str_eq(left, right), i == j, "{left} vs {right}");
            }
        }
    }

    #[test]
    fn the_boundary_macro_builds_the_day_it_names_and_min_when_it_cannot() {
        const REAL: TradeDay = boundary!(2024 - 10 - 1);
        const LEAP: TradeDay = boundary!(2024 - 2 - 29);
        const IMPOSSIBLE: TradeDay = boundary!(2025 - 2 - 29);
        assert_eq!(REAL, day(2024, 10, 1));
        assert_eq!(LEAP, day(2024, 2, 29));
        assert_eq!(
            IMPOSSIBLE,
            TradeDay::MIN,
            "an impossible date collapses to MIN, which is_shipping_shape then refuses"
        );
        // And that collapse is exactly what the compile-time shape check sees.
        assert!(
            !table([
                Some(DatedRow::verified(IMPOSSIBLE, 2, "b")),
                None,
                None,
                None,
                None,
            ])
            .is_shipping_shape()
        );
    }

    #[test]
    fn a_dated_value_is_debuggable_and_comparable() {
        assert_eq!(
            format!("{:?}", Dated::Verified(75u8)),
            "Verified(75)",
            "a refusal must be distinguishable from a value in a log line"
        );
        assert_eq!(format!("{:?}", Dated::<u8>::Unverified), "Unverified");
        assert_ne!(Dated::Verified(75u8), Dated::Unverified);
        assert_eq!(Dated::Verified(75u8), Dated::Verified(75));
        let row = DatedRow::verified(TradeDay::MIN, 1u8, "a");
        assert_eq!(row, DatedRow::verified(TradeDay::MIN, 1, "a"));
        assert_ne!(row, DatedRow::unverified(TradeDay::MIN, "a"));
        assert!(format!("{row:?}").contains("Verified(1)"));
    }
}
