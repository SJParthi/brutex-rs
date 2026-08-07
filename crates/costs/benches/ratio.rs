//! Gate 8 for `crates/costs` — a regime lookup costs the same whatever it is
//! asked.
//!
//! # What is being claimed, and what would falsify it
//!
//! `crates/costs/src/regime.rs` claims O(1) **by construction**: a table is an
//! anchor row plus a fixed-size array of at most `MAX_LATER_ROWS` more, so the
//! one loop runs a compile-time-bounded number of times. Structure is the
//! proof. These measurements are the cross-check on the structure, and each
//! one names the shape of failure it would catch:
//!
//! * **Which row is selected must not matter.** A lookup that got slower the
//!   further down the table it landed would mean the scan was doing work per
//!   row that the anchor does not pay — the beginning of a linear scan.
//! * **How many rows the table has must not matter.** The two-row and
//!   three-row tables must cost the same. If they do not, the trip count is
//!   being paid for at runtime and a fourth row would cost more again.
//! * **A refusal must cost what a rate costs.** The predecessor raised an
//!   exception here, and an exception is a slow path. If refusing were
//!   expensive, a sweep over a mostly-unverified window would be a different
//!   engine from one over a verified window — and the refusal would quietly
//!   become something callers avoid asking for.
//!
//! # Stage two: the option arithmetic
//!
//! The strike, the moneyness, the quantity and the expiry are claimed O(1) for
//! a different reason — they are **closed-form integer arithmetic**, with no
//! table to walk at all. The failure each measurement below would catch is the
//! same shape in every case: a magnitude that starts to cost.
//!
//! * **A far strike must cost what a near one does.** If it did not, something
//!   is walking the grid rung by rung instead of dividing.
//! * **A thousand lots must cost what one lot does.** If not, the
//!   multiplication became a loop.
//! * **A monthly expiry that rolls must cost what one that does not costs**,
//!   and December must cost what January costs. If the month index started to
//!   matter, something is walking the calendar.
//!
//! There is no benchmarking framework, for the reason `crates/store` gives:
//! it would be a dependency. Every ratio below is integer arithmetic.

// A money literal is written `rupees_paisa` — `24_000_00` reads as the
// twenty-four thousand rupees a screen would show, where `2_400_000` reads as
// nothing at all. The grouping is deliberate and consistent through this file.
#![allow(clippy::inconsistent_digit_grouping)]

use std::hint::black_box;
use std::time::Instant;

use brutex_core::instrument::{Exchange, OptionSide};
use brutex_core::price::Paisa;
use brutex_core::symbol::Symbol;
use costs::day::TradeDay;
use costs::expiry::{next_monthly_expiry, next_weekly_expiry};
use costs::lot::{LotSize, contract_quantity, lot_size_on, notional};
use costs::moneyness::{MoneynessSteps, moneyness_steps};
use costs::rate::BpsX100;
use costs::regime::{exchange_charge_rate, stt_exercise_rate, stt_options_rate};
use costs::strike::{StrikeStep, at_the_money, strike_at, strike_step_on};
use costs::venue::{SweptSlot, swept_slot};

/// A ratio above this is a failure. Thousandths, so the comparison is integer.
///
/// 3.0x — `docs/04-invariants.md`, the shared-CI number.
const CEILING_PERMILLE: u128 = 3_000;

/// How many times each measurement is repeated before the minimum is taken.
const TRIALS: u32 = 60;

/// How many calls each timed run makes.
const REPS: u32 = 20_000;

/// Times one closure, in picoseconds per call, as a minimum over [`TRIALS`].
fn cost_ps<T>(mut op: impl FnMut() -> T) -> u128 {
    let mut best = u128::MAX;
    for _ in 0..TRIALS {
        let start = Instant::now();
        for _ in 0..REPS {
            black_box(op());
        }
        let ps = start.elapsed().as_nanos() * 1_000 / u128::from(REPS);
        if ps < best {
            best = ps;
        }
    }
    best
}

/// Prints one measurement and returns whether it stayed under the ceiling.
fn ratio(label: &str, base_ps: u128, at_ps: u128) -> bool {
    if base_ps == 0 {
        println!("  {label:<52} UNMEASURABLE — the baseline timed at zero");
        return false;
    }
    let permille = at_ps * 1_000 / base_ps;
    let ok = permille <= CEILING_PERMILLE;
    println!(
        "  {label:<52} {base_ps:>8} ps -> {at_ps:>8} ps   ratio {}.{:03}x  {}",
        permille / 1_000,
        permille % 1_000,
        if ok { "ok" } else { "BREACH" }
    );
    ok
}

/// A date, or a loud failure. The bench refuses to measure a lookup it could
/// not set up, rather than measuring a different one.
fn day(year: u16, month: u8, d: u8) -> Option<TradeDay> {
    match TradeDay::new(year, month, d) {
        Ok(value) => Some(value),
        Err(reason) => {
            println!("  BENCH SETUP FAILED for {year:04}-{month:02}-{d:02}: {reason}");
            None
        }
    }
}

/// C-K-01 — the cost of a lookup does not track which row it selects.
fn the_selected_row_does_not_change_the_cost() -> bool {
    let (Some(first), Some(middle), Some(last)) =
        (day(1990, 1, 1), day(2025, 1, 1), day(2100, 12, 31))
    else {
        return false;
    };

    let time = |when: TradeDay| cost_ps(|| stt_options_rate(black_box(when)));
    let base = time(first);
    let mut ok = true;
    ok &= ratio(
        "C-K-01 STT options, anchor row vs middle row",
        base,
        time(middle),
    );
    ok &= ratio(
        "C-K-01 STT options, anchor row vs last row",
        base,
        time(last),
    );
    ok
}

/// C-K-02 — the cost of a lookup does not track how many rows the table has.
fn the_row_count_does_not_change_the_cost() -> bool {
    let Some(when) = day(2100, 12, 31) else {
        return false;
    };

    // Three rows against two: the last row of each, so both scans run to the
    // end of their array.
    let base = cost_ps(|| stt_options_rate(black_box(when)));
    let two_rows = cost_ps(|| stt_exercise_rate(black_box(when)));
    let venue = cost_ps(|| exchange_charge_rate(black_box(Exchange::Nse), black_box(when)));

    let mut ok = true;
    ok &= ratio("C-K-02 three-row table vs two-row table", base, two_rows);
    ok &= ratio("C-K-02 three-row table vs venue-scoped table", base, venue);
    ok
}

/// C-K-03 — refusing costs what pricing costs.
fn a_refusal_costs_what_a_rate_costs() -> bool {
    let (Some(refused), Some(priced)) = (day(2024, 9, 30), day(2024, 10, 1)) else {
        return false;
    };

    let mut ok = true;
    for exchange in [Exchange::Nse, Exchange::Bse] {
        let base = cost_ps(|| exchange_charge_rate(black_box(exchange), black_box(priced)));
        let at = cost_ps(|| exchange_charge_rate(black_box(exchange), black_box(refused)));
        let label = match exchange {
            Exchange::Nse => "C-K-03 NSE, a verified rate vs a refusal",
            Exchange::Bse => "C-K-03 BSE, a verified rate vs a refusal",
        };
        ok &= ratio(label, base, at);
    }
    ok
}

/// C-K-04 — the refusal is a refusal, not merely a fast one.
///
/// Speed is half of it. A lookup that got fast by handing back the current
/// rate for an unverified window would pass every ratio above and be exactly
/// the failure this crate exists to prevent.
fn the_pre_boundary_window_still_refuses() -> bool {
    let (Some(before_boundary), Some(on_boundary)) = (day(2024, 9, 30), day(2024, 10, 1)) else {
        return false;
    };
    let refuses = exchange_charge_rate(Exchange::Nse, before_boundary).is_err()
        && exchange_charge_rate(Exchange::Bse, before_boundary).is_err();
    let charges = exchange_charge_rate(Exchange::Nse, on_boundary).map(BpsX100::get) == Ok(3_503)
        && exchange_charge_rate(Exchange::Bse, on_boundary).map(BpsX100::get) == Ok(3_250);
    println!(
        "  {:<52} refuses={refuses} prices-the-boundary={charges}  {}",
        "C-K-04 pre-2024-10-01 refuses and 2024-10-01 prices",
        if refuses && charges { "ok" } else { "BREACH" }
    );
    refuses && charges
}

/// A swept underlying's slot, or a loud failure.
fn slot(underlying: &str) -> Option<SweptSlot> {
    let Ok(symbol) = Symbol::new(underlying) else {
        println!("  BENCH SETUP FAILED: {underlying} is not a valid symbol");
        return None;
    };
    match swept_slot(symbol) {
        Ok(found) => Some(found),
        Err(reason) => {
            println!("  BENCH SETUP FAILED for {underlying}: {reason}");
            None
        }
    }
}

/// The NIFTY grid step on a day inside the verified window, or a loud failure.
fn nifty_step() -> Option<StrikeStep> {
    let (Some(nifty), Some(when)) = (slot("NIFTY"), day(2024, 10, 1)) else {
        return None;
    };
    match strike_step_on(nifty, when) {
        Ok(step) => Some(step),
        Err(reason) => {
            println!("  BENCH SETUP FAILED for the NIFTY step: {reason}");
            None
        }
    }
}

/// C-K-05 — the at-the-money rung costs the same wherever the spot is.
///
/// A rounding is a division. A search over listed strikes would cost more the
/// further the spot sat from wherever the search began.
fn the_spot_magnitude_does_not_change_the_rung_cost() -> bool {
    let Some(step) = nifty_step() else {
        return false;
    };

    let time =
        |spot: i64| cost_ps(|| at_the_money(black_box(Paisa::from_raw(spot)), black_box(step)));
    let base = time(50_00);
    let mut ok = true;
    ok &= ratio(
        "C-K-05 rung, a 50-rupee spot vs a 24,000-rupee spot",
        base,
        time(24_000_00),
    );
    ok &= ratio(
        "C-K-05 rung, a 50-rupee spot vs a 90-lakh-crore spot",
        base,
        time(i64::MAX / 4),
    );
    ok
}

/// C-K-06 — the resolved strike costs the same however deep the moneyness.
///
/// `ATM`, `OTM+10` at the chain edge, and a million steps out: one
/// multiplication each, or the walk is stepping rung by rung.
fn the_moneyness_depth_does_not_change_the_strike_cost() -> bool {
    let (Some(step), Some(atm)) = (nifty_step(), Some(Paisa::from_raw(24_000_00))) else {
        return false;
    };

    let time = |steps: i32| {
        cost_ps(|| {
            strike_at(
                black_box(atm),
                black_box(MoneynessSteps::new(steps)),
                black_box(step),
                black_box(OptionSide::Call),
            )
        })
    };
    let base = time(0);
    let mut ok = true;
    ok &= ratio(
        "C-K-06 strike, ATM vs the chain edge OTM+10",
        base,
        time(10),
    );
    ok &= ratio("C-K-06 strike, ATM vs OTM+1,000,000", base, time(1_000_000));
    ok &= ratio("C-K-06 strike, ATM vs ITM-400 (deep)", base, time(-400));

    // And the inverse direction: reading a moneyness off a far strike must
    // cost what reading it off a near one does.
    let read = |strike: i64| {
        cost_ps(|| {
            moneyness_steps(
                black_box(Paisa::from_raw(strike)),
                black_box(atm),
                black_box(step),
                black_box(OptionSide::Put),
            )
        })
    };
    let near = read(24_050_00);
    ok &= ratio(
        "C-K-06 moneyness, one step out vs 400 steps out",
        near,
        read(44_000_00),
    );
    ok
}

/// C-K-07 — the quantity costs the same however many lots there are.
///
/// One `checked_mul`. If a thousand lots cost more than one, the
/// multiplication became an accumulation.
fn the_lot_count_does_not_change_the_quantity_cost() -> bool {
    let (Some(nifty), Some(when)) = (slot("NIFTY"), day(2024, 11, 20)) else {
        return false;
    };
    let lot = match lot_size_on(nifty, when) {
        Ok(found) => found,
        Err(reason) => {
            println!("  BENCH SETUP FAILED for the lot size: {reason}");
            return false;
        }
    };

    let time = |lots: i64| cost_ps(|| contract_quantity(black_box(lots), black_box(lot)));
    let base = time(1);
    let mut ok = true;
    ok &= ratio("C-K-07 quantity, 1 lot vs 1,000 lots", base, time(1_000));
    ok &= ratio(
        "C-K-07 quantity, 1 lot vs 1,000,000 lots",
        base,
        time(1_000_000),
    );

    let priced = |quantity: i64| {
        cost_ps(|| notional(black_box(Paisa::from_raw(125_50)), black_box(quantity)))
    };
    let one = priced(75);
    ok &= ratio(
        "C-K-07 notional, 75 units vs 75,000,000 units",
        one,
        priced(75_000_000),
    );
    ok
}

/// C-K-08 — the expiry costs the same wherever in the calendar it is asked.
///
/// The monthly law has two arms — this month's expiry is still ahead, or it has
/// passed and the month rolls once. Both must cost the same, and December must
/// cost what January costs, or something is walking the calendar.
fn the_calendar_position_does_not_change_the_expiry_cost() -> bool {
    let Some(nifty) = slot("NIFTY") else {
        return false;
    };
    let (Some(early_january), Some(late_january), Some(late_december), Some(mid_year)) = (
        day(2024, 1, 2),
        day(2024, 1, 31),
        day(2024, 12, 30),
        day(2024, 6, 14),
    ) else {
        return false;
    };

    let monthly = |on: TradeDay| cost_ps(|| next_monthly_expiry(black_box(nifty), black_box(on)));
    let base = monthly(early_january);
    let mut ok = true;
    ok &= ratio(
        "C-K-08 monthly, in-month arm vs the rollover arm",
        base,
        monthly(late_january),
    );
    ok &= ratio(
        "C-K-08 monthly, January vs a December that rolls the year",
        base,
        monthly(late_december),
    );
    ok &= ratio("C-K-08 monthly, January vs June", base, monthly(mid_year));

    // Weekly: zero days ahead against six.
    let weekly = |on: TradeDay| cost_ps(|| next_weekly_expiry(black_box(nifty), black_box(on)));
    let (Some(on_the_day), Some(six_ahead)) = (day(2024, 6, 6), day(2024, 6, 7)) else {
        return false;
    };
    ok &= ratio(
        "C-K-08 weekly, 0 days ahead vs 6 days ahead",
        weekly(on_the_day),
        weekly(six_ahead),
    );
    ok
}

/// C-K-09 — stage two's refusals are refusals, not merely fast ones.
///
/// The companion to C-K-04. A lot-size lookup that got fast by handing back the
/// first recorded lot for a 2019 trade would pass every ratio above and be the
/// silent extrapolation this crate exists to refuse.
fn the_stage_two_pre_history_windows_still_refuse() -> bool {
    let (Some(nifty), Some(before_lots), Some(on_lots), Some(before_expiry)) = (
        slot("NIFTY"),
        day(2020, 12, 31),
        day(2021, 1, 1),
        day(2019, 12, 31),
    ) else {
        return false;
    };

    let lots_refuse =
        lot_size_on(nifty, before_lots).is_err() && strike_step_on(nifty, before_lots).is_err();
    let lots_price = lot_size_on(nifty, on_lots).map(LotSize::units) == Ok(75)
        && strike_step_on(nifty, on_lots).map(StrikeStep::raw) == Ok(5_000);
    let expiry_refuses = next_weekly_expiry(nifty, before_expiry).is_err()
        && next_monthly_expiry(nifty, before_expiry).is_err();
    let ok = lots_refuse && lots_price && expiry_refuses;
    println!(
        "  {:<52} lots={lots_refuse} prices={lots_price} expiry={expiry_refuses}  {}",
        "C-K-09 the pre-history windows refuse",
        if ok { "ok" } else { "BREACH" }
    );
    ok
}

fn main() {
    println!("crates/costs — gate 8, per-lookup cost");
    println!(
        "  ceiling {}.{:03}x",
        CEILING_PERMILLE / 1_000,
        CEILING_PERMILLE % 1_000
    );

    let mut ok = true;
    ok &= the_selected_row_does_not_change_the_cost();
    ok &= the_row_count_does_not_change_the_cost();
    ok &= a_refusal_costs_what_a_rate_costs();
    ok &= the_pre_boundary_window_still_refuses();
    ok &= the_spot_magnitude_does_not_change_the_rung_cost();
    ok &= the_moneyness_depth_does_not_change_the_strike_cost();
    ok &= the_lot_count_does_not_change_the_quantity_cost();
    ok &= the_calendar_position_does_not_change_the_expiry_cost();
    ok &= the_stage_two_pre_history_windows_still_refuse();

    if ok {
        println!("  every ratio held.");
    } else {
        println!("  A BOUND WAS BREACHED. See docs/04-invariants.md.");
        std::process::exit(1);
    }
}
