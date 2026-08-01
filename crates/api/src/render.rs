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
details.notes{margin:0 auto 16px;max-width:1180px;background:var(--panel);\
border:1px solid var(--line);border-radius:13px;box-shadow:var(--sh);\
color:var(--dim);font-size:13px;overflow:hidden}\
details.notes summary{cursor:pointer;padding:11px 18px;font-weight:650;list-style:none;\
user-select:none;transition:background .18s}\
details.notes summary::-webkit-details-marker{display:none}\
details.notes summary:before{content:'▸ ';font-weight:800;color:var(--acc)}\
details.notes[open] summary:before{content:'▾ '}\
details.notes summary:hover{background:color-mix(in srgb,var(--acc) 6%,transparent)}\
details.notes summary b{color:var(--bad);font-variant-numeric:tabular-nums}\
details.notes ul{list-style:none;padding:2px 18px 14px 34px;margin:0;\
border-top:1px solid var(--line)}\
details.notes li{margin:5px 0;position:relative;line-height:1.5}\
details.notes li:before{content:'';position:absolute;left:-16px;top:7px;width:6px;height:6px;\
border-radius:50%;background:var(--dim)}\
details.notes li.loud{color:var(--bad);font-weight:700}\
details.notes li.loud:before{background:var(--bad)}\
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
.pager{display:flex;gap:12px;align-items:center;justify-content:center;margin:16px auto 0;\
max-width:1180px;padding:0 20px;font-size:13.5px}\
.pager a{color:var(--acc);text-decoration:none;font-weight:700;padding:8px 16px;border-radius:10px;\
border:1px solid var(--line);background:var(--panel);transition:all .2s}\
.pager a:hover{background:linear-gradient(135deg,var(--acc),var(--acc2));color:#fff;border-color:transparent}\
.pager span{color:var(--dim);font-variant-numeric:tabular-nums}\
.hero{position:relative;overflow:hidden;padding:56px 0 46px;color:#fff;margin-bottom:26px;\
background:linear-gradient(135deg,#1e1b4b 0%,#4338ca 44%,#0891b2 100%)}\
.hero:before{content:'';position:absolute;inset:-45%;background:\
radial-gradient(circle at 20% 30%,rgba(255,255,255,.18),transparent 40%),\
radial-gradient(circle at 78% 68%,rgba(255,255,255,.13),transparent 38%);\
animation:drift 22s ease-in-out infinite alternate}\
@keyframes drift{to{transform:translate3d(5%,-5%,0) rotate(8deg)}}\
.hero .hw{position:relative;max-width:1180px;margin:0 auto;padding:0 20px}\
.eyebrow{display:flex;align-items:center;gap:10px;font-weight:800;font-size:12px;\
letter-spacing:1.3px;opacity:.9}\
.dot{width:9px;height:9px;border-radius:50%;background:#5eead4;animation:pulse 1.9s infinite}\
@keyframes pulse{0%{box-shadow:0 0 0 0 rgba(94,234,212,.7)}70%{box-shadow:0 0 0 14px rgba(94,234,212,0)}100%{box-shadow:0 0 0 0 rgba(94,234,212,0)}}\
.hero h1{font-size:clamp(30px,5.4vw,48px);letter-spacing:-1.8px;font-weight:850;\
margin:14px 0 12px;line-height:1.05;color:#fff;padding:0;max-width:none;\
animation:rise .7s cubic-bezier(.2,.8,.2,1) both}\
.lede{max-width:64ch;opacity:.93;font-size:16.5px;line-height:1.6;\
animation:rise .7s .08s cubic-bezier(.2,.8,.2,1) both}\
.badge{display:inline-block;margin-top:16px;padding:7px 16px;border-radius:99px;\
font-size:12px;font-weight:850;letter-spacing:1px;text-transform:uppercase;\
background:rgba(255,255,255,.16);backdrop-filter:blur(8px);\
animation:rise .7s .16s cubic-bezier(.2,.8,.2,1) both}\
.badge.good{background:rgba(52,211,153,.26);color:#a7f3d0}\
.badge.bad{background:rgba(251,113,133,.26);color:#fecdd3}\
.cbar{height:5px;border-radius:99px;background:var(--line);overflow:hidden;margin:8px 0 7px}\
.cbar>span{display:block;height:100%;border-radius:99px;\
background:linear-gradient(90deg,var(--acc),var(--acc2));\
animation:fill 1.1s .3s cubic-bezier(.2,.8,.2,1) both}\
.card.loud .cbar>span{background:linear-gradient(90deg,var(--bad),#fb7185)}\
@keyframes fill{from{width:0!important}}\
nav.top{position:sticky;top:0;z-index:10;background:color-mix(in srgb,var(--panel) 88%,transparent);\
backdrop-filter:blur(12px);border-bottom:1px solid var(--line)}\
nav.top .inner{max-width:1180px;margin:0 auto;padding:0 20px;display:flex;align-items:center;gap:22px;height:56px}\
nav.top .logo{font-weight:850;font-size:16px;letter-spacing:-.6px;text-decoration:none;color:var(--ink)}\
nav.top .logo b{color:var(--acc)}\
nav.top .links{display:flex;gap:4px;flex-wrap:wrap}\
nav.top .lnk{padding:7px 13px;border-radius:9px;font-size:13.5px;font-weight:650;\
text-decoration:none;color:var(--dim);transition:all .18s}\
nav.top a.lnk:hover{color:var(--ink);background:color-mix(in srgb,var(--acc) 9%,transparent)}\
nav.top .lnk.on{color:#fff;background:linear-gradient(135deg,var(--acc),var(--acc2))}\
nav.top .lnk.off{opacity:.38;cursor:not-allowed}\
nav.top .lnk.off:after{content:' ·';font-size:10px}\
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:13px;\
max-width:1180px;margin:0 auto 18px;padding:0 20px}\
.card{background:var(--panel);border:1px solid var(--line);border-radius:15px;padding:16px;\
box-shadow:var(--sh);animation:rise .5s both;transition:transform .22s,box-shadow .22s}\
.card:hover{transform:translateY(-4px);box-shadow:0 14px 34px rgba(16,24,40,.15)}\
.card.loud{border-color:color-mix(in srgb,var(--bad) 45%,transparent)}\
.ck{font-size:10.5px;font-weight:760;letter-spacing:.9px;text-transform:uppercase;color:var(--dim)}\
.cv{font-size:29px;font-weight:850;letter-spacing:-1.5px;margin:5px 0 4px;font-variant-numeric:tabular-nums}\
.card.loud .cv{color:var(--bad)}\
.cn{font-size:11.5px;color:var(--dim)}\
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

impl View<'_> {
    /// The query string every link on the page shares, with optional overrides.
    ///
    /// # Why one builder
    ///
    /// Each link used to compose its own. The sort headers carried only the
    /// search, so clicking a column dropped the universe filter; the pills
    /// carried only the filter, so clicking one dropped the sort. The table
    /// reordered *and* changed which rows it was ordering, which reads as the
    /// data having moved under you.
    ///
    /// One builder makes forgetting a parameter impossible: a new parameter is
    /// added here and every link gains it at once.
    ///
    /// `None` keeps the current value; `Some` overrides it. Sorting and
    /// filtering pass `Some(0)` for the page deliberately — page 3 of an ISIN
    /// ordering is not page 3 of a symbol ordering, so keeping the number would
    /// land the operator somewhere arbitrary.
    #[must_use]
    pub fn link_params(
        &self,
        sort: Option<&str>,
        active: Option<&str>,
        page: Option<usize>,
    ) -> String {
        let mut out = String::with_capacity(64);
        let _ = write!(
            out,
            "q={}&amp;sort={}&amp;u={}",
            escape(self.query),
            escape(sort.unwrap_or(self.sort)),
            escape(active.unwrap_or(self.active)),
        );
        if self.all {
            out.push_str("&amp;all=1");
        }
        let page = page.unwrap_or(self.page);
        if page > 0 {
            let _ = write!(out, "&amp;page={page}");
        }
        out
    }
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
fn filter_pills(view: &View<'_>) -> String {
    let counts = view.counts;
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
    let pill = |slug: &str, label: &str, n: usize| -> String {
        let on = if view.active == slug {
            " class=\"on\""
        } else {
            ""
        };
        format!(
            "<a href=\"/instruments?{}\"{on}>{label}<b>{n}</b></a>",
            view.link_params(None, Some(slug), Some(0))
        )
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
fn table(rows: &[Row], view: &View<'_>) -> String {
    let mut body = String::with_capacity(256 + rows.len() * 256);
    // SORTABLE HEADERS. Links, not script: the order is part of the URL, so a
    // sorted view is linkable, reloadable and back-buttonable, and the server
    // decides it once from data already in memory.
    //
    // The link carries EVERY other parameter. It used to carry only the search,
    // so clicking a column header silently dropped the universe filter and the
    // page — the table reordered AND changed which rows it was ordering, which
    // reads as the data having moved.
    //
    // Paging resets to page 1 deliberately: page 3 of an ISIN ordering is not
    // page 3 of a symbol ordering, and keeping the number would land the
    // operator somewhere arbitrary.
    // ASCENDING AND DESCENDING. Clicking the column you are already sorted by
    // REVERSES it; clicking a different column starts that one ascending.
    //
    // The direction is a `-` prefix on the column name, so it needs no second
    // parameter and an old one-way link still means exactly what it did.
    let sort = view.sort;
    let sort_link = |col: &str, label: &str| -> String {
        let (active, descending) = match sort.strip_prefix('-') {
            Some(base) => (base == col, true),
            None => (sort == col, false),
        };
        // Already on this column ascending -> offer descending, and vice versa.
        let next = if active && !descending {
            format!("-{col}")
        } else {
            col.to_owned()
        };
        let mark = match (active, descending) {
            (true, false) => " ▲",
            (true, true) => " ▼",
            _ => "",
        };
        format!(
            "<th><a href=\"/instruments?{}\">{label}{mark}</a></th>",
            view.link_params(Some(&next), None, Some(0))
        )
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
    /// Which page of rows this is, zero-based.
    pub page: usize,
    /// The highest page number that has rows.
    pub last_page: usize,
    /// Every line an operator has to be told.
    pub notes: &'a [String],
}

/// Shortens a note that has become a list.
///
/// One note names 31 instruments inline. Rendered whole it fills the screen
/// above the table in red and reads as wallpaper rather than as a warning —
/// the opposite of what a loud line is for. The head of the message carries the
/// fact and the count; the tail is a dump.
///
/// Truncates on a boundary the message itself provides rather than at a
/// character count, so it never cuts a name in half.
fn clamp(note: &str) -> String {
    const MAX: usize = 160;
    if note.len() <= MAX {
        return note.to_owned();
    }
    let head = note
        .char_indices()
        .take_while(|(i, _)| *i < MAX)
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    let cut = note[..head].rfind(", ").unwrap_or(head);
    let rest = note[cut..].matches(", ").count();
    format!("{}… and {rest} more", &note[..cut])
}

/// The navigation bar, on every page.
///
/// # Why unbuilt pages are shown rather than hidden
///
/// A nav that lists only what exists tells you nothing about what is coming;
/// a nav that links to a page which does not answer is worse. These are
/// rendered as disabled with the reason, so the shape of the system is visible
/// and nothing lies about being ready.
fn nav(current: &str) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("<nav class=\"top\"><div class=\"inner\">");
    out.push_str("<a class=\"logo\" href=\"/\">brutex</a><div class=\"links\">");
    for (href, label, built) in [
        ("/", "Dashboard", true),
        ("/instruments", "Instruments", true),
        ("/pull", "Ingest", false),
        ("/store", "Store", false),
        ("/runs", "Runs", false),
    ] {
        let on = if href == current { " on" } else { "" };
        if built {
            let _ = write!(out, "<a class=\"lnk{on}\" href=\"{href}\">{label}</a>");
        } else {
            let _ = write!(
                out,
                "<span class=\"lnk off\" title=\"not built yet\">{label}</span>"
            );
        }
    }
    out.push_str("</div></nav>");
    out
}

/// One figure on the dashboard.
#[derive(Debug, Clone, Copy)]
pub struct Stat<'a> {
    /// What it counts.
    pub label: &'a str,
    /// The count.
    pub value: &'a str,
    /// One line of context under it.
    pub note: &'a str,
    /// Whether this figure is a problem.
    pub loud: bool,
}

/// The dashboard: what the engine knows, and what it does not.
///
/// Every figure is a counter already held in memory — nothing here scans, so
/// the page costs the same whether the store holds two instruments or two
/// hundred thousand.
#[must_use]
pub fn dashboard_page(status: &str, figures: &[Stat<'_>], notes: &[String]) -> String {
    let mut body = String::with_capacity(2048);
    body.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    body.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    body.push_str("<title>brutex · dashboard</title><style>");
    body.push_str(STYLE);
    body.push_str("</style></head><body>");
    body.push_str(&nav("/"));
    let _ = write!(
        body,
        "<header class=\"hero\"><div class=\"hw\">\
         <div class=\"eyebrow\"><span class=\"dot\"></span>NSE · TWO FEEDS · ONE IDENTITY</div>\
         <h1>Every instrument,<br>cross&#8209;checked.</h1>\
         <p class=\"lede\">A brute&#8209;force backtesting engine for Indian indices. \
         Every figure below is a counter, never a scan — this page costs the same \
         whether the store holds two instruments or two hundred thousand.</p>\
         <span class=\"badge {}\">{}</span></div></header>",
        if status == "ok" { "good" } else { "bad" },
        escape(status),
    );

    body.push_str("<section class=\"cards\">");
    // INTEGER arithmetic, not float. `clippy::float_arithmetic` is denied
    // workspace-wide so that a price can never touch a float (CLAUDE.md §7),
    // and a bar width has no business being the first exception — it is a
    // percentage of a count, which integers express exactly.
    let peak = figures
        .iter()
        .filter_map(|f| f.value.parse::<u64>().ok())
        .max()
        .unwrap_or(1)
        .max(1);
    for (i, f) in figures.iter().enumerate() {
        let cls = if f.loud { " loud" } else { "" };
        // The bar shows each figure against the largest on the page, so the
        // relationship between the numbers is visible without reading them.
        // A floor of 3% so a zero still draws something: a bar of no width
        // reads as "not rendered" rather than as "none", and zero
        // disagreements is the figure most worth seeing.
        let pct = f
            .value
            .parse::<u64>()
            .map_or(100, |v| (v.saturating_mul(100) / peak).max(3));
        let _ = write!(
            body,
            "<div class=\"card{cls}\" style=\"animation-delay:{}ms\">\
             <div class=\"ck\">{}</div><div class=\"cv\">{}</div>\
             <div class=\"cbar\"><span style=\"width:{pct}%\"></span></div>\
             <div class=\"cn\">{}</div></div>",
            i * 60,
            escape(f.label),
            escape(f.value),
            escape(f.note),
        );
    }
    body.push_str("</section>");

    let loud_count = notes
        .iter()
        .filter(|n| LOUD.iter().any(|w| n.contains(w)))
        .count();
    let _ = write!(
        body,
        "<details class=\"notes\"><summary>{} note{} · <b>{loud_count}</b> needing attention</summary><ul>",
        notes.len(),
        if notes.len() == 1 { "" } else { "s" },
    );
    for note in notes {
        let loud = if LOUD.iter().any(|w| note.contains(w)) {
            " class=\"loud\""
        } else {
            ""
        };
        let _ = write!(body, "<li{loud}>{}</li>", escape(&clamp(note)));
    }
    body.push_str("</ul></details>");

    body.push_str(
        "<footer>Rendered on the server. No JavaScript — CLAUDE.md section 2 \
         does not permit it, and CI gate 1 enforces that.</footer></body></html>",
    );
    body
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
    // `sort`, `counts` and `active` are not bound here: they are read through
    // `view` by `table`, `filter_pills` and `View::link_params`, which is the
    // point of routing every link through one builder.
    let View {
        title,
        total,
        rows,
        query,
        all,
        page,
        last_page,
        notes,
        ..
    } = *view;
    let mut body = String::with_capacity(1024 + rows.len() * 256);
    body.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    body.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    body.push_str("<title>");
    body.push_str(&escape(title));
    body.push_str("</title><style>");
    body.push_str(STYLE);
    body.push_str("</style></head><body>");
    body.push_str(&nav("/instruments"));

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
    // THE NOTES, COLLAPSED BY DEFAULT.
    //
    // They were an always-open panel. With 31 index names in one red line it
    // filled the screen above the table and read as wallpaper rather than as a
    // warning — the opposite of the point. A `<details>` element collapses it
    // with no script at all, and the summary line still states the counts and
    // how many lines are loud, so nothing is hidden, only folded.
    //
    // It opens automatically when something IS loud, because a warning nobody
    // is told about is a warning nobody reads.
    let loud_count = notes
        .iter()
        .filter(|n| LOUD.iter().any(|w| n.contains(w)))
        .count();
    let _ = write!(
        body,
        "<details class=\"notes\"><summary>{} note{} · <b>{loud_count}</b> needing attention</summary><ul>",
        notes.len(),
        if notes.len() == 1 { "" } else { "s" },
    );
    for note in notes {
        let loud = if LOUD.iter().any(|w| note.contains(w)) {
            " class=\"loud\""
        } else {
            ""
        };
        let _ = write!(body, "<li{loud}>{}</li>", escape(&clamp(note)));
    }
    body.push_str("</ul></details>");

    body.push_str(&filter_pills(view));

    body.push_str(&table(rows, view));

    // PAGING LINKS. Without them the row cap is a wall: the page rendered 200
    // of 785 and scrolling could not reveal what was never sent. Links rather
    // than script, so a page is bookmarkable and the browser's back button
    // works. Every other parameter rides along, so paging does not silently
    // clear a search, a sort or a filter.
    if last_page > 0 {
        let carry = view.link_params(None, None, Some(0));
        let carry = carry.trim_end_matches("&amp;page=0").to_owned();
        body.push_str("<nav class=\"pager\">");
        if page > 0 {
            let _ = write!(
                body,
                "<a href=\"/instruments?{carry}&amp;page={}\">&larr; previous</a>",
                page - 1
            );
        }
        let _ = write!(body, "<span>page {} of {}</span>", page + 1, last_page + 1);
        if page < last_page {
            let _ = write!(
                body,
                "<a href=\"/instruments?{carry}&amp;page={}\">next &rarr;</a>",
                page + 1
            );
        }
        body.push_str("</nav>");
    }
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
            page: 0,
            last_page: 0,
            notes: &[],
        })
    }

    #[test]
    fn one_link_builder_carries_every_parameter_and_overrides_only_what_it_is_told() {
        // This is the function that fixed the bug where sorting dropped the
        // universe filter and filtering dropped the sort. It is tested directly
        // because every link on the page goes through it, so a defect here is a
        // defect in all of them at once.
        let v = View {
            title: "t",
            total: 0,
            rows: &[],
            query: "M&M",
            sort: "isin",
            all: true,
            counts: UniverseCounts::default(),
            active: "ntm",
            page: 3,
            last_page: 9,
            notes: &[],
        };

        // No overrides: the current state, verbatim. The ampersand in the
        // search is escaped, because a symbol really can contain one.
        let keep = v.link_params(None, None, None);
        assert!(keep.contains("q=M&amp;M"), "the search is escaped: {keep}");
        assert!(keep.contains("sort=isin"));
        assert!(keep.contains("u=ntm"));
        assert!(keep.contains("&amp;all=1"));
        assert!(keep.contains("&amp;page=3"), "the page rides along: {keep}");

        // Overriding the sort keeps the filter — the exact bug this replaced.
        let sorted = v.link_params(Some("kind"), None, Some(0));
        assert!(sorted.contains("sort=kind"));
        assert!(sorted.contains("u=ntm"), "sorting must not drop the filter");
        assert!(!sorted.contains("page="), "a new order starts at page one");

        // Overriding the filter keeps the sort — the same bug, other direction.
        let filtered = v.link_params(None, Some("idx"), Some(0));
        assert!(filtered.contains("u=idx"));
        assert!(
            filtered.contains("sort=isin"),
            "filtering must not drop the sort"
        );

        // Not widened: no all flag anywhere in the link.
        let narrow = View { all: false, ..v };
        assert!(!narrow.link_params(None, None, None).contains("all=1"));
    }

    #[test]
    fn the_pager_leads_both_ways_and_carries_every_other_parameter() {
        let r = [row(nifty(), &[Vendor::Groww])];
        let view = |page: usize, last_page: usize, all: bool| View {
            title: "t",
            total: 785,
            rows: &r,
            query: "NIF",
            sort: "isin",
            all,
            counts: UniverseCounts::default(),
            active: "ntm",
            page,
            last_page,
            notes: &[],
        };

        // A single page shows no pager: navigation that leads nowhere is worse
        // than none at all.
        assert!(!instruments_page(&view(0, 0, false)).contains("class=\"pager\""));

        // First page: next only, no previous to nowhere.
        let first = instruments_page(&view(0, 3, false));
        assert!(first.contains("next"));
        assert!(!first.contains("previous"));
        assert!(first.contains("page 1 of 4"));

        // Middle: both directions.
        let mid = instruments_page(&view(1, 3, false));
        assert!(mid.contains("previous") && mid.contains("next"));
        assert!(mid.contains("page 2 of 4"));

        // Last page: previous only.
        let last = instruments_page(&view(3, 3, false));
        assert!(last.contains("previous"));
        assert!(!last.contains("next"));
        assert!(last.contains("page 4 of 4"));

        // EVERY other parameter rides along. Paging that silently cleared the
        // search, the sort or the filter would look like the data changed.
        assert!(mid.contains("q=NIF"), "the search is carried");
        assert!(mid.contains("sort=isin"), "the sort is carried");
        assert!(mid.contains("u=ntm"), "the universe filter is carried");
        // Scoped to the pager's own carry (`&amp;all=1`), not a bare `all=1`:
        // the escape-hatch link beside the search box is `?all=1` and is
        // present on every un-widened page, so a bare match would always hit.
        assert!(
            !mid.contains("&amp;all=1"),
            "not widened, so the pager carries no all flag"
        );

        // And when widened, the flag rides along too, so paging through every
        // NSE listing does not silently snap back to the tracked universe.
        let wide = instruments_page(&view(1, 3, true));
        assert!(wide.contains("all=1"), "the widened view is carried");
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
            page: 0,
            last_page: 0,
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
            page: 0,
            last_page: 0,
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
