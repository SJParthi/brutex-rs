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

use crate::audit::{DROP_REASONS, Drops};
use crate::calendar;
use crate::census::{Coverage, VendorCensus};
use crate::ingest::{Series, SpotTarget};
use brutex_core::instrument::{InstrumentKey, Kind};
use brutex_core::isin::Isin;
use brutex_core::universe::Universe;
use brutex_core::vendor::{Vendor, VendorSet};
use pull::session::{
    BARS_PER_REGULAR_SESSION, Day, SESSION_CLOSE_MINUTE, SESSION_OPEN_MINUTE, Window,
};
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
.filters{padding:18px 20px}\
.fbar{display:flex;flex-wrap:wrap;gap:16px 22px;align-items:flex-end;margin:0;max-width:none;padding:0}\
.fgroup{display:flex;flex-direction:column;gap:7px}\
.flabel{font-size:10.5px;font-weight:750;letter-spacing:1.2px;text-transform:uppercase;color:var(--dim)}\
.pills{display:flex;flex-wrap:wrap;gap:6px}\
.pills input{position:absolute;opacity:0;width:0;height:0;pointer-events:none}\
.pills label{cursor:pointer;font-size:13px;font-weight:650;padding:8px 14px;border-radius:999px;\
border:1px solid var(--line);color:var(--dim);background:var(--panel);transition:all .16s;\
user-select:none;white-space:nowrap}\
.pills label:hover{border-color:var(--acc);color:var(--ink);transform:translateY(-1px)}\
.pills input:checked+label{background:linear-gradient(135deg,var(--acc),var(--acc2));color:#fff;\
border-color:transparent;box-shadow:0 3px 10px color-mix(in srgb,var(--acc) 34%,transparent)}\
.fbar input[type=text]{font:inherit;font-size:14px;padding:9px 13px;border:1px solid var(--line);\
border-radius:10px;background:var(--panel);color:var(--ink);min-width:0;width:auto;\
transition:border-color .16s,box-shadow .16s}\
.fbar input[type=text]:focus{outline:none;border-color:var(--acc);\
box-shadow:0 0 0 3px color-mix(in srgb,var(--acc) 18%,transparent)}\
.fgo{flex-direction:row;align-items:center;gap:12px}\
.fbar button{font:inherit;font-size:14px;font-weight:700;padding:10px 22px;border:0;border-radius:10px;\
cursor:pointer;color:#fff;background:linear-gradient(135deg,var(--acc),var(--acc2));\
box-shadow:0 3px 12px color-mix(in srgb,var(--acc) 34%,transparent);transition:transform .16s}\
.fbar button:hover{transform:translateY(-1px)}\
.fbar .clear{font-size:13px;color:var(--dim);text-decoration:none;border-bottom:1px solid transparent}\
.fbar .clear:hover{color:var(--acc);border-bottom-color:var(--acc)}\
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
.wrap{max-width:1180px;margin:0 auto 20px;padding:0 20px}\
.duo{display:grid;grid-template-columns:repeat(auto-fit,minmax(330px,1fr));gap:16px;\
max-width:1180px;margin:0 auto 20px;padding:0 20px;align-items:start}\
.panel{background:var(--panel);border:1px solid var(--line);border-radius:16px;padding:20px;\
box-shadow:var(--sh);animation:rise .5s both;transition:transform .22s,box-shadow .22s}\
.panel:hover{transform:translateY(-3px);box-shadow:0 14px 34px rgba(16,24,40,.15)}\
.panel h2{font-size:17px;font-weight:830;letter-spacing:-.5px;margin:0 0 3px;\
display:flex;align-items:center;gap:9px}\
.panel h2 .dot{background:var(--acc)}\
.panel .lead{color:var(--dim);font-size:13px;margin:0 0 15px;line-height:1.5}\
form.pull{display:block;max-width:none;margin:0;padding:0}\
.field{margin:0 0 12px}\
.field>span{display:block;font-size:10.5px;font-weight:790;letter-spacing:.85px;\
text-transform:uppercase;color:var(--dim);margin:0 0 5px}\
.field input,.field select{font:inherit;width:100%;padding:10px 13px;border:1px solid var(--line);\
border-radius:11px;background:var(--bg);color:var(--ink);\
transition:border-color .2s,box-shadow .2s}\
.field input:focus,.field select:focus{outline:0;border-color:var(--acc);\
box-shadow:0 0 0 4px color-mix(in srgb,var(--acc) 18%,transparent)}\
.pair{display:grid;grid-template-columns:1fr 1fr;gap:11px}\
.fine{color:var(--dim);font-size:11.5px;line-height:1.65;margin:12px 0 0;\
border-top:1px dashed var(--line);padding-top:11px}\
.fine b{color:var(--warn);font-weight:800}\
.fine code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:11px;\
background:color-mix(in srgb,var(--acc) 10%,transparent);padding:1px 5px;border-radius:5px}\
.halt{padding:15px 18px;border-radius:14px;font-size:13.5px;line-height:1.6;\
background:color-mix(in srgb,var(--bad) 10%,var(--panel));\
border:1px solid color-mix(in srgb,var(--bad) 42%,transparent);color:var(--bad);\
animation:rise .5s both}\
.halt b{display:block;font-size:11px;letter-spacing:1.2px;text-transform:uppercase;\
margin-bottom:5px;font-weight:850}\
.halt.good{background:color-mix(in srgb,var(--ok) 10%,var(--panel));\
border-color:color-mix(in srgb,var(--ok) 42%,transparent);color:var(--ok)}\
.kv{width:100%;font-size:13.5px}\
.kv th{text-align:left;color:var(--dim);font-weight:700;width:15rem;\
padding:9px 0;border-bottom:1px solid var(--line);white-space:nowrap}\
.kv td{padding:9px 0;border-bottom:1px solid var(--line);\
font-variant-numeric:tabular-nums}\
.meters{display:grid;grid-template-columns:repeat(auto-fit,minmax(210px,1fr));gap:13px;margin:4px 0 0}\
.meter{background:var(--bg);border:1px solid var(--line);border-radius:13px;padding:14px}\
.meter .ck{font-size:10.5px;font-weight:770;letter-spacing:.9px;text-transform:uppercase;color:var(--dim)}\
.meter .cv{font-size:25px;font-weight:850;letter-spacing:-1.2px;margin:5px 0 4px;\
font-variant-numeric:tabular-nums}\
.meter.none .cv{color:var(--dim)}\
.hscroll{overflow-x:auto;max-width:1180px;margin:0 auto;padding:0 20px}\
td.miss{color:var(--dim)}\
svg.sw{vertical-align:-2px;margin-right:7px;flex:none}\
.sw.q1{color:color-mix(in srgb,var(--ok) 30%,var(--line))}\
.sw.q2{color:color-mix(in srgb,var(--ok) 54%,var(--line))}\
.sw.q3{color:color-mix(in srgb,var(--ok) 77%,var(--line))}\
.sw.q4{color:var(--ok)}\
.sw.void{color:var(--dim);opacity:.5}\
tr.void td{color:var(--dim)}\
tr.void td.num{font-weight:400}\
tr.swept td.num{font-weight:750;font-variant-numeric:tabular-nums}\
svg.strip{display:block;width:100%;height:16px;border-radius:5px;background:var(--line)}\
svg.strip rect.on{fill:var(--ok)}\
svg.strip rect.gap{fill:transparent}\
svg.share{display:block;width:100%;height:11px;border-radius:99px;background:var(--line)}\
svg.share rect.r0{fill:var(--acc)}\
svg.share rect.r1{fill:var(--acc2)}\
svg.share rect.r2{fill:var(--warn)}\
svg.share rect.r3{fill:var(--bad)}\
svg.share rect.none{fill:var(--line)}\
.legend{display:flex;gap:15px;flex-wrap:wrap;align-items:center;color:var(--dim);\
font-size:11.5px;margin:12px 0 0}\
.legend span{display:inline-flex;align-items:center}\
.key{display:inline-block;width:11px;height:11px;border-radius:3px;margin-right:6px}\
.key.r0{background:var(--acc)}.key.r1{background:var(--acc2)}\
.key.r2{background:var(--warn)}.key.r3{background:var(--bad)}\
td.when{font-variant-numeric:tabular-nums;color:var(--dim);font-size:12.5px}\
td.verdict{font-weight:820;font-size:11px;letter-spacing:.7px}\
td.verdict.loud{color:var(--bad)}\
td.verdict.calm{color:var(--ok)}\
td.wide{white-space:normal;max-width:22rem;font-size:12.5px;color:var(--dim)}\
tr.fault td{background:color-mix(in srgb,var(--bad) 10%,transparent);color:var(--bad)}\
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
/// Counted **once at load** by [`crate::catalog::Catalog`], never from the
/// rendered page and never per request: a pill whose number counts only the 200
/// rows on screen says 52 when the answer is 208, and looks authoritative doing
/// it — and a pill that re-counts the whole universe on every request is the
/// D-0042 defect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
///
/// `/pull` and `/store` moved from disabled to real in D-0038 and now answer.
/// `/runs` has not: there is no sweep yet, so it stays a `lnk off`, which is
/// what keeps this rule a rule rather than a phase the nav passed through.
fn nav(current: &str) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("<nav class=\"top\"><div class=\"inner\">");
    out.push_str("<a class=\"logo\" href=\"/\">brutex</a><div class=\"links\">");
    for (href, label, built) in [
        ("/", "Dashboard", true),
        ("/instruments", "Instruments", true),
        ("/pull", "Ingest", true),
        ("/audit", "Audit", true),
        ("/store", "Store", true),
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
         <input type=\"text\" name=\"q\" autocomplete=\"off\" \
         placeholder=\"type a symbol and press Enter — NIFTY, RELIANCE…\" \
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

// ===========================================================================
// The ingest and store pages. D-0038.
//
// One design language, not two: the shell, the gradient hero, the animated
// cards and the loud-note rules above are reused verbatim. A second visual
// vocabulary on a second page is how an operator learns to read one screen and
// then has to relearn the next.
// ===========================================================================

/// Everything before the first section: head, stylesheet, body, nav.
///
/// Extracted because three pages now open identically, and three copies of a
/// `<head>` is three places for a `<meta viewport>` to go missing from one.
fn open(title: &str, current: &str) -> String {
    open_with(title, current, "")
}

/// [`open`], plus a stylesheet only one page needs.
///
/// The ingest page carries the picker's geometry — one rule per (year, month)
/// offered, computed by [`calendar::dynamic_css`] because CSS cannot work out
/// which column a 1st falls in. It is emitted **only there**: the store and
/// audit pages have no date fields, and shipping a few thousand unused
/// selectors to them would be paying for a widget they do not draw.
fn open_with(title: &str, current: &str, extra: &str) -> String {
    let mut out = String::with_capacity(STYLE.len() + extra.len() + 512);
    out.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    out.push_str("<title>");
    out.push_str(&escape(title));
    out.push_str("</title><style>");
    out.push_str(STYLE);
    out.push_str(extra);
    out.push_str("</style></head><body>");
    out.push_str(&nav(current));
    out
}

/// The footer every page carries, and the closing tags.
const FOOT: &str = "<footer>Rendered on the server. No JavaScript — \
     CLAUDE.md section 2 does not permit it, and CI gate 1 enforces that.\
     </footer></body></html>";

/// The gradient hero, with the eyebrow, heading, lede and status badge.
///
/// `heading` is raw HTML because every caller needs a `<br>` and a
/// non-breaking hyphen in it; everything else is escaped.
fn hero(eyebrow: &str, heading: &str, lede: &str, good: bool, badge: &str) -> String {
    let mut out = String::with_capacity(768);
    let _ = write!(
        out,
        "<header class=\"hero\"><div class=\"hw\">\
         <div class=\"eyebrow\"><span class=\"dot\"></span>{}</div>\
         <h1>{heading}</h1><p class=\"lede\">{}</p>\
         <span class=\"badge {}\">{}</span></div></header>",
        escape(eyebrow),
        escape(lede),
        if good { "good" } else { "bad" },
        escape(badge),
    );
    out
}

/// A loud block naming something an operator must not miss.
fn halt_block(title: &str, body: &str) -> String {
    format!(
        "<section class=\"wrap\"><div class=\"halt\"><b>{}</b>{}</div></section>",
        escape(title),
        escape(body)
    )
}

/// The collapsible note list, identical on every page that has one.
fn notes_block(notes: &[String]) -> String {
    let loud_count = notes
        .iter()
        .filter(|n| LOUD.iter().any(|w| n.contains(w)))
        .count();
    let mut out = String::with_capacity(256 + notes.len() * 96);
    let _ = write!(
        out,
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
        let _ = write!(out, "<li{loud}>{}</li>", escape(&clamp(note)));
    }
    out.push_str("</ul></details>");
    out
}

/// A number, or an em dash when there is no number to show.
///
/// **A dash is not a zero.** "Nothing was dropped" and "nothing measured this"
/// are different facts, and rendering the second as `0` is exactly the silent
/// degradation `CLAUDE.md` §4 forbids.
fn count_cell(n: Option<u64>) -> String {
    n.map_or_else(|| "—".to_owned(), |v| v.to_string())
}

/// `HH:MM` from minutes since midnight. Integer division; no float anywhere.
fn hhmm(minute_of_day: u32) -> String {
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

/// How many parts in `whole` the value `n` is, on a five-step scale.
///
/// Integer arithmetic, because `clippy::float_arithmetic` is denied
/// workspace-wide (CLAUDE.md §7) and a swatch has no business being the first
/// exception. `0` means nothing at all; `1..=4` are the quartiles of `whole`,
/// and anything above zero is at least `1` so a single held bar is still
/// visibly held rather than rounding away to the empty shade.
fn quartile(n: u64, whole: u64) -> u8 {
    if n == 0 {
        return 0;
    }
    let whole = whole.max(1);
    let step = n.saturating_mul(4) / whole;
    u8::try_from(step.clamp(1, 4)).unwrap_or(4)
}

/// A cell's swatch: solid when the month is held, hollow and crossed when it is
/// not.
///
/// **This is the thing that makes a grid readable without reading it.** Colour
/// alone would fail a monochrome print and a colour-blind reader, so the two
/// states differ in *shape* as well: filled square against an outlined square
/// with a stroke through it. `role="img"` and the label carry the same fact to
/// a screen reader, and the number stays in the cell beside it.
fn swatch(n: Option<u64>, peak: u64) -> String {
    match n {
        None | Some(0) => "<svg class=\"sw void\" viewBox=\"0 0 12 12\" width=\"12\" \
             height=\"12\" role=\"img\" aria-label=\"not held\">\
             <rect x=\"1.5\" y=\"1.5\" width=\"9\" height=\"9\" rx=\"2.5\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"1.2\"/>\
             <line x1=\"3\" y1=\"9\" x2=\"9\" y2=\"3\" stroke=\"currentColor\" \
             stroke-width=\"1.2\"/></svg>"
            .to_owned(),
        Some(rows) => format!(
            "<svg class=\"sw q{}\" viewBox=\"0 0 12 12\" width=\"12\" height=\"12\" \
             role=\"img\" aria-label=\"held, {rows} row(s)\">\
             <rect x=\"1\" y=\"1\" width=\"10\" height=\"10\" rx=\"2.5\" \
             fill=\"currentColor\"/></svg>",
            quartile(rows, peak)
        ),
    }
}

/// A strip of ticks, one per row shown, filled where the row is held.
///
/// The whole page's coverage in one glance, and bounded by construction: it
/// draws one rect per row already on the page, so it costs what the table
/// beside it costs. `api::render::the_coverage_strip_draws_one_tick_per_row_and_no_more`
/// is what holds it to one tick per row and no more.
fn coverage_strip(held: &[bool]) -> String {
    let n = held.len();
    if n == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(64 + n * 48);
    let _ = write!(
        out,
        "<svg class=\"strip\" viewBox=\"0 0 {n} 10\" preserveAspectRatio=\"none\" \
         role=\"img\" aria-label=\"{} of {n} row(s) on this page are held\">",
        held.iter().filter(|h| **h).count()
    );
    for (i, on) in held.iter().enumerate() {
        if *on {
            let _ = write!(
                out,
                "<rect class=\"on\" x=\"{i}\" y=\"0\" width=\"1\" height=\"10\"/>"
            );
        }
    }
    out.push_str("</svg>");
    out
}

/// One stacked bar: each drop reason's share of the total, in integer percent.
///
/// Percentages are computed against the total and the last segment takes
/// whatever rounding left over, so the four always add to the width and the bar
/// never shows a gap that is really a division remainder.
fn share_bar(drops: Drops) -> String {
    let total = drops.total();
    if total == 0 {
        return "<svg class=\"share\" viewBox=\"0 0 100 10\" preserveAspectRatio=\"none\" \
                role=\"img\" aria-label=\"nothing was dropped\">\
                <rect class=\"none\" x=\"0\" y=\"0\" width=\"100\" height=\"10\"/></svg>"
            .to_owned();
    }
    let mut out = String::with_capacity(512);
    out.push_str(
        "<svg class=\"share\" viewBox=\"0 0 100 10\" preserveAspectRatio=\"none\" role=\"img\" \
         aria-label=\"each drop reason's share\">",
    );
    let mut at = 0u64;
    for (i, reason) in DROP_REASONS.into_iter().enumerate() {
        let last = i + 1 == DROP_REASONS.len();
        let width = if last {
            100u64.saturating_sub(at)
        } else {
            drops.of(reason).saturating_mul(100) / total
        };
        if width > 0 {
            let _ = write!(
                out,
                "<rect class=\"r{i}\" x=\"{at}\" y=\"0\" width=\"{width}\" height=\"10\"><title>{}</title></rect>",
                escape(&format!("{}: {}", reason.label(), drops.of(reason)))
            );
        }
        at = at.saturating_add(width);
    }
    out.push_str("</svg>");
    out
}

/// The key under a stacked bar, so a colour means something.
fn share_legend() -> String {
    let mut out = String::with_capacity(384);
    out.push_str("<div class=\"legend\">");
    for (i, reason) in DROP_REASONS.into_iter().enumerate() {
        let _ = write!(
            out,
            "<span><i class=\"key r{i}\"></i>{}</span>",
            escape(reason.label())
        );
    }
    out.push_str("</div>");
    out
}

/// A duration an operator reads, from microseconds, with no float anywhere.
///
/// Milliseconds to three places under a second, then seconds to three places.
/// The remainder is printed as a zero-padded fraction rather than divided into
/// one, because `clippy::float_arithmetic` is denied and a wall-clock reading
/// is not the place to make the first exception.
fn elapsed(micros: u64) -> String {
    if micros == 0 {
        return "—".to_owned();
    }
    if micros < 1_000_000 {
        return format!("{}.{:03} ms", micros / 1_000, micros % 1_000);
    }
    format!(
        "{}.{:03} s",
        micros / 1_000_000,
        (micros % 1_000_000) / 1_000
    )
}

/// The day before `day`, or `day` itself at the start of the epoch.
///
/// The expiry input's `max`: an operator cannot even offer a contract that has
/// not expired. The server refuses one anyway — see [`crate::ingest::parse_fno`]
/// — because an attribute is a courtesy and a parser is a rule.
fn yesterday(day: Day) -> Day {
    day.days_from_epoch()
        .checked_sub(1)
        .and_then(|d| Day::from_days(d).ok())
        .unwrap_or(day)
}

/// One labelled form control.
fn field(label: &str, control: &str) -> String {
    format!(
        "<label class=\"field\"><span>{}</span>{control}</label>",
        escape(label)
    )
}

/// A labelled control that must **not** be wrapped in a `<label>`.
///
/// # Why this exists, and what it fixes
///
/// [`field`] wraps its control in `<label class="field">` with no `for`. For a
/// lone `<input>` or `<select>` that is correct and convenient: the labelled
/// control is the one inside, so clicking the caption focuses it.
///
/// For a date picker it is a **defect**. With `for` absent, HTML makes the
/// labelled control the *first labelable descendant in tree order* — which is
/// the picker's popover checkbox. And a `<label>` forwards a click to its
/// labelled control unless that click targeted **interactive content**. The
/// weekday header `<span>Mo</span>`, the explanatory `<p>`, the grid gutters
/// and the popover's own padding are none of those, so clicking any of them
/// toggled the checkbox and **slammed the calendar shut**. The sharpest case:
/// a greyed impossible day carries `pointer-events:none`, so the click fell
/// through to the grid behind it and closed the picker rather than being
/// ignored — the one interaction that should most obviously do nothing.
///
/// It also made the markup invalid twice over: a `<label>`'s content model
/// forbids descendant `<label>` elements *and* forbids labelable descendants
/// other than its own labelled control, and each picker nests 56 of each.
///
/// Nothing is lost by using a `<div>`: the picker already supplies its own
/// `<label class="readout" for="o-…">`, which is a real, explicit association.
fn field_unlabelled(label: &str, control: &str) -> String {
    format!(
        "<div class=\"field\"><span>{}</span>{control}</div>",
        escape(label)
    )
}

/// A date field, as a clickable month grid capped at `max`.
///
/// # Three shapes tried, and why this is the third
///
/// `<input type="date">` came first and was wrong: it renders in the *browser's*
/// locale, which this server cannot influence, and on this operator's machine
/// that was `dd/mm/yyyy`. `01/07/2025` is 1 July here and 7 January in half the
/// world — and this codebase has already been bitten by exactly that ambiguity,
/// because GDFL writes `DD/MM/YYYY` and reading it the other way shifts every
/// bar by months into a file that is internally consistent and totally wrong.
///
/// A plain text box with `placeholder="YYYY-MM-DD"` came second. It fixed the
/// ambiguity by making the operator **type**, which is not a fix. Dhan puts a
/// month grid on screen and you click a number; so does `TradingView`.
///
/// So: a month grid, rendered here, in HTML and CSS with no script — see
/// [`crate::calendar`] for how, and for why the absence of a script is a
/// property rather than a limitation.
///
/// `form` scopes the element ids. **Both forms on the ingest page have a field
/// called `from`**, and an `id` is document-wide — so without the prefix the
/// spot picker's label would carry `for="o-from"` and toggle whichever latch the
/// browser found first, which is the F&O one. The `name` stays unprefixed,
/// because that is what the server reads.
///
/// It is also the form the picker's `Close` points at, so every form that calls
/// this must emit [`calendar::shut`] once — see that function for why the pairing
/// is per form rather than per page.
fn date_input(form: &str, name: &str, max: Day) -> String {
    calendar::picker(name, &format!("{form}{name}"), form, max)
}

// THERE IS NO `suggestions()` HERE ANY MORE, AND ITS ABSENCE IS THE FIX.
//
// A `<datalist>` over the 750-symbol universe was put on the search box and the
// F&O underlying so that typing would narrow the choices. It does narrow — but
// **Chrome opens the full list on focus, before a single character is typed**,
// and draws a `▼` indicator beside the field. Both make it read as a dropdown
// of 750 items, which is exactly the shape it was meant to replace.
//
// Suppressing the indicator with `::-webkit-calendar-picker-indicator` removed
// the arrow and did NOT stop the focus behaviour: that popup belongs to the
// browser, not to this page, and no CSS or attribute cancels it.
//
// Three ways to get true type-to-filter, and only one is available here:
//
//   1. A script filtering a list on each keystroke. `CLAUDE.md` §2 forbids it,
//      and four tests assert `<script>` never appears in any page emitted here.
//   2. A server round-trip per keystroke. Also needs a script to fire it.
//   3. A server round-trip on SUBMIT: type a symbol, press Enter, get matches.
//
// Option 3 already existed on the instruments page and always had: the
// `method="get"` form searches the whole universe rather than the rendered
// page, and its result is a linkable, reloadable URL. The datalist was not
// adding to that — it was covering it with a list nobody asked for. Removed
// rather than tuned, because a control that does the wrong thing well is still
// doing the wrong thing.
//
// `docs/06-limits.md` §30 records what that leaves: there is **no incremental
// type-ahead** on this surface, and there cannot be one without a script.

/// One run, as the page shows it.
///
/// **It is a run that HAPPENED, not one in flight.** A pull here is synchronous
/// — the POST does the work and answers with the result — so there is no such
/// thing as a capture to watch, and a panel that implied otherwise would be a
/// progress bar over a process that does not exist. Every figure below is read
/// back out of [`crate::audit`], which is a file, so it survives the restart
/// that empties a process.
///
/// Held rather than invented: [`Drops`] carries the same four counts
/// [`pull::session::DropCensus`] recorded, so every reason the page names is a
/// reason the filter actually counts.
#[derive(Debug, Clone, Copy)]
pub struct Capture<'a> {
    /// What was asked for, in one line.
    pub what: &'a str,
    /// When it ran, in IST, already rendered.
    pub when: &'a str,
    /// How it ended: one word from [`crate::audit::Outcome`].
    pub outcome: &'a str,
    /// Whether that word is one an operator must not miss.
    pub loud: bool,
    /// The operator's inclusive window.
    pub window: Window,
    /// Rows the source held, before any filtering.
    pub fetched: u64,
    /// Bars that landed in the store.
    pub stored: u64,
    /// Rows merged into a bar that was already open.
    ///
    /// Neither stored nor dropped, and the reason the books balance. It was
    /// missing from this panel while `pull::ingest::Ingested` already carried
    /// it, so an operator could see 354,675 read and 62,978 stored and had no
    /// way to find the other 291,527.
    pub folded: u64,
    /// What was dropped, and why.
    pub drops: Drops,
    /// How long it took, in microseconds.
    pub took_micros: u64,
}

/// Where the journal is and what it holds, in one line for a page footer.
#[derive(Debug, Clone, Copy)]
pub struct JournalNote<'a> {
    /// The file every record is appended to.
    pub path: &'a str,
    /// How many whole records it holds.
    pub records: u64,
    /// How large it is, so the growth is a number rather than a worry.
    pub bytes: u64,
    /// Whether the file is there at all.
    pub present: bool,
    /// What is wrong with the file itself, when something is.
    pub trouble: Option<&'a str>,
}

/// Everything one rendering of the ingest page needs.
#[derive(Debug, Clone, Copy)]
pub struct PullView<'a> {
    /// Today in IST: the expiry gate's reference, and the date inputs' ceiling.
    pub today: Day,
    /// How many instruments each spot target covers, in [`SpotTarget::ALL`]
    /// order. Counted from the loaded universe, never guessed.
    pub targets: &'a [(SpotTarget, usize)],
    /// The last run on the record. `None` renders every counter as `—`.
    pub capture: Option<Capture<'a>>,
    /// Why there is no run to show, when there is none.
    ///
    /// Rendered under the panel so a dash always has a sentence beside it.
    pub no_capture: &'a str,
    /// Where the record of every run lives.
    pub journal: JournalNote<'a>,
    /// What this build cannot do, named.
    ///
    /// `Some` puts a loud block at the top of the page. `CLAUDE.md` §4: degrade
    /// loudly and name the reason — and name it *accurately*, which is the
    /// whole reason this constant was rewritten.
    pub halt: Option<&'a str>,
    /// Everything an operator has to be told.
    pub notes: &'a [String],
    /// Folders holding CSVs, walked ONCE at startup by [`folder_suggestions`].
    ///
    /// Passed in rather than found here: walking them per render cost 72 ms
    /// against /store's 1 ms, and grew with a directory this repository does
    /// not own. See [`folder_input`].
    pub folders: &'a [String],
}

/// The two forms. Split out of [`pull_page`] to stay under clippy's 100-line
/// ceiling; the split is a lint, not a design.
fn pull_forms(view: &PullView<'_>) -> String {
    let today = view.today;
    let mut out = String::with_capacity(4096);
    out.push_str("<section class=\"duo\">");

    // ---- SPOT ------------------------------------------------------------
    out.push_str(
        "<div class=\"panel\"><h2><span class=\"dot\"></span>Spot</h2>\
         <p class=\"lead\">Index and cash series, one-minute bars. \
         NSE only — there is no BSE and no MCX on this surface.</p>\
         <form class=\"pull\" method=\"post\" action=\"/pull/spot\">",
    );
    // The resting member of this form's popover group, and the reason every
    // panel in it starts closed. One per form: a radio group is scoped to its
    // form owner, so the F&O form below carries its own.
    out.push_str(&calendar::shut("s"));
    let mut options = String::with_capacity(512);
    for &(target, members) in view.targets {
        let _ = write!(
            options,
            "<option value=\"{}\">{} · {members} instrument(s) — {}</option>",
            escape(target.slug()),
            escape(target.label()),
            escape(target.note()),
        );
    }
    out.push_str(&field(
        "Target",
        &format!("<select name=\"target\" required>{options}</select>"),
    ));
    // WHICH BROKER. Offered as a control rather than fixed in the route,
    // because `CLAUDE.md` makes adding a vendor a row in `pull::vendor` and a
    // page that names one defeats that.
    //
    // Dhan is first, and therefore the default, because its descriptor is the
    // only one VERIFIED against a live body — the first real call returned
    // `DH-905 securityId is required`, which is a vendor confirming its own
    // shape. Groww's response shape has never been seen, and the note below
    // says so rather than letting an operator discover it.
    out.push_str(&field(
        "Broker",
        "<select name=\"vendor\">\
         <option value=\"dhan\">Dhan — reached live; descriptor confirmed by the vendor</option>\
         <option value=\"groww\">Groww — never reached; response shape UNVERIFIED</option>\
         </select>",
    ));
    let _ = write!(
        out,
        "<div class=\"pair\">{}{}</div>",
        field_unlabelled("From (inclusive)", &date_input("s", "from", today)),
        field_unlabelled("To (inclusive)", &date_input("s", "to", today)),
    );
    // THE LOCAL-ARCHIVE FIELD. Half the vendors are not APIs — TrueData and
    // GDFL sell folders of CSVs — and that half needs no socket, no token and
    // no rate governor, so it is the half that works today. Left blank, the
    // request takes the HTTP path, which does not exist yet and refuses loudly
    // rather than doing nothing.
    let _ = write!(
        out,
        "{}",
        field(
            "Local vendor folder (optional)",
            &folder_input(view.folders),
        )
    );
    out.push_str("<button type=\"submit\">Start spot pull</button>");
    out.push_str("</form>");
    out.push_str(&pull_fine_print(today));
    out.push_str("</div>");

    // ---- EXPIRED F&O -----------------------------------------------------
    out.push_str(
        "<div class=\"panel\"><h2><span class=\"dot\"></span>Expired F&amp;O</h2>\
         <p class=\"lead\">Futures and option chains on series that have \
         already expired. These are stored and never swept.</p>\
         <form class=\"pull\" method=\"post\" action=\"/pull/fno\">",
    );
    // This form's own popover group. See the spot form above.
    out.push_str(&calendar::shut("f"));
    out.push_str(&field(
        "Underlying",
        "<input type=\"text\" name=\"underlying\" autocomplete=\"off\" \
         placeholder=\"NIFTY, BANKNIFTY, RELIANCE…\" required>",
    ));
    let mut series = String::with_capacity(128);
    for s in Series::ALL {
        let _ = write!(
            series,
            "<option value=\"{}\">{}</option>",
            escape(s.slug()),
            escape(s.label())
        );
    }
    let expiry_max = yesterday(today);
    let _ = write!(
        out,
        "<div class=\"pair\">{}{}</div>",
        field(
            "Series",
            &format!("<select name=\"series\" required>{series}</select>")
        ),
        field_unlabelled("Expiry", &date_input("f", "expiry", expiry_max)),
    );
    let _ = write!(
        out,
        "<div class=\"pair\">{}{}</div>",
        field_unlabelled("From (inclusive)", &date_input("f", "from", expiry_max)),
        field_unlabelled("To (inclusive)", &date_input("f", "to", expiry_max)),
    );
    out.push_str("<button type=\"submit\">Start expired-series pull</button>");
    out.push_str("</form>");
    let _ = write!(
        out,
        "<p class=\"fine\"><b>A LIVE CONTRACT CAN NEVER BE REQUESTED.</b> The \
         expiry field will not accept a date after <code>{expiry_max}</code>, \
         and the server refuses one anyway — an attribute is a courtesy, a \
         parser is a rule. The window may not run past the expiry either: there \
         are no bars there to fetch.</p>",
    );
    out.push_str("</div></section>");
    out
}

/// The small print under the spot form: the two corrections an operator would
/// otherwise have to know about.
fn pull_fine_print(today: Day) -> String {
    format!(
        "<p class=\"fine\">Both dates are <b>inclusive</b>. Dhan's \
         <code>toDate</code> is <b>not</b> — so what goes on the wire is the day \
         <em>after</em> the one you pick, and the extra day's bars come back and \
         are dropped as <code>after the requested window</code>, counted below. \
         The correction is stated because a correction you cannot see is the kind \
         this repository forbids.<br>Bars are kept only inside <code>{}</code> to \
         <code>{}</code> IST, exclusive at the close — {BARS_PER_REGULAR_SESSION} \
         one-minute bars in a regular session. Nothing after <code>{today}</code> \
         can be asked for.</p>",
        hhmm(SESSION_OPEN_MINUTE),
        hhmm(SESSION_CLOSE_MINUTE),
    )
}

/// The six meters over the last recorded run.
///
/// Split out of [`capture_panel`] to stay under clippy's line ceiling; the
/// split is a lint, not a design.
fn capture_meters(capture: Option<Capture<'_>>) -> String {
    let mut out = String::with_capacity(1536);
    out.push_str("<div class=\"meters\">");
    for (label, value, note) in [
        (
            "Asked for",
            capture.map_or_else(|| "—".to_owned(), |c| escape(c.what)),
            "target or folder",
        ),
        (
            "Window",
            capture.map_or_else(|| "—".to_owned(), |c| escape(&c.window.to_string())),
            "inclusive, both ends",
        ),
        (
            "Rows read",
            count_cell(capture.map(|c| c.fetched)),
            "as the source sent them",
        ),
        (
            "Bars stored",
            count_cell(capture.map(|c| c.stored)),
            "written to the bar file",
        ),
        (
            "Rows folded",
            count_cell(capture.map(|c| c.folded)),
            "merged into an open bar",
        ),
        (
            "Rows dropped",
            count_cell(capture.map(|c| c.drops.total())),
            "by the reasons below",
        ),
    ] {
        let cls = if value == "—" { " none" } else { "" };
        let _ = write!(
            out,
            "<div class=\"meter{cls}\"><div class=\"ck\">{}</div>\
             <div class=\"cv\">{value}</div><div class=\"cn\">{}</div></div>",
            escape(label),
            escape(note)
        );
    }
    out.push_str("</div>");
    out
}

/// The panel over the last recorded run: what landed, and what was dropped, by
/// reason.
///
/// It reads the journal, not a variable: the figures are the ones
/// [`crate::audit::Record`] holds, so they are the same figures after a restart
/// as before one.
fn capture_panel(capture: Option<Capture<'_>>, absent: &str) -> String {
    let drops = capture.map(|c| c.drops);
    let mut out = String::with_capacity(3072);
    let _ = write!(
        out,
        "<section class=\"wrap\"><div class=\"panel\">\
         <h2><span class=\"dot\"></span>Last recorded run</h2>\
         <p class=\"lead\">Not a live capture — a pull here is one POST that does \
         the work and answers with the result, so there is nothing to watch. \
         These are the counters of the most recent run, read back out of the \
         journal on disk.{}</p>",
        capture.map_or_else(
            || format!(" <b>{}</b>", escape(absent)),
            |c| format!(
                " Ran {} · <b>{}</b> · took {}.",
                escape(c.when),
                escape(c.outcome),
                escape(&elapsed(c.took_micros))
            )
        )
    );
    out.push_str(&capture_meters(capture));

    // The bar widths are INTEGER percentages of the largest drop on the panel.
    // `clippy::float_arithmetic` is denied workspace-wide (CLAUDE.md §7) and a
    // progress bar has no business being the first exception.
    let peak = drops.map_or(1, Drops::peak);
    out.push_str(
        "<table><thead><tr><th>Dropped because</th><th>Rows</th><th>Share</th></tr></thead><tbody>",
    );
    for reason in DROP_REASONS {
        let n = drops.map(|d| d.of(reason));
        let bar = n.map_or_else(String::new, |v| {
            format!(
                "<div class=\"cbar\"><span style=\"width:{}%\"></span></div>",
                (v.saturating_mul(100) / peak).max(3)
            )
        });
        // ONE `class` attribute, not two. This was `class="num"{cls}` with
        // `cls` supplying a second `class="miss"`, which is malformed HTML: a
        // browser keeps the first and silently drops the second, so the "not
        // measured" styling was never applied and nothing on screen said so.
        let cls = if n.is_none() { " miss" } else { "" };
        let _ = write!(
            out,
            "<tr><td>{}</td><td class=\"num{cls}\">{}</td><td>{bar}</td></tr>",
            escape(reason.label()),
            count_cell(n),
        );
    }
    out.push_str("</tbody></table>");
    if let Some(c) = capture {
        let _ = write!(
            out,
            "<p class=\"fine\">Every reason's share of the {} row(s) dropped:</p>{}{}",
            c.drops.total(),
            share_bar(c.drops),
            share_legend()
        );
    }
    out.push_str("</div></section>");
    out
}

/// Where the record of every run lives, said on the page rather than assumed.
///
/// **The point of the sentence is the word "disk".** An operator has to know
/// whether the history they are reading survives a restart, and the only way to
/// know is to be told which it is and where.
fn journal_note(note: JournalNote<'_>) -> String {
    let mut out = String::with_capacity(768);
    out.push_str("<section class=\"wrap\"><div class=\"panel\">");
    let _ = write!(
        out,
        "<h2><span class=\"dot\"></span>Where the record is kept</h2>\
         <p class=\"lead\">Every pull — accepted, refused or failed — appends one \
         256-byte record to <code>{}</code>. <b>It is a file on disk, not memory: \
         restarting this process loses nothing.</b> {}</p>",
        escape(note.path),
        if note.present {
            format!(
                "{} record(s), {} byte(s) so far. <a href=\"/audit\">Read them</a>.",
                note.records, note.bytes
            )
        } else {
            "No file yet — nothing has been pulled through this store root.".to_owned()
        }
    );
    if let Some(trouble) = note.trouble {
        let _ = write!(
            out,
            "<div class=\"halt\"><b>the journal itself</b>{}</div>",
            escape(trouble)
        );
    }
    out.push_str("</div></section>");
    out
}

/// The ingest page: two forms, and an honest account of what this build can do.
#[must_use]
pub fn pull_page(view: &PullView<'_>) -> String {
    // The two ceilings this page actually uses, passed rather than assumed:
    // today for spot, yesterday for anything carrying an expiry. Every picker
    // shares the rules generated from them.
    let caps = [view.today, yesterday(view.today)];
    let mut body = open_with(
        "brutex · ingest",
        "/pull",
        &format!("{}{}", calendar::CAL_STYLE, calendar::dynamic_css(&caps)),
    );
    body.push_str(&hero(
        "NSE · SPOT AND EXPIRED F&O · POST ONLY",
        "Ingest,<br>on the record.",
        "Two forms, because a spot pull and an expired-series pull do not take \
         the same fields. Starting one is a POST and only a POST — a crawler, a \
         refresh or a back button must never fetch a vendor.",
        view.halt.is_none(),
        if view.halt.is_none() {
            "ready"
        } else {
            "ARCHIVE ONLY"
        },
    ));
    if let Some(reason) = view.halt {
        body.push_str(&halt_block("what this build can and cannot do", reason));
    }
    body.push_str(&pull_forms(view));
    body.push_str(&capture_panel(view.capture, view.no_capture));
    body.push_str(&journal_note(view.journal));
    body.push_str(&notes_block(view.notes));
    body.push_str(FOOT);
    body
}

/// What one POST decided.
#[derive(Debug, Clone)]
pub struct Receipt<'a> {
    /// Which form it came from.
    pub scope: &'a str,
    /// The one word at the top: what happened.
    pub verdict: &'a str,
    /// The loud line: the refusal, or why an accepted request did not run.
    pub reason: &'a str,
    /// Whether the verdict is one to be pleased about.
    ///
    /// **This was hardcoded `false`, and a run that stored 62,978 bars came
    /// back under a red `badge bad`.** That was harmless when the only thing
    /// this page could ever say was `REFUSED` or `NOT STARTED`; it stopped
    /// being harmless the moment a run could succeed. A field, because the
    /// caller is the only thing that knows.
    pub good: bool,
    /// The request as the server understood it, field by field.
    ///
    /// Rendered even for a refusal, because "what you asked for" and "what I
    /// read" differing is itself the diagnosis.
    pub facts: &'a [(&'a str, String)],
    /// The last line: what this answer means for the store.
    ///
    /// **A field, because the line under it used to be the constant sentence
    /// "Nothing here was written to the store."** That was true of every
    /// receipt this page could produce when it was written, and it went on
    /// being printed under a run that wrote 194 bar files and 9.8 MB. Only the
    /// caller knows which happened, so only the caller writes it.
    pub footnote: &'a str,
}

/// The page one POST answers with.
///
/// Never a redirect back to the form: an operator who submits a window and gets
/// the empty form back has been told nothing, and the browser's own re-POST
/// warning is the only thing standing between a refresh and a second pull.
#[must_use]
pub fn receipt_page(receipt: &Receipt<'_>) -> String {
    let mut body = open("brutex · ingest · result", "/pull");
    body.push_str(&hero(
        "POST · ONE REQUEST · ONE ANSWER",
        "Asked, read,<br>answered.",
        "Every field below is what the server understood, not what was typed. \
         Where the two differ, that is the diagnosis.",
        receipt.good,
        receipt.verdict,
    ));
    let _ = write!(
        body,
        "<section class=\"wrap\"><div class=\"halt{}\"><b>{}</b>{}</div></section>",
        if receipt.good { " good" } else { "" },
        escape(receipt.scope),
        escape(receipt.reason)
    );
    body.push_str("<section class=\"wrap\"><div class=\"panel\"><table class=\"kv\"><tbody>");
    for &(label, ref value) in receipt.facts {
        let _ = write!(
            body,
            "<tr><th>{}</th><td>{}</td></tr>",
            escape(label),
            escape(value)
        );
    }
    body.push_str("</tbody></table>");
    let _ = write!(
        body,
        "<p class=\"fine\">{} <a href=\"/pull\">Back to the forms</a> · \
         <a href=\"/audit\">the record of every run</a>.</p>",
        escape(receipt.footnote)
    );
    body.push_str("</div></section>");
    body.push_str(FOOT);
    body
}

/// Everything one rendering of the store page needs.
#[derive(Debug, Clone, Copy)]
pub struct StoreView<'a> {
    /// Today in IST: the newest month the coverage grid reaches.
    pub today: Day,
    /// One per vendor, in [`Vendor::ALL`] order.
    pub censuses: &'a [VendorCensus],
    /// The coverage rows this page shows, already selected by ordinal.
    pub rows: &'a [Coverage],
    /// Which page of the grid this is, zero-based.
    pub page: usize,
    /// The highest page number that has rows.
    pub last_page: usize,
    /// How many rows the whole grid has.
    pub total: usize,
    /// Everything an operator has to be told.
    pub notes: &'a [String],
    /// What the operator narrowed to, so the controls render already set and a
    /// link is shareable. `None` on a page that has no filter bar.
    pub filter: Option<&'a crate::census::StoreFilter>,
    /// How many entries the store holds before this filter — the `of M` in
    /// "showing N of M". Zero when nothing has been ingested.
    pub held: usize,
    /// Whether the page is listing what is HELD, or the full month product.
    pub held_only: bool,
}

/// The filter bar: what to look at, in four narrowings.
///
/// # Why this exists
///
/// `/store` used to render `instruments × 36 months` and nothing else, so a
/// store holding 194 entries opened on 36 consecutive blank rows of one index
/// and the real data began on page 2. The question an operator actually has is
/// never "show me the product" — it is "do I have NIFTY futures for July",
/// which is four narrowings: kind, name, bar length, and a span of months.
///
/// A `method="get"` form, so every narrowing is in the URL: the result is
/// bookmarkable, reloadable and sendable to someone else. That is the same
/// reason `/instruments` search is a GET, and it is why no script is needed for
/// any of it.
fn store_filter_bar(filter: &crate::census::StoreFilter, held_only: bool) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str(
        "<section class=\"wrap\"><div class=\"panel filters\">\
         <form method=\"get\" action=\"/store\" class=\"fbar\">",
    );

    // ---- KIND -----------------------------------------------------------
    // Radio pills rather than a <select>: the whole set is four, and a control
    // that shows its options without being opened is one fewer click.
    out.push_str("<div class=\"fgroup\"><span class=\"flabel\">Kind</span><div class=\"pills\">");
    for (value, label) in [
        ("", "All"),
        ("INDEX", "Indices"),
        ("CASH", "Stocks"),
        ("FNO", "Futures &amp; Options"),
    ] {
        let on = match filter.segment {
            None => value.is_empty(),
            Some(s) => s.as_str() == value,
        };
        let checked = if on { " checked" } else { "" };
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"kind\" id=\"k-{value}\" value=\"{value}\"{checked}>\
             <label for=\"k-{value}\">{label}</label>"
        );
    }
    out.push_str("</div></div>");

    // ---- SYMBOL ---------------------------------------------------------
    let symbol = filter.symbol.as_deref().unwrap_or("");
    let _ = write!(
        out,
        "<div class=\"fgroup\"><span class=\"flabel\">Instrument</span>\
         <input type=\"text\" name=\"symbol\" value=\"{}\" \
         placeholder=\"NIFTY, RELIANCE, ABB…\" autocomplete=\"off\"></div>",
        escape(symbol)
    );

    // ---- MONTHS ---------------------------------------------------------
    // Typed as YYYY-MM. A month is not a date and the calendar picker would be
    // the wrong control: it offers 31 days the store has no opinion about.
    let month_value =
        |m: Option<store::path::YearMonth>| m.map_or_else(String::new, |m| format!("{m}"));
    let _ = write!(
        out,
        "<div class=\"fgroup\"><span class=\"flabel\">From month</span>\
         <input type=\"text\" name=\"from\" value=\"{}\" placeholder=\"2025-07\" \
         autocomplete=\"off\" size=\"9\"></div>\
         <div class=\"fgroup\"><span class=\"flabel\">To month</span>\
         <input type=\"text\" name=\"to\" value=\"{}\" placeholder=\"2026-08\" \
         autocomplete=\"off\" size=\"9\"></div>",
        escape(&month_value(filter.from)),
        escape(&month_value(filter.to))
    );

    // ---- WHAT TO LIST ----------------------------------------------------
    // The default is what is HELD. The full product is still one click away,
    // because "what am I missing" is a real question — just not the first one.
    let gaps = if held_only { "" } else { " checked" };
    let _ = write!(
        out,
        "<div class=\"fgroup\"><span class=\"flabel\">Show</span><div class=\"pills\">\
         <input type=\"radio\" name=\"show\" id=\"s-held\" value=\"\"{}>\
         <label for=\"s-held\">What I have</label>\
         <input type=\"radio\" name=\"show\" id=\"s-gaps\" value=\"gaps\"{gaps}>\
         <label for=\"s-gaps\">Every month, incl. gaps</label>\
         </div></div>",
        if held_only { " checked" } else { "" }
    );

    out.push_str(
        "<div class=\"fgroup fgo\"><button type=\"submit\">Show</button>\
         <a class=\"clear\" href=\"/store\">clear</a></div>",
    );
    out.push_str("</form></div></section>");
    out
}

/// The per-vendor counter cards. One field read each, never a walk.
fn census_cards(censuses: &[VendorCensus]) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<section class=\"cards\">");
    for (i, census) in censuses.iter().enumerate() {
        let (months, rows, entries) = census
            .counters()
            .map_or((None, None, None), |(m, r, e)| (Some(m), Some(r), Some(e)));
        let cls = if census.is_loud() { " loud" } else { "" };
        let _ = write!(
            out,
            "<div class=\"card{cls}\" style=\"animation-delay:{}ms\">\
             <div class=\"ck\">{} · months held</div><div class=\"cv\">{}</div>\
             <div class=\"cn\">{} row(s) across {} committed entr(ies)</div>\
             <div class=\"cn\">{}</div></div>",
            i * 60,
            escape(census.vendor.as_str()),
            count_cell(months),
            count_cell(rows),
            count_cell(entries),
            escape(&format!("generation {}", count_cell(census.generation()))),
        );
    }
    out.push_str("</section>");
    out
}

/// The largest row count anywhere on this page, or one.
///
/// The swatch shades are quartiles of **the page**, not of an invented ideal.
/// Nobody knows how many one-minute bars a month "should" hold without a
/// trading calendar, and `crates/pull/src/session.rs` says the calendar filter
/// does not exist yet (`docs/04-invariants.md` P-03). Shading against a made-up
/// denominator would be exactly the invention `CLAUDE.md` §3 rule 1 forbids, so
/// the scale is stated on the page as what it is: relative to the fullest month
/// shown.
fn page_peak(rows: &[Coverage]) -> u64 {
    let mut peak = 1u64;
    for row in rows {
        for &(_, n) in &row.rows {
            if let Some(v) = n
                && v > peak
            {
                peak = v;
            }
        }
    }
    peak
}

/// The coverage grid: one row per instrument-month, one hash probe per cell.
///
/// Every cell carries a **swatch as well as a number**, so held and not-held
/// differ in shape before they differ in digits, and the row is tinted by which
/// it is. That is the whole change: a grid of numbers is a grid nobody scans.
fn coverage_table(view: &StoreView<'_>) -> String {
    let peak = page_peak(view.rows);
    let mut out = String::with_capacity(768 + view.rows.len() * 320);
    let held: Vec<bool> = view.rows.iter().map(Coverage::is_held).collect();
    let filled = held.iter().filter(|h| **h).count();
    let _ = write!(
        out,
        "<section class=\"wrap\"><div class=\"panel\">\
         <h2><span class=\"dot\"></span>Coverage at a glance</h2>\
         <p class=\"lead\">One tick per row on this page, filled where some \
         vendor holds that instrument-month. <b>{filled} of {}</b> shown row(s) \
         are held. {}</p>{}</div></section>",
        held.len(),
        if filled == 0 {
            // NO SCALE IS STATED WHEN THERE IS NOTHING TO SCALE. `peak` falls
            // back to 1 so the division is safe, and printing "quartiles of 1"
            // over an empty page would be a number that means nothing dressed
            // as one that does.
            "Every cell on this page is empty, so there is no shading scale to \
             state — an empty page is a real answer, not a missing one."
                .to_owned()
        } else {
            format!(
                "The swatches in the table are quartiles of {peak} — the fullest \
                 month on this page — and not of an ideal month, because no \
                 trading calendar exists in this build to say what a full month is."
            )
        },
        coverage_strip(&held)
    );
    out.push_str("<div class=\"hscroll\"><table><thead><tr>");
    out.push_str("<th>Instrument</th><th>Month</th><th>Timeframe</th>");
    for vendor in Vendor::ALL {
        let _ = write!(out, "<th>{} rows</th>", escape(vendor.as_str()));
    }
    out.push_str("</tr></thead><tbody>");
    for row in view.rows {
        let cls = if row.is_held() { "swept" } else { "void" };
        let _ = write!(
            out,
            "<tr class=\"{cls}\"><td>{}</td><td>{}</td><td>1m</td>",
            escape(&row.series.to_string()),
            escape(&row.month.to_string()),
        );
        for &(_, n) in &row.rows {
            let miss = if n.is_none() { " miss" } else { "" };
            let _ = write!(
                out,
                "<td class=\"num{miss}\">{}{}</td>",
                swatch(n, peak),
                count_cell(n)
            );
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    out
}

/// The store page: the counters, then the coverage grid over them.
#[must_use]
pub fn store_page(view: &StoreView<'_>) -> String {
    let loud = view.censuses.iter().any(VendorCensus::is_loud);
    let mut body = open("brutex · store", "/store");
    body.push_str(&hero(
        "LAYER 13 · COUNTERS, NEVER A DIRECTORY WALK",
        "What is held,<br>without looking.",
        "Every figure on this page is a field read from one manifest header, and \
         every cell in the grid is one hash probe. Nothing here lists a \
         directory — answering \"how many months do I hold\" by walking ~248,000 \
         files is the cost this layer exists to remove.",
        !loud,
        if loud { "DEGRADED" } else { "ok" },
    ));
    for census in view.censuses {
        if census.is_loud() {
            body.push_str(&halt_block(census.vendor.as_str(), &census.note()));
        }
    }
    body.push_str(&census_cards(view.censuses));
    if let Some(filter) = view.filter {
        body.push_str(&store_filter_bar(filter, view.held_only));
    }
    // THE SUMMARY SAYS WHICH QUESTION IS BEING ANSWERED. "4 of 7,056" and
    // "194 held" are both true and mean opposite things, and a page that shows
    // one number while the reader assumes the other is the shape D-0048 was
    // written about.
    if view.held_only {
        let narrowed = view.filter.is_some_and(|f| !f.is_empty());
        let _ = write!(
            body,
            "<p class=\"sub\"><b>{}</b> instrument-month(s) held{} · showing {} \
             on this page{}</p>",
            view.total,
            if narrowed {
                format!(" of {} in the store", view.held)
            } else {
                String::new()
            },
            view.rows.len(),
            if view.total == 0 && view.held > 0 {
                " — nothing matches this filter; the store is not empty"
            } else if view.held == 0 {
                " — nothing has been ingested yet"
            } else {
                ""
            }
        );
    } else {
        let _ = write!(
            body,
            "<p class=\"sub\">{} instrument-month(s) in the grid · showing {} · \
             newest month is {}-{:02}, three years back · \
             <b>{}</b> of them are held</p>",
            view.total,
            view.rows.len(),
            view.today.year(),
            view.today.month(),
            view.held,
        );
    }
    body.push_str(&coverage_table(view));
    body.push_str(&pager("/store", view.page, view.last_page));
    body.push_str(&notes_block(view.notes));
    body.push_str(FOOT);
    body
}

/// Everything one page of prices needs.
#[derive(Debug)]
pub struct BarsView<'a> {
    /// Which instrument, as the store names it.
    pub symbol: &'a str,
    /// Its segment — the store path's, not a label.
    pub segment: &'a str,
    /// Whose bars these are.
    pub vendor: &'a str,
    /// The month, already rendered.
    pub month: &'a str,
    /// The bars on this page, already read by index.
    pub rows: &'a [store::format::Bar],
    /// How many bars the month holds, off the header.
    pub total: usize,
    /// Which page, zero-based.
    pub page: usize,
    /// The last page with rows on it.
    pub last_page: usize,
    /// What went wrong, when something did.
    pub trouble: Option<&'a str>,
    /// Where the file was looked for, so an absence is actionable.
    pub store_root: &'a str,
}

/// The prices page.
///
/// **The first surface in this repository that shows a price.** Everything else
/// counts rows, draws swatches or reports coverage; this renders what is
/// actually on the disk, and it is what makes every other number checkable.
#[must_use]
pub fn bars_page(view: &BarsView<'_>) -> String {
    let mut body = open_with(
        &format!("brutex · {} · {}", view.symbol, view.month),
        "/store",
        crate::bars::BARS_STYLE,
    );
    let ok = view.trouble.is_none() && !view.rows.is_empty();
    body.push_str(&hero(
        &format!(
            "{} · {} · {} · 1 MINUTE",
            escape(view.vendor).to_uppercase(),
            escape(view.segment),
            escape(view.month)
        ),
        &format!("{}<br>as it is stored.", escape(view.symbol)),
        "Every row below is a record read out of the bar file by index — one          seek and one fixed-length read each, so the last page costs what the          first does. Prices are paisa integers all the way here and are split          into rupees rather than divided, because a float has no business          anywhere near a price.",
        ok,
        if ok { "held" } else { "nothing to show" },
    ));

    if let Some(why) = view.trouble {
        body.push_str(&halt_block("this month", why));
    }

    let _ = write!(
        body,
        "<p class=\"sub\"><b>{}</b> bar(s) in {} · showing {} · page {} of {} · \
         store root {}</p>",
        view.total,
        escape(view.month),
        view.rows.len(),
        view.page + 1,
        view.last_page + 1,
        escape(view.store_root)
    );

    if view.rows.is_empty() && view.trouble.is_none() {
        body.push_str(&halt_block(
            "empty month",
            "the file opened and holds no committed record. That is a real \
             answer and a different one from a file that is missing — nothing \
             was ever appended to this month.",
        ));
    }

    body.push_str("<section class=\"wrap\"><div class=\"panel\">");
    body.push_str(&crate::bars::table(view.rows));
    body.push_str("</div></section>");

    // The pager keeps every narrowing in the URL, so a page of prices is
    // linkable — the same property `/store` and `/instruments` have.
    let base = format!(
        "/bars?symbol={}&segment={}&vendor={}&month={}",
        escape(view.symbol),
        escape(view.segment),
        escape(view.vendor),
        escape(view.month)
    );
    body.push_str(&pager(&base, view.page, view.last_page));
    body.push_str(
        "<section class=\"wrap\"><p class=\"fine\">\
         <a href=\"/store\">← back to what is held</a></p></section>",
    );
    body.push_str(FOOT);
    body
}

/// The previous/next control, shared by every paged surface.
///
/// One function rather than one per page, for the reason
/// [`crate::catalog::PAGE_ROWS`] is one constant: two copies drift, and a pager
/// that disagrees with its table about where a page ends is a page an operator
/// cannot trust.
fn pager(base: &str, page: usize, last_page: usize) -> String {
    if last_page == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(256);
    out.push_str("<nav class=\"pager\">");
    if page > 0 {
        let _ = write!(
            out,
            "<a href=\"{base}?page={}\">&larr; previous</a>",
            page.saturating_sub(1)
        );
    }
    let _ = write!(
        out,
        "<span>page {} of {}</span>",
        page.saturating_add(1),
        last_page.saturating_add(1)
    );
    if page < last_page {
        let _ = write!(
            out,
            "<a href=\"{base}?page={}\">next &rarr;</a>",
            page.saturating_add(1)
        );
    }
    out.push_str("</nav>");
    out
}

/// One row of the audit page: one pull, as it was written down.
///
/// The strings are owned rather than borrowed because every one of them is
/// built for this render — a formatted timestamp, a window, a truncated note —
/// and a caller holding six borrowed buffers per row so the renderer can avoid
/// one allocation is a worse trade than the allocation. The count is bounded by
/// [`crate::catalog::PAGE_ROWS`], so it is O(rows shown).
#[derive(Debug, Clone)]
pub struct AuditRow {
    /// Its position in the journal, counted from the first record ever written.
    pub ordinal: u64,
    /// Why the record could not be believed, when it could not be.
    ///
    /// `Some` means every figure below is absent rather than zero, and the row
    /// renders as a refusal in place. One damaged record never blanks the page.
    pub fault: Option<String>,
    /// When it ran, in IST.
    pub when: String,
    /// Which form it came from.
    pub scope: String,
    /// How it ended, in one word.
    pub outcome: String,
    /// Whether that word is one an operator must not miss.
    pub loud: bool,
    /// What was asked for.
    pub source: String,
    /// The window, inclusive at both ends.
    pub window: String,
    /// Members the folder held.
    pub members: u64,
    /// Rows read out of the source.
    pub rows_read: u64,
    /// Bars written to the store.
    pub bars_stored: u64,
    /// Rows merged into a bar that was already open.
    pub rows_folded: u64,
    /// Slices the census holds after the run.
    pub counted: u64,
    /// Rows dropped, by reason.
    pub drops: Drops,
    /// Members that failed.
    pub failures: u64,
    /// How long it took, in microseconds.
    pub took_micros: u64,
    /// Why it ended the way it did.
    pub note: String,
}

/// Everything one rendering of the audit page needs.
#[derive(Debug, Clone, Copy)]
pub struct AuditView<'a> {
    /// Today in IST, for the page header.
    pub today: Day,
    /// Where the journal is and what it holds.
    pub journal: JournalNote<'a>,
    /// The records this page shows, newest first.
    pub rows: &'a [AuditRow],
    /// Which page this is, zero-based.
    pub page: usize,
    /// The highest page number that has rows.
    pub last_page: usize,
    /// Everything an operator has to be told.
    pub notes: &'a [String],
}

/// One record's row in the audit table.
fn audit_row(row: &AuditRow) -> String {
    if let Some(ref fault) = row.fault {
        return format!(
            "<tr class=\"fault\"><td class=\"num\">#{}</td>\
             <td class=\"wide\" colspan=\"10\">RECORD REFUSED — {}</td></tr>",
            row.ordinal,
            escape(fault)
        );
    }
    let verdict = if row.loud { "loud" } else { "calm" };
    let mut out = String::with_capacity(768);
    let _ = write!(
        out,
        "<tr><td class=\"num\">#{}</td><td class=\"when\">{}</td>\
         <td>{}</td><td class=\"verdict {verdict}\">{}</td><td>{}</td>\
         <td class=\"when\">{}</td>\
         <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>\
         <td class=\"num\">{}</td><td class=\"num\">{}</td>",
        row.ordinal,
        escape(&row.when),
        escape(&row.scope),
        escape(&row.outcome),
        escape(&row.source),
        escape(&elapsed(row.took_micros)),
        row.members,
        row.rows_read,
        row.bars_stored,
        row.rows_folded,
        row.counted,
    );
    out.push_str("</tr>");
    let _ = write!(
        out,
        "<tr class=\"{}\"><td></td><td class=\"wide\" colspan=\"4\">{}</td>\
         <td class=\"wide\" colspan=\"6\">{} row(s) dropped · {} member(s) failed{}</td></tr>",
        if row.loud { "fault" } else { "void" },
        escape(&row.note),
        row.drops.total(),
        row.failures,
        if row.drops.total() == 0 {
            String::new()
        } else {
            format!("<br>{}", share_bar(row.drops))
        }
    );
    out
}

/// The audit page: every pull this store root has seen, newest first.
///
/// Bounded by construction. The journal's record count is `len / 256`, one
/// `metadata` call, so the pager is arithmetic; and only the records on this
/// page are ever read off disk —
/// `api::audit::the_tail_reads_only_the_records_it_shows` is the proof of the
/// read and `api::render::the_audit_page_draws_only_the_rows_it_was_given` is
/// the proof of the render.
#[must_use]
pub fn audit_page(view: &AuditView<'_>) -> String {
    let loud =
        view.journal.trouble.is_some() || view.rows.iter().any(|r| r.loud || r.fault.is_some());
    let mut body = open("brutex · audit", "/audit");
    body.push_str(&hero(
        "EVERY PULL · ON DISK · SURVIVES A RESTART",
        "What was asked,<br>and what it did.",
        "One 256-byte record per pull, appended and fsync-ed before the answer \
         is rendered. Nothing here is in memory, so a restart loses none of it — \
         which is the point, because a log is wanted exactly when a process has \
         just stopped.",
        !loud,
        if loud { "ATTENTION" } else { "ok" },
    ));
    if let Some(trouble) = view.journal.trouble {
        body.push_str(&halt_block("the journal itself", trouble));
    }
    body.push_str(&journal_note(view.journal));
    let _ = write!(
        body,
        "<p class=\"sub\">{} record(s) held · showing {} · today is {}</p>",
        view.journal.records,
        view.rows.len(),
        view.today,
    );
    if view.rows.is_empty() {
        body.push_str(&halt_block(
            "nothing recorded yet",
            "No pull has been run against this store root. The table below is \
             empty because there is nothing in it — not because a figure is \
             missing. Start one from the ingest page.",
        ));
    } else {
        body.push_str("<div class=\"hscroll\"><table><thead><tr>");
        for head in [
            "#",
            "When (IST)",
            "Form",
            "Outcome",
            "Asked for",
            "Took",
            "Members",
            "Rows read",
            "Bars stored",
            "Rows folded",
            "Counted",
        ] {
            let _ = write!(body, "<th>{}</th>", escape(head));
        }
        body.push_str("</tr></thead><tbody>");
        for row in view.rows {
            body.push_str(&audit_row(row));
        }
        body.push_str("</tbody></table></div>");
        body.push_str("<section class=\"wrap\">");
        body.push_str(&share_legend());
        body.push_str("</section>");
    }
    body.push_str(&pager("/audit", view.page, view.last_page));
    body.push_str(&notes_block(view.notes));
    body.push_str(FOOT);
    body
}

/// The folder field, with every archive folder found on disk offered as a
/// choice rather than as a path to be typed.
///
/// # Why a datalist and not a dropdown
///
/// A `<select>` would refuse a folder this build did not find, and a text box
/// alone made an operator type
/// `/Users/you/Downloads/GDFL/GFDLNFO_TICK_01072025/Futures/-III` by hand and
/// get one character wrong. `<datalist>` is both: **click a suggestion, or type
/// anything.** It is markup, so it needs no JavaScript — `CLAUDE.md` §2 does
/// not permit a `.js` file and CI gate 1 enforces that.
///
/// # What it looks for, and what it refuses to do
///
/// Two roots, both fixed: `~/Downloads` and `~/.brutex/vendor-data`. It
/// descends **six levels at most** and offers only directories that already
/// hold a `.csv`, so an operator is never offered a folder that will refuse.
///
/// Six, not three, and the number was measured rather than chosen: the
/// operator's own data sits at
/// `Downloads/GDFL/GFDLNFO_TICK_01072025/Futures/-III`, which is five levels
/// below `~`. A depth of three found `Downloads` and `Downloads/GDFL` and
/// stopped two levels short of every folder he would actually pick — a
/// suggestion list that offers only the folders nobody wants is worse than none.
///
/// It does **not** search the whole home directory. An unbounded walk is an
/// unbounded wait — `docs/07-o1-architecture.md` law 5 — and a suggestion list
/// is a convenience, not an index. [`MAX_FOLDER_SUGGESTIONS`] caps it, and the
/// cap is stated on the page rather than silently truncating: a list that
/// quietly stops is a list that appears to have found everything.
fn folder_input(suggestions: &[String]) -> String {
    // THE WALK USED TO HAPPEN HERE, ON EVERY RENDER, AND IT COST 70× THE PAGE.
    //
    // `folder_input` called `collect_csv_dirs` over `~/Downloads` and
    // `~/.brutex/vendor-data` to depth 6 each time `/pull` was requested.
    // Measured on this operator's machine: **72 ms for `/pull` against 1 ms for
    // `/store`**, walking 39,803 entries under `~/Downloads` — and growing with
    // a directory this repository does not own and cannot bound.
    //
    // `MAX_FOLDER_SUGGESTIONS` did not save it. The cap gates `out.len()`, so a
    // tree holding fewer than sixty CSV-bearing folders is walked *in full*
    // every time, and the commonest case is exactly that.
    //
    // This is the defect D-0039 already removed once, when the instrument
    // master was read per request at 150 ms. `docs/07-o1-architecture.md` law 3
    // — never scan to answer a question — and `CLAUDE.md` §3 rule 4. It also
    // made `crate::census`'s opening sentence false: there was a `read_dir`
    // under `crates/api` after all, one module over from the one promising
    // there was not.
    //
    // The list is now walked ONCE, at startup, into `server::Site`, and handed
    // down. See `census::held_series` for the same bargain and
    // `docs/06-limits.md` §34 for what it costs.
    let capped = suggestions.len() > MAX_FOLDER_SUGGESTIONS;
    let found: Vec<&String> = suggestions.iter().take(MAX_FOLDER_SUGGESTIONS).collect();

    let mut out = String::with_capacity(1024);
    out.push_str(
        "<input type=\"text\" name=\"folder\" list=\"folders\" \
         placeholder=\"click to choose, or type a path\">",
    );
    out.push_str("<datalist id=\"folders\">");
    for f in &found {
        let _ = write!(out, "<option value=\"{}\">", escape(f));
    }
    out.push_str("</datalist>");
    let _ = write!(
        out,
        "<p class=\"fine\">{} folder(s) holding CSVs found under \
         <code>~/Downloads</code> and <code>~/.brutex/vendor-data</code>{}. \
         The list is a convenience, not an index — type any path you like.</p>",
        found.len(),
        if capped {
            format!(", capped at {MAX_FOLDER_SUGGESTIONS}")
        } else {
            String::new()
        }
    );
    out
}

/// The most folders the picker will offer.
///
/// Bounded because the list is rendered into every page load. A cap that is
/// stated is a cap; a cap that silently truncates is a lie about completeness.
const MAX_FOLDER_SUGGESTIONS: usize = 60;

/// Every folder holding CSVs, walked **once** and handed to every later render.
///
/// # Call this at startup and nowhere else
///
/// This is the only `read_dir` under `crates/api`, and it is O(entries under
/// `$HOME/Downloads`) — unbounded by anything this repository controls.
/// `server::Site::new` calls it once and holds the result for the process's
/// lifetime, which is the same bargain D-0039 struck for the instrument master
/// and `census::held_series` strikes for the coverage axis. Calling it from a
/// request handler is the defect this function was extracted to make visible.
///
/// `docs/06-limits.md` §34 records the cost and what is not bounded about it.
#[must_use]
pub fn folder_suggestions() -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        for root in [
            home.join("Downloads"),
            home.join(".brutex").join("vendor-data"),
        ] {
            collect_csv_dirs(&root, 6, &mut found);
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Directories at or under `dir` that directly contain a `.csv`.
///
/// Depth-limited and allocation-bounded. `depth` counts down, so the recursion
/// cannot outlive the number it was given — there is no cycle check because
/// there is no cycle a bounded depth can complete.
fn collect_csv_dirs(dir: &std::path::Path, depth: usize, out: &mut Vec<String>) {
    if depth == 0 || out.len() >= MAX_FOLDER_SUGGESTIONS || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut has_csv = false;
    let mut children: Vec<std::path::PathBuf> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        // The macOS shadow tree is not data — the same rule `pull::csv::is_ghost`
        // applies, for the same reason: those entries end in `.csv` and are
        // binary, so a folder full of them would be offered as a real choice.
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        if name
            .as_deref()
            .is_some_and(|n| n.starts_with('.') || n == "__MACOSX")
        {
            continue;
        }
        if p.is_dir() {
            children.push(p);
        } else if p.extension().is_some_and(|x| x == "csv") {
            has_csv = true;
        }
    }
    if has_csv {
        out.push(dir.to_string_lossy().into_owned());
    }
    for c in children {
        collect_csv_dirs(&c, depth - 1, out);
    }
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
    // The counter type itself is only reachable from a test now: `Capture`
    // holds `Drops`, which is the same four counts as a value, because a
    // record read back off disk cannot rebuild a counter without counting.
    use pull::session::{DropCensus, DropReason};

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
    fn the_sort_header_marks_the_active_column_and_offers_the_other_direction() {
        // Found by `cargo mutants` while D-0038 was measuring `render.rs`: the
        // header link's four decisions — which column is active, which
        // direction it is in, which direction the link offers next, and which
        // arrow it draws — were reachable but not distinguished by any
        // assertion, so `==` could be `!=` and the ▲/▼ arms could be deleted
        // with the suite green.
        let r = [row(nifty(), &[Vendor::Groww])];
        let view = |sort: &'static str, active: &'static str| -> String {
            instruments_page(&View {
                title: "t",
                total: 1,
                rows: &r,
                query: "",
                sort,
                all: false,
                counts: UniverseCounts::default(),
                active,
                page: 0,
                last_page: 0,
                notes: &[],
            })
        };

        // Ascending on `symbol`: that header alone is marked ▲, and its own
        // link offers the descending order.
        //
        // The assertions are on the WHOLE header anchor, not on a `sort=`
        // substring: the universe pills carry the current sort too, so a bare
        // substring is present whichever direction the header offers.
        let up = view("symbol", "");
        assert!(
            up.contains("sort=-symbol&amp;u=\">Underlying ▲</a>"),
            "marked ascending, offering descending: {up}"
        );
        assert_eq!(up.matches(" ▲").count(), 1, "exactly one column is active");
        assert_eq!(up.matches(" ▼").count(), 0);
        // Another column is neither marked nor reversed.
        assert!(up.contains("sort=isin&amp;u=\">ISIN</a>"), "{up}");

        // Descending on the same column: ▼, and the link offers ascending back.
        let down = view("-symbol", "");
        assert!(
            down.contains("sort=symbol&amp;u=\">Underlying ▼</a>"),
            "marked descending, offering ascending: {down}"
        );
        assert_eq!(down.matches(" ▼").count(), 1);
        assert_eq!(down.matches(" ▲").count(), 0);

        // A column nobody sorted by is unmarked in both directions, and its
        // link starts that column ascending.
        let none = view("", "");
        assert!(
            none.contains("sort=symbol&amp;u=\">Underlying</a>"),
            "{none}"
        );
        assert_eq!(none.matches(" ▲").count(), 0, "{none}");
        assert_eq!(none.matches(" ▼").count(), 0, "{none}");

        // And the universe pill: exactly the selected one is `on`, and it is
        // the one whose slug matches.
        let ntm = view("", "ntm");
        assert_eq!(
            ntm.matches("\" class=\"on\">").count(),
            1,
            "exactly one pill is selected: {ntm}"
        );
        assert!(
            ntm.contains("u=ntm\" class=\"on\">NIFTY Total Market"),
            "and it is the one that was asked for: {ntm}"
        );
        // With nothing selected, "All" carries it — the empty slug is a slug.
        assert!(none.contains("u=\" class=\"on\">All"), "{none}");
        assert_eq!(none.matches("\" class=\"on\">").count(), 1);
    }

    #[test]
    fn a_bar_is_the_figures_share_of_the_largest_one_on_the_page() {
        // Found by `cargo mutants` while D-0038 was measuring `render.rs`: with
        // every figure equal to the peak, `v * 100 / peak` and `v * 100 % peak`
        // are indistinguishable, so the division could be a modulo or a
        // multiplication and no assertion would notice. Three different figures
        // pin the arithmetic.
        let figures = [
            Stat {
                label: "Peak",
                value: "40",
                note: "the largest",
                loud: false,
            },
            Stat {
                label: "Quarter",
                value: "10",
                note: "a quarter of it",
                loud: false,
            },
            Stat {
                label: "None",
                value: "0",
                note: "and none at all",
                loud: false,
            },
        ];
        let html = dashboard_page("ok", &figures, &[]);
        assert!(html.contains("width:100%"), "the largest fills it: {html}");
        assert!(html.contains("width:25%"), "10 of 40 is a quarter: {html}");
        assert!(
            html.contains("width:3%"),
            "and zero still draws the floor, so it reads as none rather than \
             as not rendered: {html}"
        );
        assert!(!html.contains("width:0%"), "{html}");
        // Integer arithmetic all the way: no fraction reaches the attribute.
        assert!(!html.contains("width:25.0"), "{html}");
    }

    #[test]
    fn a_note_that_became_a_list_is_clamped_on_a_boundary_it_provides() {
        // Short notes pass through untouched.
        let short = "groww: 2750 kept, 0 unreadable";
        assert_eq!(clamp(short), short);

        // A long one is cut at a ", " boundary, never mid-name, and says how
        // many it dropped. One note really does name 31 instruments inline.
        let names: Vec<String> = (0..40).map(|i| format!("NSE-SYMBOL{i:02}")).collect();
        let long = format!(
            "UNCHECKED IDENTITY · named by one vendor only: {}",
            names.join(", ")
        );
        let cut = clamp(&long);
        assert!(cut.len() < long.len(), "it was clamped");
        assert!(
            cut.contains("… and "),
            "it says how many were dropped: {cut}"
        );
        assert!(cut.contains("more"));
        assert!(
            cut.starts_with("UNCHECKED IDENTITY"),
            "the head carries the fact: {cut}"
        );
        assert!(
            !cut.contains("NSE-SYMBOL3"),
            "the tail is dropped, not truncated mid-name: {cut}"
        );

        // A long note with no separator at all still clamps rather than
        // panicking on a missing boundary.
        let unbroken = "X".repeat(400);
        let clamped = clamp(&unbroken);
        assert!(clamped.len() < unbroken.len());

        // THE BOUND IS THE FIRST BYTE THAT IS TOO MANY, AND NOT ONE BEFORE.
        // Found by `cargo mutants` while D-0038 was measuring `render.rs`: with
        // no note sitting exactly on the limit, `<=` could be `<` and nothing
        // would notice.
        let exactly = "Y".repeat(160);
        assert_eq!(clamp(&exactly), exactly, "160 bytes passes through whole");
        let one_more = "Y".repeat(161);
        assert_ne!(clamp(&one_more), one_more, "161 does not");

        // AND THE CUT IS ON A CHARACTER BOUNDARY, NOT A BYTE ONE. `·` is two
        // bytes, so a note built from it crosses 160 mid-character; adding each
        // character's own width is what keeps the slice legal. Without a
        // multi-byte note that addition could be a multiplication, or the slice
        // could panic, with the suite green — and a vendor note really does
        // carry `·` between its tallies.
        let wide = "·".repeat(200);
        let cut = clamp(&wide);
        assert!(cut.len() < wide.len(), "it was clamped: {cut}");
        assert!(cut.starts_with('·'), "and it still starts with one: {cut}");
        // The head is the whole characters that fit inside 160 bytes: 80 of
        // them, because each is two bytes. One more or one fewer means the
        // boundary arithmetic moved.
        assert_eq!(
            cut.matches('·').count(),
            80,
            "160 bytes is exactly 80 two-byte characters: {cut}"
        );
    }

    #[test]
    fn the_dashboard_counts_and_marks_what_needs_attention() {
        let figures = [
            Stat {
                label: "Tracked",
                value: "785",
                note: "both feeds",
                loud: false,
            },
            Stat {
                label: "Disagreements",
                value: "0",
                note: "identity",
                loud: true,
            },
            Stat {
                label: "Unparseable",
                value: "n/a",
                note: "not a number",
                loud: false,
            },
        ];
        let notes = vec![
            "groww: 2 kept".to_owned(),
            "dhan UNREADABLE · malformed identifier ×104".to_owned(),
        ];
        let html = dashboard_page("ok", &figures, &notes);

        assert!(
            html.contains("class=\"hero\""),
            "the dashboard has a header"
        );
        assert!(html.contains("badge good"), "a clean read is badged clean");
        assert!(html.contains("nav class"), "and carries the nav");
        assert!(html.contains("785") && html.contains("Tracked"));
        assert!(html.contains("card loud"), "a loud figure is marked");
        assert!(
            !html.contains("<tbody>"),
            "it counts; it does not list rows"
        );
        // The loud NOTE is marked too, and the summary says how many.
        assert!(html.contains("class=\"loud\">dhan UNREADABLE"));
        assert!(html.contains("<b>1</b> needing attention"));
        // A value that is not a number still draws a full bar rather than
        // vanishing -- "n/a" is information, not an error.
        assert!(html.contains("width:100%"));

        // A degraded read is badged differently, and the summary reports zero
        // loud notes when there are none.
        let clean = dashboard_page("DEGRADED", &figures, &[]);
        assert!(clean.contains("badge bad"));
        assert!(clean.contains("0 notes · <b>0</b> needing attention"));

        // Exactly one note is singular. The plural arm is covered by the two-
        // note case above and the zero case here; without this the singular
        // branch is a region no test enters, and "1 notes" ships.
        let one = dashboard_page("ok", &figures, &["only one".to_owned()]);
        assert!(
            one.contains("1 note · <b>0</b> needing attention"),
            "one note is singular, not \"1 notes\""
        );
    }

    /// A day, for the pages that take one.
    fn d(y: u16, m: u8, day: u8) -> Day {
        Day::new(y, m, day).expect("a real date")
    }

    /// A journal note for the pages that carry one.
    const JOURNAL: JournalNote<'static> = JournalNote {
        path: "/tmp/store/audit/pull.journal",
        records: 3,
        bytes: 768,
        present: true,
        trouble: None,
    };

    /// An ingest view — the last recorded run, or the absence of one.
    fn pull_view<'a>(halt: Option<&'a str>, capture: Option<Capture<'a>>) -> PullView<'a> {
        const TARGETS: [(SpotTarget, usize); 3] = [
            (SpotTarget::Swept, 2),
            (SpotTarget::Indices, 24),
            (SpotTarget::Equities, 750),
        ];
        PullView {
            today: d(2026, 8, 7),
            targets: &TARGETS,
            capture,
            no_capture: "No pull has been recorded against this store root yet.",
            journal: JOURNAL,
            halt,
            notes: &[],
            // EMPTY ON PURPOSE. A render must not depend on the machine it runs
            // on; the walk that used to happen here made the page 72x slower
            // and made its own test a function of $HOME. See folder_suggestions.
            folders: &[],
        }
    }

    #[test]
    fn the_ingest_page_carries_two_forms_and_never_a_get_that_starts_a_pull() {
        let html = pull_page(&pull_view(Some("the fetch is not built"), None));
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.ends_with("</html>"));
        assert!(
            html.contains("class=\"hero\""),
            "one design language, not two"
        );
        assert!(html.contains("nav class"), "and the same nav");

        // TWO FORMS. Spot and F&O take different fields, so they are different
        // forms rather than one form with a mode switch.
        assert_eq!(html.matches("<form class=\"pull\"").count(), 2);
        assert_eq!(html.matches("method=\"post\"").count(), 2);
        assert_eq!(html.matches("method=\"get\"").count(), 0);
        assert!(
            html.contains("Expired F&amp;O"),
            "and never a raw ampersand"
        );
        assert!(!html.contains("Expired F&O<"), "{html}");

        // The counts on the form are the ones passed in, not invented.
        assert!(html.contains("2 instrument(s)"));
        assert!(html.contains("24 instrument(s)"));
        assert!(html.contains("750 instrument(s)"));

        // The expiry ceiling is a day behind today, so a live contract is not
        // even offerable. The server refuses one too — see ingest::parse_fno.
        // `max` is INERT on a text input, so the bound is asserted where it now
        // lives — in the title the operator can read — and the PARSER is what
        // enforces it. The page already says: an attribute is a courtesy, a
        // parser is a rule.
        assert!(html.contains("2026-08-06"), "the expiry bound is stated");
        assert!(html.contains("LIVE CONTRACT CAN NEVER BE REQUESTED"));

        // The two corrections an operator would otherwise have to know.
        assert!(html.contains("toDate"), "{html}");
        assert!(
            html.contains("<b>not</b>"),
            "the non-inclusive rule: {html}"
        );
        assert!(html.contains("09:15") && html.contains("15:30"));
        assert!(html.contains("375"));

        // And no script of any kind reaches the page.
        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }

    #[test]
    fn a_capture_that_is_not_running_shows_dashes_and_a_named_reason() {
        // A DASH IS NOT A ZERO. `0 dropped` is a measurement; `—` is the
        // absence of one, and rendering the second as the first is exactly the
        // silent degradation CLAUDE.md section 4 forbids.
        //
        // CHANGED, AND NAMED: the badge used to read CAPTURE UNAVAILABLE and
        // the meters used to be five. Both were wrong once the local-archive
        // path started running — capture is not unavailable, the HTTP transport
        // is — and the sixth meter is `Rows folded`, which is the number that
        // makes the books balance and had nowhere to be shown.
        let html = pull_page(&pull_view(Some("there is no HTTP transport"), None));
        assert!(
            html.contains("class=\"halt\""),
            "the reason is loud: {html}"
        );
        assert!(html.contains("there is no HTTP transport"));
        assert!(html.contains("ARCHIVE ONLY"), "and badged: {html}");
        assert!(!html.contains("badge good"));
        assert!(!html.contains("<div class=\"cv\">0</div>"), "{html}");
        assert!(html.contains("<div class=\"cv\">—</div>"));
        assert!(html.contains("class=\"num miss\">—</td>"), "{html}");
        // EVERY meter is marked as carrying no measurement, not merely
        // rendered with a dash in it: the class is what greys it, and a class
        // applied to the wrong half would leave a dash looking like a figure.
        assert_eq!(
            html.matches("class=\"meter none\"").count(),
            6,
            "all six meters carry no measurement: {html}"
        );
        assert_eq!(html.matches("class=\"meter\"").count(), 0, "{html}");
        assert!(
            !html.contains("<span style=\"width:"),
            "no bar is drawn over a number nobody has: {html}"
        );
        // And the dash has a sentence beside it naming the empty FILE, not an
        // absent module.
        assert!(
            html.contains("No pull has been recorded against this store root yet"),
            "{html}"
        );
        // Every reason the filter counts is still NAMED, so the shape of what
        // will be measured is visible even while nothing is measured.
        for reason in DROP_REASONS {
            let label = reason.label();
            assert!(html.contains(label), "{label} must be named: {html}");
        }
    }

    #[test]
    fn a_recorded_run_reports_real_counts_with_integer_bars() {
        // The other half of the same renderer, driven by the four counts a real
        // DropCensus recorded. A renderer proved only in its empty state is a
        // renderer nobody has checked.
        let mut census = DropCensus::new();
        for _ in 0..40 {
            census.count(DropReason::BeforeSessionOpen);
        }
        for _ in 0..10 {
            census.count(DropReason::AfterWindow);
        }
        let window = Window::new(d(2022, 1, 8), d(2022, 2, 8)).expect("forwards");
        let capture = Capture {
            what: "NSE-NIFTY · 1m",
            when: "2022-02-08 15:31:02 IST",
            outcome: "STORED",
            loud: false,
            window,
            fetched: 12_040,
            stored: 11_990,
            folded: 0,
            drops: Drops::of_census(census),
            took_micros: 4_512_903,
        };
        let html = pull_page(&pull_view(None, Some(capture)));

        assert!(html.contains("badge good"), "nothing is halted: {html}");
        assert!(!html.contains("class=\"halt\""), "{html}");
        // The mirror of the empty case: every meter now carries a measurement,
        // so none of them is greyed.
        assert_eq!(html.matches("class=\"meter none\"").count(), 0, "{html}");
        assert_eq!(html.matches("class=\"meter\"").count(), 6, "{html}");
        assert!(html.contains("NSE-NIFTY · 1m"));
        assert!(html.contains("2022-01-08..=2022-02-08"), "{html}");
        assert!(html.contains("12040") && html.contains("11990"));
        assert!(html.contains(">50<"), "the drop total: {html}");
        assert!(html.contains(">40<") && html.contains(">10<"));
        assert!(
            html.contains("2022-02-08 15:31:02 IST"),
            "when it ran: {html}"
        );
        assert!(html.contains("4.512 s"), "how long it took: {html}");
        // INTEGER bar widths, never a float: 40/40 is 100%, 10/40 is 25%, and
        // a reason with nothing dropped still draws the 3% floor so that a zero
        // reads as "none" rather than as "not rendered".
        assert!(html.contains("width:100%"), "{html}");
        assert!(html.contains("width:25%"), "{html}");
        assert!(html.contains("width:3%"), "{html}");
        assert!(!html.contains("width:0%"), "{html}");
        // THE SHARE BAR: four segments summing to exactly the full width, with
        // the last taking the rounding remainder so there is never a gap that
        // is really a division leftover.
        assert!(html.contains("class=\"share\""), "{html}");
        assert!(
            html.contains("class=\"r0\"") || html.contains("class=\"r1\""),
            "{html}"
        );
        assert!(html.contains("class=\"key r3\""), "and a legend: {html}");

        // ── THE HALF OF A-09 THIS TEST DID NOT PROVE ────────────────────────
        //
        // The invariant is two claims joined by an "and": every reason the
        // filter counts is **named on the page by `DropReason::label`**, AND
        // the bar width is integer arithmetic. Everything above proves the
        // second. Nothing proved the first — the counts `>40<` and `>10<` say
        // a number reached the page, not that the reason beside it did, and a
        // renderer that dropped a row entirely would still satisfy them.
        //
        // It matters because a reason with a zero is the one most likely to be
        // quietly omitted, and a filter reason absent from the page is a filter
        // nobody can audit. All four are asserted, measured or not.
        for reason in DROP_REASONS {
            assert!(
                html.contains(reason.label()),
                "every drop reason is named on the page by its own label; \
                 {:?} is missing: {html}",
                reason.label()
            );
        }
        assert_eq!(
            DROP_REASONS.len(),
            4,
            "a fifth reason must be added to the panel, not silently counted"
        );

        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }

    #[test]
    fn the_page_says_the_journal_is_a_file_on_disk_and_where() {
        // THE OPERATOR'S QUESTION IS "does this survive a restart". A page that
        // shows a history without saying where it lives cannot answer it, and a
        // history that lives in memory is missing exactly when it is wanted.
        let html = pull_page(&pull_view(Some("x"), None));
        assert!(
            html.contains("It is a file on disk, not memory"),
            "the page must say which: {html}"
        );
        assert!(html.contains("/tmp/store/audit/pull.journal"), "{html}");
        assert!(html.contains("3 record(s), 768 byte(s)"), "{html}");
        assert!(
            html.contains("href=\"/audit\""),
            "and a way to read it: {html}"
        );

        // An absent file says so rather than showing zeroes as if measured.
        let mut view = pull_view(Some("x"), None);
        view.journal = JournalNote {
            path: "/tmp/store/audit/pull.journal",
            records: 0,
            bytes: 0,
            present: false,
            trouble: None,
        };
        let html = pull_page(&view);
        assert!(html.contains("No file yet"), "{html}");
        assert!(!html.contains("0 record(s), 0 byte(s)"), "{html}");

        // A torn tail is loud on the page that reads it.
        view.journal = JournalNote {
            path: "/tmp/store/audit/pull.journal",
            records: 4,
            bytes: 1124,
            present: true,
            trouble: Some("TORN WRITE — 100 byte(s) past the last whole record"),
        };
        let html = pull_page(&view);
        assert!(html.contains("TORN WRITE"), "{html}");
    }

    #[test]
    fn a_duration_is_integer_arithmetic_in_both_directions() {
        assert_eq!(elapsed(0), "—", "zero is not a measurement");
        assert_eq!(elapsed(1), "0.001 ms");
        assert_eq!(elapsed(999_999), "999.999 ms");
        assert_eq!(elapsed(1_000_000), "1.000 s");
        assert_eq!(elapsed(4_512_903), "4.512 s");
        assert_eq!(elapsed(u64::MAX), "18446744073709.551 s");
    }

    #[test]
    fn a_swatch_is_a_shape_before_it_is_a_shade() {
        // COLOUR ALONE IS NOT A DISTINCTION. A monochrome print and a
        // colour-blind reader both need the two states to differ in FORM, so
        // held is a filled square and not-held is an outlined square with a
        // stroke through it.
        let empty = swatch(None, 100);
        let zero = swatch(Some(0), 100);
        let full = swatch(Some(100), 100);
        assert!(empty.contains("class=\"sw void\""), "{empty}");
        assert!(empty.contains("<line"), "the cross is the shape: {empty}");
        assert!(empty.contains("fill=\"none\""), "{empty}");
        assert!(empty.contains("aria-label=\"not held\""), "{empty}");
        assert_eq!(zero, empty, "zero rows held is not held");
        assert!(full.contains("class=\"sw q4\""), "{full}");
        assert!(full.contains("fill=\"currentColor\""), "{full}");
        assert!(
            !full.contains("<line"),
            "a filled swatch has no cross: {full}"
        );
        assert!(full.contains("held, 100 row(s)"), "{full}");

        // Quartiles are integer arithmetic and one held bar never rounds away.
        assert_eq!(quartile(0, 100), 0);
        assert_eq!(quartile(1, 100), 1, "one row is still visibly held");
        assert_eq!(quartile(25, 100), 1);
        assert_eq!(quartile(50, 100), 2);
        assert_eq!(quartile(75, 100), 3);
        assert_eq!(quartile(100, 100), 4);
        assert_eq!(
            quartile(500, 100),
            4,
            "and it never runs past the top shade"
        );
        assert_eq!(
            quartile(5, 0),
            4,
            "a zero denominator does not divide by zero"
        );
    }

    #[test]
    fn a_share_bar_always_fills_its_width_and_never_hides_a_remainder() {
        // 1 + 1 + 1 = 3, and 1/3 in integer percent is 33 — so three equal
        // reasons would draw 99 and leave a sliver that is really a rounding
        // artefact. The last segment takes the remainder, so the four always
        // add to 100.
        let drops = Drops {
            before_window: 1,
            after_window: 1,
            before_open: 1,
            after_close: 0,
        };
        let svg = share_bar(drops);
        let total: u64 = svg
            .split("width=\"")
            .skip(1)
            .filter_map(|s| s.split('"').next())
            .filter_map(|s| s.parse::<u64>().ok())
            .sum();
        assert_eq!(total, 100, "the segments fill the bar exactly: {svg}");
        assert!(svg.contains("<title>"), "each segment names itself: {svg}");

        // Nothing dropped draws one flat track, not four zero-width segments.
        let none = share_bar(Drops::default());
        assert!(none.contains("class=\"none\""), "{none}");
        assert!(none.contains("nothing was dropped"), "{none}");
        assert_eq!(none.matches("<rect").count(), 1, "{none}");
    }

    #[test]
    fn the_coverage_strip_draws_one_tick_per_row_and_no_more() {
        assert_eq!(
            coverage_strip(&[]),
            "",
            "no rows is no strip, not an empty box"
        );
        let strip = coverage_strip(&[true, false, true, true]);
        assert!(strip.contains("viewBox=\"0 0 4 10\""), "{strip}");
        assert_eq!(
            strip.matches("<rect").count(),
            3,
            "one rect per HELD row, and none for the gaps: {strip}"
        );
        assert!(
            strip.contains("3 of 4 row(s) on this page are held"),
            "{strip}"
        );
        assert!(
            strip.contains("x=\"0\"") && strip.contains("x=\"2\"") && strip.contains("x=\"3\"")
        );
        assert!(!strip.contains("x=\"1\""), "row 1 is not held: {strip}");
    }

    /// An audit row, for the page that shows them.
    fn audit_row_fixture(ordinal: u64, loud: bool) -> AuditRow {
        AuditRow {
            ordinal,
            fault: None,
            when: "2025-07-01 15:31:02 IST".to_owned(),
            scope: "spot".to_owned(),
            outcome: if loud { "FAILED" } else { "STORED" }.to_owned(),
            loud,
            source: "/Users/you/Downloads/GDFL/Futures/-III".to_owned(),
            window: "2025-07-01..=2025-07-01".to_owned(),
            members: 194,
            rows_read: 354_675,
            bars_stored: 62_978,
            rows_folded: 291_527,
            counted: 194,
            drops: Drops {
                before_open: 0,
                after_close: 170,
                before_window: 0,
                after_window: 0,
            },
            failures: u64::from(loud),
            took_micros: 4_512_903,
            note: "every row accounted for".to_owned(),
        }
    }

    fn audit_view<'a>(rows: &'a [AuditRow], notes: &'a [String]) -> AuditView<'a> {
        AuditView {
            today: d(2026, 8, 7),
            journal: JOURNAL,
            rows,
            page: 0,
            last_page: 0,
            notes,
        }
    }

    #[test]
    fn the_audit_page_draws_only_the_rows_it_was_given() {
        // THE BOUND, ASSERTED AS A COUNT. The renderer draws what it is handed
        // and nothing else — two records in, two record rows out — so the page
        // cost is the page's, not the journal's. The reader half of the same
        // bound is api::audit::the_tail_reads_only_the_records_it_shows.
        let rows = [audit_row_fixture(41, false), audit_row_fixture(40, true)];
        let html = audit_page(&audit_view(&rows, &[]));
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.ends_with("</html>"));
        assert_eq!(
            html.matches("<td class=\"num\">#").count(),
            2,
            "two records in, two rows out: {html}"
        );
        assert!(html.contains("#41") && html.contains("#40"), "{html}");
        assert!(
            html.contains("2 record(s) held") || html.contains("showing 2"),
            "{html}"
        );

        // Every figure the operator asked for is on the row: what was asked,
        // what landed, what dropped and why, what failed, and how long it took.
        assert!(html.contains("2025-07-01 15:31:02 IST"), "when: {html}");
        assert!(html.contains("GDFL"), "what was asked: {html}");
        assert!(html.contains(">354675<"), "rows read: {html}");
        assert!(html.contains(">62978<"), "bars stored: {html}");
        assert!(html.contains(">291527<"), "rows folded: {html}");
        assert!(html.contains("4.512 s"), "how long: {html}");
        assert!(html.contains("170 row(s) dropped"), "what dropped: {html}");
        assert!(html.contains("1 member(s) failed"), "what failed: {html}");
        assert!(
            html.contains("class=\"share\""),
            "and by which share: {html}"
        );

        // A loud outcome is loud, a calm one is not, and the badge follows.
        assert!(html.contains("class=\"verdict loud\">FAILED"), "{html}");
        assert!(html.contains("class=\"verdict calm\">STORED"), "{html}");
        assert!(html.contains("ATTENTION"), "{html}");
        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }

    #[test]
    fn a_refused_record_takes_one_row_and_leaves_the_others_alone() {
        let mut damaged = audit_row_fixture(7, false);
        damaged.fault = Some("record checksum 0x00000001 != 0x00000002".to_owned());
        damaged.loud = true;
        let rows = [audit_row_fixture(8, false), damaged];
        let html = audit_page(&audit_view(&rows, &[]));
        assert!(html.contains("RECORD REFUSED"), "{html}");
        assert!(
            html.contains("record checksum"),
            "with both numbers: {html}"
        );
        assert!(
            html.contains("#8"),
            "the row beside it still renders: {html}"
        );
        assert_eq!(
            html.matches("RECORD REFUSED").count(),
            1,
            "one damaged record is one damaged row: {html}"
        );
    }

    #[test]
    fn an_empty_audit_page_says_nothing_was_recorded_and_shows_no_table() {
        let html = audit_page(&audit_view(&[], &[]));
        assert!(html.contains("nothing recorded yet"), "{html}");
        assert!(
            html.contains("not because a figure is missing"),
            "empty is not the same as unmeasured, and the page says so: {html}"
        );
        assert!(!html.contains("<tbody>"), "{html}");
        assert!(
            html.contains("badge good"),
            "an empty log is not a fault: {html}"
        );
    }

    #[test]
    fn the_pager_is_one_function_and_both_directions_are_reachable() {
        assert_eq!(pager("/audit", 0, 0), "", "one page needs no pager");
        let first = pager("/audit", 0, 2);
        assert!(!first.contains("previous"), "{first}");
        assert!(first.contains("/audit?page=1"), "{first}");
        assert!(first.contains("page 1 of 3"), "{first}");
        let middle = pager("/store", 1, 2);
        assert!(middle.contains("/store?page=0"), "{middle}");
        assert!(middle.contains("/store?page=2"), "{middle}");
        assert!(middle.contains("page 2 of 3"), "{middle}");
        let last = pager("/store", 2, 2);
        assert!(last.contains("/store?page=1"), "{last}");
        assert!(!last.contains("next"), "{last}");
    }

    #[test]
    fn a_receipt_states_what_the_server_read_and_that_nothing_ran() {
        let facts = [
            ("Target", "Swept indices".to_owned()),
            ("toDate on the wire", "2022-02-09".to_owned()),
        ];
        let html = receipt_page(&Receipt {
            scope: "Spot pull",
            verdict: "NOT STARTED",
            reason: "there is no HTTP transport",
            good: false,
            facts: &facts,
            footnote: "Nothing here was written to the store.",
        });
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("NOT STARTED"));
        assert!(html.contains("Spot pull"));
        assert!(html.contains("there is no HTTP transport"));
        assert!(html.contains("<th>Target</th><td>Swept indices</td>"));
        assert!(html.contains("2022-02-09"));
        assert!(html.contains("Nothing here was written to the store"));
        assert!(html.contains("href=\"/pull\""), "a way back: {html}");
        assert!(
            html.contains("href=\"/audit\""),
            "and to the record: {html}"
        );
        assert!(html.contains("badge bad"), "nothing ran: {html}");
        assert!(!html.contains("class=\"halt good\""), "{html}");

        // A RUN THAT STORED IS BADGED AS ONE, AND SAYS SO. Both halves used to
        // be hardcoded: `badge bad` on every receipt, and the closing sentence
        // "Nothing here was written to the store" under a run that wrote 194
        // bar files.
        let facts = [("Bars stored", "62978".to_owned())];
        let html = receipt_page(&Receipt {
            scope: "Spot pull",
            verdict: "STORED",
            reason: "every row is accounted for",
            good: true,
            facts: &facts,
            footnote: "The bars named above ARE on the store root named above.",
        });
        assert!(html.contains("badge good"), "{html}");
        assert!(html.contains("class=\"halt good\""), "{html}");
        assert!(!html.contains("Nothing here was written"), "{html}");
        assert!(html.contains("ARE on the store root"), "{html}");

        // A refusal is escaped like everything else.
        let facts = [("Field", "<b>x</b>".to_owned())];
        let html = receipt_page(&Receipt {
            scope: "<i>s</i>",
            verdict: "REFUSED",
            reason: "M&M",
            good: false,
            facts: &facts,
            footnote: "<em>nothing</em>",
        });
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(html.contains("&lt;i&gt;s&lt;/i&gt;"));
        assert!(html.contains("M&amp;M"));
        assert!(html.contains("&lt;em&gt;nothing&lt;/em&gt;"), "{html}");
        assert!(!html.contains("<td><b>"));
    }

    #[test]
    fn the_store_page_marks_a_held_month_and_dashes_one_it_does_not_hold() {
        let nifty_key = crate::census::Series {
            exchange: Exchange::Nse,
            segment: brutex_core::instrument::Segment::Index,
            symbol: brutex_core::symbol::Symbol::new("NIFTY").expect("valid"),
            timeframe: store::path::Timeframe::MINUTE_1,
        };
        let month = store::path::YearMonth::new(2026, 7).expect("valid");
        let held = Coverage {
            series: nifty_key,
            month,
            rows: vec![(Vendor::Groww, Some(8_250)), (Vendor::Dhan, None)],
        };
        let absent = Coverage {
            series: nifty_key,
            month: store::path::YearMonth::new(2026, 6).expect("valid"),
            rows: vec![(Vendor::Groww, None), (Vendor::Dhan, None)],
        };
        let rows = [held, absent];
        let html = store_page(&StoreView {
            today: d(2026, 8, 7),
            censuses: &[],
            rows: &rows,
            page: 0,
            last_page: 0,
            total: 2,
            notes: &["store root: /tmp/x".to_owned()],
            // This fixture is about the TABLE, not the filter bar, so it
            // renders without one — `filter: None` is what a page with no bar
            // looks like, and the assertions below stay about the rows.
            filter: None,
            held: 1,
            held_only: false,
        });
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.ends_with("</html>"));
        assert!(html.contains("NSE-INDEX-NIFTY"));
        assert!(html.contains("2026-07") && html.contains("2026-06"));
        assert!(html.contains(">8250<"), "the row count: {html}");
        // CHANGED, AND NAMED: every cell now carries a swatch BEFORE its
        // number, so the not-held cell is `class="num miss"` followed by the
        // hollow swatch and then the dash. The assertion moved from one string
        // to the two facts it was standing for.
        assert!(html.contains("class=\"num miss\">"), "{html}");
        assert!(html.contains("class=\"sw void\""), "{html}");
        assert!(html.contains(">—</td>"), "{html}");
        assert_eq!(
            html.matches("<tr class=\"swept\">").count(),
            1,
            "exactly one of the two months is held"
        );
        assert_eq!(
            html.matches("<tr class=\"void\">").count(),
            1,
            "and the other is tinted as not held rather than left unmarked"
        );
        // THE POINT OF THE WHOLE CHANGE: held and not-held differ in SHAPE, so
        // the grid is readable without reading a number. One filled square, one
        // outlined square with a stroke through it.
        assert_eq!(html.matches("class=\"sw q").count(), 1, "{html}");
        assert_eq!(html.matches("class=\"sw void\"").count(), 3, "{html}");
        // And the page says what the shades are relative to, because there is
        // no trading calendar in this build to say what a full month is.
        assert!(html.contains("1 of 2</b> shown row(s) are held"), "{html}");
        assert!(html.contains("quartiles of 8250"), "{html}");
        assert!(html.contains("class=\"strip\""), "the glance strip: {html}");
        assert!(html.contains("2 instrument-month(s) in the grid"));
        assert!(html.contains("showing 2"));
        assert!(!html.contains("class=\"pager\""), "one page, no pager");
        assert!(html.contains("badge good"), "no census is not a bad census");
        assert!(html.contains("groww rows") && html.contains("dhan rows"));
        // The claim the whole page rests on is written on it.
        assert!(html.contains("hash probe"), "{html}");
        assert!(html.contains("248,000"), "{html}");
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
