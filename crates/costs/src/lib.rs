//! Indian F&O statutory rates: the dated regime table, and the refusal that
//! stands where a rate was never verified.
//!
//! This is stage 1 of the cost calculator: **the rates and the refusal
//! contract**. It answers "what rate was in force" and "is that rate
//! citation-grounded". It does not yet compute a charge, a GST total or a
//! net P&L — those are later stages, and nothing here guesses at them.
//!
//! # The one idea this crate exists to preserve
//!
//! Before 2024-10-01 the exchange transaction charge was a member
//! **monthly-turnover slab ladder**, and the circulars that fixed those slabs
//! could not be retrieved. There is no honest flat per-trade equivalent. So
//! that window carries **no rate at all** — not a zero, not the current rate,
//! not a configurable default. A date landing in it produces a
//! [`error::Refusal`], never a number.
//!
//! That is enforced by the type system rather than by a runtime check. The
//! [`regime::Rate`] enum has two variants, and the unverified one **has no
//! numeric field**. There is nowhere for a fabricated rate to live, so there
//! is no flag, no keyword and no fall-back that could reach one. The only way
//! a caller ever holds a [`rate::BpsX100`] is to have been handed one out of a
//! `Rate::Verified` row, because [`rate::BpsX100`]'s constructor is
//! crate-private and every table in this crate is a `const` item this crate
//! owns. `CLAUDE.md` §4: degrade loudly and name the reason, or refuse.
//!
//! # Constant per-operation cost
//!
//! Every lookup here is O(1) **by construction**, and the construction is the
//! proof rather than a measurement standing in for one:
//!
//! * A regime table is `anchor + [Option<RegimeRow>; MAX_LATER_ROWS]`. The
//!   trip count of the one loop is bounded by [`regime::MAX_LATER_ROWS`], a
//!   compile-time constant. No argument can change it, because the tables are
//!   `const` items fixed before any input exists.
//! * The anchor row starts at [`day::TradeDay::MIN`], asserted at **compile
//!   time**, so every representable date selects a row. "Before the table" is
//!   not a representable state and costs no branch.
//! * Comparing two dates is one `i32` compare. [`day::TradeDay`] carries a day
//!   ordinal, so ordering never walks fields.
//!
//! The predecessor did this with `bisect_right` and documented itself
//! as O(log N), "effectively O(1)" because N ≤ 3 — an honest hedge for a
//! variable-length list. There is no list here to bisect and no N to grow, so
//! the hedge is not needed and is not made. `crates/costs/benches/ratio.rs`
//! measures the claim.
//!
//! # Money and rates are integers
//!
//! * Money is `i64` paisa ([`brutex_core::price::Paisa`]). `CLAUDE.md` §7.
//! * A rate is an integer in the source's `bps_x100` encoding — see
//!   [`rate::BpsX100`], which states the exact scale and where the name comes
//!   from.
//! * There is no `f64` anywhere in this crate, on any path, in any test.
//!
//! # Where this came from
//!
//! Ported from the predecessor repository's `brutex/costs/constants.py` and
//! `brutex/costs/types.py`, with the decision structure of
//! `rust/fno-math/src/optcost/regime.rs` read for reference. That crate links
//! `PyO3` and could not be depended on: `CLAUDE.md` §2 bans a binding to another
//! language without exception, and `deny.toml` lists `pyo3` by name. The
//! arithmetic was lifted; the binding was left behind.
//!
//! See `docs/05-decisions.md` D-0039.
//!
//! # Stage two: the option arithmetic
//!
//! Stage two adds the four things a charge has to be computed **on** — which
//! strike, how many units, how far from the money, and which expiry — and every
//! one of them is integer arithmetic with no search anywhere:
//!
//! * A strike grid is an arithmetic progression, so the at-the-money rung is a
//!   **rounding**, not a lookup in a chain ([`strike`]).
//! * Moneyness is a **signed step count** from that rung, negative into the
//!   money and positive out of it ([`moneyness`]).
//! * A position is `lots × lot size`, one multiplication ([`lot`]).
//! * An expiry is a **remainder mod 7** and, for a monthly, one month roll —
//!   never a walk over a calendar ([`expiry`]).
//!
//! Three of those four are **dated**, because the lot size moved seven times
//! and the expiry weekday five times between 2021 and 2026, and a backtest that
//! used one value across the window would mis-size every position in it. They
//! carry the same refusal contract as a statutory rate, through the one
//! mechanism in `dated`.
//!
//! Stage two still computes **no charge**. There is no GST total, no round
//! trip, no slippage and no net P&L — those are stage three's.
//!
//! # Stage three: the round trip
//!
//! [`trip::price`] takes one round trip — two bars, two days, a quantity, an
//! instrument, a direction and a broker — and returns every charge itemised
//! plus the total, in paisa `i64`. That is what the first two stages were for.
//!
//! Five things in an Indian F&O cost stack are commonly got wrong, and each of
//! them is a defect the predecessor repository records finding and fixing:
//!
//! 1. **The transaction tax is sell-side, and on the PREMIUM** — not both
//!    sides, and not on `strike × lots`.
//! 2. **Stamp duty is buy-side only.** Charging both legs doubles it.
//! 3. **GST is 18% on the services base, rounded ONCE.** Rounding CGST and SGST
//!    separately at 9% each is a systematic **+₹1 overcharge on every trade**.
//! 4. **Brokerage is per executed ORDER, flat.** One lot and a thousand lots
//!    pay the same.
//! 5. **An unverified regime refuses the WHOLE round trip** — not a component
//!    quietly priced at zero, and not today's rate applied backwards.
//!
//! Every one of them is enforced by the shape of the code rather than by a
//! comment, and every one is pinned by a test against the predecessor's own
//! worked examples. See [`trip`] for where each is handled.
//!
//! Stage three also carries across the two subtleties underneath the stack:
//! the **fill law** ([`fill`]), where each leg anchors on its own bar's adverse
//! extreme and then pays one further tick, and the **two rounding laws**
//! ([`money`]), where a statutory levy floors to the paisa and then ceils to
//! the rupee while a non-statutory one ceils to the paisa per leg.
//!
//! # What is here
//!
//! | Module | Owns |
//! |---|---|
//! | [`day`] | the trade date and the weekday, as arithmetic on an ordinal |
//! | [`error`] | every refusal this crate can return, and what it says |
//! | `dated` (private) | one dated table, generic over what it dates |
//! | [`rate`] | the flat rates, each beside its citation |
//! | [`regime`] | the dated rate tables, the lookup, and the refusal rows |
//! | [`venue`] | which exchange's circular governs an underlying, and its slot |
//! | [`strike`] | the grid step and the strike a moneyness names |
//! | [`moneyness`] | how far from the money a strike is, in signed steps |
//! | [`lot`] | the dated lot size, the quantity and the notional |
//! | [`expiry`] | the next weekly and the next monthly expiry |
//! | [`money`] | the two rounding laws, and the trap between them |
//! | [`fill`] | the worst-case fill law and the bar it anchors on |
//! | [`scope`] | which segments bear the charge stack at all |
//! | [`trip`] | the round trip, itemised, and the total |

#![forbid(unsafe_code)]

mod dated;

pub mod day;
pub mod error;
pub mod expiry;
pub mod fill;
pub mod lot;
pub mod money;
pub mod moneyness;
pub mod rate;
pub mod regime;
pub mod scope;
pub mod strike;
pub mod trip;
pub mod venue;
