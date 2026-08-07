//! The operator's window onto the store, and the process that serves it.
//!
//! Everything the binary does lives here rather than in `main.rs`, so that
//! every branch of it is reachable from a test. `main.rs` holds one call and
//! nothing else — a line of logic in a `main` is a line no unit test can enter,
//! and `docs/04-invariants.md` X-06 does not exempt binaries.
//!
//! # What the page is for
//!
//! It shows what decoded, what was declined and — crucially — what could not
//! be read at all. A refused row is the thing that hides an instrument, so it
//! is on the page rather than in a log. Since the equity gate landed it also
//! shows every ISIN two vendors disagree about, because a disagreement nobody
//! sees decides the run on its own.

use crate::catalog::{Catalog, PAGE_ROWS, Selection};
use crate::{census, ingest, master, merge, render};
use brutex_core::instrument::InstrumentKey;
use brutex_core::vendor::Vendor;
use pull::session::Day;
use std::fmt::Write as _;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The default listen address when none is given.
///
/// Loopback, not `0.0.0.0`: this page is an operator's window onto a local
/// store, and a default that listens on every interface is a decision nobody
/// took.
pub const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);

/// The longest request body either form may send.
///
/// # Why the number is small and why it is stated
///
/// The two forms are five short fields between them: a target slug, a series
/// slug, an underlying symbol of at most `brutex_core::symbol::SYMBOL_CAPACITY`
/// bytes, and two ten-character dates. Percent-encoding is at worst 3× per
/// byte. 8 KiB is roughly 80× the largest honest body and still small enough
/// that a body over it is unambiguously not a form this server serves.
///
/// It is stated because a bound nobody wrote down is a bound nobody owns.
/// `axum` applies a 2 MiB default, so a 2 MiB `to_uppercase()` of a field that
/// `Symbol::new` then refuses for being over 24 bytes was reachable, and the
/// only thing standing between this server and that allocation was a
/// dependency's default value. A request past this answers `413` — the refusal
/// is the framework's and it is loud, not a truncation.
const MAX_FORM_BYTES: usize = 8 * 1024;

/// The signal that stops the server.
///
/// A boxed future rather than a type parameter, deliberately. A generic
/// `serve` is a *different function* for every caller — one for the binary's
/// ctrl-C, one for each test's ready future — and coverage is accounted per
/// instantiation, so the binary's copy would be permanently short of the arms
/// only a test exercises. One allocation per process buys one function with
/// one set of counters. The same reasoning applies to the argument list below.
pub type Shutdown = std::pin::Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>;

/// What the operator asked the binary to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Bind and serve until the shutdown signal arrives.
    Serve(SocketAddr),
    /// Print the decode tallies for every master and exit.
    Report,
}

impl Command {
    /// Parses the command line.
    ///
    /// # Errors
    ///
    /// A usage message naming what was not understood. An unrecognised
    /// argument is a refusal rather than a fallback to serving: a typo that
    /// silently starts a server is a typo nobody finds.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut args = args.iter().map(String::as_str);
        match (args.next(), args.next()) {
            // No command at all serves, because that is what the operator has
            // always typed. A WRONG command does not: see the last arm.
            (None, _) | (Some("serve"), None) => Ok(Self::Serve(DEFAULT_ADDR)),
            (Some("serve"), Some(addr)) => addr
                .parse()
                .map(Self::Serve)
                .map_err(|e| format!("not a socket address: {addr:?} ({e})")),
            (Some("report"), None) => Ok(Self::Report),
            (Some(other), _) => Err(format!(
                "unknown argument {other:?}; usage: api [serve [ADDR] | report]"
            )),
        }
    }
}

/// Where each vendor's master is expected.
#[must_use]
pub fn master_paths(dir: &Path) -> Vec<(Vendor, PathBuf)> {
    vec![
        (Vendor::Groww, dir.join("groww_instruments.csv")),
        (Vendor::Dhan, dir.join("dhan_scrip.csv")),
    ]
}

/// The directory the masters are read from.
///
/// `BRUTEX_MASTERS`, or `$HOME/.brutex/masters`. This is the only place the
/// environment is consulted; everything below takes the directory as an
/// argument, so nothing else has to touch process-wide state to be
/// deterministic — and nothing can, since setting an environment variable is
/// `unsafe` under edition 2024 and this crate forbids `unsafe`.
#[must_use]
pub fn masters_dir() -> PathBuf {
    masters_dir_from(std::env::var_os("BRUTEX_MASTERS"))
}

/// The masters directory implied by a value of `BRUTEX_MASTERS`.
///
/// Split from [`masters_dir`] so both outcomes are testable without mutating
/// the environment of a process running tests in parallel.
#[must_use]
fn masters_dir_from(value: Option<std::ffi::OsString>) -> PathBuf {
    value.map_or_else(default_masters_dir, PathBuf::from)
}

/// The directory the bar store and its manifests are read from.
///
/// `BRUTEX_STORE`, or `$HOME/.brutex/store`. Split exactly as
/// [`masters_dir`] is, and for the same reason: the environment is consulted in
/// one place and every function below takes the directory as an argument.
#[must_use]
pub fn store_dir() -> PathBuf {
    store_dir_from(std::env::var_os("BRUTEX_STORE"), std::env::var_os("HOME"))
}

/// The store directory implied by values of `BRUTEX_STORE` and `HOME`.
///
/// Both outcomes have to be testable and a test cannot set either variable:
/// `set_var` is `unsafe` under edition 2024, this crate forbids `unsafe`, and
/// mutating process-wide state would race every other test in the binary.
#[must_use]
fn store_dir_from(value: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> PathBuf {
    value.map_or_else(
        || {
            home.map_or_else(
                || PathBuf::from("."),
                |h| PathBuf::from(h).join(".brutex").join("store"),
            )
        },
        PathBuf::from,
    )
}

/// Where the masters live when `BRUTEX_MASTERS` says nothing.
///
/// `$HOME/.brutex/masters`, not the working directory. The masters are ~50 MB
/// of vendor CSV and `.csv` is not an allowed tracked extension (CLAUDE.md §2),
/// so they can never live in the repository — which means "the working
/// directory" is only ever right when the operator happens to have `cd`-ed
/// somewhere specific. Launching from anywhere else found no files and rendered
/// `UNAVAILABLE`, correctly reporting a real absence caused entirely by the
/// default.
///
/// Falls back to `.` only if `HOME` is unset, which is a broken environment
/// rather than a supported one; the page then says `UNAVAILABLE` and names the
/// vendor, as it does for any missing file.
fn default_masters_dir() -> PathBuf {
    default_masters_dir_from(std::env::var_os("HOME"))
}

/// The default implied by a value of `HOME`.
///
/// Split for the same reason [`masters_dir_from`] is: both outcomes have to be
/// testable, and a test cannot unset `HOME` — `set_var` is `unsafe` under
/// edition 2024, this crate forbids `unsafe`, and mutating process-wide state
/// would race every other test in the binary.
#[must_use]
fn default_masters_dir_from(home: Option<std::ffi::OsString>) -> PathBuf {
    home.map_or_else(
        || PathBuf::from("."),
        |home| PathBuf::from(home).join(".brutex").join("masters"),
    )
}

/// What one read of the masters produced: the universe, the notes, and whether
/// any of it may be believed.
#[derive(Debug)]
pub struct Read {
    /// One entry per distinct instrument.
    pub merged: merge::Merged,
    /// Everything an operator has to be told: per-vendor tallies, every
    /// decline reason, every unreadable row's reason, and every disagreement.
    pub notes: Vec<String>,
    /// Whether a vendor was never read at all.
    ///
    /// Distinct from a merge conflict, and tracked separately because "the two
    /// vendors disagree" and "there was only one vendor" are different facts
    /// that must not collapse into one status.
    pub unavailable: bool,
    /// How many rows were declined under a listing class nobody recognises.
    ///
    /// Not a merge disagreement — it is one vendor's file using a code this
    /// engine has never measured, which is how an alphabet moves under a
    /// gate. Counted here so it reaches the status and the exit code, because
    /// the reason string alone sat beside six routine ones and read like them.
    pub unrecognised: usize,

    /// How many rows no decoder could read, across every vendor.
    ///
    /// Carried as a field because `is_clean` must consult it, and a count
    /// folded into a note string is not consultable.
    pub unreadable: usize,
    /// Every ordering and every filter the pages offer, decided once.
    ///
    /// The reason it is here and not built per request is `docs/05-decisions.md`
    /// D-0042: the masters are parsed once into an `Arc<Site>`, and that parse
    /// is the only place a whole-universe pass may happen.
    pub catalog: Catalog,
}

impl Read {
    /// Builds a read, computing everything a request must not compute.
    ///
    /// Struct-literal construction is deliberately not the way in: the
    /// precomputed [`Catalog`] is the whole of D-0042, and a caller that filled
    /// the fields by hand would get a page that silently went back to scanning.
    #[must_use]
    pub fn new(
        merged: merge::Merged,
        notes: Vec<String>,
        unavailable: bool,
        unrecognised: usize,
        unreadable: usize,
    ) -> Self {
        let catalog = Catalog::build(&merged);
        Self {
            merged,
            notes,
            unavailable,
            unrecognised,
            unreadable,
            catalog,
        }
    }

    /// Whether this read is fit to be believed.
    ///
    /// A missing vendor counts. So does any disagreement. So does a listing
    /// class nobody recognises — that one is the difference between a routine
    /// bond and an alphabet moving under us.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        // `errors` is load-bearing here and was missing, which made this the
        // worst shape of bug this repository hunts: a plausible wrong answer.
        //
        // A permutation audit traced the path. If BOTH master files fail to
        // decode entirely, no row ever reaches the series gate, so
        // `unrecognised` stays 0; nothing is kept, so there is nothing to
        // disagree about and `merged.verdict()` is `Clean`; the files were
        // found, so `unavailable` is false. Every term was true and
        // `status()` answered "ok" over a universe of zero instruments.
        //
        // A monitor reading one word would have seen a healthy server. The
        // decode failures were never hidden — `errors_by_reason` renders them
        // on the page — but the machine-readable word did not consult them,
        // which is `CLAUDE.md` §4's "fallback that hides a failure" arriving
        // by omission rather than by design.
        //
        // `api::unit::a_total_decode_failure_is_not_clean` is what holds it up.
        self.unreadable == 0
            && !self.unavailable
            && self.unrecognised == 0
            && self.merged.verdict() == merge::Verdict::Clean
    }

    /// The one-word status a machine reads first.
    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.is_clean() { "ok" } else { "DEGRADED" }
    }
}

/// Every vendor's kept listings, merged, plus one note per vendor.
///
/// A vendor whose file is missing produces a note saying so and no listings.
/// It does not produce an empty success: `UNAVAILABLE` on the page is the
/// difference between "this vendor lists nothing" and "this vendor was never
/// read", and collapsing the two is exactly the silent degradation
/// `CLAUDE.md` §4 forbids.
#[must_use]
pub fn universe(dir: &Path) -> Read {
    let mut notes = Vec::new();
    let mut sources = Vec::new();
    let mut unavailable = false;
    let mut unrecognised = 0;
    let mut unreadable = 0;
    for (vendor, path) in master_paths(dir) {
        match master::load(&path, vendor) {
            Ok(l) => {
                let mut note = format!(
                    "{}: {} kept, {} declined, {} unreadable",
                    vendor.as_str(),
                    l.kept.len(),
                    l.skipped_total(),
                    l.errors.len()
                );
                unreadable += l.errors.len();
                for (reason, n) in l.skipped_by_reason() {
                    let _ = write!(note, " · {reason} {n}");
                }
                notes.push(note);
                // AN UNREADABLE ROW SAYS WHY, AND WHERE. The reasons were
                // collected with their line numbers and then read only for
                // `.len()`, so `104 unreadable` was the whole of what an
                // operator was told and the cause had to be grepped out of the
                // raw CSV. Grouped by reason, so the output is bounded without
                // any reason being hidden.
                for (reason, n, first) in l.errors_by_reason() {
                    notes.push(format!(
                        "{} UNREADABLE · {reason} ×{n}, first at line {first}",
                        vendor.as_str()
                    ));
                }
                // The COUNT says an alphabet moved; the CODE says which one.
                for (code, n) in &l.unrecognised {
                    unrecognised += n;
                    notes.push(format!(
                        "{} UNRECOGNISED LISTING CLASS · {code:?} ×{n} — \
                         this is not a bond, it is a code this engine has never seen",
                        vendor.as_str()
                    ));
                }
                sources.push(merge::Source {
                    vendor,
                    kept: l.kept,
                    declined: l.declined,
                });
            }
            Err(e) => {
                unavailable = true;
                notes.push(format!("{}: UNAVAILABLE — {e}", vendor.as_str()));
            }
        }
    }
    let merged = merge::merge(&sources);
    for line in &merged.conflicts {
        notes.push(format!("ISIN CONFLICT · {line}"));
    }
    for line in &merged.eligibility {
        notes.push(format!("ELIGIBILITY CONFLICT · {line}"));
    }
    for (name, present, both) in merged.universe_census() {
        notes.push(format!(
            "{name}: {present} resolved, {both} confirmed by both vendors"
        ));
    }
    // An index carries no ISIN, so nothing cross-checks its identity. Saying
    // which members rest on one vendor is the only honest substitute.
    let alone = merged.single_vendor_members();
    if !alone.is_empty() {
        notes.push(format!(
            "UNCHECKED IDENTITY · {} universe member(s) named by one vendor only: {}",
            alone.len(),
            alone.join(", ")
        ));
    }
    Read::new(merged, notes, unavailable, unrecognised, unreadable)
}

/// Liveness plus the decode tallies, so a machine can check what a human sees.
///
/// The first line is `ok` or `DEGRADED`, and it is not decoration: it used to
/// be an unconditional `ok` printed beside `dhan: UNAVAILABLE`, so a monitor
/// reading the status, the exit code or the HTTP code saw green while one of
/// the two vendors had never been read.
#[must_use]
pub fn report(dir: &Path) -> (String, bool) {
    report_from(&universe(dir))
}

/// The decode report, from a universe that is **already loaded**.
///
/// Split for the same reason as [`instruments_html_from`]: `/health` is what a
/// monitor polls, and polling it must not re-parse both masters. See that
/// function for the measured cost.
#[must_use]
pub fn report_from(read: &Read) -> (String, bool) {
    let mut out = format!("{}\n", read.status());
    for note in &read.notes {
        // Writing to a String is infallible; `unwrap_used` and `expect_used`
        // are denied workspace-wide and neither belongs here.
        let _ = writeln!(out, "{note}");
    }
    let _ = writeln!(
        out,
        "merged: {} instruments, {} isin conflicts, {} eligibility conflicts",
        read.merged.len(),
        read.merged.conflicts.len(),
        read.merged.eligibility.len()
    );
    (out, read.is_clean())
}

/// Parses `?q=...` without a query-string dependency.
///
/// `+` and `%XX` are decoded because a symbol may be typed with spaces.
/// Anything undecodable is kept literally rather than dropped — a query must
/// never silently become a different query.
#[must_use]
pub fn parse_query(raw: &str) -> String {
    param(raw, "q")
}

/// One named parameter out of a query string, decoded.
///
/// Named rather than positional because the page now carries two — the search
/// text and the sort column — and reading the second by position would make
/// `?sort=isin&q=NIFTY` mean something different from `?q=NIFTY&sort=isin`.
#[must_use]
pub fn param(raw: &str, name: &str) -> String {
    let prefix = format!("{name}=");
    for pair in raw.split('&') {
        if let Some(v) = pair.strip_prefix(prefix.as_str()) {
            return percent_decode(v);
        }
    }
    String::new()
}

/// The zero-based page number a query string asks for.
///
/// Anything unparseable is page one. This is the one parameter that is
/// defaulted rather than refused, and it is defensible only because a page
/// number selects a VIEW: it can never change what the data says, so a mangled
/// bookmark should land on the first page rather than on an error.
#[must_use]
pub fn page_number(raw: &str) -> usize {
    param(raw, "page").parse().unwrap_or(0)
}

/// Decodes `+` and `%XX` escapes.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    // `while let` rather than `while i < len` with an indexed read: the
    // indexed form needs a `None` arm that cannot happen, and a branch no test
    // can enter is a branch nobody has checked.
    while let Some(&c) = b.get(i) {
        match c {
            b'+' => out.push(b' '),
            b'%' => match hex_escape(b, i) {
                Some(byte) => {
                    out.push(byte);
                    i += 2;
                }
                // A truncated or non-hex escape is kept LITERALLY. Dropping it
                // would turn one query into a different query in silence.
                None => out.push(b'%'),
            },
            _ => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte a `%XX` at `at` encodes, or `None` if it is not one.
fn hex_escape(b: &[u8], at: usize) -> Option<u8> {
    let hi = hex_digit(*b.get(at + 1)?)?;
    let lo = hex_digit(*b.get(at + 2)?)?;
    // At most 15 * 16 + 15 = 255, so this cannot overflow a byte.
    Some(hi * 16 + lo)
}

/// The value of one hexadecimal digit, in either case.
fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Renders the instruments page for one query.
///
/// # Why the notes are not part of the title
///
/// They were, and only when the query was empty — the search branch built a
/// title from the query alone and dropped `notes` on the floor. `notes` is the
/// sole carrier of `<vendor>: UNAVAILABLE` and of every conflict line, so
/// typing anything into the search box made the page stop saying that a
/// vendor's master had never been read, and a row rendered with one vendor tag
/// was byte-identical to a genuine single-vendor listing. With `PAGE_ROWS` at
/// 200 against thousands of instruments, search is the only way to reach most
/// of them, so that was not a corner case — it was the normal path. The notes
/// are now a banner the page always carries, whatever was typed.
#[must_use]
pub fn instruments_html(dir: &Path, query: &str) -> String {
    instruments_html_from(&universe(dir), query, "", false, "", 0)
}

/// The instruments page, rendered from a universe that is **already loaded**.
///
/// # Why this split exists
///
/// [`instruments_html`] reads both masters from disk on every call. Serving a
/// page through it parsed ~200,000 CSV rows to render 200 of them, and
/// measured **150 ms per request** against the real files — cost proportional
/// to the size of the masters, on a path `CLAUDE.md` §3 rule 4 requires to be
/// constant.
///
/// No test caught it. A fixture of four rows parses in microseconds and passes
/// with 100% coverage; only the 50 MB file shows the shape of the curve. That
/// is the difference between *covered* and *correct*.
///
/// The masters are therefore read **once**, at startup, and every request
/// renders from that. Re-reading is now an explicit operator action rather
/// than a side effect of looking at the page.
#[must_use]
pub fn instruments_html_from(
    read: &Read,
    query: &str,
    sort: &str,
    all: bool,
    universe_filter: &str,
    page: usize,
) -> String {
    let needle = query.to_uppercase();

    // THE TRACKED UNIVERSE, and nothing else.
    //
    // The masters carry every NSE listing the gate accepts as a real share --
    // about 2,700 per vendor. The engine tracks a strict subset: the NIFTY
    // Total Market constituents, plus the index series. The other ~1,900 NSE
    // equities are decoded, counted and declined here rather than in the
    // decoder, because "is this a share" and "is this a share I follow" are
    // different questions and collapsing them would mean anything outside the
    // list never got validated at all.
    //
    // F&O needs no clause: every F&O stock underlying is already a Total
    // Market constituent -- 208 of 208, measured, zero outside. F&O is a
    // filter label on instruments already here, not a third source of rows.
    // `all` lifts the universe filter so every listing the gate accepted is
    // reachable. The DEFAULT is the tracked universe, because a page opening on
    // 2,700 rows hides the 785 that matter — but a filter with no way past it
    // hides a bug instead, so the escape hatch is a link on the page, not a
    // recompile.
    //
    // EVERY ONE OF THOSE DECISIONS IS MADE AT LOAD TIME, not here. The scope,
    // the pill and the ordering pick 1 of 48 lists that already exist; the pill
    // COUNTS are four `usize` reads. What this function used to do -- fold the
    // whole map, filter it into a fresh `Vec`, sort that vector and reverse it,
    // on every request, to draw a fixed 200 rows -- is `docs/05-decisions.md`
    // D-0042 and it is gone. `crates/api/benches/ratio.rs` asserts the shape.
    let selection = Selection::new(all, universe_filter, sort);
    let counts = read.catalog.counts(selection);
    let total = counts.all;

    // PAGING, so nothing is unreachable.
    //
    // The cap alone rendered 200 of 785 and offered no way to the rest —
    // scrolling cannot reveal rows that were never sent, and the page said
    // "showing 200" as though that were the whole answer. A bound with no
    // navigation past it is the same class of defect as a filter with no
    // escape hatch.
    //
    // The offset is clamped to the last page rather than refused: `?page=999`
    // is a stale bookmark, not an attack, and it should land somewhere real.
    //
    // SEARCH IS THE ONE PATH THAT STILL LOOKS AT ROWS IT WILL NOT DRAW, and it
    // says so: `Catalog::search` narrows by a trigram index and names the two
    // cases that stay linear. `docs/06-limits.md` §24 carries the measurement.
    let view = if needle.is_empty() {
        read.catalog.page(selection, page)
    } else {
        read.catalog.search(selection, &needle, page)
    };
    let (rows, matched, page, last_page) = (view.rows, view.matched, view.page, view.last_page);

    // `total` is the whole universe; `matched` is what the filter selected.
    // Reporting the right one keeps the page honest about what it looked at.
    let (title, denominator) = if needle.is_empty() {
        (format!("brutex · instruments · {}", read.status()), total)
    } else {
        (
            format!(
                "brutex · {} · search {query:?} · {matched} matched",
                read.status()
            ),
            matched,
        )
    };
    render::instruments_page(&render::View {
        title: &title,
        total: denominator,
        rows: &rows,
        query,
        sort,
        all,
        counts,
        active: universe_filter,
        page,
        last_page,
        notes: &read.notes,
    })
}

/// The dashboard.
///
/// Every figure is a counter already in memory. Nothing here scans, so the page
/// costs the same whether the store holds two instruments or two hundred
/// thousand — which is the whole point of `docs/04-invariants.md` C-01.
async fn home(
    axum::extract::State(site): axum::extract::State<Loaded>,
) -> axum::response::Html<String> {
    axum::response::Html(dashboard_html(&site.read))
}

/// The dashboard, from a universe already loaded.
#[must_use]
pub fn dashboard_html(read: &Read) -> String {
    // TWO WHOLE-MAP SCANS USED TO BE HERE, under a docstring that already
    // claimed "nothing here scans". Measured before D-0042: 1,720 ns at 2
    // instruments, 138,702 ns at 50,000 — an 80× curve under a comment saying
    // it was flat. Both are now counters computed once at load.
    let (counts, both) = read.catalog.dashboard_counts();
    let disputes = read.merged.conflicts.len() + read.merged.eligibility.len();

    let (all, fno, ntm, idx) = (counts.all, counts.fno, counts.ntm, counts.index);
    let n = |v: usize| v.to_string();
    let stats = [
        render::Stat {
            label: "Tracked",
            value: &n(all),
            note: "NIFTY Total Market + indices",
            loud: false,
        },
        render::Stat {
            label: "NIFTY Total Market",
            value: &n(ntm),
            note: "constituents",
            loud: false,
        },
        render::Stat {
            label: "F&O underlyings",
            value: &n(fno),
            note: "all inside Total Market",
            loud: false,
        },
        render::Stat {
            label: "Indices",
            value: &n(idx),
            note: "NSE index series",
            loud: false,
        },
        render::Stat {
            label: "Confirmed by both feeds",
            value: &n(both),
            note: "cross-checked identity",
            loud: false,
        },
        render::Stat {
            label: "Disagreements",
            value: &n(disputes),
            note: "identity + eligibility",
            loud: disputes > 0,
        },
    ];
    render::dashboard_page(read.status(), &stats, &read.notes)
}

/// The instruments page.
async fn page(
    axum::extract::State(site): axum::extract::State<Loaded>,
    uri: axum::http::Uri,
) -> axum::response::Html<String> {
    let raw = uri.query().unwrap_or("");
    let typed = parse_query(raw);
    let sort = param(raw, "sort");
    let all = param(raw, "all") == "1";
    let u = param(raw, "u");
    let page = page_number(raw);
    axum::response::Html(instruments_html_from(
        &site.read, &typed, &sort, all, &u, page,
    ))
}

/// The health endpoint.
///
/// 200 only when the read is clean. A degraded universe answers 503, because a
/// monitor reads the status code and nothing else — this used to return 200
/// with `ok` on the first line while a vendor had never been read.
async fn health(
    axum::extract::State(site): axum::extract::State<Loaded>,
) -> (axum::http::StatusCode, String) {
    let (body, clean) = report_from(&site.read);
    let code = if clean {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (code, body)
}

/// Why no vendor call can be made in this build.
///
/// `CLAUDE.md` §4 permits degrading loudly and naming the reason, or refusing.
/// This is the reason, written once and rendered wherever a counter would
/// otherwise have to invent a number. **It names the modules that are absent**,
/// so an operator can check the claim rather than take it.
///
/// When `pull::fetch` and `pull::rate` land, this constant is what stops being
/// passed to [`render::PullView::halt`] — one place, not a search.
pub const CAPTURE_UNAVAILABLE: &str = "crates/pull exposes no vendor fetch and \
     no rate governor in this build: there is no pull::fetch and no pull::rate, \
     and docs/04-invariants.md P-01 through P-04 still stand at '—'. Every \
     capture counter therefore reads '—' and not '0' — nothing has been \
     measured, and a zero would be a claim that it had. A request is still \
     parsed, validated and echoed back with the exact dates that would go on \
     the wire, and is then REFUSED with 503. No vendor is contacted and nothing \
     is written.";

/// Everything every request renders from, read once at startup.
///
/// `Arc` rather than a clone per request: [`Read`] owns a `HashMap` of every
/// instrument, and cloning it per request would trade one O(rows) cost for
/// another. An `Arc` clone is a refcount bump — constant, and the same bytes
/// are shared by every concurrent request.
///
/// Shared **immutably**, so there is no lock on the read path and no
/// contention that grows with concurrent readers.
#[derive(Debug)]
pub struct Site {
    /// The instrument universe, merged from both masters.
    pub read: Read,
    /// One manifest census per vendor, in [`Vendor::ALL`] order.
    pub censuses: Vec<census::VendorCensus>,
    /// The instruments the coverage grid is built over, sorted.
    pub indices: Vec<InstrumentKey>,
    /// How many instruments each spot target covers, in
    /// [`ingest::SpotTarget::ALL`] order.
    ///
    /// Counted once, here, rather than folded over the universe per request:
    /// the form shows a real number and the page still costs O(rows shown).
    pub targets: [usize; 3],
    /// Where the manifests were read from, named on the page so an absence is
    /// actionable rather than mysterious.
    pub store_root: PathBuf,
}

impl Site {
    /// Assembles the derived counts once, from parts a caller already holds.
    #[must_use]
    pub fn new(read: Read, censuses: Vec<census::VendorCensus>, store_root: PathBuf) -> Self {
        let mut targets = [0usize; 3];
        for (key, entry) in &read.merged.by_key {
            for (slot, target) in ingest::SpotTarget::ALL.into_iter().enumerate() {
                let counted = match target {
                    ingest::SpotTarget::Swept => key.is_sweepable(),
                    _ => entry.universe.contains(target.universe()),
                };
                if counted && let Some(n) = targets.get_mut(slot) {
                    *n += 1;
                }
            }
        }
        // THE GRID FALLS BACK TO THE TWO SWEPT SERIES WHEN THERE IS NO
        // UNIVERSE, and that is not a fallback that hides anything: a missing
        // master is already `UNAVAILABLE` in `read.notes`, which the store page
        // renders. Showing an empty grid instead would hide the two instruments
        // whose coverage is the entire point of the page.
        let mut indices = census::grid_instruments(read.merged.by_key.keys());
        if indices.is_empty() {
            indices = census::swept_instruments();
        }
        Self {
            read,
            censuses,
            indices,
            targets,
            store_root,
        }
    }

    /// The whole site, read off disk once.
    #[must_use]
    pub fn load(masters: &Path, store_root: &Path) -> Self {
        Self::new(
            universe(masters),
            census::read_all(store_root),
            store_root.to_path_buf(),
        )
    }
}

/// The shared, immutable site every handler renders from.
pub type Loaded = std::sync::Arc<Site>;

/// Answers with `f`'s page when the clock names a day, and with a named
/// refusal when it does not.
///
/// # Why the day arrives as an argument
///
/// The same reason [`run_in`] takes a directory and [`masters_dir_from`] takes
/// a value: a branch that only a broken machine clock could enter is a branch
/// no test can hold, and an untestable arm inside three handlers is three arms
/// nobody has checked. Every page that needs today goes through here, so the
/// refusal is written once and both of its arms are reachable from a test.
fn dated<F>(
    today: Result<Day, ingest::Refusal>,
    scope: &'static str,
    f: F,
) -> (axum::http::StatusCode, String)
where
    F: FnOnce(Day) -> (axum::http::StatusCode, String),
{
    match today {
        Ok(day) => f(day),
        Err(why) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            refusal_html(scope, &why),
        ),
    }
}

/// The ingest page.
///
/// GET, and GET does nothing but render. Starting a pull is a POST to
/// `/pull/spot` or `/pull/fno`, so a crawler, a refresh or a back button
/// cannot begin one.
async fn pull_get(
    axum::extract::State(site): axum::extract::State<Loaded>,
) -> (axum::http::StatusCode, axum::response::Html<String>) {
    let (code, body) = dated(ingest::today_ist(), "Ingest", |today| {
        (axum::http::StatusCode::OK, pull_html(&site, today))
    });
    (code, axum::response::Html(body))
}

/// The ingest page, from a site already loaded.
#[must_use]
pub fn pull_html(site: &Site, today: Day) -> String {
    let targets: Vec<(ingest::SpotTarget, usize)> = ingest::SpotTarget::ALL
        .into_iter()
        .enumerate()
        .map(|(i, t)| (t, site.targets.get(i).copied().unwrap_or(0)))
        .collect();
    render::pull_page(&render::PullView {
        today,
        targets: &targets,
        // NOT A FABRICATED PROGRESS BAR. There is nothing running, so there is
        // nothing to report, and the page says so where the numbers would be.
        capture: None,
        halt: Some(CAPTURE_UNAVAILABLE),
        notes: &site.read.notes,
    })
}

/// The page a refused request answers with.
#[must_use]
pub fn refusal_html(scope: &str, why: &ingest::Refusal) -> String {
    let facts = [(
        "Outcome",
        "nothing was requested, no vendor was contacted, and nothing was written".to_owned(),
    )];
    render::receipt_page(&render::Receipt {
        scope,
        verdict: "REFUSED",
        reason: &why.to_string(),
        facts: &facts,
    })
}

/// The page a valid request answers with, given that nothing can run.
fn accepted_html(scope: &str, mut facts: Vec<(&'static str, String)>) -> String {
    facts.push(("Status", "NOT STARTED".to_owned()));
    render::receipt_page(&render::Receipt {
        scope,
        verdict: "NOT STARTED",
        reason: CAPTURE_UNAVAILABLE,
        facts: &facts,
    })
}

/// The window's facts, including the correction the operator never has to make.
fn window_facts(window: pull::session::Window) -> Vec<(&'static str, String)> {
    vec![
        // `Window`'s own Display, which is `from..=to` — the inclusive
        // notation. Writing the two ends out here instead would be a second
        // definition of what the range means, and the two would drift.
        ("Window", window.to_string()),
        (
            "From",
            format!("{} — inclusive", window.from()),
        ),
        ("To", format!("{} — inclusive", window.to())),
        ("Calendar days", window.days().to_string()),
        (
            "toDate on the wire",
            window.wire_to().map_or_else(
                |e| format!("REFUSED — {e}"),
                |d| format!("{d} — the day AFTER your last day, because the vendor's toDate is not inclusive"),
            ),
        ),
        ("Timeframe", "1 minute".to_owned()),
    ]
}

/// What one spot request is answered with, given a day to check the window
/// against.
///
/// Split from the handler for the same reason [`fno_answer`] is: the gate must
/// be driven by a value, not by the machine's clock — `CLAUDE.md` §3 rule 5, and
/// a gate that reads the clock inside cannot be tested at its own boundary
/// without waiting for midnight.
///
/// **This split is the fix for a real defect.** The F&O side has had this shape
/// since it was written; the spot side went straight to the parser with no
/// `today` at all, so the only thing stopping a window that ends in the future
/// was the `max` attribute the page renders. The panel one div over says *"an
/// attribute is a courtesy, a parser is a rule"* — and spot had only the
/// courtesy. An attribute is absent from a `curl`, from a replayed POST, and
/// from any client that is not the browser the form was rendered for.
fn spot_answer(body: &str, today: Day, site: &Site) -> (axum::http::StatusCode, String) {
    match ingest::parse_spot(body, today) {
        Err(why) => (
            axum::http::StatusCode::BAD_REQUEST,
            refusal_html("Spot pull", &why),
        ),
        Ok(asked) => {
            let slot = ingest::SpotTarget::ALL
                .into_iter()
                .position(|t| t == asked.target)
                .unwrap_or(0);
            let mut facts = vec![
                ("Target", asked.target.label().to_owned()),
                (
                    "Instruments covered",
                    site.targets.get(slot).copied().unwrap_or(0).to_string(),
                ),
            ];
            facts.extend(window_facts(asked.window));
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                accepted_html("Spot pull", facts),
            )
        }
    }
}

/// Starting a spot pull. **POST only.**
async fn pull_spot(
    axum::extract::State(site): axum::extract::State<Loaded>,
    body: String,
) -> (axum::http::StatusCode, axum::response::Html<String>) {
    let (code, page) = dated(ingest::today_ist(), "Spot pull", |today| {
        spot_answer(&body, today, &site)
    });
    (code, axum::response::Html(page))
}

/// What one expired-series request is answered with, given a day to check the
/// expiry against.
///
/// Split from the handler so the expiry gate is driven by a value rather than
/// by the machine's clock — `CLAUDE.md` §3 rule 5.
fn fno_answer(body: &str, today: Day) -> (axum::http::StatusCode, String) {
    match ingest::parse_fno(body, today) {
        Err(why) => (
            axum::http::StatusCode::BAD_REQUEST,
            refusal_html("Expired F&O pull", &why),
        ),
        Ok(asked) => {
            let mut facts = vec![
                ("Underlying", asked.underlying.as_str().to_owned()),
                ("Series", asked.series.label().to_owned()),
                (
                    "Expiry",
                    format!("{} — expired, checked against {today}", asked.expiry),
                ),
            ];
            facts.extend(window_facts(asked.window));
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                accepted_html("Expired F&O pull", facts),
            )
        }
    }
}

/// Starting an expired-series pull. **POST only.**
async fn pull_fno(body: String) -> (axum::http::StatusCode, axum::response::Html<String>) {
    let (code, page) = dated(ingest::today_ist(), "Expired F&O pull", |today| {
        fno_answer(&body, today)
    });
    (code, axum::response::Html(page))
}

/// The store page.
async fn store_get(
    axum::extract::State(site): axum::extract::State<Loaded>,
    uri: axum::http::Uri,
) -> (axum::http::StatusCode, axum::response::Html<String>) {
    let (code, body) = dated(ingest::today_ist(), "Store", |today| {
        (
            axum::http::StatusCode::OK,
            store_html(&site, today, page_number(uri.query().unwrap_or(""))),
        )
    });
    (code, axum::response::Html(body))
}

/// The store page, from a site already loaded.
///
/// An absent manifest renders; it never fails. Before the first ingest there is
/// no file, and a page that 500s on a fresh install is a page that is broken
/// exactly when an operator most needs to look at it.
#[must_use]
pub fn store_html(site: &Site, today: Day, page: usize) -> String {
    // The grid is instruments × months and both factors are bounded, so the
    // last page is a division rather than a walk, and `?page=999` clamps to
    // somewhere real for the same reason `/instruments` does.
    let total = census::grid_rows(site.indices.len());
    let last_page = total.saturating_sub(1) / PAGE_ROWS;
    let page = page.min(last_page);
    let rows = census::coverage_page(
        &site.indices,
        &site.censuses,
        today,
        page.saturating_mul(PAGE_ROWS),
        PAGE_ROWS,
    );
    let mut notes = vec![format!(
        "store root: {} — every figure here is a field read of one manifest \
         header, and every grid cell is one hash probe",
        site.store_root.display()
    )];
    notes.extend(site.censuses.iter().map(census::VendorCensus::note));
    // The master notes ride along, because an `UNAVAILABLE` master is why the
    // grid may be down to the two swept series.
    notes.extend(site.read.notes.iter().cloned());
    render::store_page(&render::StoreView {
        today,
        censuses: &site.censuses,
        rows: &rows,
        page,
        last_page,
        total,
        notes: &notes,
    })
}

/// Every route this server answers, serving from an already-loaded site.
///
/// Takes the site rather than a directory: a router holding a `PathBuf` can
/// only re-read, and re-reading per request is the O(rows) cost this split
/// exists to remove.
///
/// **`/pull/spot` and `/pull/fno` are `post` and nothing else.** A `get` on
/// either answers 405 without touching a vendor, which is the whole point: a
/// crawler follows links, a browser refetches on back, and either would
/// otherwise start an ingest nobody asked for.
///
/// **The request body is bounded here, by a number this repository states.**
/// `crates/api/src/ingest.rs` says every parser it holds works over "a form
/// body whose length the server caps". That was true, and it was true by
/// accident: the cap was `axum`'s own default and no line in this repository
/// named it. A framework default is a bound somebody else may change, and
/// `docs/07-o1-architecture.md` law 5 is bound every input **at the boundary**
/// — which means the boundary says the number.
pub fn router(site: Loaded) -> axum::Router {
    axum::Router::new()
        .route("/", axum::routing::get(home))
        .route("/instruments", axum::routing::get(page))
        .route("/pull", axum::routing::get(pull_get))
        .route("/pull/spot", axum::routing::post(pull_spot))
        .route("/pull/fno", axum::routing::post(pull_fno))
        .route("/store", axum::routing::get(store_get))
        .route("/health", axum::routing::get(health))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_FORM_BYTES))
        .with_state(site)
}

/// Serves on an already-bound listener until `shutdown` resolves.
///
/// The shutdown signal is a parameter rather than a `ctrl_c()` buried inside,
/// so that a test can drive the whole serve path — accept, route, respond,
/// stop — without a signal and without a hard kill.
///
/// # Errors
///
/// Whatever the server failed with. A server that stops for a reason is a
/// reason the operator gets to read.
pub async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    shutdown: Shutdown,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            // The signal's own error is not actionable: a failed ctrl-c
            // registration still means stop, and there is nothing else to do
            // about it here.
            let _ = shutdown.await;
        })
        .await
}

/// The exit code of a server that has stopped, and the reason if it fell over.
///
/// A separate function because a server that fails while accepting is not a
/// state a test can conjure on demand — and an untestable arm inside `run`
/// would be an uncovered branch that the coverage gate could never accept, so
/// the branch lives where it can be exercised directly.
fn stopped(outcome: std::io::Result<()>) -> u8 {
    match outcome {
        Ok(()) => OK,
        Err(e) => {
            eprintln!("server stopped: {e}");
            FAILED
        }
    }
}

/// Everything went as asked.
pub const OK: u8 = 0;
/// It was asked for something reasonable and could not do it.
pub const FAILED: u8 = 1;
/// It was asked for something it does not understand. Distinct from
/// [`FAILED`]: "I do not know what you mean" is not "I tried and failed".
pub const MISUSED: u8 = 2;
/// It did the work and the answer must not be trusted.
///
/// A vendor was never read, or two vendors disagreed, or a listing class
/// nobody recognises turned up. Distinct from [`FAILED`] because the work
/// completed and the output is real — it is the *universe* that is refused,
/// not the run. D-0026. Zero here is what let a monitor read green while one
/// of the two masters was missing.
pub const DEGRADED: u8 = 3;

/// Prints the report for `dir` and returns the exit code it earned.
///
/// A separate function for the same reason [`stopped`] is one: which directory
/// `run` reads comes from the environment, and a test cannot set an
/// environment variable here — `set_var` is `unsafe` under edition 2024 and
/// this crate forbids `unsafe`. Left inline, the `OK` arm would be reachable
/// only from the child process in `tests/binary.rs`, and a branch that only a
/// subprocess can enter is a branch the coverage gate cannot hold. Taking the
/// directory as an argument puts both arms where a test can drive them
/// directly.
fn reported(dir: &Path) -> u8 {
    let (text, clean) = report(dir);
    print!("{text}");
    if clean { OK } else { DEGRADED }
}

/// Runs one command to completion and reports how it went.
///
/// Returns the exit code as a number rather than calling `exit`, so the whole
/// thing — every arm of it — is callable from a test.
pub async fn run(args: &[String], shutdown: Shutdown) -> u8 {
    run_in(&masters_dir(), args, shutdown).await
}

/// The store root a served process reads its manifests from.
///
/// Read here rather than threaded through [`run_in`] for the same reason
/// [`masters_dir`] is read in [`run`]: the environment is consulted once, at
/// the edge, and everything below takes a path.
fn served_store_root() -> PathBuf {
    store_dir()
}

/// [`run`], over a directory the caller names.
///
/// Split for the same reason [`masters_dir_from`] and [`reported`] are split,
/// and for a reason found the hard way. With the directory read inside this
/// function, the only test that could reach the `Report` arm had to call the
/// real entry point, which read `$HOME/.brutex/masters` — so on the operator's
/// machine the test parsed 53 MB of real vendor CSV and returned `OK`, and on a
/// CI runner with an empty `HOME` the same test returned `DEGRADED`. Its
/// assertions were written to survive both (`assert_ne!(code, FAILED)`,
/// `assert_ne!(code, MISUSED)`), and since [`reported`] returns only `OK` or
/// `DEGRADED`, **no input, machine or environment could ever falsify them** —
/// a mutant pinning `reported` to `OK` survived. CLAUDE.md §4 bans a test that
/// asserts nothing; §3 rule 5 wants the same inputs to give the same outputs.
///
/// Taking the directory as an argument makes the answer a property of the
/// files, which a test owns, rather than of the machine, which it does not.
async fn run_in(dir: &Path, args: &[String], shutdown: Shutdown) -> u8 {
    match Command::parse(args) {
        Ok(Command::Report) => reported(dir),
        Ok(Command::Serve(addr)) => match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let store_root = served_store_root();
                println!("brutex api listening on http://{addr}/instruments");
                println!("  masters: {}", dir.display());
                println!("  store:   {}", store_root.display());
                let site = Site::load(dir, &store_root);
                stopped(serve(listener, router(Loaded::new(site)), shutdown).await)
            }
            Err(e) => {
                eprintln!("cannot bind {addr}: {e}");
                FAILED
            }
        },
        Err(usage) => {
            eprintln!("{usage}");
            MISUSED
        }
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
    use std::io::Write as _;

    /// A directory holding one or both masters, named after the test.
    fn masters(name: &str, groww: Option<&str>, dhan: Option<&str>) -> PathBuf {
        let dir = crate::scratch::path(&format!("server-{name}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (file, body) in [("groww_instruments.csv", groww), ("dhan_scrip.csv", dhan)] {
            let path = dir.join(file);
            match body {
                Some(text) => {
                    let mut f = std::fs::File::create(&path).expect("create");
                    f.write_all(text.as_bytes()).expect("write");
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        dir
    }

    /// An empty store root, named after the test.
    ///
    /// Empty on purpose in most tests: an absent manifest is the ordinary
    /// state before the first ingest, and it must render rather than fail.
    fn store_root(name: &str) -> PathBuf {
        let dir = crate::scratch::path(&format!("store-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("manifest")).expect("mkdir");
        dir
    }

    /// A day this build can name, for the pages that take one.
    fn day(y: u16, m: u8, d: u8) -> Day {
        Day::new(y, m, d).expect("a real date")
    }

    /// A site over the given masters and an empty store.
    fn site(name: &str, dir: &Path) -> Site {
        Site::load(dir, &store_root(name))
    }

    const GROWW_HEAD: &str = "exchange,segment,underlying_symbol,trading_symbol,\
                              instrument_type,series,isin,expiry_date,strike_price\n";
    // `SERIES` is the column the gate reads; `INSTRUMENT_TYPE` is present
    // because the real file has it, and is deliberately never read. D-0025.
    const DHAN_HEAD: &str = "EXCH_ID,SEGMENT,ISIN,INSTRUMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,\
                             INSTRUMENT_TYPE,SERIES,SM_EXPIRY_DATE,STRIKE_PRICE,OPTION_TYPE\n";

    /// Both vendors, agreeing about NIFTY and RELIANCE.
    fn agreeing(name: &str) -> PathBuf {
        masters(
            name,
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,NIFTY,IDX,,NIFTY,,\n\
                 NSE,CASH,,RELIANCE,EQ,EQ,INE002A01018,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}\
                 NSE,I,NA,INDEX,NIFTY,NIFTY,INDEX,NA,0001-01-01,,\n\
                 NSE,E,INE002A01018,EQUITY,RELIANCE,RELIANCE INDUSTRIES,ES,EQ,,,\n"
            )),
        )
    }

    #[test]
    fn the_command_line_is_parsed_or_refused_and_never_guessed() {
        let parse = |args: &[&str]| {
            let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            Command::parse(&owned)
        };
        assert_eq!(parse(&[]), Ok(Command::Serve(DEFAULT_ADDR)));
        assert_eq!(parse(&["serve"]), Ok(Command::Serve(DEFAULT_ADDR)));
        assert_eq!(
            parse(&["serve", "0.0.0.0:9100"]),
            Ok(Command::Serve("0.0.0.0:9100".parse().expect("valid")))
        );
        assert_eq!(parse(&["report"]), Ok(Command::Report));

        // A typo that silently started a server is a typo nobody finds.
        let e = parse(&["serv"]).expect_err("must refuse");
        assert!(e.contains("unknown argument") && e.contains("usage"), "{e}");
        let e = parse(&["report", "now"]).expect_err("must refuse");
        assert!(e.contains("unknown"), "{e}");
        let e = parse(&["serve", "not-an-address"]).expect_err("must refuse");
        assert!(e.contains("not a socket address"), "{e}");
        assert_eq!(DEFAULT_ADDR.port(), 8080);
    }

    #[test]
    fn the_masters_directory_comes_from_the_environment_or_defaults_under_home() {
        // Tested through the value rather than by setting the variable: under
        // edition 2024 `set_var` is unsafe, this crate forbids unsafe, and a
        // test that mutated process-wide state would be racing every other
        // test in the binary anyway.
        //
        // The default is asserted by SHAPE, not by a literal: the home
        // directory differs per machine and per CI runner, and hard-coding one
        // would pass here and fail everywhere else.
        // Absent value delegates to the default, exactly. Asserted against the
        // function rather than against a literal path: the home directory
        // differs per machine and per CI runner, so a literal would pass here
        // and fail everywhere else. The default's own two arms are pinned to
        // literals below, where the input IS controlled.
        assert_eq!(masters_dir_from(None), default_masters_dir());
        // And the default is exactly the pure function over the SAME
        // environment. Compared against `default_masters_dir_from` rather than
        // only against `masters_dir_from(None)`, which calls it: a function
        // compared with its own caller cannot fail, and `cargo mutants` proved
        // it by replacing `default_masters_dir` with an empty path and passing.
        assert_eq!(
            default_masters_dir(),
            default_masters_dir_from(std::env::var_os("HOME"))
        );
        assert!(
            !default_masters_dir().as_os_str().is_empty(),
            "it always names somewhere; an empty path is not a directory"
        );
        assert_eq!(
            masters_dir_from(Some("/somewhere/else".into())),
            PathBuf::from("/somewhere/else")
        );
        // Both arms of the default, driven by value rather than by mutating
        // the environment.
        assert_eq!(
            default_masters_dir_from(Some("/home/who".into())),
            PathBuf::from("/home/who/.brutex/masters")
        );
        assert_eq!(
            default_masters_dir_from(None),
            PathBuf::from("."),
            "no HOME is a broken environment, not a supported one — the page \
             then says UNAVAILABLE and names the vendor"
        );
    }

    #[test]
    fn a_server_that_falls_over_says_so_and_exits_non_zero() {
        assert_eq!(stopped(Ok(())), OK);
        assert_eq!(
            stopped(Err(std::io::Error::other("the socket went away"))),
            FAILED
        );
        assert_ne!(FAILED, MISUSED, "a misuse is not a failure");
    }

    #[test]
    fn each_vendor_is_looked_for_under_its_own_file_name() {
        let paths = master_paths(Path::new("/m"));
        assert_eq!(paths.len(), 2);
        assert!(paths[0].1.ends_with("groww_instruments.csv"));
        assert!(paths[1].1.ends_with("dhan_scrip.csv"));
    }

    #[test]
    fn a_query_string_is_decoded_without_a_dependency() {
        assert_eq!(parse_query("q=NIFTY"), "NIFTY");
        assert_eq!(parse_query("x=1&q=BANK"), "BANK");
        assert_eq!(parse_query(""), "");
        assert_eq!(parse_query("x=1"), "");
        assert_eq!(parse_query("q=M%26M"), "M&M");
        assert_eq!(parse_query("q=NIFTY+50"), "NIFTY 50");
        // Both cases of hex, because a browser may send either.
        assert_eq!(parse_query("q=%2f%2F"), "//");
        assert_eq!(parse_query("q=%7e"), "~");
        // Undecodable input is kept LITERALLY. A query must never silently
        // become a different query.
        assert_eq!(parse_query("q=100%"), "100%", "an escape at the very end");
        assert_eq!(parse_query("q=a%2"), "a%2", "a truncated escape");
        assert_eq!(parse_query("q=%zz"), "%zz", "not hex at all");
        assert_eq!(parse_query("q=%2z"), "%2z", "half hex is not hex");
    }

    #[test]
    fn the_report_names_every_vendor_and_every_decline_reason() {
        let dir = agreeing("report");
        let (text, clean) = report(&dir);
        assert!(text.starts_with("ok\n"));
        assert!(clean, "both vendors read, nothing disagreed");
        assert!(text.contains("groww: 2 kept"), "{text}");
        assert!(text.contains("dhan: 2 kept"), "{text}");
        assert!(text.contains("merged: 2 instruments"), "{text}");
        assert!(text.contains("0 isin conflicts"), "{text}");
        assert!(text.contains("0 eligibility conflicts"), "{text}");
        // The census is stated on every run, and it separates what two vendors
        // confirmed from what one asserted.
        assert!(
            text.contains("F&O underlyings: 2 resolved, 2 confirmed by both vendors"),
            "{text}"
        );
        assert!(
            text.contains("NIFTY Total Market: 1 resolved, 1 confirmed by both vendors"),
            "{text}"
        );
        assert!(
            !text.contains("UNCHECKED"),
            "nothing rests on one vendor here"
        );
    }

    #[test]
    fn a_missing_master_is_named_as_unavailable_and_the_status_says_so() {
        // "This vendor lists nothing" and "this vendor was never read" are
        // different facts, and collapsing them is the silent degradation the
        // charter forbids. That was true of the NOTE and false of the STATUS:
        // the first line said `ok` and the exit code was 0, so every monitor
        // read green while half the universe had never been opened.
        let dir = masters(
            "missing",
            Some(&format!("{GROWW_HEAD}NSE,CASH,,NIFTY,IDX,,NIFTY,,\n")),
            None,
        );
        let (text, clean) = report(&dir);
        assert!(text.starts_with("DEGRADED\n"), "{text}");
        assert!(!clean, "a vendor that was never read is not a clean read");
        assert!(text.contains("groww: 1 kept"), "{text}");
        assert!(text.contains("dhan: UNAVAILABLE"), "{text}");
        assert!(text.contains("merged: 1 instruments"), "{text}");
    }

    #[test]
    fn the_report_counts_declines_by_reason() {
        let dir = masters(
            "reasons",
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,SOMEBOND,EQ,N2,INE002A01018,,\n\
                 NSE,CASH,,SOMESME,EQ,SM,INE002A01018,,\n"
            )),
            None,
        );
        let (text, _) = report(&dir);
        assert!(text.contains("not an equity listing 1"), "{text}");
        assert!(text.contains("SME board 1"), "{text}");
    }

    #[test]
    fn an_unrecognised_listing_class_names_the_code_and_degrades_the_run() {
        // Renaming the equity series on the real Dhan master dropped 2,438
        // shares under the same label a debenture gets, while the report
        // printed `ok` and exited 0. The code is now named, the reason is its
        // own, and the run is not clean.
        let dir = masters(
            "unrecognised",
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,RELIANCE,EQ,EQX,INE002A01018,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}NSE,E,INE002A01018,EQUITY,RELIANCE,RELIANCE INDUSTRIES,ES,EQX,,,\n"
            )),
        );
        let (text, clean) = report(&dir);
        assert!(!clean, "an alphabet moving is not a routine skip");
        assert!(text.starts_with("DEGRADED\n"), "{text}");
        assert!(text.contains("unrecognised listing class 1"), "{text}");
        assert!(
            text.contains("UNRECOGNISED LISTING CLASS · \"EQX\" ×1"),
            "the CODE is named, not just the count: {text}"
        );
        assert!(
            text.contains("this engine has never seen"),
            "and it says what that means: {text}"
        );
        // A routine bond, by contrast, leaves the run clean.
        let dir = masters(
            "routinebond",
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,SOMEBOND,EQ,N2,INE002A01018,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}NSE,E,INE002A01018,EQUITY,SOMEBOND,SOME BOND,DEB,N2,,,\n"
            )),
        );
        let (text, clean) = report(&dir);
        assert!(clean, "{text}");
        assert!(text.contains("not an equity listing 1"), "{text}");
    }

    #[test]
    fn an_eligibility_disagreement_is_reported_and_degrades_the_run() {
        // One vendor calls it an equity, the other calls it a fund. Both rows
        // carry the SAME ISIN, so the check has the key it needs -- and before
        // this the declined row was dropped at the reader, so the report said
        // `0 conflicts` and exited 0.
        let dir = masters(
            "eligibility",
            // Groww declines it: series MF is a fund.
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,FISTIPD3GP,EQ,MF,INF090I01VS3,,\n"
            )),
            // Dhan keeps it: series EQ.
            Some(&format!(
                "{DHAN_HEAD}NSE,E,INF090I01VS3,EQUITY,FISTIPD3GP,FRANKLIN PLAN,ETF,EQ,,,\n"
            )),
        );
        let (text, clean) = report(&dir);
        assert!(!clean);
        assert!(text.contains("ELIGIBILITY CONFLICT"), "{text}");
        assert!(text.contains("1 eligibility conflicts"), "{text}");
        assert!(text.contains("dhan kept it"), "{text}");
        assert!(text.contains("groww declined it"), "{text}");
        assert!(text.contains("INF090I01VS3"), "{text}");
        // And it reaches a SEARCHED page, not only the unfiltered one.
        let html = instruments_html(&dir, "FISTIP");
        assert!(html.contains("ELIGIBILITY CONFLICT"), "{html}");
    }

    #[test]
    fn an_isin_conflict_reaches_the_report_and_the_page() {
        // The two vendors give one ticker two different ISINs. Neither the
        // report nor the page may swallow that.
        let dir = masters(
            "conflict",
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,CHOLAFIN,EQ,EQ,INE121A01024,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}NSE,E,INE121A08PJ0,EQUITY,CHOLAFIN,CHOLA,ES,EQ,,,\n"
            )),
        );
        let (text, clean) = report(&dir);
        assert!(text.contains("ISIN CONFLICT"), "{text}");
        assert!(text.contains("1 isin conflicts"), "{text}");
        assert!(
            text.contains("INE121A01024") && text.contains("INE121A08PJ0"),
            "{text}"
        );
        assert!(
            !clean,
            "D-0026: a disagreement REFUSES the universe rather than logging it"
        );
        assert!(text.starts_with("DEGRADED\n"), "{text}");

        let html = instruments_html(&dir, "");
        assert!(html.contains("ISIN CONFLICT"), "the page says so too");
        assert!(html.contains("clash"), "and the row is marked");
    }

    #[test]
    fn a_page_number_that_is_not_a_number_is_page_one() {
        // `?page=abc` is a mangled bookmark or a hand-typed URL. It must render
        // the first page, not fail and not 500 -- but note this is the ONE
        // place a bad parameter is quietly defaulted rather than refused, and
        // it is defensible only because a page number selects a VIEW and can
        // never change what the data says.
        assert_eq!(page_number("page=abc"), 0);
        assert_eq!(page_number("page="), 0);
        assert_eq!(page_number("page=-1"), 0);
        assert_eq!(page_number("page=2"), 2);
        assert_eq!(page_number(""), 0);
    }

    #[test]
    fn the_escape_hatch_reaches_a_listing_the_tracked_universe_excludes() {
        // RAJESHEXPO is a real-shaped equity row that is in neither NIFTY Total
        // Market nor the index series, so it is the only kind of instrument the
        // tracked filter actually removes. Without a row like this the filter
        // can never be observed doing anything, and `all` can never be observed
        // undoing it.
        let dir = masters(
            "hatchreach",
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,RAJESHEXPO,EQ,EQ,INE343B01030,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}\
                 NSE,E,INE343B01030,EQUITY,RAJESHEXPO,RAJESH EXPORTS,ES,EQ,,,\n"
            )),
        );
        let read = universe(&dir);

        // Tracked view: the gate accepted it as a share, and the universe
        // filter still excludes it — those are different questions.
        let tracked = instruments_html_from(&read, "", "", false, "", 0);
        assert!(
            !tracked.contains("NSE-RAJESHEXPO"),
            "not in NIFTY Total Market, so not tracked"
        );
        assert!(tracked.contains("0 instruments total"));

        // The escape hatch reaches it. This is the whole reason the hatch
        // exists: an instrument the filter hides must still be inspectable.
        let every = instruments_html_from(&read, "", "", true, "", 0);
        assert!(
            every.contains("NSE-RAJESHEXPO"),
            "?all=1 reaches every listing"
        );
        assert!(every.contains("1 instruments total"));
    }

    #[test]
    fn a_leading_minus_reverses_the_order_without_a_second_comparator() {
        // Ascending and descending share ONE ordering rule per column: the
        // keys are sorted ascending and then reversed. Two comparators is how
        // ties silently break differently in each direction.
        let read = universe(&agreeing("desc"));
        let up = instruments_html_from(&read, "", "symbol", false, "", 0);
        let down = instruments_html_from(&read, "", "-symbol", false, "", 0);

        let first = |html: &str| -> String {
            html.split("<tr class=")
                .nth(1)
                .and_then(|r| r.split("<td>").nth(1))
                .and_then(|c| c.split("</td>").next())
                .unwrap_or_default()
                .to_owned()
        };
        assert_ne!(first(&up), first(&down), "the two orders differ");
        assert!(up.contains("NSE-NIFTY") && up.contains("NSE-RELIANCE"));
        assert!(down.contains("NSE-NIFTY") && down.contains("NSE-RELIANCE"));

        // An unknown column reverses the DEFAULT order rather than erroring.
        let stale = instruments_html_from(&read, "", "-no-such-column", false, "", 0);
        assert!(stale.contains("NSE-NIFTY") && stale.contains("NSE-RELIANCE"));
    }

    #[test]
    fn every_row_is_reachable_by_paging_and_a_stale_page_clamps() {
        // The cap alone was a wall: 200 of 785 rendered and nothing led to the
        // rest. Scrolling cannot reveal rows that were never sent.
        let read = universe(&agreeing("paging"));

        // This fixture is smaller than one page, so there is no pager at all --
        // navigation that leads nowhere is worse than none.
        let one = instruments_html_from(&read, "", "", false, "", 0);
        assert!(
            !one.contains("class=\"pager\""),
            "no pager for a single page"
        );
        assert!(one.contains("NSE-NIFTY") && one.contains("NSE-RELIANCE"));

        // A page beyond the end CLAMPS to the last page rather than rendering
        // an empty table: `?page=999` is a stale bookmark, not an attack, and
        // it should land somewhere real.
        let past = instruments_html_from(&read, "", "", false, "", 999);
        assert!(
            past.contains("NSE-NIFTY") && past.contains("NSE-RELIANCE"),
            "an out-of-range page clamps to the last one, it does not empty out"
        );
        assert_eq!(past, one, "clamping lands exactly on the last page");
    }

    #[test]
    fn every_sort_column_orders_and_none_of_them_errors() {
        // Each arm of the sort match is a separate closure; an arm no test
        // enters is an arm that can be wrong forever. RELIANCE is in NIFTY
        // Total Market and NIFTY is an index, so the two rows differ on every
        // column the page can order by.
        let read = universe(&agreeing("sortcols"));
        for column in ["key", "symbol", "isin", "universe", "kind", "vendors", ""] {
            let html = instruments_html_from(&read, "", column, false, "", 0);
            assert!(
                html.contains("NSE-NIFTY") && html.contains("NSE-RELIANCE"),
                "sort={column:?} lost a row"
            );
            // Every named column has a header link. The empty column is the
            // default order and names no column, so it is checked separately
            // rather than through a short-circuit whose right side never runs.
            if !column.is_empty() {
                assert!(
                    html.contains(&format!("sort={column}")),
                    "sort={column:?} has no header link"
                );
            }
        }
        // An unrecognised column is the DEFAULT order, not an error and not an
        // empty page: a stale bookmark should still render.
        let stale = instruments_html_from(&read, "", "no-such-column", false, "", 0);
        assert!(stale.contains("NSE-NIFTY") && stale.contains("NSE-RELIANCE"));
    }

    #[test]
    fn the_universe_pill_selects_from_the_whole_set_not_from_the_page() {
        let read = universe(&agreeing("pills"));

        // Counts are of the tracked set. Both rows are tracked: NIFTY is an
        // index, RELIANCE is a Total Market constituent.
        let all = instruments_html_from(&read, "", "", false, "", 0);
        assert!(all.contains("2 instruments total"));

        // Indices selects the index and drops the equity.
        let idx = instruments_html_from(&read, "", "", false, "idx", 0);
        assert!(idx.contains("NSE-NIFTY"), "the index survives");
        assert!(!idx.contains("NSE-RELIANCE"), "the equity is filtered out");

        // Total Market selects the equity and drops the index.
        let ntm = instruments_html_from(&read, "", "", false, "ntm", 0);
        assert!(ntm.contains("NSE-RELIANCE"));
        assert!(!ntm.contains("NSE-NIFTY"));

        // F&O selects BOTH, because both are F&O underlyings.
        //
        // `universe::of_instrument` gives an index `INDEX ∪ of_equity(symbol)`,
        // so NIFTY carries INDEX **and** FNO — it is an index *and* the
        // underlying of its options. The universes deliberately overlap; a
        // filter treating them as disjoint would drop NIFTY from the F&O view,
        // which is the one instrument that most needs to be there.
        let fno = instruments_html_from(&read, "", "", false, "fno", 0);
        assert!(
            fno.contains("NSE-RELIANCE"),
            "RELIANCE is an F&O underlying"
        );
        assert!(
            fno.contains("NSE-NIFTY"),
            "NIFTY is INDEX and FNO both — see universe::of_instrument"
        );

        // An unrecognised value selects EVERYTHING, so a stale bookmark renders
        // the page rather than an empty one.
        let stale = instruments_html_from(&read, "", "", false, "no-such-universe", 0);
        assert!(stale.contains("NSE-NIFTY") && stale.contains("NSE-RELIANCE"));
    }

    #[test]
    fn the_escape_hatch_says_which_direction_it_goes() {
        let read = universe(&agreeing("hatch"));
        // Default: the link offers to widen.
        let tracked = instruments_html_from(&read, "", "", false, "", 0);
        assert!(tracked.contains("show every NSE listing"));
        assert!(tracked.contains("all=1"));
        // Widened: the SAME link offers to narrow again. Without this arm the
        // page can be widened and never returned from.
        let every = instruments_html_from(&read, "", "", true, "", 0);
        assert!(every.contains("show tracked only"));
        assert!(every.contains("all=0"));
    }

    #[test]
    fn the_page_filters_on_the_query_and_says_which_total_it_means() {
        let dir = agreeing("page");
        let all = instruments_html(&dir, "");
        assert!(all.contains("2 instruments total"));
        assert!(all.contains("NSE-NIFTY") && all.contains("NSE-RELIANCE"));

        let filtered = instruments_html(&dir, "reliance");
        assert!(filtered.contains("1 matched"), "case-insensitive match");
        assert!(filtered.contains("NSE-RELIANCE"));
        assert!(!filtered.contains("NSE-NIFTY"));

        let none = instruments_html(&dir, "NOTHINGLIKETHIS");
        assert!(none.contains("0 matched"));
        assert!(none.contains("<tbody></tbody>"));
    }

    #[test]
    fn a_searched_page_still_says_a_vendor_was_never_read() {
        // THE COLLAPSE `universe`'s OWN DOC SAYS SECTION 4 FORBIDS. The notes
        // were folded into the title only when the query was empty, so typing
        // anything made the page stop saying that a master was missing -- and
        // with PAGE_ROWS at 200 against thousands of instruments, searching is
        // the only way to reach most of them.
        let dir = masters(
            "searchnotes",
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,RELIANCE,EQ,EQ,INE002A01018,,\n"
            )),
            None,
        );
        for query in ["", "RELIANCE", "NOTHINGLIKETHIS"] {
            let html = instruments_html(&dir, query);
            assert!(
                html.contains("dhan: UNAVAILABLE"),
                "query {query:?} must not hide an unread vendor: {html}"
            );
            assert!(
                html.contains("DEGRADED"),
                "query {query:?} must not claim the read was clean"
            );
        }
        // A row from one vendor when the other was never read must not be
        // byte-identical to a genuine single-vendor listing; the banner is
        // what distinguishes them, and it is present above.
        assert!(instruments_html(&dir, "RELIANCE").contains("1 matched"));
    }

    #[test]
    fn an_unreadable_row_says_why_and_where_rather_than_only_how_many() {
        // The line numbers and reasons were collected and then read only for
        // `.len()`, so `104 unreadable` was the whole of what an operator was
        // ever told. `NIFTY 100` is a real Dhan index ticker -- a space is not
        // a legal Symbol -- and 104 rows of the real master are exactly that.
        let dir = masters(
            "unreadable",
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,RELIANCE,EQ,EQ,INE002A01018,,\n\
                 NSE,CASH,,NIFTY 100,IDX,,NIFTY,,\n\
                 NSE,CASH,,NIFTY 200,IDX,,NIFTY,,\n"
            )),
            None,
        );
        let (text, _) = report(&dir);
        assert!(text.contains("2 unreadable"), "{text}");
        assert!(
            text.contains("groww UNREADABLE · malformed instrument identifier ×2, first at line 3"),
            "the reason and the line, not a bare count: {text}"
        );
        assert!(instruments_html(&dir, "RELIANCE").contains("UNREADABLE"));
    }

    #[test]
    fn a_row_too_short_for_its_columns_is_unreadable_never_a_routine_decline() {
        // A truncated row used to default its missing fields to "", which the
        // gate read as a series it does not recognise -- so a genuine share
        // was dropped and reported as ordinary business with `0 unreadable`.
        // The RELIANCE row below names the instrument and then stops before
        // the series column.
        let dir = masters(
            "short",
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,RELIANCE,EQ\n\
                 NSE,CASH,,CHOLAFIN,EQ,EQ,INE121A01024,,\n"
            )),
            None,
        );
        let (text, _) = report(&dir);
        assert!(text.contains("groww: 1 kept"), "{text}");
        assert!(text.contains("1 unreadable"), "{text}");
        assert!(
            text.contains("row has 5 field(s); the columns this vendor needs run to 9"),
            "the shortfall is named: {text}"
        );
        assert!(
            !text.contains("not an equity listing"),
            "a truncated share is NOT a bond: {text}"
        );
    }

    #[test]
    fn the_swept_instruments_are_rendered_first() {
        let dir = agreeing("order");
        let html = instruments_html(&dir, "");
        let nifty = html.find("NSE-NIFTY").expect("present");
        let reliance = html.find("NSE-RELIANCE").expect("present");
        assert!(nifty < reliance, "a swept instrument leads the page");
    }

    /// Reads one HTTP response off a fresh connection.
    ///
    /// A blocking client on a blocking thread, deliberately: it needs nothing
    /// from `tokio` that this crate's feature set does not already have, and
    /// awaiting it yields the runtime to the server task under test.
    async fn get(addr: SocketAddr, path: &str) -> String {
        let request = format!("GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
        tokio::task::spawn_blocking(move || {
            use std::io::Read as _;
            let mut s = std::net::TcpStream::connect(addr).expect("connect");
            s.write_all(request.as_bytes()).expect("write");
            let mut buf = String::new();
            s.read_to_string(&mut buf).expect("read");
            buf
        })
        .await
        .expect("the client thread must not panic")
    }

    #[tokio::test]
    async fn the_server_answers_every_route_and_then_shuts_down_gracefully() {
        let dir = agreeing("serve");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");

        // The shutdown signal is a second listener rather than a channel: it
        // needs nothing this crate does not already depend on, and connecting
        // to it is an unambiguous "stop now".
        let stopper = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let stop_addr = stopper.local_addr().expect("addr");
        let served = tokio::spawn(serve(
            listener,
            router(Loaded::new(Site::load(&dir, &store_root("serve")))),
            Box::pin(async move { stopper.accept().await.map(|_| ()) }),
        ));

        let health = get(addr, "/health").await;
        assert!(health.contains("200 OK"), "{health}");
        assert!(health.contains("merged: 2 instruments"), "{health}");

        let page = get(addr, "/instruments?q=NIFTY").await;
        assert!(page.contains("200 OK"));
        assert!(page.contains("NSE-NIFTY"));
        assert!(
            page.contains("groww: 2 kept"),
            "a searched page still carries the notes: {page}"
        );

        // `/` is the DASHBOARD, not a second copy of the instruments page.
        // It carries the nav, so every other page is one click away, and its
        // figures are counters rather than a row listing.
        let root = get(addr, "/").await;
        assert!(root.contains("200 OK"));
        assert!(root.contains("nav class"), "the dashboard carries the nav");
        assert!(
            root.contains("NIFTY Total Market"),
            "the dashboard names the universes it counts: {root}"
        );
        assert!(
            !root.contains("<tbody>"),
            "the dashboard counts; it does not list rows"
        );
        // THE NAV'S PREMISE CHANGED WITH D-0038, AND ONLY HALF OF IT.
        //
        // This assertion used to be `root.contains("lnk off")` with `Ingest`
        // beside it, because /pull and /store were rendered disabled. They now
        // answer, so `Ingest` and `Store` are real links — asserting they are
        // still greyed out would be asserting the opposite of what shipped.
        //
        // The RULE the old assertion protected is unchanged and is still
        // checked: a page that does not exist is shown disabled rather than
        // hidden or linked. `/runs` is that page — there is no sweep yet — so
        // `lnk off` must still be here, and it must be Runs that carries it.
        assert!(
            root.contains("<a class=\"lnk\" href=\"/pull\">Ingest</a>"),
            "Ingest is a real link now: {root}"
        );
        assert!(
            root.contains("<a class=\"lnk\" href=\"/store\">Store</a>"),
            "Store is a real link now: {root}"
        );
        assert!(
            root.contains("<span class=\"lnk off\" title=\"not built yet\">Runs</span>"),
            "and Runs is still shown, disabled, because there is no sweep: {root}"
        );
        assert_eq!(
            root.matches("lnk off").count(),
            1,
            "exactly one page is still unbuilt"
        );

        // Both new pages answer, and the store page answers even though the
        // store root is empty — an absent manifest is the ordinary state
        // before the first ingest and must never be a 500.
        let ingest_page = get(addr, "/pull").await;
        assert!(ingest_page.contains("200 OK"), "{ingest_page}");
        assert!(ingest_page.contains("action=\"/pull/spot\""));
        assert!(ingest_page.contains("action=\"/pull/fno\""));
        let store_page = get(addr, "/store").await;
        assert!(store_page.contains("200 OK"), "{store_page}");
        assert!(!store_page.contains("500"), "{store_page}");
        assert!(store_page.contains("UNAVAILABLE"), "{store_page}");

        // A GET ON A POST ROUTE STARTS NOTHING. A crawler follows links and a
        // browser refetches on back; either would otherwise begin an ingest.
        for path in ["/pull/spot", "/pull/fno"] {
            let crawled = get(addr, path).await;
            assert!(
                crawled.contains("405"),
                "GET {path} must not be a route at all: {crawled}"
            );
            assert!(
                !crawled.contains("NOT STARTED"),
                "GET {path} must not even reach the parser: {crawled}"
            );
        }

        let missing = get(addr, "/nope").await;
        assert!(missing.contains("404"), "{missing}");

        let _ = tokio::net::TcpStream::connect(stop_addr).await;
        let outcome = served.await.expect("the serve task must not panic");
        assert!(outcome.is_ok(), "a graceful shutdown is not a failure");
    }

    /// A signal that has already fired.
    fn fired() -> Shutdown {
        Box::pin(std::future::ready(Ok(())))
    }

    /// One command line, owned.
    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Sends one form POST and reads the whole response.
    async fn post(addr: SocketAddr, path: &str, form: &str) -> String {
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: t\r\n\
             Content-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{form}",
            form.len()
        );
        tokio::task::spawn_blocking(move || {
            use std::io::Read as _;
            let mut s = std::net::TcpStream::connect(addr).expect("connect");
            s.write_all(request.as_bytes()).expect("write");
            let mut buf = String::new();
            s.read_to_string(&mut buf).expect("read");
            buf
        })
        .await
        .expect("the client thread must not panic")
    }

    /// Runs `body` against a live server over the agreeing fixture.
    async fn with_server<F, Fut>(name: &str, body: F)
    where
        F: FnOnce(SocketAddr) -> Fut,
        Fut: Future<Output = ()>,
    {
        let dir = agreeing(name);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let stopper = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let stop_addr = stopper.local_addr().expect("addr");
        let served = tokio::spawn(serve(
            listener,
            router(Loaded::new(site(name, &dir))),
            Box::pin(async move { stopper.accept().await.map(|_| ()) }),
        ));
        body(addr).await;
        let _ = tokio::net::TcpStream::connect(stop_addr).await;
        served
            .await
            .expect("task")
            .expect("a graceful shutdown is not a failure");
    }

    #[tokio::test]
    async fn a_malformed_or_backwards_window_is_refused_with_the_reason_named() {
        with_server("badwindow", |addr| async move {
            // A date that is not a date. The refusal names the FIELD and what
            // arrived, so an operator is not sent to guess which of four it
            // meant.
            let out = post(
                addr,
                "/pull/spot",
                "target=swept&from=08/01/2022&to=2022-02-08",
            )
            .await;
            assert!(out.contains("400 Bad Request"), "{out}");
            assert!(out.contains("REFUSED"), "{out}");
            assert!(out.contains("08/01/2022"), "it names what arrived: {out}");
            assert!(out.contains("YYYY-MM-DD"), "and what it wanted: {out}");
            assert!(
                out.contains("nothing was written"),
                "and that nothing happened: {out}"
            );

            // A day that does not exist reaches the calendar and is refused by
            // it, not by a second opinion here.
            let leap = post(
                addr,
                "/pull/spot",
                "target=swept&from=2023-02-29&to=2023-03-01",
            )
            .await;
            assert!(leap.contains("400 Bad Request"), "{leap}");
            assert!(leap.contains("2023-02-29"), "{leap}");

            // Backwards is refused, never silently swapped.
            let back = post(
                addr,
                "/pull/spot",
                "target=swept&from=2022-02-08&to=2022-01-08",
            )
            .await;
            assert!(back.contains("400 Bad Request"), "{back}");
            assert!(back.contains("runs backwards"), "{back}");
            assert!(
                back.contains("2022-02-08") && back.contains("2022-01-08"),
                "both ends are named: {back}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn a_valid_window_is_echoed_with_the_wire_date_and_still_starts_nothing() {
        with_server("goodwindow", |addr| async move {
            // A RANGE ACROSS A YEAR BOUNDARY. Four days, both ends included,
            // and the wire `toDate` is the day after the last one.
            let over = post(
                addr,
                "/pull/spot",
                "target=indices&from=2021-12-30&to=2022-01-02",
            )
            .await;
            assert!(
                over.contains("503 Service Unavailable"),
                "valid, and still not started: {over}"
            );
            assert!(over.contains("NOT STARTED"), "{over}");
            assert!(over.contains("Reference indices"), "{over}");
            assert!(
                over.contains("2021-12-30..=2022-01-02"),
                "the range is stated in Window's own inclusive notation: {over}"
            );
            assert!(
                over.contains("<td>4</td>"),
                "30, 31, 1, 2 — four days: {over}"
            );
            assert!(
                over.contains("2022-01-03"),
                "the non-inclusive toDate is the day AFTER, and is shown: {over}"
            );
            assert!(
                over.contains("not inclusive"),
                "and the correction is stated, not performed in silence: {over}"
            );
            assert!(
                over.contains("no vendor was contacted") || over.contains("No vendor is contacted"),
                "and it says nothing ran: {over}"
            );

            // A SINGLE-DAY RANGE, which is the commonest resume shape.
            let one = post(
                addr,
                "/pull/spot",
                "target=equities&from=2022-01-08&to=2022-01-08",
            )
            .await;
            assert!(one.contains("503"), "{one}");
            assert!(one.contains("<td>1</td>"), "one calendar day: {one}");
            assert!(one.contains("2022-01-09"), "and the wire date: {one}");

            // THE RECEIPT NAMES THE TARGET THAT WAS ASKED FOR, not whichever
            // one the lookup happened to land on. Each of the three is posted
            // and each comes back as itself; `cargo mutants` found that with
            // only one target exercised, the `==` selecting its member count
            // could be a `!=` and nothing would notice.
            for (slug, label) in [
                ("swept", "Swept indices"),
                ("indices", "Reference indices"),
                ("equities", "NIFTY Total Market equities"),
            ] {
                let form = format!("target={slug}&from=2022-01-08&to=2022-01-08");
                let out = post(addr, "/pull/spot", &form).await;
                assert!(out.contains(label), "{slug} must echo as {label}: {out}");
            }
            // The fixture has one index and one Total Market constituent, and
            // the receipt states each target's own population — so the three
            // answers are not interchangeable.
            let swept = post(
                addr,
                "/pull/spot",
                "target=swept&from=2022-01-08&to=2022-01-08",
            )
            .await;
            assert!(
                swept.contains("<th>Instruments covered</th><td>1</td>"),
                "the swept count is the swept count: {swept}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn the_fno_form_cannot_request_a_live_contract_over_http_either() {
        with_server("livecontract", |addr| async move {
            // 9998-12-31 is live under any clock this build can run on, and
            // 2020-01-30 has expired under every one of them. Neither depends
            // on when the test runs.
            let live = post(
                addr,
                "/pull/fno",
                "underlying=NIFTY&series=opt&expiry=9998-12-31&from=9998-12-01&to=9998-12-31",
            )
            .await;
            assert!(live.contains("400 Bad Request"), "{live}");
            assert!(
                live.contains("LIVE CONTRACT IS NEVER STORED"),
                "the rule is named, loudly: {live}"
            );
            assert!(live.contains("9998-12-31"), "{live}");

            let expired = post(
                addr,
                "/pull/fno",
                "underlying=nifty&series=fut&expiry=2020-01-30&from=2020-01-01&to=2020-01-30",
            )
            .await;
            assert!(expired.contains("503"), "expired is acceptable: {expired}");
            assert!(expired.contains("NIFTY"), "canonicalised: {expired}");
            assert!(expired.contains("2020-01-31"), "the wire date: {expired}");

            // A window past the expiry asks for bars that cannot exist.
            let past = post(
                addr,
                "/pull/fno",
                "underlying=NIFTY&series=fut&expiry=2020-01-30&from=2020-01-01&to=2020-02-05",
            )
            .await;
            assert!(past.contains("400"), "{past}");
            assert!(past.contains("no bars there"), "{past}");

            // An underlying with no derivative on it.
            let spot_only = post(
                addr,
                "/pull/fno",
                "underlying=RAJESHEXPO&series=fut&expiry=2020-01-30&from=2020-01-01&to=2020-01-30",
            )
            .await;
            assert!(spot_only.contains("400"), "{spot_only}");
            assert!(spot_only.contains("no F&amp;O series"), "{spot_only}");
        })
        .await;
    }

    #[tokio::test]
    async fn a_body_larger_than_this_server_reads_is_refused_and_never_parsed() {
        // `ingest.rs` says every parser it holds works over "a form body whose
        // length the server caps". It was true, and it was true by accident:
        // the cap was axum's 2 MiB default and no line here named it. A
        // dependency's default is a bound this repository does not own, so the
        // boundary now states the number and this is the test that proves the
        // number is the one in force.
        with_server("bodylimit", |addr| async move {
            // Just inside: a legitimate body still answers on its own merits.
            let ok = post(
                addr,
                "/pull/spot",
                "target=swept&from=2024-01-01&to=2024-01-31",
            )
            .await;
            assert!(
                !ok.contains("413"),
                "an ordinary form is nowhere near the bound: {ok}"
            );

            // Just outside: the field is padded past MAX_FORM_BYTES. It is
            // refused for its SIZE, before any parser sees it -- so the reply
            // carries neither the accepted page nor a named field refusal.
            let huge = format!(
                "target=swept&from=2024-01-01&to=2024-01-31&pad={}",
                "x".repeat(MAX_FORM_BYTES + 1)
            );
            let refused = post(addr, "/pull/spot", &huge).await;
            assert!(
                refused.contains("413"),
                "a body past the bound is refused loudly: {refused}"
            );
            assert!(
                !refused.contains("REFUSED ·"),
                "and it never reached the form parser: {refused}"
            );
        })
        .await;
    }

    #[test]
    fn the_ingest_page_counts_its_targets_and_never_draws_a_bar_over_nothing() {
        let dir = agreeing("pullpage");
        let site = site("pullpage", &dir);
        let html = pull_html(&site, day(2026, 8, 7));

        // Two forms, not one form with a dropdown.
        assert_eq!(html.matches("<form class=\"pull\"").count(), 2);
        assert!(html.contains("action=\"/pull/spot\">"));
        assert!(html.contains("action=\"/pull/fno\""));
        assert!(
            html.contains("method=\"post\""),
            "starting a pull is a POST"
        );
        assert!(!html.contains("method=\"get\""), "and never a GET: {html}");

        // The date fields are `type=date`, which submits YYYY-MM-DD.
        assert_eq!(
            html.matches("type=\"date\"").count(),
            5,
            "two on the spot form, three on the F&O form"
        );
        assert!(html.contains("max=\"2026-08-07\""), "no future spot window");
        assert!(
            html.contains("max=\"2026-08-06\""),
            "and the expiry stops a day earlier — a live contract is unofferable"
        );

        // The counts are the real ones from the loaded universe: the fixture
        // has NIFTY (an index, and swept) and RELIANCE (Total Market).
        assert!(html.contains("1 instrument(s)"), "{html}");
        assert!(html.contains("Swept indices"), "{html}");
        assert!(html.contains("NIFTY Total Market equities"), "{html}");

        // NOT A FABRICATED PROGRESS BAR. Nothing is running, so every counter
        // is a dash and the reason is named at the top of the page.
        assert!(html.contains("CAPTURE UNAVAILABLE"), "{html}");
        assert!(html.contains("pull::fetch"), "the missing module is named");
        assert!(html.contains("pull::rate"), "and so is the governor");
        assert!(
            !html.contains("<div class=\"cv\">0</div>"),
            "a zero would claim a measurement nobody took: {html}"
        );
        assert!(html.contains("<div class=\"cv\">—</div>"), "{html}");

        // Every drop reason the filter counts is on the page, by its own label.
        for reason in [
            "before the session open",
            "at or after the session close",
            "before the requested window",
            "after the requested window",
        ] {
            assert!(html.contains(reason), "{reason} must be shown: {html}");
        }
        // And the session bounds it filters on.
        assert!(html.contains("09:15") && html.contains("15:30"), "{html}");
        assert!(
            html.contains("375"),
            "375 bars in a regular session: {html}"
        );
    }

    #[test]
    fn the_store_page_renders_an_absent_manifest_rather_than_failing() {
        // The ordinary state of a fresh install: no manifest file at all. The
        // page must say what is missing and where it looked, and it must not
        // be an error — this is the page you look at to find out why.
        let dir = agreeing("storeabsent");
        let site = site("storeabsent", &dir);
        let html = store_html(&site, day(2026, 8, 7), 0);

        assert!(html.starts_with("<!doctype html>"));
        assert!(html.ends_with("</html>"));
        assert!(html.contains("UNAVAILABLE"), "{html}");
        assert!(html.contains("groww") && html.contains("dhan"), "{html}");
        assert!(html.contains("no manifest at"), "{html}");
        // The path that was looked at, named on the page. Asserted against the
        // scratch directory's own name rather than a literal prefix, because
        // `crate::scratch::path` owns the shape and a second spelling of it
        // here would be a test pinned to a naming rule it does not own.
        let looked_at = site
            .censuses
            .first()
            .expect("groww")
            .path
            .display()
            .to_string();
        assert!(html.contains(&looked_at), "the path: {html}");
        assert!(looked_at.contains("store-storeabsent"), "{looked_at}");
        assert!(html.contains("DEGRADED"), "and the badge says so: {html}");
        // Counters are dashes, never zeros: "nothing ingested" and "the counter
        // says zero" are different claims.
        assert!(html.contains("<div class=\"cv\">—</div>"), "{html}");
        // The grid still shows the instruments, with every cell a miss.
        assert!(html.contains("NSE-NIFTY"), "{html}");
        assert!(html.contains("class=\"num miss\""), "{html}");
    }

    #[test]
    fn the_store_page_reads_zero_as_zero_and_pages_past_the_end_by_clamping() {
        // A genesis manifest for each vendor: the store exists and holds
        // nothing. That is a real answer and a DIFFERENT one from an absence,
        // so the counters read 0 and the page is not degraded.
        let root = store_root("storezero");
        for vendor in Vendor::ALL {
            let header = pull::manifest::ManifestHeader::genesis(vendor);
            let mut bytes = vec![0u8; 32_768];
            bytes.splice(..64, header.image());
            std::fs::write(pull::manifest::manifest_path(&root, vendor), &bytes).expect("write");
        }
        let dir = agreeing("storezero");
        let site = Site::new(universe(&dir), census::read_all(&root), root);

        let html = store_html(&site, day(2026, 8, 7), 0);
        assert!(
            html.contains("<div class=\"cv\">0</div>"),
            "zero months: {html}"
        );
        assert!(html.contains("0 month(s), 0 row(s)"), "{html}");
        assert!(
            !html.contains("UNAVAILABLE"),
            "an empty store is not absent"
        );
        assert!(
            html.contains("badge good"),
            "and it is not degraded: {html}"
        );
        // Still nothing held, so every grid cell is a miss rather than a zero.
        assert!(html.contains("class=\"num miss\""), "{html}");

        // PAGING PAST THE END CLAMPS. `?page=999` is a stale bookmark, not an
        // attack, and it must land somewhere real.
        let first = store_html(&site, day(2026, 8, 7), 0);
        let past = store_html(&site, day(2026, 8, 7), 999);
        assert_eq!(past, first, "an out-of-range page clamps to the last one");
        assert!(past.contains("NSE-NIFTY"), "and still has rows: {past}");
        // The fixture's one index × 36 months fits one page, so there is no
        // pager at all — navigation that leads nowhere is worse than none.
        assert!(!first.contains("class=\"pager\""), "{first}");
    }

    #[test]
    fn the_store_grid_pages_when_it_is_larger_than_one_page() {
        // 200 rows per page against 36 months means the pager appears at six
        // instruments. Without a fixture that large the paging arms are code no
        // test enters.
        let dir = agreeing("storepager");
        let mut site = site("storepager", &dir);
        site.indices = (0..10)
            .filter_map(|i| {
                brutex_core::instrument::InstrumentKey::index(
                    brutex_core::instrument::Exchange::Nse,
                    &format!("IDX{i:02}"),
                )
                .ok()
            })
            .collect();
        assert_eq!(census::grid_rows(site.indices.len()), 360);

        let first = store_html(&site, day(2026, 8, 7), 0);
        assert!(first.contains("page 1 of 2"), "{first}");
        assert!(first.contains("next"), "{first}");
        assert!(!first.contains("previous"), "no previous to nowhere");
        assert!(first.contains("360 instrument-month(s)"), "{first}");
        assert!(first.contains("showing 200"), "{first}");

        let last = store_html(&site, day(2026, 8, 7), 1);
        assert!(last.contains("page 2 of 2"), "{last}");
        assert!(last.contains("previous"), "{last}");
        assert!(!last.contains("next &rarr;"), "{last}");
        assert!(last.contains("showing 160"), "the remainder: {last}");

        // And past the end clamps onto that last page exactly.
        assert_eq!(store_html(&site, day(2026, 8, 7), 99), last);
    }

    #[test]
    fn the_store_root_comes_from_the_environment_or_defaults_under_home() {
        assert_eq!(
            store_dir_from(Some("/somewhere/else".into()), Some("/home/who".into())),
            PathBuf::from("/somewhere/else"),
            "an explicit value wins"
        );
        assert_eq!(
            store_dir_from(None, Some("/home/who".into())),
            PathBuf::from("/home/who/.brutex/store")
        );
        assert_eq!(
            store_dir_from(None, None),
            PathBuf::from("."),
            "no HOME is a broken environment, not a supported one"
        );
        // And the environment is read in exactly one place, which is this one.
        // Asserted against the pure function fed the SAME environment rather
        // than against `served_store_root`, which calls `store_dir` itself:
        // comparing a function with its own caller cannot fail, and
        // `cargo mutants` proved it by replacing `store_dir` with an empty
        // path and passing.
        assert_eq!(
            store_dir(),
            store_dir_from(std::env::var_os("BRUTEX_STORE"), std::env::var_os("HOME")),
            "store_dir is exactly store_dir_from over the environment"
        );
        assert!(
            !store_dir().as_os_str().is_empty(),
            "and it always names somewhere; an empty path is not a store root"
        );
        assert_eq!(served_store_root(), store_dir(), "one reader, one answer");
    }

    #[test]
    fn a_clock_that_names_no_day_refuses_the_page_rather_than_guessing_one() {
        // An expiry gate compared against an invented "today" is a gate that
        // passes for the wrong reason, so there is no default. Both arms of the
        // one place that reads the clock are driven here by VALUE, because a
        // branch only a broken machine could enter is a branch no test can
        // hold — the same split `run_in` and `masters_dir_from` already use.
        let broken = ingest::ist_day(
            std::time::SystemTime::UNIX_EPOCH - std::time::Duration::from_hours(24),
        );
        let why = broken.clone().expect_err("no such day");
        let html = refusal_html("Ingest", &why);
        assert!(html.contains("REFUSED"), "{html}");
        assert!(html.contains("clock"), "{html}");
        assert!(html.contains("nothing was written"), "{html}");

        // ONE closure, driven through BOTH arms. A closure body that only the
        // passing arm can reach is a body no test enters, so the same
        // non-capturing renderer is handed to the refusing call and to the
        // succeeding one -- and the refusing call proves it was never invoked
        // by answering with the refusal page rather than with a date.
        let render = |d: Day| (axum::http::StatusCode::OK, d.to_string());
        let (code, page) = dated(broken, "Ingest", render);
        assert_eq!(code, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(page.contains("clock"), "{page}");
        assert!(
            !page.contains("1970-01-01"),
            "the closure must not have run: {page}"
        );

        // And the other arm hands the day straight through.
        let (code, page) = dated(Ok(day(2026, 8, 7)), "Ingest", render);
        assert_eq!(code, axum::http::StatusCode::OK);
        assert_eq!(page, "2026-08-07");
    }

    #[tokio::test]
    async fn each_spot_target_reports_its_own_population_and_not_a_neighbours() {
        // Found by `cargo mutants` while D-0038 was measuring `server.rs`: the
        // receipt looks the requested target up to state how many instruments
        // it covers, and on a fixture where all three populations happen to be
        // equal the `==` doing that lookup can be a `!=` with the suite green.
        //
        // INDIAVIX is an index and is NOT swept — `CLAUDE.md` §1 makes it
        // reference-only — so this fixture has one swept series, TWO index
        // series and one Total Market constituent, and the three answers are
        // three different numbers.
        let dir = masters(
            "targetcounts",
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,NIFTY,IDX,,NIFTY,,\n\
                 NSE,CASH,,INDIAVIX,IDX,,NIFTY,,\n\
                 NSE,CASH,,RELIANCE,EQ,EQ,INE002A01018,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}\
                 NSE,I,NA,INDEX,NIFTY,NIFTY,INDEX,NA,0001-01-01,,\n\
                 NSE,I,NA,INDEX,INDIAVIX,INDIA VIX,INDEX,NA,0001-01-01,,\n\
                 NSE,E,INE002A01018,EQUITY,RELIANCE,RELIANCE INDUSTRIES,ES,EQ,,,\n"
            )),
        );
        let built = site("targetcounts", &dir);
        assert_eq!(
            built.targets,
            [1, 2, 1],
            "one swept, two index series, one Total Market constituent"
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let stopper = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let stop_addr = stopper.local_addr().expect("addr");
        let served = tokio::spawn(serve(
            listener,
            router(Loaded::new(built)),
            Box::pin(async move { stopper.accept().await.map(|_| ()) }),
        ));

        for (slug, label, covered) in [
            ("swept", "Swept indices", 1),
            ("indices", "Reference indices", 2),
            ("equities", "NIFTY Total Market equities", 1),
        ] {
            let form = format!("target={slug}&from=2022-01-08&to=2022-01-08");
            let out = post(addr, "/pull/spot", &form).await;
            assert!(out.contains(label), "{slug} echoes as {label}: {out}");
            assert!(
                out.contains(&format!("<th>Instruments covered</th><td>{covered}</td>")),
                "{slug} covers {covered}: {out}"
            );
        }

        let _ = tokio::net::TcpStream::connect(stop_addr).await;
        served
            .await
            .expect("task")
            .expect("a graceful shutdown is not a failure");
    }

    #[test]
    fn the_dashboard_counts_each_universe_and_shouts_only_when_something_disagrees() {
        // Found by `cargo mutants` while D-0038 was measuring `server.rs`: the
        // dashboard's fold and its `disputes > 0` flag were reachable but not
        // distinguished, so `a + 1` could be `a * 1` and `> 0` could be `== 0`
        // with the suite green. Each figure is now pinned to its own number.
        //
        // The fixture is NIFTY — an index, and an F&O underlying — plus
        // RELIANCE, which is a Total Market constituent and an F&O underlying.
        let clean = universe(&agreeing("dashclean"));
        let html = dashboard_html(&clean);
        for (label, value) in [
            ("Tracked", "2"),
            ("NIFTY Total Market", "1"),
            ("F&amp;O underlyings", "2"),
            ("Indices", "1"),
            ("Confirmed by both feeds", "2"),
            ("Disagreements", "0"),
        ] {
            let expected =
                format!("<div class=\"ck\">{label}</div><div class=\"cv\">{value}</div>");
            assert!(html.contains(&expected), "{label} must be {value}: {html}");
        }
        assert!(
            !html.contains("card loud"),
            "nothing disagreed, so nothing shouts: {html}"
        );
        assert!(html.contains("badge good"), "{html}");

        // One disagreement, and the same figure is loud. Both sides of the
        // comparison, driven by data rather than by a constructed `Stat`.
        let disputed = universe(&masters(
            "dashloud",
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,CHOLAFIN,EQ,EQ,INE121A01024,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}NSE,E,INE121A08PJ0,EQUITY,CHOLAFIN,CHOLA,ES,EQ,,,\n"
            )),
        ));
        let loud = dashboard_html(&disputed);
        assert!(loud.contains("card loud"), "a disagreement shouts: {loud}");
        assert!(
            loud.contains("<div class=\"ck\">Disagreements</div><div class=\"cv\">1</div>"),
            "{loud}"
        );
        assert!(loud.contains("badge bad"), "{loud}");

        // THE FIGURE IS THE SUM OF BOTH KINDS OF DISAGREEMENT, and this is the
        // fixture that says so: CHOLAFIN is one ISIN conflict and FISTIPD3GP is
        // one eligibility conflict, so the answer is 2. With only one kind
        // present the addition could be a subtraction — `cargo mutants` found
        // exactly that.
        let both = universe(&masters(
            "dashboth",
            Some(&format!(
                "{GROWW_HEAD}\
                 NSE,CASH,,CHOLAFIN,EQ,EQ,INE121A01024,,\n\
                 NSE,CASH,,FISTIPD3GP,EQ,MF,INF090I01VS3,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}\
                 NSE,E,INE121A08PJ0,EQUITY,CHOLAFIN,CHOLA,ES,EQ,,,\n\
                 NSE,E,INF090I01VS3,EQUITY,FISTIPD3GP,FRANKLIN PLAN,ETF,EQ,,,\n"
            )),
        ));
        let two = dashboard_html(&both);
        assert!(
            two.contains("<div class=\"ck\">Disagreements</div><div class=\"cv\">2</div>"),
            "one identity conflict plus one eligibility conflict is two: {two}"
        );
    }

    #[tokio::test]
    async fn the_escape_hatch_is_read_off_the_query_string_and_not_off_its_presence() {
        // Found by `cargo mutants` while D-0038 was measuring `server.rs`: the
        // handler reads `all=1`, and with only one value ever requested over
        // HTTP the `==` could be a `!=` — so `?all=0` would widen and `?all=1`
        // would narrow, which is the page doing exactly the opposite of what
        // the link says.
        let dir = masters(
            "hatchhttp",
            Some(&format!(
                "{GROWW_HEAD}NSE,CASH,,RAJESHEXPO,EQ,EQ,INE343B01030,,\n"
            )),
            Some(&format!(
                "{DHAN_HEAD}NSE,E,INE343B01030,EQUITY,RAJESHEXPO,RAJESH EXPORTS,ES,EQ,,,\n"
            )),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let stopper = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let stop_addr = stopper.local_addr().expect("addr");
        let served = tokio::spawn(serve(
            listener,
            router(Loaded::new(Site::load(&dir, &store_root("hatchhttp")))),
            Box::pin(async move { stopper.accept().await.map(|_| ()) }),
        ));

        // RAJESHEXPO is in neither tracked universe, so it is the only kind of
        // row the filter actually removes.
        let widened = get(addr, "/instruments?all=1").await;
        assert!(widened.contains("NSE-RAJESHEXPO"), "{widened}");
        for narrow in ["/instruments", "/instruments?all=0", "/instruments?all=yes"] {
            let page = get(addr, narrow).await;
            assert!(
                !page.contains("NSE-RAJESHEXPO"),
                "{narrow} must NOT widen: {page}"
            );
        }

        let _ = tokio::net::TcpStream::connect(stop_addr).await;
        served
            .await
            .expect("task")
            .expect("a graceful shutdown is not a failure");
    }

    #[test]
    fn a_site_with_no_universe_still_shows_the_two_instruments_that_matter() {
        // The masters were never read, so the merge is empty and the coverage
        // grid would otherwise have no rows at all — hiding exactly the two
        // series the whole page exists for. It falls back to them, and the
        // missing master is still `UNAVAILABLE` on the page, so nothing is
        // hidden by the fall-back.
        let empty = masters("nomasters", None, None);
        let site = site("nomasters", &empty);
        assert_eq!(site.indices.len(), 2, "the engine surface, exactly");
        assert_eq!(site.targets, [0, 0, 0], "and no target has members");

        let html = store_html(&site, day(2026, 8, 7), 0);
        assert!(
            html.contains("NSE-NIFTY") && html.contains("NSE-BANKNIFTY"),
            "{html}"
        );
        assert!(
            html.contains("groww: UNAVAILABLE") && html.contains("dhan: UNAVAILABLE"),
            "the masters' own absence rides along: {html}"
        );

        // The ingest page then offers every target with a truthful zero.
        let ingest_html = pull_html(&site, day(2026, 8, 7));
        assert!(ingest_html.contains("0 instrument(s)"), "{ingest_html}");
    }

    #[test]
    fn a_window_whose_wire_date_does_not_exist_says_so_rather_than_wrapping() {
        // The vendor's `toDate` is the day after the operator's last day, and
        // after 9999-12-31 there is no such day. Refused by name in the fact
        // itself — a wrapped date would silently ask for a window ending in
        // 1970.
        let window =
            pull::session::Window::new(day(9990, 1, 1), day(9999, 12, 31)).expect("forwards");
        let facts = window_facts(window);
        let wire = facts
            .iter()
            .find(|&&(k, _)| k == "toDate on the wire")
            .map(|(_, v)| v.clone())
            .expect("the fact is always present");
        assert!(wire.starts_with("REFUSED — "), "{wire}");
        assert!(wire.contains("9999-12-31"), "{wire}");

        // An ordinary window states the day after, and says why it is that day.
        let ordinary = window_facts(
            pull::session::Window::new(day(2022, 1, 8), day(2022, 2, 8)).expect("forwards"),
        );
        let wire = ordinary
            .iter()
            .find(|&&(k, _)| k == "toDate on the wire")
            .map(|(_, v)| v.clone())
            .expect("present");
        assert!(wire.starts_with("2022-02-09"), "{wire}");
        assert!(wire.contains("not inclusive"), "{wire}");
    }

    #[test]
    fn an_expiry_gate_driven_by_value_answers_both_ways_without_a_clock() {
        // `fno_answer` is the half of the handler the expiry rule lives in, and
        // it takes the day rather than reading one, so both outcomes are pinned
        // rather than being properties of when the suite ran.
        let (code, page) = fno_answer(
            "underlying=NIFTY&series=fut&expiry=2026-07-30&from=2026-07-01&to=2026-07-30",
            day(2026, 8, 7),
        );
        assert_eq!(code, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(page.contains("NOT STARTED"), "{page}");
        assert!(
            page.contains("expired, checked against 2026-08-07"),
            "{page}"
        );

        let (code, page) = fno_answer(
            "underlying=NIFTY&series=fut&expiry=2026-08-07&from=2026-08-01&to=2026-08-07",
            day(2026, 8, 7),
        );
        assert_eq!(code, axum::http::StatusCode::BAD_REQUEST);
        assert!(page.contains("LIVE CONTRACT IS NEVER STORED"), "{page}");
    }

    #[tokio::test]
    async fn run_serves_until_the_signal_and_exits_zero() {
        // The server binds an ephemeral port, accepts nothing, and stops --
        // which is the whole path through `run` minus the waiting.
        assert_eq!(run(&argv(&["serve", "127.0.0.1:0"]), fired()).await, OK);
    }

    #[tokio::test]
    async fn run_refuses_an_address_it_cannot_bind_and_says_which() {
        let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = taken.local_addr().expect("addr");
        assert_eq!(
            run(&argv(&["serve", &addr.to_string()]), fired()).await,
            FAILED,
            "an address already in use is a refusal, not a silent retry"
        );
    }

    #[tokio::test]
    async fn health_answers_503_when_a_vendor_was_never_read() {
        // A monitor reads the status code and nothing else. 200 with `ok` on
        // the first line, beside `dhan: UNAVAILABLE` in the body, is the exact
        // shape of a green light over a half-read universe.
        let dir = masters(
            "health503",
            Some(&format!("{GROWW_HEAD}NSE,CASH,,NIFTY,IDX,,NIFTY,,\n")),
            None,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let stopper = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let stop_addr = stopper.local_addr().expect("addr");
        let served = tokio::spawn(serve(
            listener,
            router(Loaded::new(Site::load(&dir, &store_root("health503")))),
            Box::pin(async move { stopper.accept().await.map(|_| ()) }),
        ));

        let health = get(addr, "/health").await;
        assert!(health.contains("503"), "{health}");
        assert!(health.contains("DEGRADED"), "{health}");
        assert!(health.contains("dhan: UNAVAILABLE"), "{health}");

        let _ = tokio::net::TcpStream::connect(stop_addr).await;
        served
            .await
            .expect("task")
            .expect("a graceful shutdown is not a failure");
    }

    #[test]
    fn a_report_earns_zero_only_when_the_read_was_clean() {
        // Both arms, driven directly. Which directory `run` reads comes from
        // the environment and a test cannot set one here, so leaving this
        // branch inside `run` would make the OK arm reachable only from the
        // child process in tests/binary.rs -- and a branch only a subprocess
        // can enter is a branch this gate cannot hold.
        assert_eq!(reported(&agreeing("reported")), OK);
        let missing = masters(
            "reportedmissing",
            Some(&format!("{GROWW_HEAD}NSE,CASH,,NIFTY,IDX,,NIFTY,,\n")),
            None,
        );
        assert_eq!(reported(&missing), DEGRADED);
    }

    #[tokio::test]
    async fn run_reports_and_refuses_nonsense_with_different_codes() {
        // THE ASSERTIONS HERE USED TO BE UNFALSIFIABLE, and that is worth
        // spelling out because it looked like caution. `run` read
        // $HOME/.brutex/masters, which a test cannot set, so its answer was a
        // property of the machine: OK on an operator's laptop where the real
        // masters live, DEGRADED on a CI runner where they do not. The
        // assertions were written to pass under both -- `!= FAILED` and
        // `!= MISUSED` -- and `reported` returns ONLY OK or DEGRADED, so no
        // input, machine or environment could ever have failed them. A mutant
        // pinning `reported` to OK survived. That is a test that asserts
        // nothing, which CLAUDE.md §4 bans outright.
        //
        // `run_in` takes the directory, so the expected value is a property of
        // the files, which this test owns. Exact codes, both arms, deterministic
        // on every host.
        assert_eq!(
            run_in(&agreeing("runreport"), &argv(&["report"]), fired()).await,
            OK,
            "two masters that agree is a clean universe"
        );
        let missing = masters(
            "runreportmissing",
            Some(&format!("{GROWW_HEAD}NSE,CASH,,NIFTY,IDX,,NIFTY,,\n")),
            None,
        );
        assert_eq!(
            run_in(&missing, &argv(&["report"]), fired()).await,
            DEGRADED,
            "a vendor that was never read is a refused universe"
        );
        assert_eq!(run(&argv(&["--wat"]), fired()).await, MISUSED);
        assert_ne!(DEGRADED, OK, "a refused universe is not a success");
        assert_ne!(DEGRADED, FAILED, "the run worked; its ANSWER is refused");
        assert_ne!(DEGRADED, MISUSED);
    }
}
