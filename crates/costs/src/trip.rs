//! One round trip in, every charge and the total out.
//!
//! This is what stages one and two were for. [`price`] takes a round trip —
//! two bars, two days, a quantity, an instrument, a direction and a broker —
//! and returns a [`Charges`] with seven itemised charges, their total, the
//! gross and the net, all in paisa `i64`.
//!
//! # The five traps, and where each one is handled
//!
//! Every one of these is recorded in the predecessor repository as a defect
//! that was found and fixed. They are the reason this module is longer than the
//! arithmetic needs to be.
//!
//! 1. **STT is sell-side, and on the PREMIUM.** Not both sides, and not on
//!    `strike × lots`. The single most common error in an Indian F&O cost
//!    model, because the equity-delivery rule *is* both-sides and the
//!    futures-physical-settlement rule *is* on the settlement value. Here the
//!    tax reads [`Position::sell_notional`] and there is no other call site.
//! 2. **Stamp duty is buy-side only.** Charging both legs doubles it. The rate
//!    is fetched through [`crate::rate::stamp_duty`], which takes an
//!    [`OrderSide`] and returns zero for a sell, so "which leg" is a value the
//!    compiler carries rather than a sentence in a comment.
//! 3. **GST is rounded ONCE.** 18% on the services base, one rupee rounding.
//!    Not CGST at 9% and SGST at 9% rounded separately and added — that is a
//!    systematic **+₹1 overcharge on every trade**, which the predecessor's
//!    `DEC-COST-001` records citing section 170 of the CGST Act. An intra-state
//!    trade may be *displayed* as two halves; it is *computed* once, and this
//!    module has no split at all.
//! 4. **Brokerage is per executed ORDER, flat.** One lot and a thousand lots
//!    pay the same ₹20 a side. An earlier version of the predecessor's spec
//!    said "per contract" and was wrong. [`Rates`] resolves it once, as a
//!    round-trip total, and nothing in the stack multiplies it by anything.
//! 5. **An unverified regime refuses the WHOLE round trip.** Not a component
//!    priced at zero, not the current rate applied backwards. Stage one built
//!    that contract; [`Rates::resolve`] uses it and carries the
//!    [`crate::error::Refusal`] out whole, with its window, its citation gap and
//!    its remedy intact.
//!
//! # Constant per-operation cost
//!
//! [`charge_stack`] is a **straight-line integer sequence**. Counted exactly,
//! for every input:
//!
//! | | Count |
//! |---|---|
//! | multiplications | 12 |
//! | divisions (`div_euclid` / `rem_euclid` pairs) | 22 |
//! | additions and subtractions | 26 |
//! | comparisons | 9 |
//! | narrowings from `i128` to `i64` | 12 |
//!
//! There is **no loop, no recursion, no allocation and no collection**. The
//! only branches are the `match` on [`Direction`], the `match` on
//! [`crate::scope::Segment`], the outcome guard, and the overflow guards — none
//! of which is taken a number of times that depends on a price, a quantity, a
//! rate or a date. A thousand lots costs exactly what one lot costs, and
//! `crates/costs/benches/ratio.rs` measures that rather than asserting it.
//!
//! [`price`] adds a fixed amount on top: one dated lot lookup and two dated
//! rate lookups, each of which is a fixed-length array walk with a compile-time
//! trip count (stage one and stage two's claim, unchanged).
//!
//! # What is deliberately not here
//!
//! * **The expiry charge state machine.** STT on intrinsic value for an
//!   exercise, no STT on an assignment, brokerage only on a worthless expiry.
//!   The predecessor does not implement it either, and refuses rather than
//!   pricing an expiry with premium arithmetic. So does [`price`], through
//!   [`Outcome`].
//! * **A Muhurat brokerage waiver.** The predecessor's `COSTS_VERIFIED` §5
//!   Example 5 documents the intended ₹0 brokerage for a Muhurat session and
//!   carries an explicit banner saying the calculator has no such branch. It is
//!   not ported, for the same reason: there is nothing to port.
//! * **Iceberg order slicing.** One executed order per leg. The predecessor
//!   stamps `n_orders_per_leg = 1` and defers the rest.
//! * **The futures charge stack.** See `docs/06-limits.md` §27.

use brutex_core::instrument::Exchange;
use brutex_core::price::Paisa;

use crate::day::TradeDay;
use crate::error::CostError;
use crate::fill::{Bar, Direction, Fills, worst_case_fills};
use crate::lot::{contract_quantity, lot_size_on};
use crate::money::{levy_ceiling, narrow, statutory_levy};
use crate::rate::{
    BpsX100, Broker, GST_ON_FEE_BASE, OrderSide, SEBI_TURNOVER_FEE, brokerage_per_order, ipft,
    stamp_duty,
};
use crate::regime::{exchange_charge_rate, stt_options_rate};
use crate::scope::{Segment, is_cost_free};
use crate::venue::SweptSlot;

// A round-trip brokerage is twice the per-order figure. Both shipped figures
// are ₹20, so the doubling is exact — asserted here so that a future schedule
// large enough to overflow is a build failure rather than a wrapped charge.
const _: () = assert!(crate::rate::GROWW_BROKERAGE_PER_ORDER.raw() <= i64::MAX / 2);
const _: () = assert!(crate::rate::ZERODHA_BROKERAGE_PER_ORDER.raw() <= i64::MAX / 2);

// ---------------------------------------------------------------------------
// How the trip ended
// ---------------------------------------------------------------------------

/// How a round trip finished.
///
/// Only [`Self::NormalClose`] is priced. The other three exist so that a caller
/// can *say* what happened and be refused, rather than pass a normal close and
/// be silently mispriced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Outcome {
    /// Both legs traded in the market. The only outcome this crate prices.
    #[default]
    NormalClose,
    /// A long option was exercised at expiry. STT would be charged on intrinsic
    /// value, at [`crate::regime::stt_exercise_rate`], not on the premium.
    ExpiryExercise,
    /// A short option was assigned at expiry. No STT falls on the assigned leg.
    ExpiryAssign,
    /// The option expired out of the money. Brokerage only.
    ExpiryWorthless,
}

impl Outcome {
    /// The outcome's name, as a refusal spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NormalClose => "normal close",
            Self::ExpiryExercise => "expiry exercise",
            Self::ExpiryAssign => "expiry assign",
            Self::ExpiryWorthless => "expiry worthless",
        }
    }

    /// Whether this crate prices this outcome.
    #[must_use]
    pub const fn is_priced(self) -> bool {
        matches!(self, Self::NormalClose)
    }
}

impl core::fmt::Display for Outcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// What was traded, and when
// ---------------------------------------------------------------------------

/// What is being traded: which swept underlying, and in which segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Contract {
    underlying: SweptSlot,
    segment: Segment,
}

impl Contract {
    /// A contract on a swept underlying.
    #[must_use]
    pub const fn new(underlying: SweptSlot, segment: Segment) -> Self {
        Self {
            underlying,
            segment,
        }
    }

    /// Which of `core`'s swept underlyings this is.
    #[must_use]
    pub const fn underlying(self) -> SweptSlot {
        self.underlying
    }

    /// Spot, future or option.
    #[must_use]
    pub const fn segment(self) -> Segment {
        self.segment
    }

    /// The exchange whose circulars govern it.
    #[must_use]
    pub fn exchange(self) -> Exchange {
        self.underlying.exchange()
    }
}

/// One end of a round trip: the day it happened and the bar it filled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Leg {
    day: TradeDay,
    bar: Bar,
}

impl Leg {
    /// A leg from its day and its fill bar.
    #[must_use]
    pub const fn new(day: TradeDay, bar: Bar) -> Self {
        Self { day, bar }
    }

    /// The trading day.
    #[must_use]
    pub const fn day(self) -> TradeDay {
        self.day
    }

    /// The minute bar the leg filled on.
    #[must_use]
    pub const fn bar(self) -> Bar {
        self.bar
    }
}

// ---------------------------------------------------------------------------
// The rate set
// ---------------------------------------------------------------------------

/// Every rate one round trip is priced at.
///
/// Three of the seven are parameters, because three of them move: the STT rate
/// is dated, and the exchange transaction charge and the IPFT are both dated
/// **and** venue-scoped. The other four — the SEBI turnover fee, the stamp
/// duty, the GST rate and the flat brokerage — are read straight from
/// [`crate::rate`] and cannot be supplied, so a caller cannot get them wrong.
///
/// A [`BpsX100`] cannot be minted outside this crate, so a `Rates` can only be
/// assembled out of figures that came from a citation-grounded table. That is
/// what carries stage one's refusal contract into stage three intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rates {
    brokerage_round_trip: Paisa,
    stt: BpsX100,
    exchange: BpsX100,
    ipft: BpsX100,
}

impl Rates {
    /// A rate set from a broker and the three rates that move.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::instrument::Exchange;
    /// use costs::rate::{Broker, ipft};
    /// use costs::regime::BSE_EXCHANGE_CHARGE;
    /// use costs::day::TradeDay;
    /// use costs::trip::Rates;
    ///
    /// let stt = costs::regime::stt_options_rate(TradeDay::new(2026, 5, 15)?)?;
    /// let rates = Rates::new(Broker::Groww, stt, BSE_EXCHANGE_CHARGE, ipft(Exchange::Bse));
    /// assert_eq!(rates.brokerage_round_trip().raw(), 4_000);
    /// assert_eq!(rates.ipft().get(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn new(broker: Broker, stt: BpsX100, exchange: BpsX100, ipft: BpsX100) -> Self {
        Self {
            // Per ORDER, and a round trip is two orders. The `* 2` is exact:
            // both shipped figures are asserted at compile time to be at most
            // half of `i64::MAX`.
            brokerage_round_trip: Paisa::from_raw(brokerage_per_order(broker).raw() * 2),
            stt,
            exchange,
            ipft,
        }
    }

    /// The rate set in force at `exchange` on `day`.
    ///
    /// The **entry** day, always: the predecessor's `DEC-COST-002` fixes the
    /// regime key at the entry date, so a round trip that straddles a boundary
    /// is priced at the regime it opened under. [`price`] passes the entry day
    /// and never the exit day.
    ///
    /// # Errors
    ///
    /// [`CostError::Unverified`] carrying the [`Refusal`] of whichever lookup
    /// refused — the window it lands in, what was identified, what was never
    /// retrieved, and the one dated row that would close it. Every date before
    /// 2024-10-01 refuses, on both venues, because the exchange transaction
    /// charge was a slab ladder whose circulars were never retrieved.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::instrument::Exchange;
    /// use costs::day::TradeDay;
    /// use costs::rate::Broker;
    /// use costs::trip::Rates;
    ///
    /// let priced = Rates::resolve(Broker::Groww, Exchange::Nse, TradeDay::new(2024, 10, 1)?)?;
    /// assert_eq!(priced.exchange().get(), 3_503);
    ///
    /// let refused = Rates::resolve(Broker::Groww, Exchange::Nse, TradeDay::new(2024, 9, 30)?);
    /// assert!(refused.is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn resolve(broker: Broker, exchange: Exchange, day: TradeDay) -> Result<Self, CostError> {
        // Both lookups are taken, and whichever refused is carried out whole.
        // The STT table has no unverified row today; the exchange charge's
        // pre-2024-10-01 window does, and the refusal names which charge it is
        // about, so one arm can serve both without losing the reason.
        let (stt, exchange_rate) = match (
            stt_options_rate(day),
            exchange_charge_rate(exchange, day),
        ) {
            (Ok(tax), Ok(charge)) => (tax, charge),
            (Err(refusal), _) | (_, Err(refusal)) => return Err(refusal.into()),
        };
        Ok(Self::new(broker, stt, exchange_rate, ipft(exchange)))
    }

    /// The brokerage for the whole round trip — both orders, flat.
    #[must_use]
    pub const fn brokerage_round_trip(self) -> Paisa {
        self.brokerage_round_trip
    }

    /// The securities transaction tax rate, on sell-side premium.
    #[must_use]
    pub const fn stt(self) -> BpsX100 {
        self.stt
    }

    /// The exchange transaction charge rate, on premium, both sides.
    #[must_use]
    pub const fn exchange(self) -> BpsX100 {
        self.exchange
    }

    /// The investor protection fund rate, on premium, both sides.
    #[must_use]
    pub const fn ipft(self) -> BpsX100 {
        self.ipft
    }

    /// The SEBI turnover fee rate. Flat, and not a parameter.
    #[must_use]
    pub const fn sebi(self) -> BpsX100 {
        SEBI_TURNOVER_FEE
    }

    /// The stamp duty rate on the **buy** leg. Flat, and not a parameter.
    #[must_use]
    pub const fn stamp(self) -> BpsX100 {
        stamp_duty(OrderSide::Buy)
    }

    /// The GST rate on the services base. Flat, and not a parameter.
    #[must_use]
    pub const fn gst(self) -> BpsX100 {
        GST_ON_FEE_BASE
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// One round trip, ready to be priced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoundTrip {
    contract: Contract,
    direction: Direction,
    broker: Broker,
    outcome: Outcome,
    entry: Leg,
    exit: Leg,
    quantity: i64,
}

impl RoundTrip {
    /// A round trip with the quantity already in **units** of the underlying.
    ///
    /// # Errors
    ///
    /// * [`CostError::ExitBeforeEntry`] when the exit day precedes the entry
    ///   day.
    /// * [`CostError::NotPositive`] when the quantity is not strictly positive.
    ///   A trade of nothing is not a trade, and a negative quantity is a
    ///   direction expressed in the wrong field.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::price::Paisa;
    /// use brutex_core::symbol::Symbol;
    /// use costs::day::TradeDay;
    /// use costs::fill::{Bar, Direction};
    /// use costs::rate::Broker;
    /// use costs::scope::Segment;
    /// use costs::trip::{Contract, Leg, Outcome, RoundTrip};
    /// use costs::venue::swept_slot;
    ///
    /// let day = TradeDay::new(2026, 5, 15)?;
    /// let trip = RoundTrip::new(
    ///     Contract::new(swept_slot(Symbol::new("NIFTY")?)?, Segment::IndexOption),
    ///     Direction::Long,
    ///     Broker::Groww,
    ///     Outcome::NormalClose,
    ///     Leg::new(day, Bar::flat(Paisa::from_raw(100_00))?),
    ///     Leg::new(day, Bar::flat(Paisa::from_raw(120_00))?),
    ///     65,
    /// )?;
    /// assert_eq!(trip.quantity(), 65);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        contract: Contract,
        direction: Direction,
        broker: Broker,
        outcome: Outcome,
        entry: Leg,
        exit: Leg,
        quantity: i64,
    ) -> Result<Self, CostError> {
        if exit.day().before(entry.day()) {
            return Err(CostError::ExitBeforeEntry {
                entry: entry.day(),
                exit: exit.day(),
            });
        }
        if quantity <= 0 {
            return Err(CostError::NotPositive {
                quantity: "quantity",
                value: quantity,
            });
        }
        Ok(Self {
            contract,
            direction,
            broker,
            outcome,
            entry,
            exit,
            quantity,
        })
    }

    /// A round trip sized in **lots**, with the lot size read off the dated
    /// options table for the entry day.
    ///
    /// **The lot size is keyed on the trade date, not on the contract's
    /// vintage.** A real exchange lot revision applies per contract from a
    /// stated expiry onward, so around a transition the two disagree: NIFTY on
    /// 2024-11-19 resolves 25 and on 2024-11-20 resolves 75, a threefold sizing
    /// jump keyed purely on the day, even though a still-listed pre-revision
    /// contract kept its old lot until its own expiry. The predecessor records
    /// the same limitation and stamps its output with the basis it used; the
    /// per-circular contract-level rollout dates are unverified, so no vintage
    /// logic is invented here either. `docs/06-limits.md` §27.
    ///
    /// # Errors
    ///
    /// * [`CostError::LotSizeNotApplicable`] for anything but
    ///   [`Segment::IndexOption`]. The dated table is the **options** lot
    ///   history and its citations are options circulars.
    /// * [`CostError::Unverified`] for an entry day before the source's
    ///   recorded lot history begins.
    /// * [`CostError::NotPositive`] or [`CostError::Overflow`] from the lot
    ///   arithmetic, and everything [`Self::new`] can return.
    ///
    /// # Examples
    ///
    /// ```
    /// use brutex_core::price::Paisa;
    /// use brutex_core::symbol::Symbol;
    /// use costs::day::TradeDay;
    /// use costs::fill::{Bar, Direction};
    /// use costs::rate::Broker;
    /// use costs::scope::Segment;
    /// use costs::trip::{Contract, Leg, Outcome, RoundTrip};
    /// use costs::venue::swept_slot;
    ///
    /// let day = TradeDay::new(2026, 5, 15)?;
    /// let leg = Leg::new(day, Bar::flat(Paisa::from_raw(100_00))?);
    /// let trip = RoundTrip::in_lots(
    ///     Contract::new(swept_slot(Symbol::new("NIFTY")?)?, Segment::IndexOption),
    ///     Direction::Long,
    ///     Broker::Groww,
    ///     Outcome::NormalClose,
    ///     leg,
    ///     leg,
    ///     1,
    /// )?;
    /// // One NIFTY lot is 65 units from January 2026.
    /// assert_eq!(trip.quantity(), 65);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn in_lots(
        contract: Contract,
        direction: Direction,
        broker: Broker,
        outcome: Outcome,
        entry: Leg,
        exit: Leg,
        lots: i64,
    ) -> Result<Self, CostError> {
        if !contract.segment().is_option() {
            return Err(CostError::LotSizeNotApplicable {
                segment: contract.segment().as_str(),
            });
        }
        let lot_size = lot_size_on(contract.underlying(), entry.day())?;
        let quantity = contract_quantity(lots, lot_size)?;
        Self::new(contract, direction, broker, outcome, entry, exit, quantity)
    }

    /// What was traded.
    #[must_use]
    pub const fn contract(self) -> Contract {
        self.contract
    }

    /// Which way round the trip went.
    #[must_use]
    pub const fn direction(self) -> Direction {
        self.direction
    }

    /// Whose brokerage schedule prices it.
    #[must_use]
    pub const fn broker(self) -> Broker {
        self.broker
    }

    /// How the trip finished.
    #[must_use]
    pub const fn outcome(self) -> Outcome {
        self.outcome
    }

    /// The opening leg.
    #[must_use]
    pub const fn entry(self) -> Leg {
        self.entry
    }

    /// The closing leg.
    #[must_use]
    pub const fn exit(self) -> Leg {
        self.exit
    }

    /// How many units of the underlying, always strictly positive.
    #[must_use]
    pub const fn quantity(self) -> i64 {
        self.quantity
    }
}

// ---------------------------------------------------------------------------
// The breakdown
// ---------------------------------------------------------------------------

/// How many itemised charges a round trip carries.
pub const CHARGE_COUNT: usize = 7;

/// Every charge on one round trip, itemised, plus the roll-ups.
///
/// Every figure is paisa. Nothing here is a float, a ratio or a percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Charges {
    buy_fill: Paisa,
    sell_fill: Paisa,
    quantity: i64,
    buy_notional: Paisa,
    sell_notional: Paisa,
    brokerage: Paisa,
    stt: Paisa,
    exchange: Paisa,
    sebi: Paisa,
    ipft: Paisa,
    stamp: Paisa,
    gst: Paisa,
    slippage: Paisa,
    total: Paisa,
    gross_pnl: Paisa,
    net_pnl: Paisa,
    realized_slip_per_unit: Paisa,
}

impl Charges {
    /// The price the buy leg filled at.
    #[must_use]
    pub const fn buy_fill(self) -> Paisa {
        self.buy_fill
    }

    /// The price the sell leg filled at.
    #[must_use]
    pub const fn sell_fill(self) -> Paisa {
        self.sell_fill
    }

    /// How many units of the underlying were traded.
    #[must_use]
    pub const fn quantity(self) -> i64 {
        self.quantity
    }

    /// The buy leg's premium notional.
    #[must_use]
    pub const fn buy_notional(self) -> Paisa {
        self.buy_notional
    }

    /// The sell leg's premium notional — the base the transaction tax reads.
    #[must_use]
    pub const fn sell_notional(self) -> Paisa {
        self.sell_notional
    }

    /// Brokerage, both orders, flat.
    #[must_use]
    pub const fn brokerage(self) -> Paisa {
        self.brokerage
    }

    /// Securities transaction tax — **sell side only**, on premium.
    #[must_use]
    pub const fn stt(self) -> Paisa {
        self.stt
    }

    /// Exchange transaction charge — both legs, each ceiled to the paisa
    /// separately before they are added.
    #[must_use]
    pub const fn exchange(self) -> Paisa {
        self.exchange
    }

    /// SEBI turnover fee — both legs.
    #[must_use]
    pub const fn sebi(self) -> Paisa {
        self.sebi
    }

    /// Investor protection fund contribution — both legs.
    #[must_use]
    pub const fn ipft(self) -> Paisa {
        self.ipft
    }

    /// Stamp duty — **buy side only**.
    #[must_use]
    pub const fn stamp(self) -> Paisa {
        self.stamp
    }

    /// GST — 18% on the services base, rounded **once**.
    #[must_use]
    pub const fn gst(self) -> Paisa {
        self.gst
    }

    /// The slippage baked into the two fills. Informational: it is inside the
    /// gross already and is never subtracted from anything.
    #[must_use]
    pub const fn slippage(self) -> Paisa {
        self.slippage
    }

    /// The sum of the seven itemised charges.
    #[must_use]
    pub const fn total_charges(self) -> Paisa {
        self.total
    }

    /// The profit and loss before charges, with slippage already in the fills.
    #[must_use]
    pub const fn gross_pnl(self) -> Paisa {
        self.gross_pnl
    }

    /// The profit and loss after charges.
    #[must_use]
    pub const fn net_pnl(self) -> Paisa {
        self.net_pnl
    }

    /// The adverse movement per unit truly baked into the two fills.
    #[must_use]
    pub const fn realized_slip_per_unit(self) -> Paisa {
        self.realized_slip_per_unit
    }

    /// The seven charges, each beside its name, in the order a contract note
    /// lists them.
    ///
    /// A fixed-size array: producing it is arithmetic, not a walk over a
    /// collection that could grow.
    ///
    /// # Examples
    ///
    /// ```
    /// # use brutex_core::price::Paisa;
    /// # use brutex_core::symbol::Symbol;
    /// # use costs::day::TradeDay;
    /// # use costs::fill::{Bar, Direction};
    /// # use costs::rate::Broker;
    /// # use costs::scope::Segment;
    /// # use costs::trip::{Contract, Leg, Outcome, RoundTrip, price};
    /// # use costs::venue::swept_slot;
    /// # let day = TradeDay::new(2026, 5, 15)?;
    /// # let trip = RoundTrip::in_lots(
    /// #     Contract::new(swept_slot(Symbol::new("NIFTY")?)?, Segment::IndexOption),
    /// #     Direction::Long, Broker::Groww, Outcome::NormalClose,
    /// #     Leg::new(day, Bar::flat(Paisa::from_raw(100_00))?),
    /// #     Leg::new(day, Bar::flat(Paisa::from_raw(120_00))?),
    /// #     1,
    /// # )?;
    /// let charges = price(&trip)?;
    /// let items = charges.itemised();
    /// assert_eq!(items.len(), costs::trip::CHARGE_COUNT);
    /// assert_eq!(items[0].0, "brokerage");
    /// assert_eq!(items[0].1.raw(), 4_000);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn itemised(self) -> [(&'static str, Paisa); CHARGE_COUNT] {
        [
            ("brokerage", self.brokerage),
            ("securities transaction tax", self.stt),
            ("exchange transaction charge", self.exchange),
            ("SEBI turnover fee", self.sebi),
            ("investor protection fund", self.ipft),
            ("stamp duty", self.stamp),
            ("GST", self.gst),
        ]
    }

    /// Checks the breakdown against its own two laws.
    ///
    /// The total must be the sum of the seven itemised charges, and the net
    /// must be the gross less the total. Neither can fail if the arithmetic
    /// above is wired correctly, which is the point: a field assigned from the
    /// wrong expression is caught here rather than reported.
    fn validated(self) -> Result<Self, CostError> {
        let component_sum = i128::from(self.brokerage.raw())
            + i128::from(self.stt.raw())
            + i128::from(self.exchange.raw())
            + i128::from(self.sebi.raw())
            + i128::from(self.ipft.raw())
            + i128::from(self.stamp.raw())
            + i128::from(self.gst.raw());
        if i128::from(self.total.raw()) != component_sum {
            return Err(CostError::Inconsistent {
                law: "total == the sum of its components",
            });
        }
        if i128::from(self.net_pnl.raw())
            != i128::from(self.gross_pnl.raw()) - i128::from(self.total.raw())
        {
            return Err(CostError::Inconsistent {
                law: "net == gross - total",
            });
        }
        Ok(self)
    }
}

impl core::fmt::Display for Charges {
    /// One line, every figure in paisa, in the order the stack computes them.
    ///
    /// Written as a single `write!` on purpose: a chain of them would carry a
    /// failure path per part, and a `Display` that stops halfway renders a
    /// breakdown missing its total with nothing saying so.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} units, buy {} sell {} (notionals {} / {}); \
             brokerage {} + STT {} + exchange {} + SEBI {} + IPFT {} + stamp {} + GST {} \
             = {} charges; gross {}, net {} (slippage {} at {} per unit)",
            self.quantity,
            self.buy_fill.raw(),
            self.sell_fill.raw(),
            self.buy_notional.raw(),
            self.sell_notional.raw(),
            self.brokerage.raw(),
            self.stt.raw(),
            self.exchange.raw(),
            self.sebi.raw(),
            self.ipft.raw(),
            self.stamp.raw(),
            self.gst.raw(),
            self.total.raw(),
            self.gross_pnl.raw(),
            self.net_pnl.raw(),
            self.slippage.raw(),
            self.realized_slip_per_unit.raw(),
        )
    }
}

// ---------------------------------------------------------------------------
// The arithmetic
// ---------------------------------------------------------------------------

/// The part of a round trip that is the same whether or not it bears charges.
#[derive(Debug, Clone, Copy)]
struct Position {
    buy_notional: Paisa,
    sell_notional: Paisa,
    gross: Paisa,
    slippage: Paisa,
}

/// The notionals, the gross and the slippage line.
///
/// Shared by the charge-bearing and the signal-only arms, because "cost-free"
/// removes the charge stack and nothing else — the fills and the gross are
/// identical either way.
fn position(fills: Fills, quantity: i64, direction: Direction) -> Result<Position, CostError> {
    if quantity <= 0 {
        return Err(CostError::NotPositive {
            quantity: "quantity",
            value: quantity,
        });
    }
    let buy_notional = leg_notional(fills.buy(), quantity, "the buy notional")?;
    let sell_notional = leg_notional(fills.sell(), quantity, "the sell notional")?;

    // Both notionals are in `[0, i64::MAX]`: a `Fills` has no public
    // constructor, so its buy leg is at least two ticks and its sell leg at
    // least one, the quantity is strictly positive, and the multiplications
    // above already refused anything that left `i64`. The difference of two
    // values in `[0, i64::MAX]` is in `[-i64::MAX, i64::MAX]`, so this
    // subtraction cannot overflow and there is no branch pretending it can.
    let gross = match direction {
        Direction::Long => sell_notional.raw() - buy_notional.raw(),
        Direction::Short => buy_notional.raw() - sell_notional.raw(),
    };

    let slippage = fills
        .realized_slip_per_unit()
        .raw()
        .checked_mul(quantity)
        .map(Paisa::from_raw)
        .ok_or(CostError::Overflow {
            operation: "the slippage line",
        })?;

    Ok(Position {
        buy_notional,
        sell_notional,
        gross: Paisa::from_raw(gross),
        slippage,
    })
}

/// One leg's premium notional: the fill times the quantity, and nothing else.
fn leg_notional(fill: Paisa, quantity: i64, operation: &'static str) -> Result<Paisa, CostError> {
    fill.raw()
        .checked_mul(quantity)
        .map(Paisa::from_raw)
        .ok_or(CostError::Overflow { operation })
}

/// A non-statutory levy on both legs: each leg ceiled to the paisa **first**,
/// and only then added.
///
/// Never the other way round. The predecessor's `COSTS_VERIFIED` §5 Example 1
/// gives 502 paisa because it is `228 + 274`; ceiling the combined notional
/// once gives 501, and the difference compounds over a sweep.
fn both_legs(
    position: &Position,
    rate: BpsX100,
    operation: &'static str,
) -> Result<Paisa, CostError> {
    let buy_leg = levy_ceiling(position.buy_notional, rate)?;
    let sell_leg = levy_ceiling(position.sell_notional, rate)?;
    narrow(
        i128::from(buy_leg.raw()) + i128::from(sell_leg.raw()),
        operation,
    )
}

/// Every charge on a round trip, from fills and a rate set.
///
/// This is the pure arithmetic core: it reads no table, resolves no date and
/// cannot refuse for a citation reason. [`price`] is the entry point that does
/// those things and then calls this.
///
/// The order of the steps is the predecessor's `CALCULATOR_SPEC` §4 order,
/// preserved exactly, because the GST base is the sum of the **already
/// rounded** service components and therefore depends on it.
///
/// # Errors
///
/// * [`CostError::NotPositive`] when the quantity is not strictly positive.
/// * [`CostError::Overflow`] from any step that would leave `i64`, naming the
///   step. Nothing wraps and nothing saturates.
/// * [`CostError::Inconsistent`] if the assembled breakdown fails its own laws,
///   which is a logic error rather than an input one.
///
/// # Examples
///
/// ```
/// use brutex_core::instrument::Exchange;
/// use brutex_core::price::Paisa;
/// use costs::day::TradeDay;
/// use costs::fill::{Bar, Direction, worst_case_fills};
/// use costs::rate::Broker;
/// use costs::trip::{Rates, charge_stack};
///
/// let fills = worst_case_fills(
///     Bar::flat(Paisa::from_raw(100_00))?,
///     Bar::flat(Paisa::from_raw(120_00))?,
///     Direction::Long,
/// )?;
/// let rates = Rates::resolve(Broker::Groww, Exchange::Nse, TradeDay::new(2026, 5, 15)?)?;
/// let charges = charge_stack(fills, 65, Direction::Long, &rates)?;
/// assert_eq!(charges.total_charges().raw(), 6_712);
/// assert_eq!(charges.net_pnl().raw(), 122_638);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn charge_stack(
    fills: Fills,
    quantity: i64,
    direction: Direction,
    rates: &Rates,
) -> Result<Charges, CostError> {
    let position = position(fills, quantity, direction)?;

    // Brokerage: per executed ORDER, flat, both legs. It does not scale with
    // the quantity and nothing below multiplies it.
    let brokerage = rates.brokerage_round_trip();

    // The transaction tax: the SELL notional, and only it. Not both legs, and
    // not the strike.
    let stt = statutory_levy(position.sell_notional, rates.stt())?;

    // The three per-leg levies: ceiled to the paisa on each leg, then summed.
    let exchange = both_legs(&position, rates.exchange(), "the exchange transaction charge")?;
    let sebi = both_legs(&position, rates.sebi(), "the SEBI turnover fee")?;
    let ipft = both_legs(&position, rates.ipft(), "the investor protection fund")?;

    // Stamp duty: the BUY notional, at the buy-side rate. The sell side's rate
    // is zero by construction, so there is nothing to charge there and no call.
    let stamp = statutory_levy(position.buy_notional, rates.stamp())?;

    // GST: 18% on the services base, which is the sum of the ALREADY-ROUNDED
    // service components. The transaction tax and the stamp duty are taxes, not
    // services, and are excluded. Rounded ONCE — never as two halves.
    let gst_base = narrow(
        i128::from(brokerage.raw())
            + i128::from(exchange.raw())
            + i128::from(sebi.raw())
            + i128::from(ipft.raw()),
        "the GST base",
    )?;
    let gst = statutory_levy(gst_base, rates.gst())?;

    let total = narrow(
        i128::from(brokerage.raw())
            + i128::from(stt.raw())
            + i128::from(exchange.raw())
            + i128::from(sebi.raw())
            + i128::from(ipft.raw())
            + i128::from(stamp.raw())
            + i128::from(gst.raw()),
        "the total charges",
    )?;

    let net_pnl = position
        .gross
        .raw()
        .checked_sub(total.raw())
        .map(Paisa::from_raw)
        .ok_or(CostError::Overflow {
            operation: "the net profit and loss",
        })?;

    Charges {
        buy_fill: fills.buy(),
        sell_fill: fills.sell(),
        quantity,
        buy_notional: position.buy_notional,
        sell_notional: position.sell_notional,
        brokerage,
        stt,
        exchange,
        sebi,
        ipft,
        stamp,
        gst,
        slippage: position.slippage,
        total,
        gross_pnl: position.gross,
        net_pnl,
        realized_slip_per_unit: fills.realized_slip_per_unit(),
    }
    .validated()
}

/// The all-zero stack for a segment that is priced signal-only.
///
/// It touches no rate at all — which is what lets an index spot round trip in
/// 2019 be priced, in a window where every exchange transaction charge refuses.
fn signal_only(
    fills: Fills,
    quantity: i64,
    direction: Direction,
) -> Result<Charges, CostError> {
    let position = position(fills, quantity, direction)?;
    Charges {
        buy_fill: fills.buy(),
        sell_fill: fills.sell(),
        quantity,
        buy_notional: position.buy_notional,
        sell_notional: position.sell_notional,
        brokerage: Paisa::ZERO,
        stt: Paisa::ZERO,
        exchange: Paisa::ZERO,
        sebi: Paisa::ZERO,
        ipft: Paisa::ZERO,
        stamp: Paisa::ZERO,
        gst: Paisa::ZERO,
        slippage: position.slippage,
        total: Paisa::ZERO,
        gross_pnl: position.gross,
        net_pnl: position.gross,
        realized_slip_per_unit: fills.realized_slip_per_unit(),
    }
    .validated()
}

/// Price one round trip: every charge itemised, and the total.
///
/// The whole of stages one, two and three in one call. It resolves the fills,
/// the segment's costability and — for a cost-bearing segment — the rate set in
/// force on the **entry** day, then runs [`charge_stack`].
///
/// # Errors
///
/// * [`CostError::UnsupportedOutcome`] for anything but a normal close.
/// * [`CostError::Unverified`] when the regime in force on the entry day has no
///   verified rate. **The whole round trip refuses.** No component is priced at
///   zero, no current rate is applied backwards, and there is no argument that
///   changes it.
/// * Everything [`charge_stack`] can return, and [`CostError::Overflow`] from
///   the fill law.
///
/// # Examples
///
/// ```
/// use brutex_core::price::Paisa;
/// use brutex_core::symbol::Symbol;
/// use costs::day::TradeDay;
/// use costs::fill::{Bar, Direction};
/// use costs::rate::Broker;
/// use costs::scope::Segment;
/// use costs::trip::{Contract, Leg, Outcome, RoundTrip, price};
/// use costs::venue::swept_slot;
///
/// let day = TradeDay::new(2026, 5, 15)?;
/// let trip = RoundTrip::in_lots(
///     Contract::new(swept_slot(Symbol::new("NIFTY")?)?, Segment::IndexOption),
///     Direction::Long,
///     Broker::Groww,
///     Outcome::NormalClose,
///     Leg::new(day, Bar::flat(Paisa::from_raw(100_00))?),
///     Leg::new(day, Bar::flat(Paisa::from_raw(120_00))?),
///     1,
/// )?;
///
/// // `COSTS_VERIFIED` §5 Example 1, to the paisa.
/// let charges = price(&trip)?;
/// assert_eq!(charges.brokerage().raw(), 4_000);
/// assert_eq!(charges.stt().raw(), 1_200);
/// assert_eq!(charges.exchange().raw(), 502);
/// assert_eq!(charges.total_charges().raw(), 6_712);
/// assert_eq!(charges.net_pnl().raw(), 122_638);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn price(trip: &RoundTrip) -> Result<Charges, CostError> {
    if !trip.outcome().is_priced() {
        return Err(CostError::UnsupportedOutcome {
            outcome: trip.outcome().as_str(),
        });
    }
    let fills = worst_case_fills(trip.entry().bar(), trip.exit().bar(), trip.direction())?;
    if is_cost_free(trip.contract().segment()) {
        return signal_only(fills, trip.quantity(), trip.direction());
    }
    let rates = Rates::resolve(
        trip.broker(),
        trip.contract().exchange(),
        trip.entry().day(),
    )?;
    charge_stack(fills, trip.quantity(), trip.direction(), &rates)
}
