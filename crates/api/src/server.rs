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

use crate::{master, merge, render};
use brutex_core::universe::Universe;
use brutex_core::vendor::Vendor;
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

/// The most rows one page renders.
///
/// A page shows what a person can read; the rest cost nothing because they are
/// never touched, which is what keeps rendering O(rows shown).
const PAGE_ROWS: usize = 200;

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
}

impl Read {
    /// Whether this read is fit to be believed.
    ///
    /// A missing vendor counts. So does any disagreement. So does a listing
    /// class nobody recognises — that one is the difference between a routine
    /// bond and an alphabet moving under us.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        !self.unavailable
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
    Read {
        merged,
        notes,
        unavailable,
        unrecognised,
    }
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
    let tracked =
        |u: Universe| all || u.contains(Universe::TOTAL_MARKET) || u.contains(Universe::INDEX);
    // COUNTS OVER THE WHOLE TRACKED SET, never over the rendered page. A pill
    // that counts the 200 rows on screen says 52 when the answer is 208, and
    // looks authoritative doing it.
    let counts = read
        .merged
        .by_key
        .values()
        .filter(|e| tracked(e.universe))
        .fold(render::UniverseCounts::default(), |mut c, e| {
            c.all += 1;
            c.fno += usize::from(e.universe.contains(Universe::FNO));
            c.ntm += usize::from(e.universe.contains(Universe::TOTAL_MARKET));
            c.index += usize::from(e.universe.contains(Universe::INDEX));
            c
        });
    let total = counts.all;

    // The universe pill, applied HERE so it selects from the whole set rather
    // than hiding rows the page happened to load. An unrecognised value selects
    // everything: a stale bookmark should render the page, not an empty one.
    let selected = |u: Universe| match universe_filter {
        "fno" => u.contains(Universe::FNO),
        "ntm" => u.contains(Universe::TOTAL_MARKET),
        "idx" => u.contains(Universe::INDEX),
        _ => true,
    };

    let mut keys: Vec<_> = read
        .merged
        .by_key
        .iter()
        .filter(|(_, e)| tracked(e.universe) && selected(e.universe))
        .filter(|(k, _)| needle.is_empty() || k.to_string().to_uppercase().contains(&needle))
        .map(|(k, e)| (*k, *e))
        .collect();
    let matched = keys.len();
    // Swept instruments first, then a stable order. Sorting by the key itself
    // rather than by insertion makes the page byte-identical between reloads,
    // which a HashMap iteration order would not.
    // SORT ORDER IS PART OF THE URL, so a sorted page is linkable and a reload
    // shows the same thing. Swept instruments lead every order except when the
    // operator asked for a specific column — an implicit pin would silently
    // contradict the column they clicked.
    //
    // Every arm ends in the key itself, so the order is TOTAL: two rows with
    // equal ISINs still have one fixed order, and the page is byte-identical
    // between reloads. A HashMap's iteration order is not, which is why this
    // cannot be left to insertion.
    // A leading `-` means descending. Sorting the key list ascending and then
    // reversing keeps ONE ordering rule per column instead of two, so ascending
    // and descending can never disagree about how ties break.
    let (column, descending) = match sort.strip_prefix('-') {
        Some(base) => (base, true),
        None => (sort, false),
    };
    match column {
        "symbol" => keys.sort_unstable_by_key(|(k, _)| (k.underlying, *k)),
        "isin" => keys.sort_unstable_by_key(|(k, e)| (e.isin.map(|(_, i)| i), *k)),
        "universe" => keys.sort_unstable_by_key(|(k, e)| (e.universe.bits(), *k)),
        "kind" => keys.sort_unstable_by_key(|(k, _)| (k.kind, *k)),
        "vendors" => keys.sort_unstable_by_key(|(k, e)| (e.vendors, *k)),
        // "key", "" and anything unrecognised: the default order. An unknown
        // column is NOT an error -- a stale bookmark should still render.
        _ => keys.sort_unstable_by_key(|(k, _)| (!k.is_sweepable(), *k)),
    }
    if descending {
        keys.reverse();
    }

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
    let last_page = matched.saturating_sub(1) / PAGE_ROWS;
    let page = page.min(last_page);
    let rows: Vec<render::Row> = keys
        .into_iter()
        .skip(page * PAGE_ROWS)
        .take(PAGE_ROWS)
        .map(|(key, e)| render::Row {
            key,
            vendors: e.vendors,
            isin: e.isin.map(|(_, i)| i),
            conflict: e.conflict.map(|(_, i)| i),
            universe: e.universe,
        })
        .collect();

    // `total` is the whole universe; `matched` is what the filter selected.
    // Reporting the right one keeps the page honest about what it looked at.
    let (title, denominator) = if needle.is_empty() {
        (
            format!("brutex-rs · instruments · {}", read.status()),
            total,
        )
    } else {
        (
            format!(
                "brutex-rs · {} · search {query:?} · {matched} matched",
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

/// The instruments page.
async fn page(
    axum::extract::State(read): axum::extract::State<Loaded>,
    uri: axum::http::Uri,
) -> axum::response::Html<String> {
    let raw = uri.query().unwrap_or("");
    let typed = parse_query(raw);
    let sort = param(raw, "sort");
    let all = param(raw, "all") == "1";
    let u = param(raw, "u");
    let page = page_number(raw);
    axum::response::Html(instruments_html_from(&read, &typed, &sort, all, &u, page))
}

/// The health endpoint.
///
/// 200 only when the read is clean. A degraded universe answers 503, because a
/// monitor reads the status code and nothing else — this used to return 200
/// with `ok` on the first line while a vendor had never been read.
async fn health(
    axum::extract::State(read): axum::extract::State<Loaded>,
) -> (axum::http::StatusCode, String) {
    let (body, clean) = report_from(&read);
    let code = if clean {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (code, body)
}

/// The universe every request renders from, read once at startup.
///
/// `Arc` rather than a clone per request: [`Read`] owns a `HashMap` of every
/// instrument, and cloning it per request would trade one O(rows) cost for
/// another. An `Arc` clone is a refcount bump — constant, and the same bytes
/// are shared by every concurrent request.
///
/// Shared **immutably**, so there is no lock on the read path and no
/// contention that grows with concurrent readers.
pub type Loaded = std::sync::Arc<Read>;

/// Every route this server answers, serving from an already-loaded universe.
///
/// Takes the universe rather than a directory: a router holding a `PathBuf`
/// can only re-read, and re-reading per request is the O(rows) cost this split
/// exists to remove.
pub fn router(read: Loaded) -> axum::Router {
    axum::Router::new()
        .route("/", axum::routing::get(page))
        .route("/instruments", axum::routing::get(page))
        .route("/health", axum::routing::get(health))
        .with_state(read)
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
    let dir = masters_dir();
    match Command::parse(args) {
        Ok(Command::Report) => reported(&dir),
        Ok(Command::Serve(addr)) => match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                println!("brutex-rs api listening on http://{addr}/instruments");
                stopped(serve(listener, router(Loaded::new(universe(&dir))), shutdown).await)
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
        let dir = std::env::temp_dir().join(format!("brutex-server-{name}"));
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
            router(Loaded::new(universe(&dir))),
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

        let root = get(addr, "/").await;
        assert!(root.contains("instruments total"));

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
            router(Loaded::new(universe(&dir))),
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
        // `run` reads BRUTEX_MASTERS, and a test cannot set it -- `set_var` is
        // unsafe under edition 2024 and this crate forbids unsafe.
        //
        // So this test asserts only what it CONTROLS. It used to assert that
        // `report` returns DEGRADED, on the premise that the working directory
        // holds no master; that premise was always a property of the machine
        // rather than of the code, and it stopped holding the moment the
        // default moved to $HOME/.brutex/masters, where an operator's real
        // masters live. A test whose expected value depends on whether the
        // person running it has downloaded some files is not a test.
        //
        // `report` is exercised over a controlled directory by
        // `a_report_names_every_vendor_and_says_whether_it_is_clean`, and
        // end-to-end by `tests/binary.rs`, which CAN set the variable because
        // it spawns a real child process.
        // Two independent assertions rather than `a == OK || a == DEGRADED`:
        // whichever side of that `||` is true on this machine, the other never
        // evaluates, and an expression no test can reach is a region the 100%
        // gate cannot cover.
        let reported = run(&argv(&["report"]), fired()).await;
        assert_ne!(reported, FAILED, "report ran; it did not fail to run");
        assert_ne!(reported, MISUSED, "`report` is an understood command");
        assert_eq!(run(&argv(&["--wat"]), fired()).await, MISUSED);
        assert_ne!(DEGRADED, OK, "a refused universe is not a success");
        assert_ne!(DEGRADED, FAILED, "the run worked; its ANSWER is refused");
        assert_ne!(DEGRADED, MISUSED);
    }
}
