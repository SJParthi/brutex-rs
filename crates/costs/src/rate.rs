//! The flat rates, each beside the citation it came from.
//!
//! A rate that does not change with the trade date lives here. A rate that
//! does lives in [`crate::regime`], because a dated rate needs a table and a
//! refusal row and a flat one does not.
//!
//! Every figure in this module was carried across from the predecessor
//! repository's `brutex/costs/constants.py`, which annotates each with its
//! statutory citation or broker page. Nothing here was inferred, rounded or
//! reconstructed. Where the source marked a figure `UNVERIFIED`, the marking
//! is carried too — see [`BSE_IPFT`].
//!
//! # No floats
//!
//! Money is `i64` paisa ([`Paisa`]). A rate is an integer in the [`BpsX100`]
//! encoding. `clippy::float_arithmetic` is denied workspace-wide and there is
//! no `f64` on any path through this crate.

use std::fmt;

use brutex_core::instrument::Exchange;
use brutex_core::price::Paisa;

/// The divisor that turns a [`BpsX100`] and a notional into a charge.
///
/// `charge = notional × rate / RATE_SCALE`, with the rounding rule that
/// belongs to that particular charge applied afterwards — several of them
/// differ, and the difference is the whole subject of `DEC-COST-001`.
///
/// # The scale, stated exactly
///
/// One [`BpsX100`] unit is `1e-7` as a decimal fraction: `10 ^ -5` percent, or
/// **one rupee per crore**. So:
///
/// | Rate | Reads as | Per ₹1 crore of notional |
/// |---|---|---|
/// | `10` | 0.0001% | ₹10 |
/// | `3_503` | 0.03503% | ₹3,503 |
/// | `10_000` | 0.10% | ₹10,000 |
/// | `1_800_000` | 18% | ₹18,00,000 |
///
/// `the_scale_reads_as_rupees_per_crore` proves every shipped rate against
/// that table with integer arithmetic.
pub const RATE_SCALE: i64 = 10_000_000;

/// A statutory rate, in the source's `bps_x100` encoding.
///
/// # About the name
///
/// `bps_x100` is the predecessor's name for this encoding and is kept so that
/// every literal in this crate is the source's literal, comparable line by
/// line. Read as arithmetic the name is off by a factor of ten — one unit is
/// `0.001` basis points, not `100` of them. [`RATE_SCALE`] states the true
/// scale, and a test proves it against the per-crore figures the circulars
/// themselves quote. The name is a label; the scale is the contract.
///
/// # Why the constructor is crate-private
///
/// A [`BpsX100`] is a claim that a citation exists. The only values in
/// circulation are the ones this crate's `const` tables were built from, so a
/// caller cannot mint one and cannot therefore hand a fabricated statutory
/// rate to anything downstream. Together with [`crate::regime::Rate`] having
/// no numeric field on its unverified variant, that is what makes
/// "charge an unverified rate" unrepresentable rather than merely discouraged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct BpsX100(i64);

impl BpsX100 {
    /// A rate of exactly nothing.
    ///
    /// Distinct from an unverified rate in every way that matters: this is a
    /// measured zero, and the thing it is not is a refusal.
    pub const ZERO: Self = Self(0);

    /// Wraps a rate. Crate-private — see the type's documentation.
    pub(crate) const fn new(bps_x100: i64) -> Self {
        Self(bps_x100)
    }

    /// The rate as an integer, to be divided by [`RATE_SCALE`] after
    /// multiplication.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for BpsX100 {
    /// Renders the rate as a percentage, because that is how every circular
    /// quotes it. Integer arithmetic: five decimal places, split by division.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:05}%", self.0 / 100_000, (self.0 % 100_000).abs())
    }
}

// ---------------------------------------------------------------------------
// Brokerage — per executed order, flat
// ---------------------------------------------------------------------------

/// Which broker's schedule to price against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Broker {
    /// Groww.
    Groww,
    /// Zerodha.
    Zerodha,
}

/// Groww: ₹20 per executed order, flat for F&O.
///
/// **Per ORDER, not per contract.** One lot and one thousand lots cost the
/// same ₹20. The predecessor's `CALCULATOR_SPEC` §4.4 records correcting an
/// earlier "₹20 per contract" claim, which is the error this comment exists to
/// stop being made again. Source: `COSTS_VERIFIED` §8.1.
pub const GROWW_BROKERAGE_PER_ORDER: Paisa = Paisa::from_raw(2_000);

/// Zerodha: ₹20 per executed order, flat for F&O since June 2024.
///
/// The same flat figure as Groww, kept as its own constant because it is its
/// own citation and could move independently. Source: `COSTS_VERIFIED` §8.2.
pub const ZERODHA_BROKERAGE_PER_ORDER: Paisa = Paisa::from_raw(2_000);

/// The brokerage charged on one executed order.
///
/// Per order. A round trip is two orders and therefore twice this.
#[must_use]
pub const fn brokerage_per_order(broker: Broker) -> Paisa {
    match broker {
        Broker::Groww => GROWW_BROKERAGE_PER_ORDER,
        Broker::Zerodha => ZERODHA_BROKERAGE_PER_ORDER,
    }
}

// ---------------------------------------------------------------------------
// SEBI turnover fee
// ---------------------------------------------------------------------------

/// SEBI turnover fee: 0.0001%, ₹10 per crore, on premium **both sides**.
///
/// Source: the `sebi.gov.in` fee schedule; `COSTS_VERIFIED` §4.
pub const SEBI_TURNOVER_FEE: BpsX100 = BpsX100::new(10);

// ---------------------------------------------------------------------------
// IPFT — Investor Protection Fund Trust
// ---------------------------------------------------------------------------

/// NSE IPFT: 0.0005%, ₹50 per crore, on premium both sides.
///
/// Source: `NSE/FA/56129`; `COSTS_VERIFIED` §5.
///
/// See the note on [`crate::regime::NSE_EXCHANGE_CHARGE`] about `FA73061`,
/// which reportedly re-splits the transaction charge and this figure while
/// leaving their total unchanged. It is UNVERIFIED and deliberately not
/// encoded.
pub const NSE_IPFT: BpsX100 = BpsX100::new(50);

/// BSE IPFT: taken as zero — and **the zero is `UNVERIFIED`**.
///
/// The predecessor's `COSTS_VERIFIED` §9 lists this in its UNVERIFIED set: it
/// could not establish whether the figure is exactly zero or merely trivially
/// small, and it priced it as zero. That marking is carried across unchanged
/// rather than quietly promoted to a fact.
///
/// **Direction of error, stated because it must be:** if the true figure is
/// positive, this **under**-charges. It is not the conservative direction. It
/// is not a refusal either, because refusing here would be a behaviour change
/// this port did not have the authority to make — the source prices the trade
/// and flags the figure, and so does this. Recorded in `docs/06-limits.md`.
pub const BSE_IPFT: BpsX100 = BpsX100::ZERO;

/// The IPFT rate for a venue.
#[must_use]
pub const fn ipft(exchange: Exchange) -> BpsX100 {
    match exchange {
        Exchange::Nse => NSE_IPFT,
        Exchange::Bse => BSE_IPFT,
    }
}

// ---------------------------------------------------------------------------
// Stamp duty — buy side only
// ---------------------------------------------------------------------------

/// Which side of the round trip a charge is being computed for.
///
/// It exists so that "stamp duty is buy-side only" can be a fact about the
/// type rather than a sentence in a comment that a caller has to have read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OrderSide {
    /// The leg that pays premium.
    Buy,
    /// The leg that receives premium.
    Sell,
}

/// Stamp duty: 0.003%, ₹300 per crore, on options premium — **buy side only**.
///
/// Uniform across India since 2020-07-01 under the Indian Stamp Act
/// amendments. Source: `COSTS_VERIFIED` §6.
///
/// Prefer [`stamp_duty`], which cannot be applied to the wrong leg.
pub const STAMP_DUTY_BUY_SIDE: BpsX100 = BpsX100::new(300);

/// The stamp-duty rate for one leg: the rate on a buy, nothing on a sell.
///
/// Charging both legs is the classic doubling error in an Indian cost stack.
/// Routing the rate through the side makes it a compile-time question rather
/// than a review comment.
#[must_use]
pub const fn stamp_duty(side: OrderSide) -> BpsX100 {
    match side {
        OrderSide::Buy => STAMP_DUTY_BUY_SIDE,
        OrderSide::Sell => BpsX100::ZERO,
    }
}

// ---------------------------------------------------------------------------
// GST
// ---------------------------------------------------------------------------

/// GST: 18% on (brokerage + exchange charge + SEBI fee + IPFT).
///
/// **The total is rounded once.** Not CGST and SGST at 9% each, rounded
/// separately, then added: section 170 of the CGST Act rounds the total tax,
/// and the predecessor's `DEC-COST-001` records that the earlier
/// round-each-component spec produced a systematic **+₹1 overcharge on every
/// trade**. An intra-state trade may be *displayed* as two halves; it is
/// *computed* once.
///
/// Stage 1 carries the rate and the rule. The arithmetic that applies them —
/// which components go into the base, and where the single rounding lands — is
/// stage 2's, and is deliberately not half-written here.
pub const GST_ON_FEE_BASE: BpsX100 = BpsX100::new(1_800_000);

// ---------------------------------------------------------------------------
// Quantum
// ---------------------------------------------------------------------------

/// The option premium tick: ₹0.05, five paisa.
///
/// NSE and BSE index option premiums all move on this grid. Source: the NSE
/// and BSE option contract specifications; `COSTS_VERIFIED` §11.
pub const TICK: Paisa = Paisa::from_raw(5);

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::*;

    /// One crore rupees, in paisa. The unit every circular quotes in.
    const ONE_CRORE_PAISA: i64 = 10_000_000 * 100;

    /// The charge on a notional, before any rounding rule. Integer only.
    fn charge_paisa(notional_paisa: i64, rate: BpsX100) -> i64 {
        notional_paisa
            .checked_mul(rate.get())
            .expect("no shipped rate overflows on a one-crore notional")
            / RATE_SCALE
    }

    #[test]
    fn every_flat_rate_is_the_cited_figure() {
        assert_eq!(GROWW_BROKERAGE_PER_ORDER.raw(), 2_000);
        assert_eq!(ZERODHA_BROKERAGE_PER_ORDER.raw(), 2_000);
        assert_eq!(SEBI_TURNOVER_FEE.get(), 10);
        assert_eq!(NSE_IPFT.get(), 50);
        assert_eq!(BSE_IPFT.get(), 0);
        assert_eq!(STAMP_DUTY_BUY_SIDE.get(), 300);
        assert_eq!(GST_ON_FEE_BASE.get(), 1_800_000);
        assert_eq!(TICK.raw(), 5);
        assert_eq!(RATE_SCALE, 10_000_000);
    }

    #[test]
    fn the_scale_reads_as_rupees_per_crore() {
        // The circulars quote rupees per crore. If the encoding were off by a
        // factor of ten -- which the NAME `bps_x100` suggests it might be --
        // every one of these would miss by 10x. Integer arithmetic throughout.
        for (rate, rupees_per_crore) in [
            (SEBI_TURNOVER_FEE, 10),
            (NSE_IPFT, 50),
            (BSE_IPFT, 0),
            (STAMP_DUTY_BUY_SIDE, 300),
            (GST_ON_FEE_BASE, 1_800_000),
        ] {
            assert_eq!(
                charge_paisa(ONE_CRORE_PAISA, rate),
                rupees_per_crore * 100,
                "{rate} on one crore"
            );
        }
        // 18% of a crore is eighteen lakh, which is the same statement read
        // the other way round.
        assert_eq!(
            charge_paisa(ONE_CRORE_PAISA, GST_ON_FEE_BASE),
            ONE_CRORE_PAISA * 18 / 100
        );
    }

    #[test]
    fn a_rate_renders_as_the_percentage_a_circular_quotes() {
        assert_eq!(SEBI_TURNOVER_FEE.to_string(), "0.00010%");
        assert_eq!(NSE_IPFT.to_string(), "0.00050%");
        assert_eq!(STAMP_DUTY_BUY_SIDE.to_string(), "0.00300%");
        assert_eq!(GST_ON_FEE_BASE.to_string(), "18.00000%");
        assert_eq!(BpsX100::ZERO.to_string(), "0.00000%");
    }

    #[test]
    fn brokerage_is_flat_per_order_and_both_brokers_are_priced() {
        assert_eq!(brokerage_per_order(Broker::Groww).raw(), 2_000);
        assert_eq!(brokerage_per_order(Broker::Zerodha).raw(), 2_000);
        // Per ORDER: the figure does not scale with lots, and a round trip is
        // two orders. The predecessor corrected a "per contract" reading here.
        let round_trip = brokerage_per_order(Broker::Groww)
            .checked_add(brokerage_per_order(Broker::Groww))
            .expect("no overflow");
        assert_eq!(round_trip.raw(), 4_000);
    }

    #[test]
    fn stamp_duty_is_charged_on_the_buy_leg_and_never_on_the_sell_leg() {
        assert_eq!(stamp_duty(OrderSide::Buy), STAMP_DUTY_BUY_SIDE);
        assert_eq!(stamp_duty(OrderSide::Buy).get(), 300);
        assert_eq!(stamp_duty(OrderSide::Sell), BpsX100::ZERO);
        assert_eq!(stamp_duty(OrderSide::Sell).get(), 0);
        // A round trip pays it once, not twice. This is the doubling error.
        let round_trip = charge_paisa(ONE_CRORE_PAISA, stamp_duty(OrderSide::Buy))
            + charge_paisa(ONE_CRORE_PAISA, stamp_duty(OrderSide::Sell));
        assert_eq!(round_trip, 300 * 100, "one crore, ₹300, charged once");
    }

    #[test]
    fn ipft_is_the_nse_figure_and_the_bse_zero_is_the_unverified_one() {
        assert_eq!(ipft(Exchange::Nse), NSE_IPFT);
        assert_eq!(ipft(Exchange::Nse).get(), 50);
        assert_eq!(ipft(Exchange::Bse), BSE_IPFT);
        assert_eq!(ipft(Exchange::Bse).get(), 0);
        // A measured zero and a refusal are different things and this is the
        // first: BSE IPFT returns a number, and the number's provenance is
        // marked UNVERIFIED in the constant's own documentation and in
        // docs/06-limits.md. It does not refuse, because the source does not.
        assert_eq!(charge_paisa(ONE_CRORE_PAISA, ipft(Exchange::Bse)), 0);
        assert_eq!(charge_paisa(ONE_CRORE_PAISA, ipft(Exchange::Nse)), 5_000);
    }

    #[test]
    fn no_shipped_rate_is_negative_and_zero_is_the_only_zero() {
        for rate in [
            SEBI_TURNOVER_FEE,
            NSE_IPFT,
            BSE_IPFT,
            STAMP_DUTY_BUY_SIDE,
            GST_ON_FEE_BASE,
            BpsX100::ZERO,
        ] {
            assert!(rate.get() >= 0, "{rate} is negative");
        }
        assert_eq!(BpsX100::ZERO.get(), 0);
        assert!(BpsX100::ZERO < SEBI_TURNOVER_FEE);
        assert!(GROWW_BROKERAGE_PER_ORDER.raw() > 0);
        assert!(TICK.raw() > 0);
    }

    #[test]
    fn a_rate_is_ordered_hashable_and_debuggable() {
        use std::collections::HashSet;

        assert!(SEBI_TURNOVER_FEE < NSE_IPFT);
        assert_eq!(format!("{NSE_IPFT:?}"), "BpsX100(50)");
        let mut set = HashSet::new();
        assert!(set.insert(NSE_IPFT));
        assert!(!set.insert(BpsX100::new(50)));
        assert_eq!(set.len(), 1);
        // The side and broker enums are plain data and must compare.
        assert_ne!(Broker::Groww, Broker::Zerodha);
        assert_ne!(OrderSide::Buy, OrderSide::Sell);
        assert_eq!(format!("{:?}", Broker::Zerodha), "Zerodha");
        assert_eq!(format!("{:?}", OrderSide::Sell), "Sell");
    }
}
