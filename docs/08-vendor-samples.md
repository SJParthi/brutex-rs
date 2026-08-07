# 08 — Vendor sample archives

Where the real vendor data lives, and what was **measured** from it.

The archives themselves are **not in this repository and never will be**.
`CLAUDE.md` §2 permits seven tracked extensions and `.zip`, `.csv` and `.xlsx`
are not among them — CI gate 1 walks `git ls-files` and exits non-zero on the
first one. The four archives are also **2.5 GB**, which a public repository
would carry in every clone forever.

So this file is the index: what each archive is, where it lives, and the shape
that was read out of it. Every row below was produced by opening the file, not
by reading a vendor's documentation.

---

## Where they live

```
~/.brutex/vendor-data/
```

The same directory tree this project already uses for `masters/` and the
untracked credential path configuration. Copied out of `~/Downloads` because a
downloads folder is volatile and one cleanup would take the evidence with it.

| Archive | Size | What it is |
|---|---|---|
| `TrueData.zip` | 1.5 GB | TrueData historical, multiple segments and granularities |
| `GDFL.zip` | 641 MB | Global Datafeeds historical, NFO |
| `ezyZip.zip` | 378 MB | Futures and options CSVs, per-contract |
| `ES Trading Calculation (1).xlsx` | 20 KB | Operator's own sheet — `Pivot`, `GAPUP`, `GAPDOWN`, `VIX` tabs |

---

## The headline finding: neither vendor sells tick-by-tick

Both feeds marketed as *tick* are **one-second snapshots**. Measured, not
assumed:

| | TrueData | GDFL |
|---|---|---|
| Timestamp resolution | **second** | **second** |
| Sub-second field | **none** | **none** |
| Max rows sharing one second | **3** | **4** |
| BANKNIFTY 2022-10-03, rows in the day | **22,426** | — |
| Seconds in a 09:15–15:30 session | 22,500 | — |
| **Rows ÷ seconds** | **≈ 1.00** | — |

22,426 rows across 22,500 seconds is one snapshot per second, not a trade
feed. Rows sharing a second carry **no tiebreaker**, so their only ordering is
file order — and any re-sort destroys information that was never written down.

**Consequence for the store.** A record address cannot be computed from a
timestamp when four records claim the same second. The fixed-stride bar model
(`base + header + N·56`, one record per grid slot) does not hold for this feed.
`CLAUDE.md` §4: *"A new field is a new file version at its own stride."*

---

## What is actually in the files

### TrueData

Outer zip → **one nested zip per day** → one CSV per instrument.

```
NSE_IDX_TICK_20221003.zip     71 members (indices)
NSE_FUT_TICK_20221003.zip     futures, with a nested "Continuous Futures/" folder
NSE_OPT_TICK_20221003.zip     options
```

Archive name is `NSE_{SEGMENT}_TICK_{YYYYMMDD}.zip` — **computable from
(segment, date)**, so the path is arithmetic and never a directory scan
(`docs/07-o1-architecture.md` law 3).

No header row. Date format `YYYYMMDD`.

`NSE_IDX_TICK_20221003.zip → BANKNIFTY.csv`:

```
20221003,09:07:41,38444.90,0,0
20221003,15:31:42,38029.65,0,0
```

### GDFL

Outer zip → folder → `Options/` and `Futures/` (with a further `-III/` level)
→ one CSV per contract, named by the contract.

```
GFDLNFO_TICK_01072025/Options/NIFTY25SEP2525700PE.NFO.csv
GFDLNFO_TICK_01072025/Futures/-III/FINNIFTY-III.NFO.csv
```

**24,264 CSV members** in one day's archive — 22,980 options, 1,284 futures.

Header row present. Date format **`DD/MM/YYYY`** — `01/07/2025` is 1 July, not
7 January. Reading it as the other order shifts every bar by months, silently.

```
Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest
FINNIFTY-III.NFO,01/07/2025,09:16:16,27674,0,0,0,0,65,65
```

`LTQ` is `0` on most rows — those are **quote** updates, not trades.

---

## Column layout varies **by segment inside one vendor**

This is the finding that breaks a per-vendor layout field.

| Vendor | Segment | Columns | Shape |
|---|---|---|---|
| TrueData | index | **5** | `date, time, price, 0, 0` |
| TrueData | futures | **9** | + volume, open interest |
| GDFL | options / futures | **10** | + bid, bid size, ask, ask size, last qty |

Same vendor, same archive, same day, **different column counts**. Indices have
no volume and no open interest, so those fields are structurally absent rather
than zero. A single per-vendor layout would mis-parse one of the two, and the
failure is silent: a price column read as a volume yields plausible numbers.

**The layout must therefore be keyed by `(vendor, segment)`.**

---

## Prints exist outside the session window

From `BANKNIFTY.csv`, 2022-10-03:

| | |
|---|---|
| First row | **09:07:41** — eight minutes before the open |
| Last row | **15:31:42** — after the 15:30 close |

The session filter drops both, correctly. What matters is that they must land
in `DropCensus` under `BeforeSessionOpen` and `AtOrAfterSessionClose` rather
than disappearing — a bar that vanishes without a counted reason is
indistinguishable from a bar the vendor never sent.

These files predate the 2026-08-03 session change and therefore establish the
**old** regime empirically. They say nothing about the new one.

---

## What this means for the vendor descriptor

Every row here is a field, not a branch:

| Field | Proven by |
|---|---|
| Transport: HTTP vs local archive | TrueData and GDFL have no API on this path |
| Nesting scheme | zip-of-zips vs zip-of-folders |
| Archive naming pattern | `NSE_{SEG}_TICK_{YYYYMMDD}.zip` |
| Member naming pattern | `{CONTRACT}.NFO.csv` |
| Header row present | none vs present |
| Column layout **per (vendor, segment)** | 5 / 9 / 10 columns |
| Date format | `YYYYMMDD` vs `DD/MM/YYYY` |
| Granularity | one-second snapshot, not tick |

---

## UNVERIFIED

- **The `ezyZip.zip` contents have not been read** beyond its member listing
  (`Options.zip`, `Futures/-II/*.NFO.csv`). Its column shape is assumed to
  match GDFL and that assumption is untested.
- **`ES Trading Calculation (1).xlsx` has not been opened** beyond its sheet
  names. What its `Pivot`, `GAPUP`, `GAPDOWN` and `VIX` tabs compute is
  unknown.
- **No archive has been read end to end.** Every figure above comes from the
  first members of the first archives; a malformed row deeper in a file would
  not have been seen.
- **Nothing here has been checked against the vendors' own documentation.**
  These are observations of files, and a file can be wrong.
