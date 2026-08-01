//! Turning instruments into HTML.
//!
//! # Why HTML is rendered on the server
//!
//! `CLAUDE.md` §2 allows `.rs`, `.toml`, `.md`, `.lock`, `.html`, `.css` and
//! `.yml`. **`.js` is not on that list**, and CI gate 1 walks every tracked
//! file — so a browser UI here cannot be a script framework.
//!
//! Two routes remain. WebAssembly is one, but `crates/web` is permitted
//! exactly one dependency (`core`) and a wasm binary cannot touch the DOM
//! without `wasm-bindgen`, so that route is blocked on a decision nobody has
//! taken. Server-rendered HTML is the other, and it needs no decision at all:
//! the server already holds the data and `core` already holds every display
//! rule.
//!
//! # Why escaping is not optional
//!
//! Instrument symbols come from a vendor file. A vendor is not an attacker,
//! but a vendor is also not a validator, and `<` in a symbol would break the
//! page. [`Symbol`](core::symbol::Symbol) already refuses every byte outside
//! `A-Z 0-9 - _ &`, so the dangerous characters cannot reach here — but `&`
//! **can**, and an unescaped `&` produces malformed HTML. Escaping is applied
//! anyway, because a rule that depends on a guarantee two crates away is a
//! rule that breaks when that guarantee moves.
//!
//! # Cost
//!
//! Rendering is **O(rows on the page)**, never O(rows in the universe). A page
//! shows what a person can read; the other 90,000 instruments cost nothing
//! because they are never touched.

use core::instrument::{InstrumentKey, Kind};
use std::fmt::Write as _;

/// The stylesheet, inlined so the page is a single response.
///
/// Kept here rather than in a `.css` file on purpose: one file means one
/// request and no second route to serve, and the page is small enough that
/// splitting it would be organisation for its own sake.
const STYLE: &str = "\
body{font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;margin:2rem;color:#111;background:#fff}\
h1{font-size:1.1rem;margin:0 0 .25rem}\
p.sub{color:#666;margin:0 0 1.25rem}\
table{border-collapse:collapse;width:100%}\
th,td{text-align:left;padding:.35rem .6rem;border-bottom:1px solid #e5e5e5;white-space:nowrap}\
th{background:#fafafa;font-weight:600;border-bottom:2px solid #ddd}\
td.num{text-align:right;font-variant-numeric:tabular-nums}\
tr:hover td{background:#f6f9ff}\
.tag{font-size:11px;padding:.1rem .4rem;border-radius:3px;background:#eef;color:#334}\
.swept{background:#e6f7e6;color:#141}\
footer{margin-top:1.5rem;color:#888;font-size:12px}";

/// Escapes the five characters that change HTML meaning.
///
/// # Examples
///
/// ```
/// # use api::render::escape;
/// assert_eq!(escape("M&M"), "M&amp;M");
/// assert_eq!(escape("NIFTY"), "NIFTY");
/// ```
#[must_use]
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// One row of the instruments table.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    /// The canonical identity.
    pub key: InstrumentKey,
    /// Whether the primary broker's master listed it.
    pub from_groww: bool,
    /// Whether the secondary broker's master listed it.
    pub from_dhan: bool,
}

/// Renders one row's kind, expiry and strike cells.
fn kind_cells(kind: Kind) -> String {
    match kind {
        Kind::Index => "<td>Index</td><td>—</td><td class=\"num\">—</td>".to_owned(),
        Kind::Equity => "<td>Equity</td><td>—</td><td class=\"num\">—</td>".to_owned(),
        Kind::Future { expiry } => {
            format!("<td>Future</td><td>{expiry}</td><td class=\"num\">—</td>")
        }
        Kind::Option {
            expiry,
            strike,
            side,
        } => {
            // Strike is paisa; a human reads rupees. core owns the split so the
            // browser and the server cannot disagree about it.
            format!(
                "<td>Option {}</td><td>{}</td><td class=\"num\">{}.{:02}</td>",
                side.as_str(),
                expiry,
                strike.rupees_trunc(),
                strike.paisa_part().abs()
            )
        }
    }
}

/// Renders the vendor column, which is the visible proof of deduplication.
///
/// When both brokers list the same contract they resolve to one
/// [`InstrumentKey`] and therefore one row — so seeing `groww · dhan` in a
/// single row *is* the O(1) dedup, on screen.
fn vendor_cell(row: &Row) -> String {
    let mut tags = Vec::new();
    if row.from_groww {
        tags.push("<span class=\"tag\">groww</span>");
    }
    if row.from_dhan {
        tags.push("<span class=\"tag\">dhan</span>");
    }
    format!("<td>{}</td>", tags.join(" "))
}

/// Renders a complete instruments page.
///
/// `total` is the size of the whole universe, `rows` is only what this page
/// shows. Passing both keeps the page honest about the difference between what
/// exists and what was rendered.
#[must_use]
pub fn instruments_page(title: &str, total: usize, rows: &[Row]) -> String {
    let mut body = String::with_capacity(1024 + rows.len() * 256);
    body.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    body.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    body.push_str("<title>");
    body.push_str(&escape(title));
    body.push_str("</title><style>");
    body.push_str(STYLE);
    body.push_str("</style></head><body>");

    body.push_str("<h1>");
    body.push_str(&escape(title));
    body.push_str("</h1>");
    // Writing to a String is infallible, so the Result is deliberately
    // discarded rather than unwrapped -- `unwrap_used` and `expect_used` are
    // denied workspace-wide and neither belongs in a renderer.
    let _ = write!(
        body,
        "<p class=\"sub\">{total} instruments total · showing {} · \
         a green row is one of the two the engine sweeps</p>",
        rows.len()
    );

    body.push_str(
        "<table><thead><tr>\
         <th>Canonical key</th><th>Vendors</th><th>Underlying</th>\
         <th>Kind</th><th>Expiry</th><th>Strike</th>\
         </tr></thead><tbody>",
    );

    for row in rows {
        let swept = if row.key.is_sweepable() {
            " class=\"swept\""
        } else {
            ""
        };
        let _ = write!(
            body,
            "<tr{}><td>{}</td>{}<td>{}</td>{}</tr>",
            swept,
            escape(&row.key.to_string()),
            vendor_cell(row),
            escape(row.key.underlying.as_str()),
            kind_cells(row.key.kind),
        );
    }

    body.push_str("</tbody></table>");
    body.push_str(
        "<footer>Rendered on the server. No JavaScript — \
         CLAUDE.md section 2 does not permit it, and CI gate 1 enforces that.</footer>",
    );
    body.push_str("</body></html>");
    body
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::*;
    use core::instrument::{Exchange, Expiry, OptionSide, Segment};
    use core::price::Paisa;
    use core::symbol::Symbol;

    fn nifty() -> InstrumentKey {
        InstrumentKey::index(Exchange::Nse, "NIFTY").expect("valid")
    }

    fn opt() -> InstrumentKey {
        InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Fno,
            underlying: Symbol::new("NIFTY").expect("valid"),
            kind: Kind::Option {
                expiry: Expiry::new(2026, 8, 4).expect("valid"),
                strike: Paisa::from_raw(1_945_000),
                side: OptionSide::Call,
            },
        }
    }

    #[test]
    fn escapes_every_html_significant_character() {
        assert_eq!(escape("a&b"), "a&amp;b");
        assert_eq!(escape("a<b"), "a&lt;b");
        assert_eq!(escape("a>b"), "a&gt;b");
        assert_eq!(escape("a\"b"), "a&quot;b");
        assert_eq!(escape("a'b"), "a&#39;b");
        assert_eq!(escape("NIFTY"), "NIFTY", "ordinary text is untouched");
        assert_eq!(escape(""), "");
    }

    #[test]
    fn an_ampersand_symbol_cannot_break_the_page() {
        // Symbol permits '&' -- M&M is a real NSE listing -- so this is the
        // one dangerous character that genuinely reaches the renderer.
        let key = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Cash,
            underlying: Symbol::new("M&M").expect("valid"),
            kind: Kind::Equity,
        };
        let html = instruments_page(
            "x",
            1,
            &[Row {
                key,
                from_groww: true,
                from_dhan: false,
            }],
        );
        assert!(html.contains("M&amp;M"), "the ampersand must be escaped");
        assert!(
            !html.contains("<td>M&M<"),
            "a raw ampersand must never reach the output"
        );
    }

    #[test]
    fn a_swept_instrument_is_marked_and_others_are_not() {
        let html = instruments_page(
            "NSE",
            2,
            &[
                Row {
                    key: nifty(),
                    from_groww: true,
                    from_dhan: true,
                },
                Row {
                    key: opt(),
                    from_groww: true,
                    from_dhan: false,
                },
            ],
        );
        assert_eq!(
            html.matches("class=\"swept\"").count(),
            1,
            "exactly one of these two is swept"
        );
    }

    #[test]
    fn both_vendors_on_one_row_is_the_visible_dedup() {
        let html = instruments_page(
            "x",
            1,
            &[Row {
                key: nifty(),
                from_groww: true,
                from_dhan: true,
            }],
        );
        assert!(html.contains(">groww<"));
        assert!(html.contains(">dhan<"));
        assert_eq!(html.matches("<tr").count(), 2, "header plus ONE data row");
    }

    #[test]
    fn a_single_vendor_row_shows_only_that_vendor() {
        let only_dhan = instruments_page(
            "x",
            1,
            &[Row {
                key: nifty(),
                from_groww: false,
                from_dhan: true,
            }],
        );
        assert!(only_dhan.contains(">dhan<"));
        assert!(!only_dhan.contains(">groww<"));
    }

    #[test]
    fn every_kind_renders_its_own_cells() {
        assert!(kind_cells(Kind::Index).contains("Index"));
        assert!(kind_cells(Kind::Equity).contains("Equity"));

        let fut = kind_cells(Kind::Future {
            expiry: Expiry::new(2026, 9, 29).expect("valid"),
        });
        assert!(fut.contains("Future") && fut.contains("2026-09-29"));

        let call = kind_cells(Kind::Option {
            expiry: Expiry::new(2026, 8, 4).expect("valid"),
            strike: Paisa::from_raw(1_945_000),
            side: OptionSide::Call,
        });
        assert!(call.contains("Option CE"), "side must be shown");
        assert!(
            call.contains("19450.00"),
            "1,945,000 paisa reads as 19450.00 rupees, not as paisa"
        );

        let put = kind_cells(Kind::Option {
            expiry: Expiry::new(2026, 8, 4).expect("valid"),
            strike: Paisa::from_raw(2_700_050),
            side: OptionSide::Put,
        });
        assert!(put.contains("Option PE"));
        assert!(put.contains("27000.50"), "the paisa part must not be lost");
    }

    #[test]
    fn the_page_states_the_true_total_not_the_rendered_count() {
        // Rendering is O(rows shown). The page must not imply it looked at
        // more than it did, nor hide how large the universe is.
        let html = instruments_page(
            "NSE FNO",
            90_623,
            &[Row {
                key: nifty(),
                from_groww: true,
                from_dhan: true,
            }],
        );
        assert!(html.contains("90623 instruments total"));
        assert!(html.contains("showing 1"));
    }

    #[test]
    fn an_empty_page_is_still_well_formed() {
        let html = instruments_page("nothing here", 0, &[]);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.ends_with("</html>"));
        assert!(html.contains("<tbody></tbody>"));
        assert_eq!(html.matches("<tr").count(), 1, "header row only");
    }

    #[test]
    fn the_page_contains_no_script_at_all() {
        // CLAUDE.md section 2 does not permit JavaScript, and a renderer that
        // emitted a <script> tag would smuggle it past gate 1, which only
        // inspects tracked FILES.
        let html = instruments_page(
            "x",
            1,
            &[Row {
                key: opt(),
                from_groww: true,
                from_dhan: true,
            }],
        );
        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }

    #[test]
    fn the_title_is_escaped_too() {
        let html = instruments_page("<b>x</b>", 0, &[]);
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(!html.contains("<b>x</b>"));
    }
}
