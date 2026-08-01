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
