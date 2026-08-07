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
//!    tax reads the sell leg's premium notional and there is no other call
//!    site — and there is no strike anywhere in this module, so the
//!    strike-based error is not merely wrong but unwritable.
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
//! # Constant per-operation cost, counted
//!
//! [`charge_stack`] is a **straight-line integer sequence**. The count is
//! written out per stage rather than as a total, so that it can be checked by
//! reading rather than believed:
//!
//! | Stage | × | ÷ | ± | guarded |
//! |---|---|---|---|---|
//! | the two notionals and the slippage line | 3 | 0 | 1 | 3 |
//! | the transaction tax — one [`crate::money::statutory_levy`] | 2 | 3 | ≤1 | 2 |
//! | three per-leg levies on two legs — six [`crate::money::levy_ceiling`] | 6 | 12 | ≤6 | 6 |
//! | their three two-leg sums | 0 | 0 | 3 | 3 |
//! | the stamp duty — one `statutory_levy` | 2 | 3 | ≤1 | 2 |
//! | the GST base, then the GST — one `statutory_levy` | 2 | 3 | ≤4 | 3 |
//! | the gross, the total and the net | 0 | 0 | 7 | 2 |
//! | the two internal-law checks | 0 | 0 | 7 | 0 |
//! | **whole stack** | **15** | **21** | **≤30** | **21** |
//!
//! `÷` counts each `div_euclid` and each `rem_euclid` separately. `guarded`
//! counts every operation that refuses rather than wraps — a `checked_mul`, a
//! `checked_sub`, or a narrowing from `i128` back to `i64`. There are 13
//! comparisons: one on the quantity, one on the direction, nine on a ceiling's
//! remainder, and two on the internal laws.
//!
//! **The counts do not move with the input.** They are the same for one lot and
//! for a million, for a five-paisa premium and for a lakh-rupee one, for a
//! winning trip and for a losing one. There is **no loop, no recursion, no
//! allocation and no collection** anywhere in the stack; the `≤` on the
//! additions is one conditional `+ 1` per ceiling, taken on a remainder rather
//! than on a magnitude.
//!
//! `crates/costs/benches/ratio.rs` C-K-10 measures it rather than asserting it.
//!
//! [`price`] adds a fixed amount on top: two dated rate lookups (and, through
//! [`RoundTrip::in_lots`], one dated lot lookup), each of which is a
//! fixed-length array walk with a compile-time trip count — stage one and stage
//! two's claim, unchanged. C-K-11 measures that too.
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
use crate::error::{CostError, Refusal};
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
    sebi: BpsX100,
    stamp: BpsX100,
    gst: BpsX100,
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
            // The flat three are read straight from `crate::rate` and are not
            // parameters. The stamp duty in particular is fetched through
            // `stamp_duty(OrderSide::Buy)`, so "which leg" is a value rather
            // than a convention a caller has to remember.
            sebi: SEBI_TURNOVER_FEE,
            stamp: stamp_duty(OrderSide::Buy),
            gst: GST_ON_FEE_BASE,
        }
    }

    /// A rate set with **every** rate supplied, including the flat three.
    ///
    /// Test-only, and deliberately: the flat rates are flat because getting
    /// them wrong is one of the ways an Indian cost stack goes wrong, and
    /// nothing outside this crate can reach this. It exists so that the
    /// overflow guard on every levy in [`charge_stack`] is a **tested** path
    /// rather than one asserted to be unreachable — the shipped SEBI fee,
    /// stamp duty and GST rate are all far too small to overflow anything, and
    /// an untested guard is indistinguishable from a missing one.
    #[cfg(test)]
    pub(crate) const fn with_all(
        broker: Broker,
        stt: BpsX100,
        exchange: BpsX100,
        ipft: BpsX100,
        sebi: BpsX100,
        stamp: BpsX100,
        gst: BpsX100,
    ) -> Self {
        Self {
            brokerage_round_trip: Paisa::from_raw(brokerage_per_order(broker).raw() * 2),
            stt,
            exchange,
            ipft,
            sebi,
            stamp,
            gst,
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
        // Both lookups are taken and whichever refused is carried out whole.
        // The refusal names which charge it is about, so one path serves both
        // without losing the reason.
        let (stt, exchange_rate) =
            dated_pair(stt_options_rate(day), exchange_charge_rate(exchange, day))?;
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

    /// The SEBI turnover fee rate. Flat, and not a parameter of [`Self::new`].
    #[must_use]
    pub const fn sebi(self) -> BpsX100 {
        self.sebi
    }

    /// The stamp duty rate on the **buy** leg. Flat, and not a parameter of
    /// [`Self::new`].
    #[must_use]
    pub const fn stamp(self) -> BpsX100 {
        self.stamp
    }

    /// The GST rate on the services base. Flat, and not a parameter of
    /// [`Self::new`].
    #[must_use]
    pub const fn gst(self) -> BpsX100 {
        self.gst
    }
}

/// The two dated rates, or the first refusal of the two.
///
/// A function taking two `Result`s rather than two `?`s written inline, so that
/// **both** refusal paths are reachable from a test. The transaction-tax table
/// has no unverified row today; its guard is still exercised, against a refusal
/// built for the purpose, so the day it gains one nothing here is untried.
fn dated_pair(
    stt: Result<BpsX100, Refusal>,
    exchange: Result<BpsX100, Refusal>,
) -> Result<(BpsX100, BpsX100), Refusal> {
    Ok((stt?, exchange?))
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
    let exchange = both_legs(
        &position,
        rates.exchange(),
        "the exchange transaction charge",
    )?;
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
fn signal_only(fills: Fills, quantity: i64, direction: Direction) -> Result<Charges, CostError> {
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

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    // A money literal is written `rupees_paisa` -- `120_00` reads as the
    // hundred and twenty rupees a contract note shows, where `12_000` reads as
    // nothing at all. Consistent through this module's tests.
    clippy::inconsistent_digit_grouping
)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use brutex_core::symbol::Symbol;

    use crate::error::Refusal;
    use crate::regime::{BSE_EXCHANGE_CHARGE, NSE_EXCHANGE_CHARGE};
    use crate::scope::ALL_SEGMENTS;

    /// The day every worked example in `COSTS_VERIFIED` §5 is dated.
    ///
    /// It sits in the post-1-Apr-2026 transaction-tax regime (0.15%), in the
    /// post-1-Oct-2024 exchange-charge regime, and in the January-2026 lot
    /// regimes — 65 for NIFTY and 30 for BANKNIFTY. Every one of those is read
    /// out of this crate's own dated tables rather than restated here.
    fn example_day() -> TradeDay {
        day(2026, 5, 15)
    }

    /// The [`Refusal`] inside a refusal, or `None` for any other error.
    ///
    /// A helper rather than a `let ... else` at each site: the `else` arm of a
    /// destructuring that always succeeds is a line no test can execute, and an
    /// unexecuted line is indistinguishable from an untested one.
    fn refusal_of(error: CostError) -> Option<Refusal> {
        match error {
            CostError::Unverified(refusal) => Some(refusal),
            _ => None,
        }
    }

    fn day(year: u16, month: u8, d: u8) -> TradeDay {
        TradeDay::new(year, month, d).expect("a real date")
    }

    fn slot(underlying: &str) -> SweptSlot {
        crate::venue::swept_slot(Symbol::new(underlying).expect("a valid symbol"))
            .expect("a swept underlying")
    }

    fn leg(on: TradeDay, price: i64) -> Leg {
        Leg::new(on, Bar::flat(Paisa::from_raw(price)).expect("a legal bar"))
    }

    fn ranged_leg(on: TradeDay, high: i64, low: i64) -> Leg {
        Leg::new(
            on,
            Bar::new(Paisa::from_raw(high), Paisa::from_raw(low)).expect("a legal bar"),
        )
    }

    fn option_contract(underlying: &str) -> Contract {
        Contract::new(slot(underlying), Segment::IndexOption)
    }

    /// A long Groww option round trip on flat bars, sized in lots.
    fn lots_trip(underlying: &str, on: TradeDay, entry: i64, exit: i64, lots: i64) -> RoundTrip {
        RoundTrip::in_lots(
            option_contract(underlying),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(on, entry),
            leg(on, exit),
            lots,
        )
        .expect("a well-formed round trip")
    }

    /// The fills a flat-bar long round trip produces.
    fn flat_fills(entry: i64, exit: i64, direction: Direction) -> Fills {
        worst_case_fills(
            Bar::flat(Paisa::from_raw(entry)).expect("legal"),
            Bar::flat(Paisa::from_raw(exit)).expect("legal"),
            direction,
        )
        .expect("in range")
    }

    /// The rate set the tables hold at a venue on a day.
    fn rates_on(exchange: Exchange, on: TradeDay) -> Rates {
        Rates::resolve(Broker::Groww, exchange, on).expect("a verified regime")
    }

    // -----------------------------------------------------------------------
    // The golden examples. `COSTS_VERIFIED` §5, byte for byte.
    //
    // These are the predecessor's own enforced oracles, regenerated by it from
    // the shipped calculator under the permanent adverse-ceiling levy law. A
    // calculator with no worked example is a calculator nobody has checked.
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_worked_example_prices_one_nifty_lot_to_the_paisa() {
        // Ex.1: NIFTY long call, 1 lot, premium 100.00 -> 120.00, 2026-05-15.
        // Lot 65 and every rate come out of this crate's dated tables; only the
        // premiums, the lot count and the answers are written here.
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 1))
            .expect("a verified regime");

        assert_eq!(charges.quantity(), 65, "one lot is 65 units in May 2026");
        assert_eq!(charges.buy_fill().raw(), 100_05);
        assert_eq!(charges.sell_fill().raw(), 119_95);
        assert_eq!(charges.buy_notional().raw(), 6_503_25);
        assert_eq!(charges.sell_notional().raw(), 7_796_75);

        assert_eq!(charges.brokerage().raw(), 40_00);
        assert_eq!(charges.stt().raw(), 12_00);
        // 228 + 274 per leg. Ceiling the combined notional once gives 501.
        assert_eq!(charges.exchange().raw(), 5_02);
        assert_eq!(charges.sebi().raw(), 2);
        assert_eq!(charges.ipft().raw(), 8);
        assert_eq!(charges.stamp().raw(), 1_00);
        assert_eq!(charges.gst().raw(), 9_00);

        assert_eq!(charges.total_charges().raw(), 67_12);
        assert_eq!(charges.gross_pnl().raw(), 1_293_50);
        assert_eq!(charges.net_pnl().raw(), 1_226_38);
        assert_eq!(charges.slippage().raw(), 6_50);
        assert_eq!(charges.realized_slip_per_unit().raw(), 10);
    }

    #[test]
    fn the_second_worked_example_amortises_the_flat_brokerage_over_five_lots() {
        // Ex.2: the same premiums at 5 lots. Every charge that scales with
        // quantity does; the brokerage does not, and that is the point.
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 5))
            .expect("a verified regime");

        assert_eq!(charges.quantity(), 325);
        assert_eq!(charges.buy_notional().raw(), 32_516_25);
        assert_eq!(charges.sell_notional().raw(), 38_983_75);
        assert_eq!(charges.brokerage().raw(), 40_00, "flat, whatever the size");
        assert_eq!(charges.stt().raw(), 59_00);
        assert_eq!(charges.exchange().raw(), 25_06);
        assert_eq!(charges.sebi().raw(), 8);
        assert_eq!(charges.ipft().raw(), 37);
        assert_eq!(charges.stamp().raw(), 1_00);
        assert_eq!(charges.gst().raw(), 12_00);
        assert_eq!(charges.total_charges().raw(), 137_51);
        assert_eq!(charges.gross_pnl().raw(), 6_467_50);
        assert_eq!(charges.net_pnl().raw(), 6_329_99);

        // Per lot: ₹27.50 against ₹67.12 at one lot. The brokerage amortises
        // and nothing else does.
        assert_eq!(charges.total_charges().raw() / 5, 27_50);
    }

    #[test]
    fn the_third_worked_example_prices_one_banknifty_lot() {
        // Ex.3: BANKNIFTY, 1 lot of 30, premium 50.00 -> 60.00. A different
        // underlying, a different lot, the same rate set.
        let charges = price(&lots_trip("BANKNIFTY", example_day(), 50_00, 60_00, 1))
            .expect("a verified regime");

        assert_eq!(charges.quantity(), 30, "one BANKNIFTY lot in May 2026");
        assert_eq!(charges.buy_notional().raw(), 1_501_50);
        assert_eq!(charges.sell_notional().raw(), 1_798_50);
        assert_eq!(charges.stt().raw(), 3_00);
        assert_eq!(charges.exchange().raw(), 1_17);
        assert_eq!(charges.sebi().raw(), 2);
        assert_eq!(charges.ipft().raw(), 2);
        assert_eq!(charges.stamp().raw(), 1_00);
        assert_eq!(charges.gst().raw(), 8_00);
        assert_eq!(charges.total_charges().raw(), 53_21);
        assert_eq!(charges.gross_pnl().raw(), 297_00);
        assert_eq!(charges.net_pnl().raw(), 243_79);
    }

    #[test]
    fn the_fourth_worked_example_prices_the_bse_rate_set_through_the_pure_core() {
        // Ex.4: SENSEX, 1 lot of 20, premium 80.00 -> 100.00, on BSE.
        //
        // SENSEX is NOT in `CLAUDE.md` §1's swept set, so there is no slot for
        // it and no lot table — and adding either would be a scope change
        // smuggled in as data. The example is still a golden: the BSE rate set
        // is real, it is reachable, and the quantity goes in as units. What is
        // being checked here is the BSE arithmetic, not a SENSEX instrument.
        let bse = rates_on(Exchange::Bse, example_day());
        assert_eq!(bse.exchange(), BSE_EXCHANGE_CHARGE);
        assert_eq!(bse.ipft().get(), 0, "the BSE IPFT is the UNVERIFIED zero");

        let charges = charge_stack(
            flat_fills(80_00, 100_00, Direction::Long),
            20,
            Direction::Long,
            &bse,
        )
        .expect("in range");
        assert_eq!(charges.buy_notional().raw(), 1_601_00);
        assert_eq!(charges.sell_notional().raw(), 1_999_00);
        assert_eq!(charges.stt().raw(), 3_00);
        assert_eq!(charges.exchange().raw(), 1_18);
        assert_eq!(charges.sebi().raw(), 2);
        assert_eq!(charges.ipft().raw(), 0);
        assert_eq!(charges.stamp().raw(), 1_00);
        assert_eq!(charges.gst().raw(), 8_00);
        assert_eq!(charges.total_charges().raw(), 53_20);
        assert_eq!(charges.gross_pnl().raw(), 398_00);
        assert_eq!(charges.net_pnl().raw(), 344_80);
    }

    #[test]
    fn the_fifth_worked_example_is_not_ported_and_the_reason_is_asserted() {
        // Ex.5 is a Muhurat session with ₹0 brokerage. The predecessor carries
        // an explicit banner saying its own calculator has no Muhurat branch
        // and that the example is spec-only. Neither does this one — asserted
        // here rather than left as a sentence, so that adding the waiver
        // without adding a test fails.
        let ordinary = price(&lots_trip("NIFTY", day(2026, 5, 15), 100_00, 120_00, 1))
            .expect("a verified regime");
        // 2024-11-01 was the Diwali Muhurat session the example names. It is a
        // different lot regime and a different tax regime, so only the
        // brokerage is compared — and it is the full flat fee, not zero.
        let muhurat = price(&lots_trip("NIFTY", day(2024, 11, 1), 100_00, 120_00, 1))
            .expect("a verified regime");
        assert_eq!(muhurat.brokerage(), ordinary.brokerage());
        assert_eq!(muhurat.brokerage().raw(), 40_00, "no session waiver exists");
    }

    // -----------------------------------------------------------------------
    // The five traps
    // -----------------------------------------------------------------------

    #[test]
    fn the_transaction_tax_reads_the_sell_premium_and_nothing_else() {
        // Trap 1, three ways.
        let rates = rates_on(Exchange::Nse, example_day());
        let charges = charge_stack(
            flat_fills(100_00, 120_00, Direction::Long),
            65,
            Direction::Long,
            &rates,
        )
        .expect("in range");

        // (a) It is the SELL notional, not the buy and not the sum.
        assert_eq!(
            charges.stt(),
            statutory_levy(charges.sell_notional(), rates.stt()).expect("in range")
        );
        assert_ne!(
            charges.stt(),
            statutory_levy(charges.buy_notional(), rates.stt()).expect("in range"),
            "the tax must not read the buy leg"
        );

        // (b) One side, not two. Charging both would roughly double it.
        let both_sides = statutory_levy(charges.buy_notional(), rates.stt())
            .expect("in range")
            .raw()
            + charges.stt().raw();
        assert_eq!(charges.stt().raw(), 12_00);
        assert_eq!(both_sides, 22_00, "the doubling error, priced out");

        // (c) On the PREMIUM, never on strike x quantity. A NIFTY 24,000 strike
        // at this quantity is a tax nearly two hundred times the right one.
        let strike_based =
            statutory_levy(Paisa::from_raw(24_000_00 * 65), rates.stt()).expect("in range");
        assert_eq!(strike_based.raw(), 2_340_00);
        assert!(
            strike_based.raw() > charges.stt().raw() * 100,
            "the strike-based error is not a rounding difference"
        );
    }

    #[test]
    fn the_stamp_duty_is_charged_on_the_buy_leg_and_exactly_once() {
        // Trap 2. The rate itself is zero on a sell, so the sell leg cannot be
        // charged even by a caller that tries.
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 5))
            .expect("a verified regime");
        let rates = rates_on(Exchange::Nse, example_day());

        assert_eq!(rates.stamp(), stamp_duty(OrderSide::Buy));
        assert_eq!(stamp_duty(OrderSide::Sell).get(), 0);
        assert_eq!(
            charges.stamp(),
            statutory_levy(charges.buy_notional(), stamp_duty(OrderSide::Buy)).expect("in range")
        );
        assert_eq!(
            statutory_levy(charges.sell_notional(), stamp_duty(OrderSide::Sell)).expect("in range"),
            Paisa::ZERO,
            "the sell leg's stamp rate is zero, so its charge is too"
        );
        // Both legs charged would be twice the figure, and both legs are big
        // enough here that the doubling is visible rather than lost in the
        // rupee ceiling.
        assert_eq!(charges.stamp().raw(), 1_00);
    }

    #[test]
    fn the_gst_is_rounded_once_and_rounding_each_half_overcharges_by_a_rupee() {
        // Trap 3, priced out on the worked example. 9% CGST and 9% SGST,
        // each ceiled to the rupee, then added, is the wrong arithmetic.
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 1))
            .expect("a verified regime");

        let base = Paisa::from_raw(
            charges.brokerage().raw()
                + charges.exchange().raw()
                + charges.sebi().raw()
                + charges.ipft().raw(),
        );
        assert_eq!(base.raw(), 45_12, "brokerage + exchange + SEBI + IPFT");
        // The tax and the stamp duty are taxes, not services, and are out.
        assert!(base.raw() < charges.total_charges().raw());

        let once = statutory_levy(base, GST_ON_FEE_BASE).expect("in range");
        let half = BpsX100::new(GST_ON_FEE_BASE.get() / 2);
        let twice = statutory_levy(base, half).expect("in range").raw() * 2;

        assert_eq!(charges.gst(), once);
        assert_eq!(once.raw(), 9_00);
        assert_eq!(twice, 10_00);
        assert_eq!(twice - once.raw(), 1_00, "exactly one rupee, every trade");
    }

    #[test]
    fn the_gst_base_is_the_sum_of_all_four_service_components_with_a_plus_sign() {
        // The shipped SEBI fee is two paisa on a real trade, so a sign error on
        // it inside the GST base is invisible behind the rupee ceiling: every
        // worked example still comes out right. It only shows when the fee is
        // large, so it is made large here, at a rate only this crate can mint.
        // (Found by mutation testing: `+ sebi` mutated to `- sebi` survived
        // every golden example.)
        let loud_sebi = Rates::with_all(
            Broker::Groww,
            BpsX100::new(15_000),
            NSE_EXCHANGE_CHARGE,
            ipft(Exchange::Nse),
            BpsX100::new(100_000),
            stamp_duty(OrderSide::Buy),
            GST_ON_FEE_BASE,
        );
        let charges = charge_stack(
            flat_fills(100_00, 120_00, Direction::Long),
            65,
            Direction::Long,
            &loud_sebi,
        )
        .expect("in range");

        assert_eq!(charges.brokerage().raw(), 40_00);
        assert_eq!(charges.exchange().raw(), 5_02);
        assert_eq!(charges.sebi().raw(), 143_01);
        assert_eq!(charges.ipft().raw(), 8);

        // The base is the four of them added, and the GST is one rounding of it.
        let base = Paisa::from_raw(40_00 + 5_02 + 143_01 + 8);
        assert_eq!(base.raw(), 188_11);
        assert_eq!(
            charges.gst(),
            statutory_levy(base, GST_ON_FEE_BASE).expect("in range")
        );
        assert_eq!(charges.gst().raw(), 34_00);

        // With the fee's sign flipped the base would be negative and the GST
        // would be a credit. It is not.
        let flipped = statutory_levy(Paisa::from_raw(188_11 - 2 * 143_01), GST_ON_FEE_BASE)
            .expect("in range");
        assert_eq!(flipped.raw(), -17_00);
        assert_ne!(
            charges.gst(),
            flipped,
            "the fee must enter the base as a plus"
        );

        // And the tax and the stamp duty are still OUT of the base: adding
        // either would move the answer at this magnitude.
        assert_eq!(charges.stt().raw(), 12_00);
        assert_eq!(charges.stamp().raw(), 1_00);
        let with_taxes = statutory_levy(Paisa::from_raw(188_11 + 12_00 + 1_00), GST_ON_FEE_BASE)
            .expect("in range");
        assert_ne!(charges.gst(), with_taxes, "taxes are not services");
        assert_eq!(charges.total_charges().raw(), 235_11);
    }

    #[test]
    fn the_brokerage_is_flat_per_order_and_a_thousand_lots_pay_what_one_pays() {
        // Trap 4. Everything else scales; this does not.
        let one = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 1))
            .expect("a verified regime");
        let many = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 1_000))
            .expect("a verified regime");

        assert_eq!(one.brokerage(), many.brokerage());
        assert_eq!(many.brokerage().raw(), 40_00);
        assert_eq!(many.quantity(), 65_000);
        // And it really is two orders of the per-order figure, not one.
        assert_eq!(
            many.brokerage().raw(),
            brokerage_per_order(Broker::Groww).raw() * 2
        );
        // Every other charge did scale.
        assert!(many.stt().raw() > one.stt().raw() * 900);
        assert!(many.exchange().raw() > one.exchange().raw() * 900);
        // Both brokers are priced, and both are flat.
        for broker in [Broker::Groww, Broker::Zerodha] {
            let rates = Rates::resolve(broker, Exchange::Nse, example_day()).expect("verified");
            assert_eq!(rates.brokerage_round_trip().raw(), 40_00);
        }
    }

    #[test]
    fn a_round_trip_in_the_unverified_window_refuses_the_whole_trip_and_names_it() {
        // Trap 5. Every date before 2024-10-01 has no verified exchange
        // transaction charge, and the answer is a refusal — not a component
        // priced at zero and not today's rate applied backwards.
        let trip = lots_trip("NIFTY", day(2024, 9, 30), 100_00, 120_00, 1);
        let refused = price(&trip).expect_err("the pre-boundary window has no verified rate");

        // A refusal, and not some other error wearing the same shape.
        assert_eq!(refusal_of(CostError::MalformedDate), None);
        let refusal = refusal_of(refused).expect("the refusal must carry its window");
        assert_eq!(
            refusal.charge(),
            "exchange transaction charge (options premium)"
        );
        assert_eq!(refusal.exchange(), Some(Exchange::Nse));
        assert_eq!(refusal.on(), day(2024, 9, 30));
        assert_eq!(refusal.window_start(), TradeDay::MIN);
        assert_eq!(refusal.verified_from(), Some(day(2024, 10, 1)));
        assert!(refusal.source().contains("SLAB ladder"));
        assert!(
            refusal
                .to_string()
                .contains("Refusing to fabricate a value")
        );
        assert!(refusal.remediation().contains("crates/costs/src/regime.rs"));

        // The day after prices, so the refusal is a window and not a blanket.
        assert!(price(&lots_trip("NIFTY", day(2024, 10, 1), 100_00, 120_00, 1)).is_ok());

        // And nothing partial escaped: there is no `Charges` at all.
        assert!(price(&trip).is_err());
    }

    // -----------------------------------------------------------------------
    // Regimes, dates and the entry-day key
    // -----------------------------------------------------------------------

    #[test]
    fn the_regime_is_the_entry_days_and_the_exit_day_never_moves_it() {
        // A round trip opened on 2026-03-31 and closed on 2026-04-01 straddles
        // the Finance Act 2026 boundary that lifts the tax from 0.10% to 0.15%.
        // It is priced at the regime it OPENED under.
        let before = day(2026, 3, 31);
        let on = day(2026, 4, 1);
        let contract = option_contract("NIFTY");

        let straddling = RoundTrip::new(
            contract,
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(before, 100_00),
            leg(on, 120_00),
            65,
        )
        .expect("well formed");
        let opened_after = RoundTrip::new(
            contract,
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(on, 100_00),
            leg(on, 120_00),
            65,
        )
        .expect("well formed");

        let straddling = price(&straddling).expect("verified");
        let opened_after = price(&opened_after).expect("verified");

        // The rates themselves, so the assertion is about the tax and not
        // about some other charge moving.
        assert_eq!(rates_on(Exchange::Nse, before).stt().get(), 10_000);
        assert_eq!(rates_on(Exchange::Nse, on).stt().get(), 15_000);

        // 0.10% of 779,675 paisa floors to 779, which ceils to ₹8.
        assert_eq!(straddling.stt().raw(), 8_00);
        // 0.15% of the same notional is ₹12 — the exit day's regime, which is
        // NOT what the straddling trip paid.
        assert_eq!(opened_after.stt().raw(), 12_00);
        assert_ne!(straddling.stt(), opened_after.stt());
        // Every other charge is identical, so the tax is the only difference.
        assert_eq!(straddling.exchange(), opened_after.exchange());
        assert_eq!(straddling.gross_pnl(), opened_after.gross_pnl());
        assert_eq!(
            opened_after.total_charges().raw() - straddling.total_charges().raw(),
            4_00
        );
    }

    #[test]
    fn the_lot_size_is_the_entry_days_and_a_pre_history_entry_refuses() {
        // The lot table is dated too, and the same entry-day key applies. The
        // 2024-11-20 SEBI notional revision tripled the NIFTY lot.
        let before = RoundTrip::in_lots(
            option_contract("NIFTY"),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(day(2024, 11, 19), 100_00),
            leg(day(2024, 11, 19), 120_00),
            1,
        )
        .expect("well formed");
        let on = RoundTrip::in_lots(
            option_contract("NIFTY"),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(day(2024, 11, 20), 100_00),
            leg(day(2024, 11, 20), 120_00),
            1,
        )
        .expect("well formed");
        assert_eq!(before.quantity(), 25);
        assert_eq!(on.quantity(), 75);

        // Before the source's recorded lot history there is no lot size, and
        // the refusal is carried out of the constructor word for word.
        let refused = RoundTrip::in_lots(
            option_contract("NIFTY"),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(day(2020, 12, 31), 100_00),
            leg(day(2020, 12, 31), 120_00),
            1,
        )
        .expect_err("the pre-history window has no lot size");
        let refusal = refusal_of(refused).expect("a lot-size refusal carries its window");
        assert_eq!(refusal.charge(), "options lot size (NIFTY)");
        assert!(refusal.remediation().contains("crates/costs/src/lot.rs"));
    }

    #[test]
    fn the_rate_set_is_the_one_the_tables_hold_on_the_day() {
        let nse = rates_on(Exchange::Nse, example_day());
        assert_eq!(
            nse.stt(),
            stt_options_rate(example_day()).expect("verified")
        );
        assert_eq!(nse.exchange(), NSE_EXCHANGE_CHARGE);
        assert_eq!(nse.ipft(), ipft(Exchange::Nse));
        assert_eq!(nse.sebi(), SEBI_TURNOVER_FEE);
        assert_eq!(nse.stamp(), stamp_duty(OrderSide::Buy));
        assert_eq!(nse.gst(), GST_ON_FEE_BASE);
        assert_eq!(nse.brokerage_round_trip().raw(), 40_00);

        // `new` and `resolve` agree, so the pure core and the entry point are
        // priced identically.
        let assembled = Rates::new(
            Broker::Groww,
            stt_options_rate(example_day()).expect("verified"),
            NSE_EXCHANGE_CHARGE,
            ipft(Exchange::Nse),
        );
        assert_eq!(assembled, nse);
        assert_ne!(assembled, rates_on(Exchange::Bse, example_day()));
        assert!(format!("{nse:?}").starts_with("Rates {"));
        // Zerodha's schedule is its own citation and is priced too.
        let zerodha = Rates::resolve(Broker::Zerodha, Exchange::Nse, example_day()).expect("ok");
        assert_eq!(zerodha.brokerage_round_trip().raw(), 40_00);
    }

    // -----------------------------------------------------------------------
    // Losses, and charges that swamp them
    // -----------------------------------------------------------------------

    #[test]
    fn a_losing_round_trip_reports_a_negative_net_and_still_pays_every_charge() {
        // Premium halves. Every charge is still due — a loss is not a discount.
        let charges =
            price(&lots_trip("NIFTY", example_day(), 100_00, 50_00, 1)).expect("a verified regime");

        assert_eq!(charges.buy_notional().raw(), 6_503_25);
        assert_eq!(charges.sell_notional().raw(), 3_246_75);
        assert_eq!(charges.gross_pnl().raw(), -3_256_50);
        assert!(charges.gross_pnl().raw() < 0);
        assert!(charges.net_pnl().raw() < charges.gross_pnl().raw());
        assert_eq!(
            charges.net_pnl().raw(),
            charges.gross_pnl().raw() - charges.total_charges().raw()
        );
        // Every charge is strictly positive on a loss, and the tax is levied on
        // the (smaller) sell notional rather than waived.
        for (name, amount) in charges.itemised() {
            assert!(amount.raw() > 0, "{name} was not charged on a losing trade");
        }
        assert_eq!(charges.stt().raw(), 5_00);
        assert_eq!(charges.net_pnl().raw(), -3_314_00);
    }

    #[test]
    fn a_round_trip_whose_charges_exceed_its_gross_reports_a_negative_net() {
        // A one-tick winner on a single lot: the gross is real and positive,
        // the flat brokerage alone is larger, and the net is negative. This is
        // the case a cost model exists to surface.
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 100_15, 1))
            .expect("a verified regime");

        assert_eq!(charges.buy_fill().raw(), 100_05);
        assert_eq!(charges.sell_fill().raw(), 100_10);
        assert_eq!(charges.gross_pnl().raw(), 3_25);
        assert!(
            charges.gross_pnl().raw() > 0,
            "the gross really is a profit"
        );
        assert!(
            charges.total_charges().raw() > charges.gross_pnl().raw(),
            "the charges must swamp it"
        );
        assert_eq!(charges.total_charges().raw(), 64_66);
        assert_eq!(charges.net_pnl().raw(), -61_41);
        assert!(charges.net_pnl().raw() < 0);
        // The brokerage on its own already exceeds the gross.
        assert!(charges.brokerage().raw() > charges.gross_pnl().raw());
    }

    #[test]
    fn a_short_round_trip_inverts_the_gross_and_fills_off_the_other_two_bars() {
        let contract = option_contract("NIFTY");
        let entry = ranged_leg(example_day(), 101_00, 99_00);
        let exit = ranged_leg(example_day(), 121_00, 119_00);
        let build = |direction: Direction| {
            price(
                &RoundTrip::new(
                    contract,
                    direction,
                    Broker::Groww,
                    Outcome::NormalClose,
                    entry,
                    exit,
                    65,
                )
                .expect("well formed"),
            )
            .expect("verified")
        };

        let long = build(Direction::Long);
        let short = build(Direction::Short);

        // Long: buy the entry HIGH plus a tick, sell the exit LOW less a tick.
        assert_eq!(long.buy_fill().raw(), 101_05);
        assert_eq!(long.sell_fill().raw(), 118_95);
        assert_eq!(long.gross_pnl().raw(), 1_163_50);
        // Short: the cover buys the exit HIGH plus a tick and the opening sell
        // takes the entry LOW less a tick. Adverse on both legs.
        assert_eq!(short.buy_fill().raw(), 121_05);
        assert_eq!(short.sell_fill().raw(), 98_95);
        assert_eq!(
            short.gross_pnl().raw(),
            short.buy_notional().raw() - short.sell_notional().raw()
        );
        assert_eq!(short.gross_pnl().raw(), 1_436_50);
        assert!(short.gross_pnl().raw() > 0, "a short profits when it falls");
        // The tax still reads the SELL notional on a short, which is the
        // OPENING leg there.
        assert_eq!(short.sell_notional().raw(), 6_431_75);
        assert_eq!(
            short.net_pnl().raw(),
            short.gross_pnl().raw() - short.total_charges().raw()
        );
    }

    // -----------------------------------------------------------------------
    // Signal-only segments
    // -----------------------------------------------------------------------

    #[test]
    fn a_signal_only_segment_pays_nothing_and_its_net_is_its_gross() {
        // An index spot or an index future is priced signal-only: the fills are
        // the same worst-case fills, and there is no charge stack at all.
        for segment in [Segment::IndexSpot, Segment::IndexFuture] {
            let trip = RoundTrip::new(
                Contract::new(slot("NIFTY"), segment),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(example_day(), 24_000_00),
                leg(example_day(), 24_100_00),
                75,
            )
            .expect("well formed");
            let charges = price(&trip).expect("no rate is consulted");

            assert_eq!(
                charges.total_charges(),
                Paisa::ZERO,
                "{segment} paid a charge"
            );
            assert_eq!(charges.net_pnl(), charges.gross_pnl());
            for (name, amount) in charges.itemised() {
                assert_eq!(amount, Paisa::ZERO, "{segment} paid {name}");
            }
            // The FILLS are unchanged: cost-free removes the charges, never the
            // fill law.
            assert_eq!(charges.buy_fill().raw(), 24_000_05);
            assert_eq!(charges.sell_fill().raw(), 24_099_95);
            assert_eq!(charges.gross_pnl().raw(), 7_492_50);
            assert_eq!(charges.slippage().raw(), 750);
        }
    }

    #[test]
    fn a_signal_only_segment_prices_inside_a_window_where_every_rate_refuses() {
        // The whole point of resolving no rate: an index spot backtest over
        // 2019 works, in a window where every exchange transaction charge
        // refuses. A cost-free arm that still consulted a rate would fail here.
        let ancient = day(2019, 6, 3);
        assert!(Rates::resolve(Broker::Groww, Exchange::Nse, ancient).is_err());

        let trip = RoundTrip::new(
            Contract::new(slot("NIFTY"), Segment::IndexSpot),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(ancient, 11_800_00),
            leg(ancient, 11_850_00),
            1,
        )
        .expect("well formed");
        let charges = price(&trip).expect("a cost-free trip consults no rate");
        assert_eq!(charges.total_charges(), Paisa::ZERO);
        assert_eq!(charges.net_pnl().raw(), 49_90);

        // And the option on the same day still refuses, so the short-circuit
        // did not weaken the contract.
        let option = RoundTrip::new(
            Contract::new(slot("NIFTY"), Segment::IndexOption),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(ancient, 11_800_00),
            leg(ancient, 11_850_00),
            1,
        )
        .expect("well formed");
        assert!(price(&option).is_err());
    }

    #[test]
    fn a_lot_count_is_refused_for_a_segment_the_options_lot_table_does_not_cover() {
        // The dated table is the OPTIONS lot history. Sizing an index future
        // from it would wear an options circular's citation.
        for segment in [Segment::IndexSpot, Segment::IndexFuture] {
            let refused = RoundTrip::in_lots(
                Contract::new(slot("NIFTY"), segment),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(example_day(), 100_00),
                leg(example_day(), 120_00),
                1,
            )
            .expect_err("no options lot applies");
            assert_eq!(
                refused,
                CostError::LotSizeNotApplicable {
                    segment: segment.as_str()
                }
            );
        }
        // The option segment is the one that has a lot table.
        assert!(
            RoundTrip::in_lots(
                option_contract("NIFTY"),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(example_day(), 100_00),
                leg(example_day(), 120_00),
                1,
            )
            .is_ok()
        );
    }

    // -----------------------------------------------------------------------
    // Refusals on the way in
    // -----------------------------------------------------------------------

    #[test]
    fn an_expiry_outcome_is_refused_rather_than_priced_as_a_normal_close() {
        for outcome in [
            Outcome::ExpiryExercise,
            Outcome::ExpiryAssign,
            Outcome::ExpiryWorthless,
        ] {
            let trip = RoundTrip::new(
                option_contract("NIFTY"),
                Direction::Long,
                Broker::Groww,
                outcome,
                leg(example_day(), 100_00),
                leg(example_day(), 120_00),
                65,
            )
            .expect("well formed");
            assert!(!outcome.is_priced());
            assert_eq!(
                price(&trip),
                Err(CostError::UnsupportedOutcome {
                    outcome: outcome.as_str()
                }),
                "{outcome} was priced as a normal close"
            );
        }
        assert!(Outcome::NormalClose.is_priced());
        assert_eq!(Outcome::default(), Outcome::NormalClose);
        assert_eq!(Outcome::NormalClose.to_string(), "normal close");
        assert_eq!(format!("{:?}", Outcome::ExpiryAssign), "ExpiryAssign");
        assert!(Outcome::NormalClose < Outcome::ExpiryWorthless);
    }

    #[test]
    fn a_round_trip_of_no_contracts_is_refused_by_name_on_every_path() {
        // The constructor refuses it.
        let refused = RoundTrip::new(
            option_contract("NIFTY"),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(example_day(), 100_00),
            leg(example_day(), 120_00),
            0,
        )
        .expect_err("a trade of nothing is not a trade");
        assert_eq!(
            refused,
            CostError::NotPositive {
                quantity: "quantity",
                value: 0
            }
        );
        // A negative quantity is a direction in the wrong field.
        assert_eq!(
            RoundTrip::new(
                option_contract("NIFTY"),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(example_day(), 100_00),
                leg(example_day(), 120_00),
                -65,
            ),
            Err(CostError::NotPositive {
                quantity: "quantity",
                value: -65
            })
        );
        // Zero LOTS is refused too, by the lot arithmetic, before the quantity
        // guard is ever reached.
        assert_eq!(
            RoundTrip::in_lots(
                option_contract("NIFTY"),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(example_day(), 100_00),
                leg(example_day(), 120_00),
                0,
            ),
            Err(CostError::NotPositive {
                quantity: "lots",
                value: 0
            })
        );
        // And the pure core refuses it independently, because it is public and
        // a caller can reach it without a `RoundTrip` at all.
        let rates = rates_on(Exchange::Nse, example_day());
        assert_eq!(
            charge_stack(
                flat_fills(100_00, 120_00, Direction::Long),
                0,
                Direction::Long,
                &rates
            ),
            Err(CostError::NotPositive {
                quantity: "quantity",
                value: 0
            })
        );
    }

    #[test]
    fn an_exit_day_before_its_entry_day_is_refused() {
        let refused = RoundTrip::new(
            option_contract("NIFTY"),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(day(2026, 5, 15), 100_00),
            leg(day(2026, 5, 14), 120_00),
            65,
        )
        .expect_err("a round trip does not close before it opens");
        assert_eq!(
            refused,
            CostError::ExitBeforeEntry {
                entry: day(2026, 5, 15),
                exit: day(2026, 5, 14),
            }
        );
        // The same day is fine — every Track-2 trip is intraday.
        assert!(
            RoundTrip::new(
                option_contract("NIFTY"),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(day(2026, 5, 15), 100_00),
                leg(day(2026, 5, 15), 120_00),
                65,
            )
            .is_ok()
        );
        // And so is a later exit.
        assert!(
            RoundTrip::in_lots(
                option_contract("NIFTY"),
                Direction::Long,
                Broker::Groww,
                Outcome::NormalClose,
                leg(day(2026, 5, 15), 100_00),
                leg(day(2026, 5, 18), 120_00),
                1,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_malformed_fill_bar_is_refused_before_anything_is_priced() {
        // The fill law's refusals reach the round trip unchanged.
        let trip = RoundTrip::new(
            option_contract("NIFTY"),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            Leg::new(
                example_day(),
                Bar::new(Paisa::from_raw(i64::MAX), Paisa::from_raw(10)).expect("legal"),
            ),
            leg(example_day(), 120_00),
            65,
        )
        .expect("well formed");
        assert_eq!(
            price(&trip),
            Err(CostError::Overflow {
                operation: "the buy fill"
            })
        );
    }

    // -----------------------------------------------------------------------
    // Overflow: every named site, refused rather than wrapped
    // -----------------------------------------------------------------------

    /// A rate set built from minted rates.
    ///
    /// `BpsX100::new` is crate-private, so only a test inside this crate can
    /// build one this extreme — which is the guarantee, not a gap in it.
    fn minted(stt: i64, exchange: i64, ipft_rate: i64) -> Rates {
        Rates::new(
            Broker::Groww,
            BpsX100::new(stt),
            BpsX100::new(exchange),
            BpsX100::new(ipft_rate),
        )
    }

    #[test]
    fn every_position_overflow_site_is_refused_by_name_and_never_wrapped() {
        let rates = minted;
        let tame = rates(15_000, 3_503, 50);

        // (1) The buy notional.
        assert_eq!(
            charge_stack(
                flat_fills(1_000_000_000_000, 1_000_000_000_000, Direction::Long),
                1_000_000_000,
                Direction::Long,
                &tame,
            ),
            Err(CostError::Overflow {
                operation: "the buy notional"
            })
        );

        // (2) The sell notional, with the buy leg small enough to survive: a
        // one-tick entry bar and an enormous exit bar.
        let lopsided = worst_case_fills(
            Bar::flat(TICK_HELPER).expect("legal"),
            Bar::flat(Paisa::from_raw(i64::MAX / 4)).expect("legal"),
            Direction::Long,
        )
        .expect("in range");
        assert_eq!(
            charge_stack(lopsided, 1_000_000_000, Direction::Long, &tame),
            Err(CostError::Overflow {
                operation: "the sell notional"
            })
        );

        // (9) The slippage line: a bar whose low is deeply negative gives an
        //     enormous per-unit slippage on tiny notionals.
        let slippery = worst_case_fills(
            Bar::flat(TICK_HELPER).expect("legal"),
            Bar::new(
                Paisa::from_raw(10),
                Paisa::from_raw(-900_000_000_000_000_000),
            )
            .expect("legal"),
            Direction::Long,
        )
        .expect("in range");
        assert_eq!(
            charge_stack(slippery, 1_000, Direction::Long, &tame),
            Err(CostError::Overflow {
                operation: "the slippage line"
            })
        );

        // (10) The net: a gross at the bottom of i64 and any charge at all.
        let ruinous = worst_case_fills(
            Bar::flat(Paisa::from_raw(i64::MAX - 5)).expect("legal"),
            Bar::new(Paisa::from_raw(10), Paisa::from_raw(10)).expect("legal"),
            Direction::Long,
        )
        .expect("in range");
        assert_eq!(
            charge_stack(ruinous, 1, Direction::Long, &rates(0, 0, 0)),
            Err(CostError::Overflow {
                operation: "the net profit and loss"
            })
        );
    }

    #[test]
    fn every_levy_overflow_site_is_refused_by_name_and_never_wrapped() {
        let rates = minted;

        // (3) The transaction tax, at a rate that leaves i64 on a real notional.
        let big = flat_fills(1_000_000_000, 1_000_000_000, Direction::Long);
        assert_eq!(
            charge_stack(
                big,
                1_000_000_000,
                Direction::Long,
                &rates(1_000_000_000, 0, 0)
            ),
            Err(CostError::Overflow {
                operation: "flooring a statutory levy to the paisa"
            })
        );

        // (4) The exchange charge's per-leg ceiling.
        assert_eq!(
            charge_stack(
                big,
                1_000_000_000,
                Direction::Long,
                &rates(0, 1_000_000_000, 0)
            ),
            Err(CostError::Overflow {
                operation: "ceiling a levy to the paisa"
            })
        );

        // (5) The exchange charge's two-leg SUM, with each leg in range.
        //     Each leg is about 5e18; their sum is not.
        let both_legs_overflow = rates(0, 50_000_000, 0);
        assert_eq!(
            charge_stack(big, 1_000_000_000, Direction::Long, &both_legs_overflow),
            Err(CostError::Overflow {
                operation: "the exchange transaction charge"
            })
        );

        // (6) The IPFT, reached only because the exchange charge succeeded
        //     first — so the ordering of the stack is being asserted too.
        assert_eq!(
            charge_stack(
                big,
                1_000_000_000,
                Direction::Long,
                &rates(0, 0, 50_000_000)
            ),
            Err(CostError::Overflow {
                operation: "the investor protection fund"
            })
        );

        // (7) The GST base: the exchange charge and the IPFT each fit, and
        //     their sum does not.
        assert_eq!(
            charge_stack(
                big,
                1_000_000_000,
                Direction::Long,
                &rates(0, 25_000_000, 25_000_000)
            ),
            Err(CostError::Overflow {
                operation: "the GST base"
            })
        );

        // (8) The total: the tax is about 4.95e18 and the exchange charge about
        //     5.04e18, each comfortably inside i64, and their sum is not.
        assert_eq!(
            charge_stack(
                flat_fills(1_000_000_000, 1_000_000_000, Direction::Long),
                9_000_000_000,
                Direction::Long,
                &rates(5_500_000, 2_800_000, 0),
            ),
            Err(CostError::Overflow {
                operation: "the total charges"
            })
        );
    }

    #[test]
    fn every_flat_levy_overflow_site_is_refused_by_name_and_never_wrapped() {
        // The SEBI fee, the stamp duty and the GST rate are FLAT and small, so
        // no notional inside i64 can overflow them. Their guards are still
        // real code, and an untested guard is indistinguishable from a missing
        // one — so they are exercised against rates only this crate can mint.
        let big = flat_fills(1_000_000_000, 1_000_000_000, Direction::Long);
        let quiet = BpsX100::ZERO;
        let loud = BpsX100::new(1_000_000_000);

        // The SEBI fee, reached only because the transaction tax and the
        // exchange charge succeeded first — so the order is asserted too.
        assert_eq!(
            charge_stack(
                big,
                1_000_000_000,
                Direction::Long,
                &Rates::with_all(Broker::Groww, quiet, quiet, quiet, loud, quiet, quiet),
            ),
            Err(CostError::Overflow {
                operation: "ceiling a levy to the paisa"
            })
        );

        // The stamp duty, reached after all three per-leg levies succeeded.
        assert_eq!(
            charge_stack(
                big,
                1_000_000_000,
                Direction::Long,
                &Rates::with_all(Broker::Groww, quiet, quiet, quiet, quiet, loud, quiet),
            ),
            Err(CostError::Overflow {
                operation: "flooring a statutory levy to the paisa"
            })
        );

        // GST, reached last: the base is a real 2e15 built from a large
        // exchange charge, and the levy on it is what leaves i64.
        assert_eq!(
            charge_stack(
                flat_fills(1_000_000_000, 1_000_000_000, Direction::Long),
                1_000_000,
                Direction::Long,
                &Rates::with_all(
                    Broker::Groww,
                    quiet,
                    BpsX100::new(crate::rate::RATE_SCALE),
                    quiet,
                    quiet,
                    quiet,
                    BpsX100::new(100_000_000_000),
                ),
            ),
            Err(CostError::Overflow {
                operation: "flooring a statutory levy to the paisa"
            })
        );

        // And `with_all` really is `new` when handed the shipped flat rates —
        // so the test-only constructor cannot drift from the public one.
        let shipped = Rates::with_all(
            Broker::Groww,
            BpsX100::new(15_000),
            NSE_EXCHANGE_CHARGE,
            ipft(Exchange::Nse),
            SEBI_TURNOVER_FEE,
            stamp_duty(OrderSide::Buy),
            GST_ON_FEE_BASE,
        );
        assert_eq!(
            shipped,
            Rates::new(
                Broker::Groww,
                BpsX100::new(15_000),
                NSE_EXCHANGE_CHARGE,
                ipft(Exchange::Nse)
            )
        );
    }

    #[test]
    fn the_sell_leg_of_a_per_leg_levy_is_guarded_independently_of_the_buy_leg() {
        // A one-tick entry bar and an enormous exit bar: the buy leg's levy is
        // a thousand paisa and the sell leg's leaves i64. If the two legs
        // shared a guard, this would be indistinguishable from the buy leg's
        // own overflow, which is a different defect with a different cause.
        let lopsided = worst_case_fills(
            Bar::flat(TICK_HELPER).expect("legal"),
            Bar::flat(Paisa::from_raw(i64::MAX / 4)).expect("legal"),
            Direction::Long,
        )
        .expect("in range");
        let quiet = BpsX100::ZERO;
        let rates = Rates::with_all(
            Broker::Groww,
            quiet,
            BpsX100::new(1_000_000_000),
            quiet,
            quiet,
            quiet,
            quiet,
        );
        // The buy leg on its own is fine at this rate.
        assert_eq!(
            levy_ceiling(Paisa::from_raw(10), BpsX100::new(1_000_000_000))
                .expect("in range")
                .raw(),
            1_000
        );
        assert_eq!(
            charge_stack(lopsided, 1, Direction::Long, &rates),
            Err(CostError::Overflow {
                operation: "ceiling a levy to the paisa"
            })
        );
    }

    #[test]
    fn a_signal_only_round_trip_guards_its_arithmetic_too() {
        // Cost-free removes the charges, not the overflow guards: the notional
        // is still computed, and a quantity that leaves i64 is still refused.
        let trip = RoundTrip::new(
            Contract::new(slot("NIFTY"), Segment::IndexSpot),
            Direction::Long,
            Broker::Groww,
            Outcome::NormalClose,
            leg(example_day(), 1_000_000_000),
            leg(example_day(), 1_000_000_000),
            1_000_000_000_000,
        )
        .expect("well formed");
        assert_eq!(
            price(&trip),
            Err(CostError::Overflow {
                operation: "the buy notional"
            })
        );
    }

    #[test]
    fn the_dated_pair_carries_whichever_of_the_two_lookups_refused() {
        // The transaction-tax table has no unverified row today, so its guard
        // cannot be reached through a date. It is reached here directly,
        // against a refusal built for the purpose, so that the day the table
        // gains one there is nothing untried on the path.
        let tax_refusal = Refusal::new(
            "securities transaction tax (options sell premium)",
            None,
            example_day(),
            TradeDay::MIN,
            None,
            "a window this table does not carry today",
        );
        let charge_refusal = exchange_charge_rate(Exchange::Nse, day(2024, 9, 30))
            .expect_err("the pre-boundary window refuses");
        let rate = stt_options_rate(example_day()).expect("verified");

        assert_eq!(
            dated_pair(Err(tax_refusal), Ok(rate)),
            Err(tax_refusal),
            "the tax's refusal must be the one carried"
        );
        assert_eq!(
            dated_pair(Ok(rate), Err(charge_refusal)),
            Err(charge_refusal),
            "the exchange charge's refusal must be the one carried"
        );
        // The tax is checked FIRST, so it wins when both refuse.
        assert_eq!(
            dated_pair(Err(tax_refusal), Err(charge_refusal)),
            Err(tax_refusal)
        );
        assert_eq!(dated_pair(Ok(rate), Ok(rate)), Ok((rate, rate)));
    }

    /// One tick, as a `Paisa`. Written once so the overflow table above reads.
    const TICK_HELPER: Paisa = crate::rate::TICK;

    #[test]
    fn the_statutory_rupee_ceiling_is_reachable_from_the_stack_and_refuses() {
        // A raw statutory levy inside i64 whose CEILING to the whole rupee is
        // not. The tax stage is where it is reachable: a sell notional of
        // `i64::MAX - 5` at a rate of exactly one rate-scale unit floors to
        // itself, and the next whole rupee above it is past `i64::MAX`.
        //
        // The GST stage cannot reach this arm, and that is arithmetic rather
        // than an untested corner: the GST base is itself an i64, and 18% of
        // any i64 is at most a fifth of one, so its raw can never come within a
        // rupee of the ceiling. Recorded in docs/06-limits.md section 27.
        let fills = worst_case_fills(
            Bar::flat(Paisa::from_raw(10)).expect("legal"),
            Bar::new(Paisa::from_raw(i64::MAX), Paisa::from_raw(i64::MAX)).expect("legal"),
            Direction::Long,
        )
        .expect("in range");
        assert_eq!(fills.sell().raw(), i64::MAX - 5, "not a whole rupee");
        assert_ne!((i64::MAX - 5) % 100, 0);

        let rates = Rates::new(
            Broker::Groww,
            BpsX100::new(crate::rate::RATE_SCALE),
            BpsX100::ZERO,
            BpsX100::ZERO,
        );
        assert_eq!(
            charge_stack(fills, 1, Direction::Long, &rates),
            Err(CostError::Overflow {
                operation: "ceiling a statutory levy to the rupee"
            })
        );
    }

    // -----------------------------------------------------------------------
    // The breakdown's own laws
    // -----------------------------------------------------------------------

    #[test]
    fn a_breakdown_that_broke_its_own_law_is_refused_rather_than_reported() {
        let good = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 1))
            .expect("a verified regime");
        assert_eq!(good.validated(), Ok(good));

        // A total that is not the sum of its parts.
        let bad_total = Charges {
            total: Paisa::from_raw(good.total_charges().raw() + 1),
            ..good
        };
        assert_eq!(
            bad_total.validated(),
            Err(CostError::Inconsistent {
                law: "total == the sum of its components"
            })
        );

        // A net wired to the gross — the classic mis-wiring, and the reason
        // this check exists at all.
        let bad_net = Charges {
            net_pnl: good.gross_pnl(),
            ..good
        };
        assert_eq!(
            bad_net.validated(),
            Err(CostError::Inconsistent {
                law: "net == gross - total"
            })
        );

        // A single component moved: the total no longer matches.
        let bad_component = Charges {
            stt: Paisa::from_raw(good.stt().raw() - 100),
            ..good
        };
        assert!(bad_component.validated().is_err());
    }

    #[test]
    fn the_breakdown_itemises_seven_charges_that_sum_to_its_total() {
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 3))
            .expect("a verified regime");
        let items = charges.itemised();
        assert_eq!(items.len(), CHARGE_COUNT);
        assert_eq!(CHARGE_COUNT, 7);

        let names: [&str; CHARGE_COUNT] = [
            items[0].0, items[1].0, items[2].0, items[3].0, items[4].0, items[5].0, items[6].0,
        ];
        assert_eq!(
            names,
            [
                "brokerage",
                "securities transaction tax",
                "exchange transaction charge",
                "SEBI turnover fee",
                "investor protection fund",
                "stamp duty",
                "GST",
            ]
        );

        let mut sum = 0_i64;
        for (_, amount) in items {
            sum += amount.raw();
        }
        assert_eq!(sum, charges.total_charges().raw());
        // Each line is the accessor's own figure, so the itemisation cannot
        // drift from the fields it reports.
        assert_eq!(items[0].1, charges.brokerage());
        assert_eq!(items[1].1, charges.stt());
        assert_eq!(items[2].1, charges.exchange());
        assert_eq!(items[3].1, charges.sebi());
        assert_eq!(items[4].1, charges.ipft());
        assert_eq!(items[5].1, charges.stamp());
        assert_eq!(items[6].1, charges.gst());
    }

    #[test]
    fn a_breakdown_renders_every_figure_it_holds() {
        let charges = price(&lots_trip("NIFTY", example_day(), 100_00, 120_00, 1))
            .expect("a verified regime");
        assert_eq!(
            charges.to_string(),
            "65 units, buy 10005 sell 11995 (notionals 650325 / 779675); \
             brokerage 4000 + STT 1200 + exchange 502 + SEBI 2 + IPFT 8 + stamp 100 + GST 900 \
             = 6712 charges; gross 129350, net 122638 (slippage 650 at 10 per unit)"
        );
    }

    // -----------------------------------------------------------------------
    // The laws, across a swept envelope
    // -----------------------------------------------------------------------

    #[test]
    fn the_internal_laws_hold_across_a_deterministic_sweep_of_the_envelope() {
        // Not a random search: a fixed grid, so a failure is reproducible and
        // the count below is an assertion rather than a hope.
        let rates = rates_on(Exchange::Nse, example_day());
        let mut checked = 0_u32;
        let mut losses = 0_u32;
        let mut swamped = 0_u32;

        for entry in [5_i64, 55, 100_00, 2_500_00, 10_000_000_00] {
            for exit in [5_i64, 55, 100_00, 2_500_00, 10_000_000_00] {
                for quantity in [1_i64, 15, 65, 75_000] {
                    for direction in [Direction::Long, Direction::Short] {
                        let fills = flat_fills(entry, exit, direction);
                        let charges = charge_stack(fills, quantity, direction, &rates)
                            .expect("inside the envelope");

                        // The two internal laws.
                        assert_eq!(
                            charges.net_pnl().raw(),
                            charges.gross_pnl().raw() - charges.total_charges().raw()
                        );
                        let mut sum = 0_i64;
                        for (_, amount) in charges.itemised() {
                            sum += amount.raw();
                        }
                        assert_eq!(sum, charges.total_charges().raw());

                        // The notionals are the fills times the quantity.
                        assert_eq!(
                            charges.buy_notional().raw(),
                            charges.buy_fill().raw() * quantity
                        );
                        assert_eq!(
                            charges.sell_notional().raw(),
                            charges.sell_fill().raw() * quantity
                        );
                        assert_eq!(
                            charges.slippage().raw(),
                            charges.realized_slip_per_unit().raw() * quantity
                        );

                        // Statutory levies land on whole rupees; the others do
                        // not have to.
                        assert_eq!(charges.stt().raw() % 100, 0, "the tax is a whole rupee");
                        assert_eq!(charges.stamp().raw() % 100, 0, "stamp is a whole rupee");
                        assert_eq!(charges.gst().raw() % 100, 0, "GST is a whole rupee");

                        // Brokerage never moves.
                        assert_eq!(charges.brokerage().raw(), 40_00);
                        // Nothing is negative, and the total is at least the
                        // brokerage.
                        for (name, amount) in charges.itemised() {
                            assert!(amount.raw() >= 0, "{name} was negative");
                        }
                        assert!(charges.total_charges().raw() >= 40_00);

                        if charges.gross_pnl().raw() < 0 {
                            losses += 1;
                        }
                        if charges.gross_pnl().raw() > 0
                            && charges.total_charges().raw() > charges.gross_pnl().raw()
                        {
                            swamped += 1;
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 5 * 5 * 4 * 2);
        assert!(losses > 0, "the sweep must include losing trips");
        assert!(swamped > 0, "and trips whose charges exceed the gross");
        assert_eq!(losses, 100);
        assert_eq!(swamped, 24);
    }

    #[test]
    fn the_entry_point_and_the_pure_core_agree_on_every_swept_combination() {
        // `price` must be `charge_stack` plus the resolution, and nothing else.
        let mut checked = 0_u32;
        for underlying in ["NIFTY", "BANKNIFTY"] {
            for on in [day(2024, 10, 1), day(2025, 7, 1), example_day()] {
                for direction in [Direction::Long, Direction::Short] {
                    for segment in ALL_SEGMENTS {
                        let trip = RoundTrip::new(
                            Contract::new(slot(underlying), segment),
                            direction,
                            Broker::Zerodha,
                            Outcome::NormalClose,
                            leg(on, 100_00),
                            leg(on, 120_00),
                            50,
                        )
                        .expect("well formed");
                        let priced = price(&trip).expect("a verified regime");

                        let fills = flat_fills(100_00, 120_00, direction);
                        let expected = if is_cost_free(segment) {
                            signal_only(fills, 50, direction).expect("in range")
                        } else {
                            let rates =
                                Rates::resolve(Broker::Zerodha, Exchange::Nse, on).expect("ok");
                            charge_stack(fills, 50, direction, &rates).expect("in range")
                        };
                        assert_eq!(priced, expected, "{underlying} {on} {segment} {direction}");
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 2 * 3 * 2 * 3);
    }

    // -----------------------------------------------------------------------
    // The small types are plain, comparable data
    // -----------------------------------------------------------------------

    #[test]
    fn a_round_trip_reports_back_everything_it_was_built_from() {
        let contract = option_contract("BANKNIFTY");
        let entry = leg(day(2026, 5, 15), 100_00);
        let exit = leg(day(2026, 5, 18), 120_00);
        let trip = RoundTrip::new(
            contract,
            Direction::Short,
            Broker::Zerodha,
            Outcome::NormalClose,
            entry,
            exit,
            30,
        )
        .expect("well formed");

        assert_eq!(trip.contract(), contract);
        assert_eq!(trip.contract().underlying(), slot("BANKNIFTY"));
        assert_eq!(trip.contract().segment(), Segment::IndexOption);
        assert_eq!(trip.contract().exchange(), Exchange::Nse);
        assert_eq!(trip.direction(), Direction::Short);
        assert_eq!(trip.broker(), Broker::Zerodha);
        assert_eq!(trip.outcome(), Outcome::NormalClose);
        assert_eq!(trip.entry(), entry);
        assert_eq!(trip.entry().day(), day(2026, 5, 15));
        assert_eq!(trip.entry().bar().high().raw(), 100_00);
        assert_eq!(trip.exit(), exit);
        assert_eq!(trip.exit().bar().low().raw(), 120_00);
        assert_eq!(trip.quantity(), 30);

        // Plain data: comparable, hashable, debuggable.
        let mut set = HashSet::new();
        assert!(set.insert(trip));
        assert!(!set.insert(trip));
        assert_eq!(set.len(), 1);
        assert!(
            trip < RoundTrip::new(
                contract,
                Direction::Short,
                Broker::Zerodha,
                Outcome::NormalClose,
                entry,
                exit,
                31,
            )
            .expect("well formed")
        );
        assert!(format!("{trip:?}").starts_with("RoundTrip {"));
        assert!(format!("{contract:?}").starts_with("Contract {"));
        assert!(format!("{entry:?}").starts_with("Leg {"));
        assert_ne!(entry, exit);
        assert!(entry < exit);

        // And a `Charges` is plain data too.
        let charges = price(&trip).expect("verified");
        let mut charge_set = HashSet::new();
        assert!(charge_set.insert(charges));
        assert!(!charge_set.insert(charges));
        assert_eq!(charge_set.len(), 1);
        assert!(format!("{charges:?}").starts_with("Charges {"));
    }
}
