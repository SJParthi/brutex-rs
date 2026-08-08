//! Every refusal this crate can return, and exactly what it says.
//!
//! There is no error variant here that means "something was wrong, here is a
//! default anyway". `docs/00-charter.md` prohibition 6 and `CLAUDE.md` §4:
//! degrade loudly and name the reason, or refuse. Never both silently.
//!
//! [`Refusal`] is the load-bearing one. It is what a date in an unverified
//! window produces **instead of a rate**, and it carries everything an
//! operator needs to close the gap: which charge, which venue, which date,
//! which window, what was identified, what was never retrieved, and the one
//! action that would replace the refusal with a citation.

use std::fmt;

use brutex_core::instrument::Exchange;

use crate::day::TradeDay;

/// The default remediation line every [`Refusal`] carries.
///
/// A refusal is only useful if it tells the operator how to close it. This is
/// the predecessor's line, unchanged in substance: the fix is **data**, not
/// code — one dated row with a citation, and nothing in the lookup changes.
pub const REGIME_REMEDIATION: &str = "source the superseding circular's predecessor and add ONE \
     dated row (effective date, rate, citation) to the table in crates/costs/src/regime.rs, with \
     a docs/05-decisions.md ledger entry — zero mechanism change";

/// A rate was requested for a window whose rate was never verified.
///
/// This is returned **in place of** a number, and it is the only thing that
/// window can produce. See [`crate::regime::Rate`] for why the alternative is
/// not representable rather than merely not implemented.
///
/// `Copy`, and every field is a `&'static str` or a [`TradeDay`], so carrying
/// one costs no allocation and it can cross any boundary a rate could.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Refusal {
    charge: &'static str,
    exchange: Option<Exchange>,
    on: TradeDay,
    window_start: TradeDay,
    verified_from: Option<TradeDay>,
    source: &'static str,
    remediation: &'static str,
}

impl Refusal {
    /// Builds a refusal about a statutory rate.
    ///
    /// Crate-private on purpose. A refusal is a statement about a table this
    /// crate owns, and a caller that could mint one could also mint a
    /// misleading one.
    pub(crate) const fn new(
        charge: &'static str,
        exchange: Option<Exchange>,
        on: TradeDay,
        window_start: TradeDay,
        verified_from: Option<TradeDay>,
        source: &'static str,
    ) -> Self {
        Self::with_remediation(
            charge,
            exchange,
            on,
            window_start,
            verified_from,
            source,
            REGIME_REMEDIATION,
        )
    }

    /// Builds a refusal that names its own remedy.
    ///
    /// [`Self::new`] hard-codes [`REGIME_REMEDIATION`], which names
    /// `crates/costs/src/regime.rs`. The dated tables added by the option
    /// arithmetic — the lot size, the expiry weekday, the strike step — live in
    /// their own files, and a remedy that pointed an operator at the wrong file
    /// would be a refusal that fails at the one job a refusal has.
    pub(crate) const fn with_remediation(
        charge: &'static str,
        exchange: Option<Exchange>,
        on: TradeDay,
        window_start: TradeDay,
        verified_from: Option<TradeDay>,
        source: &'static str,
        remediation: &'static str,
    ) -> Self {
        Self {
            charge,
            exchange,
            on,
            window_start,
            verified_from,
            source,
            remediation,
        }
    }

    /// The human name of the charge that could not be priced.
    #[must_use]
    pub const fn charge(&self) -> &'static str {
        self.charge
    }

    /// The venue, when the charge is exchange-scoped. `None` for a charge that
    /// is not — a statutory tax applies at the same rate on both exchanges.
    #[must_use]
    pub const fn exchange(&self) -> Option<Exchange> {
        self.exchange
    }

    /// The date whose rate was asked for.
    #[must_use]
    pub const fn on(&self) -> TradeDay {
        self.on
    }

    /// The first day of the unverified window, inclusive.
    #[must_use]
    pub const fn window_start(&self) -> TradeDay {
        self.window_start
    }

    /// The first day a verified rate exists again — the window's exclusive
    /// end. `None` when the unverified window is the last row in the table and
    /// therefore open-ended.
    #[must_use]
    pub const fn verified_from(&self) -> Option<TradeDay> {
        self.verified_from
    }

    /// What was identified and what was never retrieved.
    ///
    /// A cited-but-unverified figure lives **here, as text**, and never in a
    /// number. That is the whole of the refusal contract in one sentence.
    #[must_use]
    pub const fn source(&self) -> &'static str {
        self.source
    }

    /// How to close this window.
    #[must_use]
    pub const fn remediation(&self) -> &'static str {
        self.remediation
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.charge)?;
        if let Some(exchange) = self.exchange {
            write!(f, " ({})", exchange.as_str())?;
        }
        write!(
            f,
            " is UNVERIFIED for {}: window [{} -> ",
            self.on, self.window_start
        )?;
        match self.verified_from {
            Some(end) => write!(f, "{end}")?,
            None => f.write_str("open")?,
        }
        write!(
            f,
            ") — {}. Refusing to fabricate a value. Remediation: {}",
            self.source, self.remediation
        )
    }
}

impl std::error::Error for Refusal {}

/// Everything this crate refuses to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CostError {
    /// The year is outside [`crate::day::FIRST_YEAR`]..=[`crate::day::LAST_YEAR`].
    ///
    /// Distinct from [`Self::MalformedDate`] because it is not a claim that
    /// the date is nonsense: 1974-03-04 is a perfectly real day. It is a
    /// refusal to price a day this crate holds no table for.
    YearOutsideWindow {
        /// The year that was asked for.
        year: u16,
    },
    /// The month or the day is not a real one, leap years accounted for.
    MalformedDate,
    /// The day ordinal is outside [`crate::day::TradeDay::MIN`]..=[`crate::day::TradeDay::MAX`].
    ///
    /// Produced by date **arithmetic** rather than by parsing: the next weekly
    /// expiry after the last representable Friday, or the month after
    /// December 2100. It is refused rather than saturated, because a saturated
    /// expiry would file a contract on a day it never expired.
    OrdinalOutsideWindow {
        /// The ordinal that was asked for, in days since 1970-01-01.
        ordinal: i32,
    },
    /// A quantity that must be strictly positive was not.
    ///
    /// A spot, a strike, a strike step, a lot count and a lot size are all
    /// strictly positive by construction. Zero and negative values are refused
    /// **by name** rather than clamped, because the alternative is an option
    /// struck at nothing, priced as if it were real.
    NotPositive {
        /// Which quantity: `"spot"`, `"strike"`, `"strike step"`, `"lots"`.
        quantity: &'static str,
        /// The value that was offered.
        value: i64,
    },
    /// Integer arithmetic left the `i64` domain.
    ///
    /// Never a wrap and never a saturation. A wrapped strike is a negative
    /// price; a saturated quantity is a position size nobody asked for.
    Overflow {
        /// The operation that could not be completed, named so the caller
        /// knows which input to look at.
        operation: &'static str,
    },
    /// The underlying is not one this crate resolves an exchange for.
    ///
    /// Not "unknown symbol": the symbol may be perfectly valid and stored.
    /// `CLAUDE.md` §1 fixes the engine surface at two NSE instruments, and
    /// [`crate::venue::exchange_for_underlying`] does not widen it. A caller
    /// that already knows the venue passes the [`Exchange`] itself.
    UnknownInstrument,
    /// A price that must be at least one tick was below it.
    ///
    /// Distinct from [`Self::NotPositive`]: five paisa is positive and is still
    /// not a price, because the option premium grid ([`crate::rate::TICK`]) has
    /// no rung below it. A bar whose **high** is under one tick never printed.
    BelowTick {
        /// Which quantity: `"bar high"`.
        quantity: &'static str,
        /// The value that was offered.
        value: i64,
    },
    /// A bar's low was above its high.
    ///
    /// Refused rather than silently swapped. A swap would fill both legs of the
    /// round trip off the wrong anchors and report a plausible number.
    InvertedBar {
        /// The high that was offered, in paisa.
        high: i64,
        /// The low that was offered, in paisa.
        low: i64,
    },
    /// A round trip whose exit day precedes its entry day.
    ///
    /// Not a costing question — it is two dates in the wrong order, and the
    /// regime in force would be read off the wrong one.
    ExitBeforeEntry {
        /// The entry day.
        entry: TradeDay,
        /// The exit day, which is earlier.
        exit: TradeDay,
    },
    /// The trade outcome is one this crate does not price.
    ///
    /// Only a normal close is implemented. The three expiry outcomes need the
    /// predecessor's `CALCULATOR_SPEC` §4.2 charge state machine — STT on
    /// intrinsic value for an exercise, no STT on an assignment, brokerage only
    /// on a worthless expiry — and pricing one of them with the premium-based
    /// normal-close arithmetic would be silently wrong rather than loudly
    /// absent.
    UnsupportedOutcome {
        /// The outcome that was asked for.
        outcome: &'static str,
    },
    /// A lot count was offered for a segment that has no lot size here.
    ///
    /// The dated table in [`crate::lot`] is the **options** lot history and its
    /// citations are options circulars. Sizing an index spot or an index
    /// futures position from it would be an unverified claim wearing a verified
    /// table's citation.
    LotSizeNotApplicable {
        /// The segment that was asked for.
        segment: &'static str,
    },
    /// A computed breakdown failed its own internal law.
    ///
    /// The total must equal the sum of its components and the net must equal
    /// the gross less the total. This is a logic error, not an input error: it
    /// means a field was wired to the wrong value, and the breakdown is refused
    /// rather than returned.
    Inconsistent {
        /// Which law failed, named so the reader knows which fields to look at.
        law: &'static str,
    },
    /// The regime in force on that date carries no verified rate.
    Unverified(Refusal),
}

impl fmt::Display for CostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::YearOutsideWindow { year } => write!(
                f,
                "year {year} is outside {}..={}, the window this crate holds rate tables for",
                crate::day::FIRST_YEAR,
                crate::day::LAST_YEAR
            ),
            Self::MalformedDate => f.write_str("not a real calendar date"),
            Self::OrdinalOutsideWindow { ordinal } => write!(
                f,
                "day ordinal {ordinal} is outside {}..={}, the days from {} to {}",
                crate::day::TradeDay::MIN.ordinal(),
                crate::day::TradeDay::MAX.ordinal(),
                crate::day::TradeDay::MIN,
                crate::day::TradeDay::MAX
            ),
            Self::NotPositive { quantity, value } => write!(
                f,
                "{quantity} must be strictly positive, got {value}; \
                 it is refused rather than clamped"
            ),
            Self::Overflow { operation } => write!(
                f,
                "{operation} left the i64 domain; it is refused rather than wrapped or saturated"
            ),
            Self::UnknownInstrument => f.write_str(
                "not an underlying this crate resolves an exchange for; \
                 pass the exchange directly if the venue is already known",
            ),
            Self::BelowTick { quantity, value } => write!(
                f,
                "{quantity} {value} is below one tick ({} paisa); \
                 there is no rung on the premium grid there",
                crate::rate::TICK.raw()
            ),
            Self::InvertedBar { high, low } => write!(
                f,
                "a bar's low {low} is above its high {high}; \
                 it is refused rather than silently swapped"
            ),
            Self::ExitBeforeEntry { entry, exit } => write!(
                f,
                "the exit day {exit} is before the entry day {entry}; \
                 a round trip does not close before it opens"
            ),
            Self::UnsupportedOutcome { outcome } => write!(
                f,
                "the trade outcome {outcome} is not priced: only a normal close is \
                 implemented, and pricing an expiry outcome with premium-based \
                 arithmetic would be silently wrong"
            ),
            Self::LotSizeNotApplicable { segment } => write!(
                f,
                "there is no lot size for {segment}: the dated table is the OPTIONS lot \
                 history and its citations are options circulars; pass the quantity in units"
            ),
            Self::Inconsistent { law } => write!(
                f,
                "the computed breakdown broke its own law ({law}); \
                 it is refused rather than reported"
            ),
            Self::Unverified(refusal) => write!(f, "{refusal}"),
        }
    }
}

impl std::error::Error for CostError {}

impl From<Refusal> for CostError {
    fn from(refusal: Refusal) -> Self {
        Self::Unverified(refusal)
    }
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

    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    /// A writer that succeeds `budget` times and then fails for good.
    ///
    /// [`Refusal`]'s `Display` writes in several parts, each with its own `?`.
    /// A writer that fails immediately reaches only the first of them; this
    /// one walks the failure forward so every `?` is taken in turn.
    struct FailsAfter {
        budget: usize,
        written: usize,
    }

    impl fmt::Write for FailsAfter {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            if self.written < self.budget {
                self.written += 1;
                Ok(())
            } else {
                Err(fmt::Error)
            }
        }
    }

    /// A budget large enough that no rendering can exhaust it.
    const AMPLE_BUDGET: usize = 64;

    /// How many budgets below [`AMPLE_BUDGET`] leave `refusal` unrendered.
    ///
    /// Rendering fails for every budget below the number of writes it needs
    /// and succeeds at or above it, so this count **is** that number — and
    /// walking the budget upward is what takes each `?` in turn.
    fn writes_needed(refusal: Refusal) -> usize {
        use fmt::Write as _;
        (0..AMPLE_BUDGET)
            .filter(|&budget| {
                let mut writer = FailsAfter { budget, written: 0 };
                write!(writer, "{refusal}").is_err()
            })
            .count()
    }

    #[test]
    fn display_propagates_a_writer_error_from_every_part_it_writes() {
        // A Display that swallowed a writer error would report success while
        // producing a truncated refusal — a refusal missing its window, or its
        // citation gap, or its remedy, with nothing saying so. Each `?` in
        // `Refusal::fmt` is a real path and this reaches all of them.
        let scoped = Refusal::new(
            "exchange transaction charge (options premium)",
            Some(Exchange::Nse),
            day(2024, 9, 30),
            TradeDay::MIN,
            Some(day(2024, 10, 1)),
            "the gap",
        );
        let unscoped = Refusal::new(
            "securities transaction tax",
            None,
            TradeDay::MAX,
            TradeDay::MIN,
            None,
            "the gap",
        );
        // Pinned counts: the venue adds parts, and an open-ended window writes
        // a literal where a dated one writes a date. The count being strictly
        // under AMPLE_BUDGET is what proves the cap is not itself the answer.
        assert_eq!(writes_needed(scoped), 30);
        assert_eq!(writes_needed(unscoped), 21);
        assert!(writes_needed(scoped) < AMPLE_BUDGET);
        {
            use fmt::Write as _;
            let mut writer = FailsAfter {
                budget: AMPLE_BUDGET,
                written: 0,
            };
            assert!(write!(writer, "{scoped}").is_ok());
            assert_eq!(writer.written, 30, "an ample budget is not all spent");
        }
        assert!(scoped.to_string().contains("(NSE)"));
        assert!(
            unscoped
                .to_string()
                .starts_with("securities transaction tax is UNVERIFIED"),
            "an unscoped charge writes no venue part at all"
        );
    }

    #[test]
    fn a_refusal_names_the_charge_the_venue_the_window_and_the_remedy() {
        let refusal = Refusal::new(
            "exchange transaction charge (options premium)",
            Some(Exchange::Nse),
            day(2024, 9, 30),
            TradeDay::MIN,
            Some(day(2024, 10, 1)),
            "slab ladders identified, none retrieved",
        );
        let rendered = refusal.to_string();
        assert_eq!(
            rendered,
            "exchange transaction charge (options premium) (NSE) is UNVERIFIED for 2024-09-30: \
             window [1990-01-01 -> 2024-10-01) — slab ladders identified, none retrieved. \
             Refusing to fabricate a value. Remediation: source the superseding circular's \
             predecessor and add ONE dated row (effective date, rate, citation) to the table in \
             crates/costs/src/regime.rs, with a docs/05-decisions.md ledger entry — zero \
             mechanism change"
        );
        assert_eq!(
            refusal.charge(),
            "exchange transaction charge (options premium)"
        );
        assert_eq!(refusal.exchange(), Some(Exchange::Nse));
        assert_eq!(refusal.on(), day(2024, 9, 30));
        assert_eq!(refusal.window_start(), TradeDay::MIN);
        assert_eq!(refusal.verified_from(), Some(day(2024, 10, 1)));
        assert_eq!(refusal.source(), "slab ladders identified, none retrieved");
        assert_eq!(refusal.remediation(), REGIME_REMEDIATION);
    }

    #[test]
    fn an_open_ended_window_says_open_and_an_unscoped_charge_names_no_venue() {
        let refusal = Refusal::new(
            "securities transaction tax",
            None,
            TradeDay::MAX,
            day(2050, 1, 1),
            None,
            "no circular exists yet",
        );
        let rendered = refusal.to_string();
        assert!(
            rendered.starts_with("securities transaction tax is UNVERIFIED for 2100-12-31: "),
            "an unscoped charge must not render an empty venue: {rendered}"
        );
        assert!(
            rendered.contains("window [2050-01-01 -> open)"),
            "a trailing window has no end date: {rendered}"
        );
        assert_eq!(refusal.exchange(), None);
        assert_eq!(refusal.verified_from(), None);
    }

    #[test]
    fn every_cost_error_renders_its_own_reason() {
        assert_eq!(
            CostError::YearOutsideWindow { year: 1974 }.to_string(),
            "year 1974 is outside 1990..=2100, the window this crate holds rate tables for"
        );
        assert_eq!(
            CostError::MalformedDate.to_string(),
            "not a real calendar date"
        );
        assert_eq!(
            CostError::UnknownInstrument.to_string(),
            "not an underlying this crate resolves an exchange for; pass the exchange directly \
             if the venue is already known"
        );
        assert_eq!(
            CostError::OrdinalOutsideWindow { ordinal: 47_847 }.to_string(),
            "day ordinal 47847 is outside 7305..=47846, the days from 1990-01-01 to 2100-12-31"
        );
        assert_eq!(
            CostError::NotPositive {
                quantity: "spot",
                value: 0
            }
            .to_string(),
            "spot must be strictly positive, got 0; it is refused rather than clamped"
        );
        assert_eq!(
            CostError::Overflow {
                operation: "lots x lot size"
            }
            .to_string(),
            "lots x lot size left the i64 domain; it is refused rather than wrapped or saturated"
        );
    }

    #[test]
    fn every_round_trip_refusal_renders_its_own_reason() {
        assert_eq!(
            CostError::BelowTick {
                quantity: "bar high",
                value: 4
            }
            .to_string(),
            "bar high 4 is below one tick (5 paisa); there is no rung on the premium grid there"
        );
        assert_eq!(
            CostError::InvertedBar {
                high: 100,
                low: 101
            }
            .to_string(),
            "a bar's low 101 is above its high 100; it is refused rather than silently swapped"
        );
        assert_eq!(
            CostError::ExitBeforeEntry {
                entry: day(2026, 5, 15),
                exit: day(2026, 5, 14),
            }
            .to_string(),
            "the exit day 2026-05-14 is before the entry day 2026-05-15; \
             a round trip does not close before it opens"
        );
        assert_eq!(
            CostError::UnsupportedOutcome {
                outcome: "expiry exercise"
            }
            .to_string(),
            "the trade outcome expiry exercise is not priced: only a normal close is \
             implemented, and pricing an expiry outcome with premium-based arithmetic \
             would be silently wrong"
        );
        assert_eq!(
            CostError::LotSizeNotApplicable {
                segment: "index spot"
            }
            .to_string(),
            "there is no lot size for index spot: the dated table is the OPTIONS lot \
             history and its citations are options circulars; pass the quantity in units"
        );
        assert_eq!(
            CostError::Inconsistent {
                law: "total == the sum of its components"
            }
            .to_string(),
            "the computed breakdown broke its own law (total == the sum of its components); \
             it is refused rather than reported"
        );
    }

    #[test]
    fn a_refusal_can_name_a_remedy_that_is_not_the_rate_tables() {
        // A lot-size refusal that told an operator to edit regime.rs would send
        // them to a file with no lot size in it.
        let refusal = Refusal::with_remediation(
            "options lot size",
            Some(Exchange::Nse),
            day(2020, 12, 31),
            TradeDay::MIN,
            Some(day(2021, 1, 1)),
            "no circular retrieved",
            "add ONE dated row to crates/costs/src/lot.rs",
        );
        assert_eq!(
            refusal.remediation(),
            "add ONE dated row to crates/costs/src/lot.rs"
        );
        assert!(
            refusal
                .to_string()
                .ends_with("Remediation: add ONE dated row to crates/costs/src/lot.rs"),
            "the remedy is the last thing an operator reads: {refusal}"
        );
        assert_ne!(refusal.remediation(), REGIME_REMEDIATION);
        // And `new` still supplies the rate-table remedy, unchanged.
        let rate_refusal = Refusal::new(
            "securities transaction tax",
            None,
            TradeDay::MIN,
            TradeDay::MIN,
            None,
            "gap",
        );
        assert_eq!(rate_refusal.remediation(), REGIME_REMEDIATION);
    }

    #[test]
    fn a_refusal_converts_into_a_cost_error_carrying_the_same_words() {
        let refusal = Refusal::new(
            "exchange transaction charge (options premium)",
            Some(Exchange::Bse),
            day(2020, 6, 1),
            TradeDay::MIN,
            Some(day(2024, 10, 1)),
            "notice identified, never retrieved",
        );
        let wrapped: CostError = refusal.into();
        assert_eq!(wrapped, CostError::Unverified(refusal));
        assert_eq!(wrapped.to_string(), refusal.to_string());
        assert!(wrapped.to_string().contains("(BSE)"));
    }

    #[test]
    fn both_error_types_are_usable_as_std_errors() {
        let refusal = Refusal::new(
            "exchange transaction charge (options premium)",
            None,
            TradeDay::MIN,
            TradeDay::MIN,
            None,
            "gap",
        );
        let boxed: Box<dyn std::error::Error> = Box::new(refusal);
        assert!(boxed.to_string().contains("Refusing to fabricate a value"));
        let boxed: Box<dyn std::error::Error> = Box::new(CostError::MalformedDate);
        assert_eq!(boxed.to_string(), "not a real calendar date");
        assert!(!refusal.source().is_empty());
    }
}
