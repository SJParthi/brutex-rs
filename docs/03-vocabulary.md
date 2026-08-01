# 03 — Condition vocabulary

74 conditions. Bit index is the identity: **never renumbered, never reused,
never reordered.** New conditions append at the next free bit.

The mask is `u128`. Bits 0–73 are live; 74–127 are free headroom — 54 more
conditions can be added without touching the mask type, the store, or any
existing result.

---

## 1. Why the index is frozen

A stored result set is a set of masks. A mask is a set of bit positions. If
bit 22 means `near_fib_50` today and `near_pivot_r4` after a helpful reorder,
every historical result silently means something different and no test can
detect it — the bytes are identical.

So: append only. A retired condition keeps its position forever as a tombstone
that always evaluates false. The position is never recycled.

---

## 2. Evaluation contract

For bar *i*, the evaluator produces one `u128` where bit *b* is set iff
condition *b* holds at that bar.

A candidate mask *M* **hits** bar *i* iff:

```rust
(bar_bits[i] & M) == M
```

This is the superset relation, and it is **anti-monotone**: adding a required
bit can only remove hits. Every pruning guarantee in the sweep rests on that
one property, so it is stated here rather than buried in the engine.

Bits are computed **once per slice** and shared read-only. They are never
recomputed per candidate, per worker, or per level.

---

## 3. Look-ahead

At bar *i* the evaluator may read bars `0..=i` and nothing else. State that
carries forward — moving averages, pivots, swing detection — updates **after**
the bar is emitted, never before.

Swing-based conditions (`bos_*`, `choch_*`, `near_swing_*`) confirm a swing
only *k* bars after it occurred. That latency is correct and must not be
"fixed": removing it would be look-ahead.

---

## 4. Conditions that do not apply to daily bars

Time-of-day bits (44–47) and VWAP bits (52–53) are meaningless on a 1-day bar.
On a daily timeframe they are cleared, not left as noise. A bit that cannot be
evaluated evaluates false — it never evaluates to "probably".

VWAP additionally requires traded volume. Spot indices carry none, so bits
52–53 permanently abstain on the three engine instruments. That is honest and
documented rather than quietly producing zeros that look like signal.

---

## 5. The table

### Moving averages — bits 0–5

| Bit | Name |
|---:|---|
| 0 | `close_above_ema20` |
| 1 | `close_below_ema20` |
| 2 | `close_above_ema200` |
| 3 | `close_below_ema200` |
| 4 | `ema20_above_ema200` |
| 5 | `ema20_below_ema200` |

### Classic pivots — bits 6–12

| Bit | Name |
|---:|---|
| 6 | `near_pivot_p` |
| 7 | `near_pivot_r1` |
| 8 | `near_pivot_r2` |
| 9 | `near_pivot_r3` |
| 10 | `near_pivot_s1` |
| 11 | `near_pivot_s2` |
| 12 | `near_pivot_s3` |

### Previous-day high / low — bits 13–18

| Bit | Name |
|---:|---|
| 13 | `close_above_pdh` |
| 14 | `close_below_pdh` |
| 15 | `close_above_pdl` |
| 16 | `close_below_pdl` |
| 17 | `near_pdh` |
| 18 | `near_pdl` |

### Fibonacci — bearish anchor (PDH) — bits 19–29

| Bit | Name |
|---:|---|
| 19 | `near_fib_0` |
| 20 | `near_fib_236` |
| 21 | `near_fib_382` |
| 22 | `near_fib_50` |
| 23 | `near_fib_618` |
| 24 | `near_fib_786` |
| 25 | `near_fib_100` |
| 26 | `near_fib_1272` |
| 27 | `near_fib_1618` |
| 28 | `near_fib_200` |
| 29 | `near_fib_2618` |

### Bar shape — bits 30–36

| Bit | Name |
|---:|---|
| 30 | `bar_bullish` |
| 31 | `bar_bearish` |
| 32 | `bar_doji` |
| 33 | `bar_large_body` |
| 34 | `bar_small_body` |
| 35 | `long_upper_wick` |
| 36 | `long_lower_wick` |

### Prior-bar sequence — bits 37–39

| Bit | Name |
|---:|---|
| 37 | `prior_n_bullish` |
| 38 | `prior_n_bearish` |
| 39 | `prior_alternating` |

### Position within the day — bits 40–43

| Bit | Name |
|---:|---|
| 40 | `close_at_day_high` |
| 41 | `close_at_day_low` |
| 42 | `close_in_upper_third` |
| 43 | `close_in_lower_third` |

### Time of day — bits 44–47

| Bit | Name |
|---:|---|
| 44 | `early_morning` |
| 45 | `mid_morning` |
| 46 | `midday` |
| 47 | `afternoon` |

### Day type — bits 48–51

| Bit | Name |
|---:|---|
| 48 | `gap_up_day` |
| 49 | `gap_down_day` |
| 50 | `inside_day` |
| 51 | `outside_day` |

### VWAP — bits 52–53

| Bit | Name |
|---:|---|
| 52 | `close_above_vwap` |
| 53 | `close_below_vwap` |

### Extended pivots — bits 54–55

| Bit | Name |
|---:|---|
| 54 | `near_pivot_r5` |
| 55 | `near_pivot_s5` |

### Market structure — bits 56–59

| Bit | Name |
|---:|---|
| 56 | `bos_bullish` |
| 57 | `bos_bearish` |
| 58 | `choch_bullish` |
| 59 | `choch_bearish` |

### Central pivot range — bits 60–63

| Bit | Name |
|---:|---|
| 60 | `above_cpr_tc` |
| 61 | `below_cpr_bc` |
| 62 | `inside_cpr` |
| 63 | `narrow_cpr_day` |

### SuperTrend — bits 64–65

| Bit | Name |
|---:|---|
| 64 | `close_above_supertrend` |
| 65 | `close_below_supertrend` |

### Gap midpoint — bits 66–68

| Bit | Name |
|---:|---|
| 66 | `close_above_gap_mid` |
| 67 | `close_below_gap_mid` |
| 68 | `near_gap_mid` |

### Fibonacci — bullish anchor (PDL) — bits 69–70

| Bit | Name |
|---:|---|
| 69 | `near_fib_bull_236` |
| 70 | `near_fib_bull_786` |

### Fibonacci — extension — bit 71

| Bit | Name |
|---:|---|
| 71 | `near_fib_424` |

### Swing levels — bits 72–73

| Bit | Name |
|---:|---|
| 72 | `near_swing_high` |
| 73 | `near_swing_low` |

---

## 6. Headroom

| | |
|---|---|
| live bits | 74 (0–73) |
| free bits | 54 (74–127) |
| mask type | `u128` |

Adding a condition: append at bit 74, implement the evaluator, add a row here,
add an entry to `docs/05-decisions.md`. No other file changes. No migration.
No existing result becomes invalid.
