//! The prices themselves — the page every other page was describing.
//!
//! # Why this file exists
//!
//! `/store` counted. It said `62,978 rows across 194 committed entries` and
//! drew a grid of swatches, and **not one page in this server ever showed a
//! single price**. A store you cannot look inside is a store you have to trust,
//! and the operator's question — *is the data any good?* — had no answer here at
//! all. Every figure was a counter about the data rather than the data.
//!
//! This reads bars back out of the file the ingest path wrote and renders them:
//! timestamp, open, high, low, close, volume, open interest. It is the first
//! surface in this repository where an operator can see what was actually
//! stored.
//!
//! # The cost, and why paging is arithmetic
//!
//! [`store::file::BarFile::read_record`] is a **seek and a fixed-length read**
//! at `offset_of(index)` — `docs/07-o1-architecture.md` layer 1, the whole point
//! of a fixed-stride format. So the row at index 40,000 costs exactly what the
//! row at index 0 costs, and a page of 200 is 200 reads regardless of which page
//! it is. Nothing here scans, and nothing here holds a month in memory: a
//! request for page 5 touches the 200 records of page 5 and no others.
//!
//! `n_valid` comes off the header, so the page count is a division rather than a
//! walk — the same arithmetic `/store` and `/instruments` page by.
//!
//! # Paisa in, rupees on screen, and nothing in between
//!
//! `CLAUDE.md` §7: prices are paisa integers and never a float. That holds all
//! the way here — [`rupees`] formats an `i64` by splitting it, not by dividing
//! it, so there is no float on this path either. `2450075` renders as
//! `24,500.75` through integer arithmetic and string assembly.

use core::fmt::Write as _;

use brutex_core::vendor::Vendor;
use store::file::BarFile;
use store::format::{Bar, OI_NULL};
use store::path::{FileKind, PathParts, StorePath, Timeframe, YearMonth};

/// How many bars one page shows.
///
/// The same 200 every other paged surface uses. A minute-bar month is about
/// 7,500 rows, so a full month is ~38 pages — readable, and every page costs
/// the same.
pub const PAGE_BARS: usize = 200;

/// A price in paisa, rendered as rupees **without a float**.
///
/// `CLAUDE.md` §7 says prices are `i64` paisa and never a float, and a renderer
/// that divides by 100.0 to show them walks that back at the last possible
/// moment — where nobody looks. This splits instead: the rupee part is
/// `paisa / 100`, the paise part is `paisa % 100`, and both are integers.
///
/// Grouped in the Indian convention — `24,500.75`, and `1,23,456.78` past a
/// lakh — because this is an NSE tool and a trader reading `123,456.78` has to
/// stop and re-count.
#[must_use]
pub fn rupees(paisa: i64) -> String {
    let negative = paisa < 0;
    // `unsigned_abs` rather than `abs`: `i64::MIN` has no positive counterpart
    // and `abs` would panic on it. It is also the open-interest null, which is
    // never routed here — but a formatter that panics on one input is a
    // formatter that panics.
    let whole = paisa.unsigned_abs() / 100;
    let hundredths = paisa.unsigned_abs() % 100;

    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + 6);
    let bytes = digits.as_bytes();
    // The last three digits group together; everything before them groups in
    // twos. That is the Indian system, and it is written as arithmetic on the
    // index rather than as a regular expression.
    for (i, b) in bytes.iter().enumerate() {
        let from_end = bytes.len() - i;
        if i > 0 && (from_end == 3 || (from_end > 3 && (from_end - 3).is_multiple_of(2))) {
            grouped.push(',');
        }
        grouped.push(char::from(*b));
    }
    format!(
        "{}{grouped}.{hundredths:02}",
        if negative { "-" } else { "" }
    )
}

/// A bar's timestamp as an IST wall clock, `HH:MM`.
///
/// The store holds UTC microseconds; a trader reads IST. The conversion is the
/// fixed +5:30 India has used since 1942 — `pull::session` owns that constant
/// and this is the same offset, applied for display only. Nothing is stored,
/// compared or filtered on this string.
#[must_use]
pub fn ist_clock(ts_micros: i64) -> String {
    // Integer division throughout: microseconds to seconds, then the offset,
    // then the two fields. `div_euclid` so a pre-epoch timestamp floors rather
    // than truncating toward zero — the store cannot hold one, and a formatter
    // that is wrong for an input it cannot receive is still wrong.
    let secs = ts_micros.div_euclid(1_000_000) + 5 * 3600 + 1800;
    let day_secs = secs.rem_euclid(86_400);
    let (h, m) = (day_secs / 3600, (day_secs % 3600) / 60);
    format!("{h:02}:{m:02}")
}

/// One month of one instrument, opened for reading.
///
/// # Errors
///
/// The path could not be rendered, or the file could not be opened — both
/// carried as the refusal's own words, so a missing month names the path it
/// looked for rather than answering "no data".
pub fn open(
    store_root: &std::path::Path,
    vendor: Vendor,
    exchange: &str,
    segment: &str,
    symbol: &str,
    month: YearMonth,
) -> Result<BarFile, String> {
    let path = StorePath::new(PathParts {
        vendor,
        exchange,
        segment,
        symbol,
        timeframe: Timeframe::MINUTE_1,
        month,
        file: FileKind::Bars,
    })
    .map_err(|why| format!("{symbol} {month}: {why}"))?;
    // THE SYMBOL ID IS DERIVED THE WAY `pull::ingest` DERIVES IT. The store
    // stamps it into the header on create and verifies it on every reopen, so a
    // reader that folded the hash differently would be refused by the file
    // itself — which is the check working, not a bug to route around.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the id is a CROSS-CHECK the store verifies on reopen, never an \
                  index — any 32 bits serve, and the low half of a 64-bit FNV-1a \
                  is what pull::ingest stamps in. A different fold here would \
                  fail to open a file this repository wrote."
    )]
    let symbol_id = brutex_core::universe::fnv1a(symbol) as u32;
    // `open_existing`, NEVER `open_or_create`. This is a GET.
    //
    // The read-write door creates what it cannot find — six directories, a
    // 32 KiB initialised bar file and a `.lock` — so this handler used to
    // manufacture a month for any symbol a caller typed into the query string,
    // and then report that month as empty. The file it described was the file
    // the request had just made.
    //
    // The disk was the smaller cost. The larger one is that creating on read
    // collapses "there is no such month" into "this month holds nothing", which
    // are different answers to an operator, and it made the missing-month arm
    // below unreachable. A `Missing` refusal now names the path it looked for.
    BarFile::open_existing(store_root, path, symbol_id).map_err(|why| why.to_string())
}

/// One page of bars, read by index.
///
/// **Each row is one seek and one fixed-length read.** No scan, no month held in
/// memory, and page 38 costs what page 1 costs.
///
/// A record the file refuses is skipped rather than aborting the page: one
/// unreadable row in the middle of a month should not blank the other 199, and
/// the caller reports the gap. The refusal is returned beside the rows so it can
/// be named on the page — `CLAUDE.md` §4, degrade loudly.
#[must_use]
pub fn page(file: &BarFile, skip: usize, take: usize) -> (Vec<Bar>, Vec<String>) {
    let n = file.header().n_valid;
    let mut rows = Vec::with_capacity(take.min(PAGE_BARS));
    let mut faults = Vec::new();
    for i in skip..skip.saturating_add(take) {
        let Ok(index) = u64::try_from(i) else {
            break;
        };
        if index >= n {
            break;
        }
        match file.read_record(index) {
            Ok(bar) => rows.push(bar),
            Err(why) => faults.push(format!("record {index}: {why}")),
        }
    }
    (rows, faults)
}

/// The table of prices, as HTML.
///
/// Every number is right-aligned and tabular so a column of prices lines up on
/// the decimal point, which is the whole reason a price table is readable at
/// all. Rising bars are tinted green and falling ones red — the convention every
/// terminal uses, applied to `close >= open`.
#[must_use]
pub fn table(rows: &[Bar]) -> String {
    let mut out = String::with_capacity(512 + rows.len() * 220);
    out.push_str(
        "<div class=\"hscroll\"><table class=\"bars\"><thead><tr>\
         <th>#</th><th>Time IST</th><th class=\"num\">Open</th><th class=\"num\">High</th>\
         <th class=\"num\">Low</th><th class=\"num\">Close</th><th class=\"num\">Change</th>\
         <th class=\"num\">Volume</th><th class=\"num\">Open interest</th>\
         </tr></thead><tbody>",
    );
    for (i, bar) in rows.iter().enumerate() {
        // `close - open` in paisa, then rendered by the same integer formatter.
        // A percentage would need a division and this file holds no float.
        let delta = bar.close.saturating_sub(bar.open);
        let dir = match delta.cmp(&0) {
            core::cmp::Ordering::Greater => "up",
            core::cmp::Ordering::Less => "down",
            core::cmp::Ordering::Equal => "flat",
        };
        let sign = if delta > 0 { "+" } else { "" };
        let _ = write!(
            out,
            "<tr class=\"{dir}\"><td class=\"idx\">{}</td><td class=\"clock\">{}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>\
             <td class=\"num close\">{}</td><td class=\"num delta\">{sign}{}</td>\
             <td class=\"num\">{}</td><td class=\"num oi\">{}</td></tr>",
            i + 1,
            ist_clock(bar.ts_micros),
            rupees(bar.open),
            rupees(bar.high),
            rupees(bar.low),
            rupees(bar.close),
            rupees(delta),
            bar.volume,
            // `i64::MIN` is the null, and zero means zero — CLAUDE.md §7. A
            // dash for the null and a `0` for a real zero are different claims
            // and must not render the same.
            if bar.open_interest == OI_NULL {
                "—".to_owned()
            } else {
                bar.open_interest.to_string()
            }
        );
    }
    out.push_str("</tbody></table></div>");
    out
}

/// The stylesheet the price table needs, appended to the shared one.
pub const BARS_STYLE: &str = "\
table.bars td.num,table.bars th.num{text-align:right;font-variant-numeric:tabular-nums;\
font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}\
table.bars td.idx{color:var(--dim);font-size:12px;font-variant-numeric:tabular-nums}\
table.bars td.clock{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;\
font-weight:650;font-variant-numeric:tabular-nums}\
table.bars td.close{font-weight:700}\
table.bars tr.up td.close,table.bars tr.up td.delta{color:var(--ok)}\
table.bars tr.down td.close,table.bars tr.down td.delta{color:var(--bad)}\
table.bars tr.flat td.delta{color:var(--dim)}\
table.bars td.oi{color:var(--dim)}\
table.bars tbody tr:hover{background:color-mix(in srgb,var(--acc) 6%,transparent)}\
";

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]
mod tests {
    use super::*;

    /// **NO FLOAT REACHES A PRICE, INCLUDING ON THE WAY TO A SCREEN.**
    ///
    /// `CLAUDE.md` §7 fixes prices as paisa integers. A renderer that divides
    /// by 100.0 to show them walks that back at the last possible moment, where
    /// nobody looks — and `24500.75` is not exactly representable in binary
    /// floating point, so the last digit is a coin toss.
    #[test]
    fn a_price_renders_from_integers_and_keeps_every_paisa() {
        assert_eq!(rupees(2_450_075), "24,500.75");
        assert_eq!(rupees(0), "0.00", "zero is a price, not an absence");
        assert_eq!(rupees(1), "0.01", "one paisa survives");
        assert_eq!(rupees(10), "0.10", "and one tenth is TEN paise");
        assert_eq!(rupees(99), "0.99");
        assert_eq!(rupees(100), "1.00");
        assert_eq!(rupees(-2_450_075), "-24,500.75", "a fall keeps its sign");
    }

    /// Grouped the way an NSE screen groups: three, then twos.
    #[test]
    fn the_grouping_is_indian_and_not_western() {
        assert_eq!(rupees(100_000), "1,000.00");
        assert_eq!(rupees(10_000_000), "1,00,000.00", "a lakh, not 100,000");
        assert_eq!(rupees(1_000_000_000), "1,00,00,000.00", "a crore");
        assert_eq!(rupees(12_345_678), "1,23,456.78");
        assert_eq!(rupees(12_345), "123.45", "no comma below a thousand");
    }

    /// The formatter must not panic on the one value it can never divide.
    #[test]
    fn the_extreme_of_the_type_formats_rather_than_panicking() {
        // `i64::MIN.abs()` panics; `unsigned_abs` does not. This value is the
        // open-interest null and is never routed here — a formatter that
        // panics on an input it "cannot receive" is still a panic.
        let shown = rupees(i64::MIN);
        assert!(shown.starts_with('-'), "{shown}");
        assert!(!shown.is_empty());
        let _ = rupees(i64::MAX);
    }

    /// The clock is IST, from a UTC store, by integer arithmetic.
    #[test]
    fn the_clock_reads_ist_from_a_utc_timestamp() {
        // 2025-07-01 03:45:00 UTC is 09:15 IST — the session open.
        assert_eq!(ist_clock(1_751_341_500_000_000), "09:15");
        assert_eq!(ist_clock(1_751_341_560_000_000), "09:16");
        // 10:00 UTC is 15:30 IST — the close.
        assert_eq!(ist_clock(1_751_364_000_000_000), "15:30");
        // And it wraps a day rather than running past 23:59.
        assert_eq!(ist_clock(1_751_400_000_000_000), "01:30");
    }

    /// A rising bar, a falling bar and a flat one are three different rows.
    #[test]
    fn direction_is_encoded_in_the_row_and_not_only_in_the_number() {
        let bar = |open: i64, close: i64| Bar {
            ts_micros: 1_751_341_500_000_000,
            open,
            high: close.max(open),
            low: close.min(open),
            close,
            volume: 100,
            open_interest: OI_NULL,
        };
        let html = table(&[
            bar(2_450_000, 2_451_000),
            bar(2_451_000, 2_450_000),
            bar(2_450_000, 2_450_000),
        ]);
        assert_eq!(html.matches("<tr class=\"up\">").count(), 1, "{html}");
        assert_eq!(html.matches("<tr class=\"down\">").count(), 1, "{html}");
        assert_eq!(html.matches("<tr class=\"flat\">").count(), 1, "{html}");
        assert!(html.contains(">+10.00<"), "a rise is signed: {html}");
        assert!(
            html.contains(">-10.00<"),
            "a fall carries its minus: {html}"
        );
        // The null open interest is a dash; a real zero would be `0`.
        assert_eq!(html.matches("<td class=\"num oi\">—</td>").count(), 3);
    }

    /// Zero open interest is a measurement and must not render as the null.
    #[test]
    fn a_real_zero_open_interest_is_not_a_dash() {
        let html = table(&[Bar {
            ts_micros: 1_751_341_500_000_000,
            open: 1,
            high: 1,
            low: 1,
            close: 1,
            volume: 0,
            open_interest: 0,
        }]);
        assert!(html.contains("<td class=\"num oi\">0</td>"), "{html}");
        assert!(!html.contains("oi\">—"), "zero means zero: {html}");
    }

    /// An empty month renders a table with no rows rather than nothing at all.
    #[test]
    fn no_bars_still_renders_a_table_with_its_headings() {
        let html = table(&[]);
        assert!(html.contains("<tbody></tbody>"), "{html}");
        assert!(html.contains("Open interest"), "the headings stay: {html}");
    }

    /// No page this server emits carries a script, and this one is no exception.
    /// **A `GET` NEVER CREATES A MONTH.**
    ///
    /// `open` used to call `BarFile::open_or_create`, the store's read-write
    /// door, so this handler manufactured whatever tuple arrived in the query
    /// string: six directories, a 32 KiB initialised bar file and a `.lock`
    /// beside it. It then rendered "this month is empty" about the file the
    /// request had just created. Reproduced live before the fix against a
    /// symbol that has never traded.
    ///
    /// The assertion that matters is the second one. It is not enough to check
    /// that the call refused — `open_or_create` also refuses eventually, on the
    /// symbol-id or timeframe cross-check, *after* the file is on disk. So this
    /// walks the tree and requires it to be empty, which is the only form of the
    /// claim a creating opener cannot satisfy.
    #[test]
    fn asking_for_a_month_that_was_never_written_creates_nothing() {
        fn walk(dir: &std::path::Path, into: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, into);
                }
                into.push(path.display().to_string());
            }
        }

        let root = std::env::temp_dir().join(format!(
            "brutex-bars-readonly-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");

        let month = YearMonth::new(2025, 7).expect("a real month");
        let outcome = open(&root, Vendor::Dhan, "NSE", "INDEX", "NOTAREALSYMBOL", month);

        let why = outcome.expect_err(
            "a month nobody has ever written must refuse, not be conjured into \
             existence so it can be reported empty",
        );
        assert!(
            why.contains("NOTAREALSYMBOL"),
            "the refusal must name the path it looked for, so an operator can \
             see WHICH month is absent: {why}"
        );

        let mut found = Vec::new();
        walk(&root, &mut found);
        assert!(
            found.is_empty(),
            "a read-only GET wrote {} entries to the store: {found:#?}",
            found.len()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_price_table_carries_no_script() {
        let html = table(&[Bar {
            ts_micros: 1,
            open: 1,
            high: 1,
            low: 1,
            close: 1,
            volume: 1,
            open_interest: 1,
        }]);
        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }
}
