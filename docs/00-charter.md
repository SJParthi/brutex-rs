# 00 — Charter

Scope, verified external facts, and the prohibitions. Written as prohibitions
because a prohibition survives a rewrite better than a goal does.

---

## 1. Scope lock

**The engine sweeps exactly two instruments, both on NSE.** Narrowed from
three by D-0017: `BSE-SENSEX` is no longer swept and BSE is no longer pulled.

| Symbol | Exchange | Segment |
|---|---|---|
| `NSE-NIFTY` | NSE | INDEX |
| `NSE-BANKNIFTY` | NSE | INDEX |

`NSE-INDIAVIX` is **reference only** — stored, stamped onto observable trades
as `vix_at_entry` / `vix_at_exit`, and never in the condition vocabulary, the
ranking inputs, or run identity.

The store may hold futures, options and single-stock series. Nothing outside
the three symbols above is ever swept.

Widening this list requires an entry in `docs/05-decisions.md`. It does not
happen because a task seemed to need it.

---

## 2. Prohibitions

1. No language other than Rust, in any form, at any layer. See `CLAUDE.md` §2.
2. No writable memory mapping of a store file. Writes go through positional
   syscalls. A writable mapping raises SIGBUS on a full disk and a signal
   cannot be caught in any language.
3. No depth parameter on the sweep. Depth ends at extinction.
4. No ORM, no query planner, no dynamic schema. The path is the index.
5. No float in a price, a cost, or a P&L. Paisa integers only.
6. No fallback that hides the reason it fired. Degrade loudly and name it, or
   refuse. Never silently.
7. No condition bit renumbered or reused. Append only.
8. No token minted by this repository. Credentials are read-only.
9. No claim of a measurement that was not taken.
10. No literal credential path in a tracked file. This repository is public.
    Documents carry the shape `/<org>/<env>/<vendor>/<field>`; the real
    segments are resolved at runtime from an untracked local configuration.
    Enforced by CI gate 1c, not by review. See D-0013.

---

## 3. Verified market facts

Sources are Indian exchange publications and the vendor documentation cited in
§4. Every row here is load-bearing; changing one changes results.

| Fact | Value |
|---|---|
| Regular session | 09:15 – 15:30 IST, NSE and BSE equity |
| Pre-open auction | 09:00 – 09:15 IST — excluded from bars |
| First index tick | 09:15:00 IST |
| Timezone | IST, fixed +05:30, no daylight saving |
| Bars per regular session | 375 at 1-minute granularity |
| Muhurat (Diwali) session | ~1 hour, an evening session on a date that is
  otherwise a holiday. Verified dates: 2020-11-14, 2021-11-04, 2022-10-24,
  2023-11-12, 2024-11-01, 2025-10-21 |
| Special weekend sessions | Union Budget sessions falling on a weekend, run at
  full regular hours. **2020-02-01 (Sat), 2025-02-01 (Sat), 2026-02-01 (Sun)**
  — 375 bars each, confirmed in the lake. This is the complete set: 1 February
  fell on a weekend in no other year from 2020 to date. Supersedes an earlier
  claim of 2021-01-30 and 2021-02-01; see D-0014. |
| Bar timestamp | the **OPEN** (left edge) of its minute. A bar covers the
  half-open window `[t, t + tf)`, left-closed and left-labelled. VERIFIED |
| Last regular 1-minute bar | **15:29:00 IST**, not 15:30. 09:15 through 15:29
  inclusive is exactly 375 bars, which is the arithmetic that closes it. |
| Forced exit | 15:20 IST, **inclusive of the 15:20 bar**: a bar is a
  forced-exit bar iff `bar_open + tf > 15:20`. The 15:19 bar closes exactly at
  15:20 and is not forced. Generalises to `session_close − 10 min`, which
  yields 14:35 for the 2025 Muhurat session and 16:50 for 2021-02-24. |
| Tick grid | 2 decimal places |
| Price storage | paisa integers, `i64` |
| Track-1 brokerage | none. Spot indices are not tradable; the sweep is
  signal-only. Cost modelling belongs to the options translation layer. |

**Muhurat is not a normal session.** A previous-day anchor must never roll
across it — the one-hour evening OHLC is not the prior regular day. This was a
real defect that silently poisoned six years of daily anchors.

Stated exactly, because "never roll across it" is not implementable:

> For a bar on any day after a Muhurat session, the previous-day anchor is the
> OHLC of the **last regular trading session strictly before the Muhurat
> date**. The Muhurat day's own OHLC never enters the previous-day anchor, the
> multi-day rolling history, or the previous-session edge.

Worked case: the anchor for 2024-11-04 (Mon) is **2024-10-31 (Thu)** — not
2024-11-01, which was the Muhurat Friday. The mechanism is that every Muhurat
date is also a non-trading date, so an anchor walk restricted to trading days
skips it structurally rather than by a special case that can be forgotten.

Muhurat sessions, confirmed bar-for-bar against the lake:

| Date | Session (IST) | Bars |
|---|---|---|
| 2020-11-14 (Sat) | 18:15 – 19:15 | 60 |
| 2021-11-04 (Thu) | 18:15 – 19:15 | 60 |
| 2022-10-24 (Mon) | 18:15 – 19:15 | 60 |
| 2023-11-12 (Sun) | 18:00 – 19:00 | **46** — a 14-bar deficit; the open is UNVERIFIED, first bar is 18:07 |
| 2024-11-01 (Fri) | 18:00 – 19:00 | 60 |
| 2025-10-21 (Tue) | 13:45 – 14:45 | 60 — an afternoon session, not an evening one |

2026 has an announced Muhurat date (2026-11-08) with **no notified timings**.
It is therefore absent rather than guessed.

---

## 4. Vendor facts

Evidence lane is recorded per row and is never promoted while copying.

### Groww — primary

| Fact | Value | Lane |
|---|---|---|
| Transport | official SDK, injected client | verified |
| History endpoint | one method for spot and derivatives | verified |
| Granularity fetched | `1minute` only. Every other timeframe is derived. | decided |
| History depth | from 2020 | documented |
| Window cap | 30 days per request at 1-minute granularity | documented |
| Response shape | row arrays: `[ts, o, h, l, c, v, oi]`, `oi` null off-derivatives | verified |
| Timestamp | native IST string, or epoch seconds defensively | verified |
| Price unit | rupees as float on the wire; converted to paisa at the boundary | verified |
| Rate limit | 500 requests per minute. **No daily quota.** | operator-confirmed, not published |
| Per-second cap | **UNVERIFIED.** The published 10/s applies to a different endpoint group. Production ceiling is 8/s, chosen not measured. | unverified |
| Auth | TOTP-derived daily token, reset 06:00 IST | verified |
| Credentials | `/<org>/<env>/<vendor>/<field>` — SecureStrings, region `ap-south-1`, read-only. Fields: `api-key`, `totp-secret`, `access-token`. Real segments resolved at runtime; see D-0013. | verified |

### Dhan — secondary, spot indices only

| Fact | Value | Lane |
|---|---|---|
| Endpoint | intraday charts, 1/5/15/25/60 min — we fetch 1 only | verified from SDK |
| Response shape | **parallel column arrays**, not rows. Unequal lengths reject the chunk. | documented |
| Timestamp | epoch seconds, UTC | verified from SDK |
| Window cap | 90 days per request | documented |
| History depth | rolling ~5 years. **Not a fixed floor** — it moves every day. | documented |
| Rate limit | 5/s, 100,000/day, no per-minute governor | documented |
| Subscription | paid data plan; enforcement surfaces as a specific error code | documented |
| Credentials | `/<org>/<env>/<vendor>/<field>` — read-only. Fields: `client-id`, `access-token`. Real segments resolved at runtime; see D-0013. | verified |
| Security ids | NIFTY = 13 (verified from the SDK's own example). BANKNIFTY 25, SENSEX 51, INDIA VIX 21 — **community sources only, unverified.** | mixed |
| India VIX candle availability | **UNVERIFIED.** No documentation states it. Treat as a hard gate before relying on it. | unverified |

### 4a. Instrument facts transcribed into source

Golden rule 1: every claim about an instrument names the route that produced
it. These are the instrument-level facts compiled into this repository as Rust
data rather than fetched, so the route has to be recorded here.

| Fact | Where it lives | Route | Lane |
|---|---|---|---|
| **NIFTY Total Market — 750 constituents** | `core::universe::NIFTY_TOTAL_MARKET` | niftyindices.com states the index holds 750 stocks: "all stocks that are part of Nifty 500 and Nifty Microcap 250". The names were transcribed from the local lake's NSE/CASH directories and matched the primary broker's equity list 750/750 with zero misses. Re-checked against both real masters 2026-08-01: 750/750 resolve as a kept equity in **both** vendors, ISINs agreeing. | derived + cross-checked; **UNVERIFIED against an NSE constituent circular** |
| **F&O underlyings — 213 names** | `core::universe::FNO_UNDERLYINGS` | Derived from the derivative rows of both vendor masters, excluding `*NSETEST`. The two brokers agree exactly: 213 each, zero on either side only. | derived + cross-checked; **UNVERIFIED against an NSE circular** |
| **NSE board series — 128 codes** | `core::vendor::EQUITY_BOARD_SERIES`, `SME_BOARD_SERIES`, `NON_EQUITY_SERIES` | The measured union of every distinct `series` on a Groww `NSE/CASH/EQ` row and every distinct `SERIES` on a Dhan `NSE/E/EQUITY` row, 2026-08-01. 6 equity-board, 2 SME, 120 debt and fund. Board verdicts agree on 4,080 of 4,080 shared ISINs. | measured from both masters; **UNVERIFIED against an NSE series circular** |

Both index lists are **snapshots** of rebalanced indices, and none of the three
has been checked against an exchange publication. `docs/06-limits.md` §11
carries what that costs. D-0025 and D-0029.

### 4b. Option-greek facts, measured from a live chain

Golden rule 1 again. `crates/greeks` makes claims about what a vendor's option
chain *means*, and a unit convention nobody states is exactly the kind of claim
that has to name its route. Every row below was measured from **one live Dhan
option-chain response, one strike, both sides**, captured 2026-08-01:
`IV 11.939337251984934 / 9.789193798280868` (percent), `delta 0.53871 /
−0.46732`, `gamma 0.00132 / 0.00109`, `theta −15.1539 / −10.61131`,
`vega 12.2025 / 12.18593`. The response carried **no rho, no spot, no strike,
no timestamp and no expiry.**

| Fact | Where it lives | Route | Lane |
|---|---|---|---|
| **Dhan publishes vega per one percentage point** | `greeks::bsm` module docs; `greeks::vendor_anchor::vega_is_published_per_percentage_point_and_the_index_level_proves_it` | The raw scaling implies an index level of **258.51**; the per-percent scaling implies **25,851.19**, which is where NIFTY trades. | measured; the vendor documents no unit |
| **Dhan publishes theta per a CALENDAR day, not a trading day** | same, and `greeks::vendor_anchor::the_trading_day_divisor_is_excluded_and_the_calendar_one_is_not_selected` | The two sides of one strike must agree on `r`. They agree to **0.9999** points under 365 and **23.2598** under 252 — a factor of 23.3. | measured — but only as an **exclusion of 252** |
| **The divisor is exactly 365** | `greeks::vendor_anchor::the_rate_criterion_has_its_root_at_370_and_365_is_a_convention_near_it` | The same criterion is affine in the divisor and has its root at **`D* = 370.0757`**, where the two sides agree to 2.8e-15 points; it also prefers **375** (spread 0.9700) to 365 (0.9999). `365` is the nearest ordinary calendar convention to that root, not a measurement of it. | **UNVERIFIED.** Withdrawn from "measured" by D-0037 |
| **Dhan's two published IVs are transposed relative to its delta/gamma/vega block** | `greeks::vendor_anchor::the_two_published_volatilities_are_transposed_and_the_identity_says_so` | The scale-free identity `vega·gamma·sigma == n(d1)^2` — no spot, strike, maturity or rate in it — gives 1.21979 / 0.82249 as published and 1.00012 / 1.00315 swapped, against gamma's own printed slack of 0.38% / 0.46%. | measured, **from one strike only.** Whether every strike is transposed the same way is **UNVERIFIED** |
| **Dhan uses standard spot BSM** | `greeks::vendor_anchor::our_greeks_reproduce_the_captured_dhan_chain` | Gamma and vega are near-identical between the two sides at one strike, which is BSM. | measured; **forward Black-76 is not excluded — UNVERIFIED** |
| **Dhan's carry is zero** | `greeks::vendor_anchor::the_carry_is_consistent_with_zero_and_the_sample_cannot_pin_it` | `q = 0` reproduces the chain, and a single volatility would need `q = −42.54%`, which is not a rate. But `q = 1%` and `q = 2%` reproduce **all eight** published fields too, gammas included, at `T = 4.085` and `3.428` calendar days. | **UNVERIFIED.** `q = 0` is *consistent*, not measured. Withdrawn from "measured" by D-0037 |
| **Dhan publishes no rho; Groww publishes all five** | `crates/greeks` ships rho regardless | The captured response has no `rho` field. Groww documents a dedicated Greeks section at `groww.in/trade-api/docs/curl/live-data`. | read from the response and from the vendor documentation |
| **Dhan's risk-free rate** | nowhere — nothing in this repository hardcodes one | Solved at **9.4619%** (call side) and **10.4618%** (put side), each ±0.55 at 95% from the printed precision of delta alone, with a 1.00-point side-to-side residual outside both intervals. | **UNVERIFIED.** A hardcoded 10.0% fits the sample and so does a market rate near 7%; this sample cannot separate them |
| **NIFTY and BANKNIFTY strike intervals** | nowhere — `Moneyness::from_ladder` takes the interval as an argument | No source states them. | **UNVERIFIED.** Assumed nowhere in code |

The maturity is the best-conditioned parameter the sample carries:
`T = 0.0141324716` years = **5.15835 calendar days**, ±0.079 at 95% over the
rounding box of the printed fields. That interval is a **rounding-box width,
not an identification result**, and it is conditional on `q = 0` and on the
transposition — at `q = 2%` the same eight fields give `T = 3.428` days. The
**day count that generates it** is UNVERIFIED without the capture timestamp.

**And the sample is internally inconsistent with spot BSM at one instant.**
Matching both published deltas forces the model's vega ratio to `0.998641`
against the vendor's `1.001360`, in a quantity containing no spot, strike,
maturity or rate. The best possible *single* contract is off by **884× the
vendor's own display precision** — measured. Everything above is fitted with
**two** mutually inconsistent contracts, one per side, which is a diagnostic
and not an agreement. D-0037, and `docs/06-limits.md` §18.

---

## 5. Run identity

```
blake3(
  strategy_mask ‖ direction ‖ instrument ‖ timeframe ‖ mode ‖
  canonical_params ‖ data_digest ‖ vocab_version ‖ commit_sha
)
```

`data_digest` is a streamed digest over **every field of every loaded bar** —
not a sample, not a count-and-endpoints fingerprint. One differing bar re-keys
the identity. That is the point.

No computation runs without that identity recorded first.

---

## 6. The sweep, in one paragraph

Load a contiguous slice of 1-minute bars for one instrument. Compute the 74
condition bits per bar, once. Walk the combination ladder from k=1: at each
level, join the previous frequent frontier with itself, prune any candidate
whose (k−1)-subsets are not all frequent, evaluate the survivors against the
bar-bit column, keep those with at least `min_hits` hits, and recurse. Stop
when a level produces nothing. Rank the survivors, persist a bounded top set,
and record the identity.

Nothing about that paragraph has a tunable depth.
