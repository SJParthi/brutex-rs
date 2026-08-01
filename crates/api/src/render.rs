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
//! page. [`brutex_core::symbol::Symbol`] already refuses every byte outside
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

use brutex_core::instrument::{InstrumentKey, Kind};
use brutex_core::isin::Isin;
use brutex_core::universe::Universe;
use brutex_core::vendor::{Vendor, VendorSet};
use std::fmt::Write as _;

/// The stylesheet, inlined so the page is a single response.
///
/// Kept here rather than in a `.css` file on purpose: one file means one
/// request and no second route to serve, and the page is small enough that
/// splitting it would be organisation for its own sake.
/// The whole stylesheet, inlined.
///
/// # Why every effect here is CSS and none of it is script
///
/// `.js` is not an allowed tracked extension (CLAUDE.md §2), so the page has
/// no scripting available to it at all. That turns out to cost nothing: the
/// animations below run on the compositor thread, which is strictly faster
/// than a script-driven equivalent and cannot block, throw, or leak. A page
/// that renders correctly with scripting disabled is also a page that cannot
/// break in a browser we never tested.
///
/// `prefers-reduced-motion` disables every animation in one rule, and
/// `prefers-color-scheme` supplies the dark palette, so both are honoured
/// without a preference toggle to store.
const STYLE: &str = "\
*{box-sizing:border-box;margin:0;padding:0}\
:root{--bg:#f5f7fb;--panel:#fff;--ink:#0a0f1e;--dim:#5a6478;--line:#e4e9f3;\
--acc:#4f46e5;--acc2:#0ea5e9;--ok:#059669;--warn:#d97706;--bad:#dc2626;\
--sh:0 1px 2px rgba(16,24,40,.05),0 10px 30px rgba(16,24,40,.07)}\
@media(prefers-color-scheme:dark){:root{--bg:#060911;--panel:#0e1524;--ink:#e9efff;--dim:#8f9db6;\
--line:#1b2540;--acc:#818cf8;--acc2:#38bdf8;--sh:0 1px 2px rgba(0,0,0,.5),0 12px 36px rgba(0,0,0,.55)}}\
body{background:var(--bg);color:var(--ink);\
font:15px/1.55 ui-sans-serif,-apple-system,Segoe UI,Inter,system-ui,sans-serif;\
-webkit-font-smoothing:antialiased;padding:0 0 64px}\
h1{font-size:clamp(22px,3.4vw,31px);letter-spacing:-1.1px;font-weight:830;line-height:1.12;\
margin:0 auto;max-width:1180px;padding:34px 20px 6px;animation:rise .6s cubic-bezier(.2,.8,.2,1) both}\
@keyframes rise{from{opacity:0;transform:translateY(12px)}}\
p.sub{color:var(--dim);font-size:14px;margin:0 auto 18px;max-width:1180px;padding:0 20px;\
animation:rise .6s .06s cubic-bezier(.2,.8,.2,1) both}\
form{margin:0 auto 16px;max-width:1180px;padding:0 20px;display:flex;gap:9px;align-items:center;flex-wrap:wrap}\
input[type=text]{font:inherit;padding:10px 14px;border:1px solid var(--line);border-radius:11px;\
min-width:min(24rem,100%);background:var(--panel);color:var(--ink);box-shadow:var(--sh);\
transition:border-color .2s,box-shadow .2s}\
input[type=text]:focus{outline:0;border-color:var(--acc);box-shadow:0 0 0 4px color-mix(in srgb,var(--acc) 18%,transparent)}\
button{font:inherit;font-weight:700;padding:10px 20px;border:0;border-radius:11px;cursor:pointer;\
background:linear-gradient(135deg,var(--acc),var(--acc2));color:#fff;\
box-shadow:0 5px 16px color-mix(in srgb,var(--acc) 34%,transparent);transition:transform .2s,box-shadow .2s}\
button:hover{transform:translateY(-2px);box-shadow:0 9px 24px color-mix(in srgb,var(--acc) 42%,transparent)}\
form a{color:var(--dim);font-size:13px;text-decoration:none}\
form a:hover{color:var(--acc)}\
ul.notes{margin:0 auto 16px;max-width:1180px;padding:14px 18px 14px 34px;list-style:none;\
background:var(--panel);border:1px solid var(--line);border-radius:13px;box-shadow:var(--sh);\
color:var(--dim);font-size:13.5px}\
ul.notes li{margin:3px 0;position:relative}\
ul.notes li:before{content:'';position:absolute;left:-16px;top:8px;width:6px;height:6px;\
border-radius:50%;background:var(--dim)}\
ul.notes li.loud{color:var(--bad);font-weight:750}\
ul.notes li.loud:before{background:var(--bad);animation:blip 1.6s infinite}\
@keyframes blip{50%{opacity:.25}}\
table{border-collapse:collapse;width:100%;font-size:13.5px;\
margin:0 auto;background:var(--panel)}\
thead th{position:sticky;top:0;z-index:2;text-align:left;padding:12px 15px;background:var(--panel);\
font-size:10.5px;letter-spacing:.85px;text-transform:uppercase;color:var(--dim);font-weight:780;\
border-bottom:1px solid var(--line);white-space:nowrap}\
td{padding:11px 15px;border-bottom:1px solid var(--line);white-space:nowrap}\
td.num{text-align:right;font-variant-numeric:tabular-nums}\
tbody tr{animation:rowin .4s both;transition:background .15s}\
@keyframes rowin{from{opacity:0;transform:translateY(5px)}}\
tbody tr:hover td{background:color-mix(in srgb,var(--acc) 7%,transparent)}\
.tag{display:inline-block;font-size:10px;font-weight:820;letter-spacing:.5px;padding:3px 8px;\
border-radius:6px;margin-right:5px;background:color-mix(in srgb,var(--acc) 14%,transparent);color:var(--acc)}\
.swept{background:color-mix(in srgb,var(--ok) 16%,transparent);color:var(--ok)}\
td.clash{background:color-mix(in srgb,var(--bad) 12%,transparent);color:var(--bad);font-weight:750}\
thead th a{color:inherit;text-decoration:none;display:block}\
thead th a:hover{color:var(--acc)}\
.filters{margin:0 auto;max-width:1180px;padding:0 20px}\
.filters input{position:absolute;opacity:0;pointer-events:none}\
.pills{display:flex;flex-wrap:wrap;gap:8px;margin:0 0 14px}\
.pills a{cursor:pointer;user-select:none;text-decoration:none;display:inline-block;padding:9px 16px;border-radius:99px;font-weight:650;\
font-size:13.5px;border:1px solid var(--line);background:var(--panel);color:var(--dim);\
transition:all .22s cubic-bezier(.2,.8,.2,1)}\
.pills a:hover{transform:translateY(-2px);border-color:var(--acc);color:var(--ink)}\
.pills a b{font-variant-numeric:tabular-nums;opacity:.65;margin-left:7px;font-weight:700}\
.pills a.on{background:linear-gradient(135deg,var(--acc),var(--acc2));border-color:transparent;color:#fff;\
box-shadow:0 6px 18px color-mix(in srgb,var(--acc) 34%,transparent);transform:translateY(-1px)}\
footer{margin:22px auto 0;max-width:1180px;padding:0 20px;color:var(--dim);font-size:12.5px;line-height:1.9}\
@media(prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important}}";

/// Words that make a note a failure rather than a tally.
///
/// A conflict line that renders in the same grey as a row count is a line
/// nobody reads. These are the notes an operator has to see even when the page
/// is full of ordinary numbers.
const LOUD: [&str; 5] = [
    "UNAVAILABLE",
    "CONFLICT",
    "UNREADABLE",
    "UNRECOGNISED",
    "UNCHECKED",
];

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
    /// Which vendors listed it.
    ///
    /// A set rather than one `bool` per vendor: a `bool` per vendor forces
    /// this crate to `match` on [`Vendor`], which is `#[non_exhaustive]`, and
    /// that match needs a wildcard arm no test can ever reach.
    pub vendors: VendorSet,
    /// The ISIN the vendors agreed on, if they carry one.
    pub isin: Option<Isin>,
    /// A second, different ISIN for the same identity. Rendered loudly:
    /// a disagreement the operator cannot see is a disagreement that decides
    /// the run on its own.
    pub conflict: Option<Isin>,
    /// Which of the engine's lists this instrument is in.
    pub universe: Universe,
}

/// Renders the universe cell.
///
/// On the page rather than only in a count, because the reason 1,117 SME
/// shares are declined is that neither list contains one — and a claim that
/// load-bearing should be visible against every row it governs.
fn universe_cell(u: Universe) -> String {
    let mut tags = Vec::new();
    for (bit, label) in [
        (Universe::INDEX, "index"),
        (Universe::FNO, "F&amp;O"),
        (Universe::TOTAL_MARKET, "total mkt"),
    ] {
        if u.contains(bit) {
            tags.push(format!("<span class=\"tag\">{label}</span>"));
        }
    }
    if tags.is_empty() {
        return "<td>—</td>".to_owned();
    }
    format!("<td>{}</td>", tags.join(" "))
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
    for v in Vendor::ALL {
        if row.vendors.contains(v) {
            tags.push(format!("<span class=\"tag\">{}</span>", escape(v.as_str())));
        }
    }
    format!("<td>{}</td>", tags.join(" "))
}

/// Renders the ISIN cell — the cross-check, and any disagreement about it.
///
/// A conflicting pair is rendered in the row rather than only counted in a
/// summary, because `docs/05-decisions.md` D-0020 requires a vendor
/// disagreement to NAME what disagreed. A count says a problem exists; this
/// says which instrument has it.
fn isin_cell(row: &Row) -> String {
    match (row.isin, row.conflict) {
        (None, _) => "<td>—</td>".to_owned(),
        (Some(i), None) => format!("<td>{}</td>", escape(i.as_str())),
        (Some(i), Some(other)) => format!(
            "<td class=\"clash\">{} ≠ {}</td>",
            escape(i.as_str()),
            escape(other.as_str())
        ),
    }
}

/// The universe filter — four radio inputs, four labels, one CSS rule.
///
/// Extracted from [`instruments_page`] because that function is at clippy's
/// 100-line ceiling; the split is a lint, not a design.
fn filter_pills(counts: UniverseCounts, query: &str, active: &str, all: bool) -> String {
    let mut out = String::with_capacity(512);
    // THE PILLS ARE LINKS, NOT A CSS TOGGLE.
    //
    // They were a `:checked ~` rule, which filtered without a round trip. But a
    // CSS rule can only hide rows the browser already HAS, and a page carries
    // 200 of 785 — so "F&O" showed 52 when the answer was 208, instantly and
    // authoritatively wrong. The count and the rows behind it disagreed.
    //
    // As links, the server selects from the whole set and the number on the
    // pill is the population behind it. The round trip costs 0.6 ms on
    // loopback, below anything a person perceives, so correctness is free here.
    //
    // `all` is carried through, so widening to every NSE listing and then
    // filtering by universe compose instead of cancelling.
    let q = escape(query);
    let suffix = if all { "&amp;all=1" } else { "" };
    let pill = |slug: &str, label: &str, n: usize| -> String {
        let on = if active == slug { " class=\"on\"" } else { "" };
        format!("<a href=\"/instruments?q={q}&amp;u={slug}{suffix}\"{on}>{label}<b>{n}</b></a>")
    };
    let _ = write!(
        out,
        "<section class=\"filters\"><div class=\"pills\">{}{}{}{}</div>",
        pill("", "All", counts.all),
        pill("fno", "F&amp;O", counts.fno),
        pill("ntm", "NIFTY Total Market", counts.ntm),
        pill("idx", "Indices", counts.index),
    );
    out
}

/// How many instruments each universe holds, over the WHOLE tracked set.
///
/// Counted by the caller from the merged map, never from the rendered page: a
/// pill whose number counts only the 200 rows on screen says 52 when the answer
/// is 208, and looks authoritative doing it.
#[derive(Debug, Clone, Copy, Default)]
pub struct UniverseCounts {
    /// Every tracked instrument.
    pub all: usize,
    /// F&O underlyings.
    pub fno: usize,
    /// NIFTY Total Market constituents.
    pub ntm: usize,
    /// Index series.
    pub index: usize,
}

/// The sortable table: header links, then one row per instrument.
///
/// Split out of [`instruments_page`] to stay under clippy's 100-line ceiling.
/// The split is a lint, not a design.
fn table(rows: &[Row], query: &str, sort: &str) -> String {
    let mut body = String::with_capacity(256 + rows.len() * 256);
    // SORTABLE HEADERS. Links, not script: the order is part of the URL, so a
    // sorted view is linkable, reloadable and back-buttonable, and the server
    // decides it once from data already in memory. The query is carried through
    // so sorting does not silently clear a search.
    let q = escape(query);
    let sort_link = |col: &str, label: &str| -> String {
        let mark = if sort == col { " ▾" } else { "" };
        format!("<th><a href=\"/instruments?q={q}&amp;sort={col}\">{label}{mark}</a></th>")
    };
    body.push_str("<table><thead><tr>");
    for (col, label) in [
        ("key", "Canonical key"),
        ("vendors", "Vendors"),
        ("symbol", "Underlying"),
        ("isin", "ISIN"),
        ("universe", "Universe"),
        ("kind", "Kind"),
    ] {
        body.push_str(&sort_link(col, label));
    }
    body.push_str("<th>Expiry</th><th>Strike</th></tr></thead><tbody>");

    for row in rows {
        // Classes carry BOTH facts: whether the engine sweeps it (colour) and
        // which lists it is in (what the filter selects on). One attribute,
        // because a second would need a wrapper element per row.
        let mut classes = String::new();
        if row.key.is_sweepable() {
            classes.push_str("swept ");
        }
        for (bit, name) in [
            (Universe::FNO, "u-fno"),
            (Universe::TOTAL_MARKET, "u-ntm"),
            (Universe::INDEX, "u-idx"),
        ] {
            if row.universe.contains(bit) {
                classes.push_str(name);
                classes.push(' ');
            }
        }
        let _ = write!(
            body,
            "<tr class=\"{}\"><td>{}</td>{}<td>{}</td>{}{}{}</tr>",
            classes.trim_end(),
            escape(&row.key.to_string()),
            vendor_cell(row),
            escape(row.key.underlying.as_str()),
            isin_cell(row),
            universe_cell(row.universe),
            kind_cells(row.key.kind),
        );
    }

    body
}

/// Everything one rendering of the instruments page needs.
///
/// A struct rather than nine parameters: clippy caps a function at seven,
/// and more to the point a positional list of four `&str` invites a caller to
/// swap `sort` and `active` with no type error and no test failure -- the page
/// would simply sort by a filter name and filter by a column name.
#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    /// The page title.
    pub title: &'a str,
    /// The denominator the title's count means.
    pub total: usize,
    /// The rows to render, already filtered and sorted.
    pub rows: &'a [Row],
    /// What was typed in the search box.
    pub query: &'a str,
    /// Which column the rows are ordered by.
    pub sort: &'a str,
    /// Whether the universe filter is lifted.
    pub all: bool,
    /// Universe sizes, over the whole tracked set.
    pub counts: UniverseCounts,
    /// Which universe pill is selected.
    pub active: &'a str,
    /// Every line an operator has to be told.
    pub notes: &'a [String],
}

/// Renders a complete instruments page.
///
/// `total` is the size of the whole universe, `rows` is only what this page
/// shows. Passing both keeps the page honest about the difference between what
/// exists and what was rendered.
///
/// `notes` is rendered on **every** page, filtered or not. It carries
/// `UNAVAILABLE` and every conflict line, and it used to be folded into the
/// title only when no query had been typed — so searching, which is the only
/// way to reach most of the universe, silently dropped the one thing the page
/// existed to say.
#[must_use]
pub fn instruments_page(view: &View<'_>) -> String {
    let View {
        title,
        total,
        rows,
        query,
        sort,
        all,
        counts,
        active,
        notes,
    } = *view;
    let mut body = String::with_capacity(1024 + rows.len() * 256);
    body.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    body.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    body.push_str("<title>");
    body.push_str(&escape(title));
    body.push_str("</title><style>");
    body.push_str(STYLE);
    body.push_str("</style></head><body>");

    // A search form. Method GET so the query lives in the URL and a result is
    // linkable and reloadable -- no JavaScript, no client state.
    let _ = write!(
        body,
        "<form method=\"get\" action=\"/instruments\">\
         <input type=\"text\" name=\"q\" placeholder=\"NIFTY, BANKNIFTY, RELIANCE…\" \
         value=\"{}\" autofocus>\
         <button type=\"submit\">Search</button>\
         <a href=\"/instruments\">clear</a>\
         <a href=\"/instruments?all={}\">{}</a></form>",
        escape(query),
        u8::from(!all),
        if all {
            "show tracked only (NIFTY Total Market + indices)"
        } else {
            "show every NSE listing"
        }
    );

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

    // THE NOTES, ON EVERY PAGE. Not folded into the title, and not conditional
    // on the query being empty.
    body.push_str("<ul class=\"notes\">");
    for note in notes {
        let loud = if LOUD.iter().any(|w| note.contains(w)) {
            " class=\"loud\""
        } else {
            ""
        };
        let _ = write!(body, "<li{loud}>{}</li>", escape(note));
    }
    body.push_str("</ul>");

    body.push_str(&filter_pills(counts, query, active, all));

    body.push_str(&table(rows, query, sort));
    body.push_str("</tbody></table></section>");
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
    use brutex_core::instrument::{Exchange, Expiry, OptionSide, Segment};
    use brutex_core::price::Paisa;
    use brutex_core::symbol::Symbol;

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

    /// A row listed by exactly the given vendors, with no ISIN.
    fn row(key: InstrumentKey, vendors: &[Vendor]) -> Row {
        let mut set = VendorSet::EMPTY;
        for &v in vendors {
            set = set.with(v);
        }
        Row {
            key,
            vendors: set,
            isin: None,
            conflict: None,
            universe: brutex_core::universe::of_instrument(&key),
        }
    }

    /// A page with no notes, for the tests that are about rows.
    fn page(title: &str, total: usize, rows: &[Row], query: &str) -> String {
        instruments_page(&View {
            title,
            total,
            rows,
            query,
            sort: "",
            all: false,
            counts: UniverseCounts::default(),
            active: "",
            notes: &[],
        })
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
        let html = page("x", 1, &[row(key, &[Vendor::Groww])], "");
        assert!(html.contains("M&amp;M"), "the ampersand must be escaped");
        assert!(
            !html.contains("<td>M&M<"),
            "a raw ampersand must never reach the output"
        );
    }

    #[test]
    fn a_swept_instrument_is_marked_and_others_are_not() {
        let html = page(
            "NSE",
            2,
            &[
                row(nifty(), &[Vendor::Groww, Vendor::Dhan]),
                row(opt(), &[Vendor::Groww]),
            ],
            "",
        );
        // Counted on the CLASS, not on the whole attribute: a row's class list
        // now also carries which universes it is in, so `class="swept"` as a
        // literal would match only a swept row that is in no universe at all —
        // a test that passes by accident and stops testing the thing it names.
        // `class="swept` — the opening of a ROW's class list, with no closing
        // quote, because the list now also carries the universes the row is in.
        // Matching the whole attribute would find only a swept row in no
        // universe; matching bare `swept` would also find the stylesheet rule.
        assert_eq!(
            html.matches("class=\"swept").count(),
            1,
            "exactly one of these two is swept"
        );
    }

    #[test]
    fn both_vendors_on_one_row_is_the_visible_dedup() {
        let html = page("x", 1, &[row(nifty(), &[Vendor::Groww, Vendor::Dhan])], "");
        assert!(html.contains(">groww<"));
        assert!(html.contains(">dhan<"));
        assert_eq!(html.matches("<tr").count(), 2, "header plus ONE data row");
    }

    #[test]
    fn a_single_vendor_row_shows_only_that_vendor() {
        let only_dhan = page("x", 1, &[row(nifty(), &[Vendor::Dhan])], "");
        assert!(only_dhan.contains(">dhan<"));
        assert!(!only_dhan.contains(">groww<"));
        let neither = page("x", 1, &[row(nifty(), &[])], "");
        assert!(!neither.contains(">dhan<"));
        assert!(!neither.contains(">groww<"));
    }

    #[test]
    fn the_isin_cell_shows_the_cross_check_and_shouts_about_a_clash() {
        let share = Isin::new("INE121A01024").expect("valid");
        let bond = Isin::new("INE121A08PJ0").expect("valid");

        // No ISIN at all -- an index -- is a dash, never a fabricated value.
        assert_eq!(isin_cell(&row(nifty(), &[Vendor::Groww])), "<td>—</td>");

        let agreed = Row {
            isin: Some(share),
            ..row(nifty(), &[Vendor::Groww])
        };
        assert_eq!(isin_cell(&agreed), "<td>INE121A01024</td>");

        // A disagreement names BOTH values in the row itself. D-0020: a
        // refusal names what disagreed; a count would not.
        let clash = Row {
            isin: Some(share),
            conflict: Some(bond),
            ..row(nifty(), &[Vendor::Groww, Vendor::Dhan])
        };
        let cell = isin_cell(&clash);
        assert!(cell.contains("INE121A01024") && cell.contains("INE121A08PJ0"));
        assert!(cell.contains("clash"), "and it is visibly marked: {cell}");

        let html = page("x", 1, &[clash], "");
        assert!(html.contains("INE121A08PJ0"), "it reaches the page");
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
        let html = page(
            "NSE FNO",
            90_623,
            &[row(nifty(), &[Vendor::Groww, Vendor::Dhan])],
            "",
        );
        assert!(html.contains("90623 instruments total"));
        assert!(html.contains("showing 1"));
    }

    #[test]
    fn an_empty_page_is_still_well_formed() {
        let html = page("nothing here", 0, &[], "");
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
        let html = page("x", 1, &[row(opt(), &[Vendor::Groww, Vendor::Dhan])], "");
        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }

    #[test]
    fn the_title_is_escaped_too() {
        let html = page("<b>x</b>", 0, &[], "");
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(!html.contains("<b>x</b>"));
    }

    #[test]
    fn the_notes_render_on_a_filtered_page_and_a_failure_is_marked_loud() {
        // The banner is not conditional on the query. It carries UNAVAILABLE
        // and every conflict line, and folding it into the title of the
        // UNFILTERED page only is what made searching hide it.
        let notes = vec![
            "groww: 2 kept, 3 declined, 0 unreadable".to_owned(),
            "dhan: UNAVAILABLE — no such file".to_owned(),
            "ISIN CONFLICT · NSE-CHOLAFIN: groww says A, dhan says B".to_owned(),
        ];
        let html = instruments_page(&View {
            title: "search",
            total: 1,
            rows: &[row(nifty(), &[Vendor::Groww])],
            query: "NIFTY",
            sort: "",
            all: false,
            counts: UniverseCounts::default(),
            active: "",
            notes: &notes,
        });
        for note in &notes {
            assert!(html.contains(&escape(note)), "{note} must be on the page");
        }
        assert_eq!(
            html.matches("class=\"loud\"").count(),
            2,
            "the tally is quiet; UNAVAILABLE and the conflict are not"
        );
        // And a note is escaped like everything else.
        let html = instruments_page(&View {
            title: "t",
            total: 0,
            rows: &[],
            query: "",
            sort: "",
            all: false,
            counts: UniverseCounts::default(),
            active: "",
            notes: &["<b>x</b>".to_owned()],
        });
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(!html.contains("<li><b>"));
    }

    #[test]
    fn the_universe_cell_names_every_list_an_instrument_is_in() {
        // NIFTY is an index AND the underlying of its own options; RELIANCE is
        // an F&O underlying AND a Total Market constituent; an option is in
        // nothing at all.
        let n = universe_cell(brutex_core::universe::of_instrument(&nifty()));
        assert!(n.contains("index") && n.contains("F&amp;O"));
        assert!(!n.contains("total mkt"), "an index is not a share");

        let reliance = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Cash,
            underlying: Symbol::new("RELIANCE").expect("valid"),
            kind: Kind::Equity,
        };
        let r = universe_cell(brutex_core::universe::of_instrument(&reliance));
        assert!(r.contains("F&amp;O") && r.contains("total mkt"));

        assert_eq!(universe_cell(Universe::NONE), "<td>—</td>");
        // Never a raw ampersand, even in a hardcoded label.
        assert!(!r.contains("F&O<"));
    }
}
