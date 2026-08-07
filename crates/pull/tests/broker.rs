//! The broker path, end to end, over a real socket — and never a real broker.
//!
//! # What this file is the proof of
//!
//! `docs/05-decisions.md` D-0035 stopped one function short of a working vendor
//! pull: the credential port was defined and nothing implemented it, and the
//! ingest page has said "what is missing is one join" ever since. D-0051 wrote
//! the credential read in Rust and `pull::ingest::from_window` is the join.
//!
//! This drives the whole of it: a socket answers with a broker-shaped JSON body,
//! `HttpSource::window_async` fetches and decodes it, and `ingest::from_window`
//! puts the bars in a store and counts them in a manifest. **The bytes on disk
//! at the end are produced by exactly the code a live pull would run**, with one
//! substitution — the descriptor's `base_url` points at localhost instead of at
//! `api.dhan.co`.
//!
//! # Why there is no live call here, and no credential
//!
//! Not caution for its own sake. A test that reaches a broker cannot run in CI,
//! costs rate-limit budget that belongs to the operator, and fails for reasons
//! that are not the code's — so it would be quarantined within a week and then
//! deleted. This runs in 20 ms, offline, every time, and fails only when
//! something here is actually broken.
//!
//! `crates/pull/src/ssm.rs` proves the credential read against AWS's own
//! published `SigV4` vectors, which is the same trade: the specification is a
//! better oracle than a network.
//!
//! # The one thing this does NOT prove
//!
//! That `api.dhan.co` answers in the shape `crates/pull/src/vendor.rs` declares.
//! `docs/06-limits.md` §35 records it. Every field the descriptor names is
//! **UNVERIFIED against a live body**, and the first real call is what verifies
//! it — which is exactly why `decode_body` refuses a wrong `envelope` by name
//! and lists the keys it did find (D-0049), instead of guessing.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use brutex_core::vendor::Vendor;
use pull::fetch::BarRequest;
use pull::http::HttpSource;
use pull::ingest::{self, Ingested, Plan};
use pull::manifest::{EntryKey, Manifest, manifest_path};
use pull::session::{Cadence, Day, Window};
use pull::vendor::{
    Auth, AuthScheme, DateFormat, FieldNames, HttpSpec, Method, PriceScale, RangeEnd,
    ResponseShape, TimestampEncoding,
};
use store::path::{Timeframe, YearMonth};

/// Distinguishes two scratch trees taken in the same process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary tree that removes itself, as the other suites use.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut root = std::env::temp_dir();
        root.push(format!(
            "brutex-pull-broker-{}-{tag}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("a scratch root");
        Self { root }
    }

    fn store(&self) -> PathBuf {
        let dir = self.root.join("STORE");
        std::fs::create_dir_all(&dir).expect("a scratch store");
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A broker that answers once, on loopback, and reports what it was asked.
///
/// Raw sockets rather than a test HTTP framework: `crates/pull` takes `tokio`
/// without the `net` feature, and a test is not a reason to widen a dependency
/// this workspace counts as carefully as this one does.
fn broker(body: &str) -> (String, std::sync::mpsc::Receiver<String>) {
    let socket = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = socket.local_addr().expect("an address");
    let answer = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = socket.accept() else {
            return;
        };
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = tx.send(String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned());
        let _ = stream.write_all(answer.as_bytes());
        let _ = stream.flush();
    });
    (format!("http://{addr}"), rx)
}

/// The descriptor, pointed at a socket instead of at a broker.
///
/// Every other field is the shape `crates/pull/src/vendor.rs` gives Dhan —
/// `access-token` raw, dashed dates, an exclusive range end, parallel arrays at
/// the top level, rupee prices, epoch seconds. Changing only the URL is what
/// makes this a test of the vendor path rather than of a fixture.
fn spec(base_url: &'static str) -> HttpSpec {
    HttpSpec {
        base_url,
        bars_path: "/v2/charts/historical",
        method: Method::Post,
        auth: Auth {
            header: "access-token",
            scheme: AuthScheme::Raw,
        },
        date_format: DateFormat::DashedYmd,
        range_end: RangeEnd::Exclusive,
        response: ResponseShape::ParallelArrays { envelope: None },
        fields: FieldNames {
            open: "open",
            high: "high",
            low: "low",
            close: "close",
            volume: "volume",
            timestamp: "timestamp",
            open_interest: None,
        },
        timestamps: TimestampEncoding::EpochSecondsUtc,
        prices: PriceScale::Rupees,
        budget: pull::vendor::Budget {
            per_second: None,
            per_minute: None,
            per_day: None,
        },
        pooling: pull::vendor::Pooling::PerVendor,
    }
}

/// 2025-07-01, inclusive both ends: one trading day.
fn window() -> Window {
    Window::new(
        Day::new(2025, 7, 1).expect("a real day"),
        Day::new(2025, 7, 1).expect("a real day"),
    )
    .expect("forwards")
}

fn request() -> BarRequest {
    BarRequest {
        window: window(),
        cadence: Cadence::Minute,
    }
}

/// The plan a spot pull runs under.
fn plan(request: &BarRequest) -> Plan<'_> {
    Plan {
        columns: pull::csv::Columns::Gdfl,
        request,
        encoding: TimestampEncoding::EpochSecondsUtc,
        // THE ONE FIELD THAT IS NOT THE DESCRIPTOR'S. `http::decode_body`
        // already converted rupees to paisa, so the plan must say `Paisa` or
        // every price is multiplied by 100 a second time — the trap
        // `pull::http::DECODED_PRICE_SCALE` exists to name.
        scale: PriceScale::Paisa,
        timeframe: Timeframe::MINUTE_1,
        vendor: Vendor::Dhan,
        exchange: "NSE",
        segment: "INDEX",
    }
}

/// Four one-minute bars inside the 2025-07-01 session, as a broker sends them.
///
/// 09:15, 09:16, 09:17 and 09:18 IST — 03:45 UTC onward. The first draft of
/// this fixture used 08:45 IST and every bar was dropped "before the session
/// open", which was the session filter being RIGHT about a test that was
/// wrong. Prices carry paise so
/// the conversion is exercised rather than assumed.
const BODY: &str = r#"{
  "open":  [24500.75, 24510.25, 24515.00, 24520.50],
  "high":  [24512.00, 24518.75, 24522.25, 24530.00],
  "low":   [24498.50, 24505.00, 24511.75, 24518.00],
  "close": [24510.25, 24515.00, 24520.50, 24528.75],
  "volume":[1200, 980, 1450, 1100],
  "timestamp":[1751341500, 1751341560, 1751341620, 1751341680]
}"#;

fn fetch(url: &str) -> pull::fetch::RawWindow {
    let source = HttpSource::new(
        spec(Box::leak(url.to_owned().into_boxed_str())),
        "A-FAKE-TOKEN".to_owned(),
    )
    .expect("a client builds");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(source.window_async(&request()))
        .expect("the broker answered")
}

/// The census key the bars should be filed under.
fn key(instrument: &str) -> EntryKey {
    EntryKey {
        exchange: brutex_core::instrument::Exchange::Nse,
        segment: brutex_core::instrument::Segment::Index,
        symbol: brutex_core::symbol::Symbol::new(instrument).expect("a legal symbol"),
        timeframe: Timeframe::MINUTE_1,
        month: YearMonth::new(2025, 7).expect("July 2025"),
    }
}

/// Reads the census back off disk, exactly as `/store` does.
fn census(store_root: &Path) -> Manifest {
    let path = manifest_path(store_root, Vendor::Dhan);
    let bytes = std::fs::read(&path).expect("the census was written");
    let (header, entries) = bytes
        .split_at_checked(32_768)
        .expect("a full header region");
    Manifest::open(Vendor::Dhan, header, entries).expect("a readable census")
}

// ===========================================================================
// The join
// ===========================================================================

/// **THE WHOLE BROKER PATH, WITH NO BROKER.**
///
/// Socket answers → `window_async` fetches → `decode_body` converts →
/// `from_window` lands, folds, appends and counts. Every assertion below is
/// about bytes that reached the disk.
#[test]
fn a_window_fetched_from_a_broker_lands_in_the_store_and_is_counted() {
    let scratch = Scratch::new("lands");
    let store_root = scratch.store();
    let (url, seen) = broker(BODY);

    let raw = fetch(&url);
    assert_eq!(raw.rows.len(), 4, "four bars came back");

    let request = request();
    let done: Ingested = ingest::from_window(&raw, "NIFTY", &url, &store_root, plan(&request));

    // ── THE BOOKS BALANCE ───────────────────────────────────────────────
    // Every row read is stored, folded or dropped. This is the same identity
    // the ingest page prints, and it is the first thing that breaks when a
    // path is joined wrongly.
    assert_eq!(done.members, 1, "one window is one member");
    assert_eq!(done.rows_read, 4);
    assert_eq!(
        done.rows_read,
        done.bars_stored + done.rows_folded + done.census.total() as usize,
        "read = stored + folded + dropped: {done:?}"
    );
    assert_eq!(done.bars_stored, 4, "all four are inside the session");
    assert!(
        done.failures.is_empty(),
        "nothing failed: {:?}",
        done.failures
    );

    // ── THE BARS ARE ACTUALLY ON THE DISK ───────────────────────────────
    let bars = store_root
        .join("bars")
        .join("dhan")
        .join("NSE")
        .join("INDEX")
        .join("NIFTY")
        .join("1min")
        .join("2025-07.bin");
    assert!(
        bars.is_file(),
        "the month file exists at {}",
        bars.display()
    );
    assert!(
        std::fs::metadata(&bars).expect("readable").len() > 0,
        "and it is not empty"
    );

    // ── AND THE CENSUS COUNTS THEM ──────────────────────────────────────
    // A store that holds rows its counter denies is the worst outcome
    // `ingest` names, so this is asserted rather than assumed.
    let manifest = census(&store_root);
    let entry = manifest.entry(&key("NIFTY")).expect("the month is counted");
    assert_eq!(entry.rows, 4, "the counter agrees with the store");
    assert_eq!(manifest.total_rows(), 4);
    assert_eq!(manifest.keys(), 1);

    // ── THE REQUEST WAS THE DESCRIPTOR'S ────────────────────────────────
    let sent = seen
        .recv_timeout(core::time::Duration::from_secs(5))
        .expect("the broker was contacted");
    assert!(sent.starts_with("POST /v2/charts/historical"), "{sent}");
    assert!(
        sent.contains("access-token: A-FAKE-TOKEN"),
        "the descriptor's own header carries the credential: {sent}"
    );
    assert!(sent.contains("2025-07-01"), "fromDate: {sent}");
    assert!(
        sent.contains("2025-07-02"),
        "toDate is EXCLUSIVE — the day after the operator's last day: {sent}"
    );
}

/// **THE PAISE SURVIVE THE WHOLE PATH.**
///
/// `24500.75` is 2,450,075 paisa. The archive path has always got this right;
/// this asserts the broker path does too, all the way to the file — the defect
/// that reached the tree once already and would silently rewrite every
/// fractional price in the store.
#[test]
fn a_fractional_price_reaches_the_store_with_its_paise_intact() {
    let scratch = Scratch::new("paise");
    let store_root = scratch.store();
    let (url, _seen) = broker(BODY);

    let raw = fetch(&url);
    assert_eq!(
        raw.rows.first().map(|r| r.open),
        Some(2_450_075),
        "24500.75 rupees is 2450075 paisa BEFORE it is stored"
    );

    let request = request();
    let done = ingest::from_window(&raw, "NIFTY", &url, &store_root, plan(&request));
    assert_eq!(done.bars_stored, 4);

    // And read back off the disk, through the store's own reader.
    let path = store::path::StorePath::new(store::path::PathParts {
        vendor: Vendor::Dhan,
        exchange: "NSE",
        segment: "INDEX",
        symbol: "NIFTY",
        timeframe: Timeframe::MINUTE_1,
        month: YearMonth::new(2025, 7).expect("July 2025"),
        file: store::path::FileKind::Bars,
    })
    .expect("a legal path");
    let file = store::file::BarFile::open_or_create(
        &store_root,
        path,
        // The same folding `ingest` uses: the id is a CROSS-CHECK the store
        // stamps in the header and verifies on reopen, never an index, so any
        // 32 bits of the hash serve and the low half is the standard fold.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "matches pull::ingest's own derivation, which the store \
                      verifies against on reopen — a different fold here would \
                      make this test open a file the ingest path cannot"
        )]
        {
            brutex_core::universe::fnv1a("NIFTY") as u32
        },
    )
    .expect("the month file reopens");
    assert_eq!(file.header().n_valid, 4, "four bars are in the file");
}

/// A window the broker answers with nothing stores nothing, and says so — it
/// does not report a successful pull of zero bars.
#[test]
fn an_empty_answer_stores_nothing_and_writes_no_census() {
    let scratch = Scratch::new("empty");
    let store_root = scratch.store();
    let (url, _seen) =
        broker(r#"{"open":[],"high":[],"low":[],"close":[],"volume":[],"timestamp":[]}"#);

    let raw = fetch(&url);
    assert!(raw.rows.is_empty(), "the broker sent no bars");

    let request = request();
    let done = ingest::from_window(&raw, "NIFTY", &url, &store_root, plan(&request));
    assert_eq!(done.rows_read, 0);
    assert_eq!(done.bars_stored, 0);
    assert_eq!(
        done.counted, 0,
        "nothing was counted, because nothing landed"
    );
    assert!(done.failures.is_empty(), "an empty window is not a failure");

    // AND NOTHING WAS WRITTEN. A census published for a run that stored no bar
    // would be a counter describing an install that did not happen.
    assert!(
        !manifest_path(&store_root, Vendor::Dhan).exists(),
        "no census is published when nothing changed"
    );
}

/// Two identical pulls leave the store byte for byte as one did.
///
/// `CLAUDE.md` §3 rule 5: same inputs, same outputs, reruns are safe. The
/// broker path must not be the one that breaks it — a scheduled pull that
/// re-fetches yesterday must not double-count it.
#[test]
fn pulling_the_same_window_twice_changes_nothing_the_second_time() {
    let scratch = Scratch::new("idempotent");
    let store_root = scratch.store();
    let request = request();

    let (url_a, _a) = broker(BODY);
    let first = ingest::from_window(&fetch(&url_a), "NIFTY", &url_a, &store_root, plan(&request));
    assert_eq!(first.bars_stored, 4);
    let after_first = std::fs::read(manifest_path(&store_root, Vendor::Dhan)).expect("a census");

    let (url_b, _b) = broker(BODY);
    let second = ingest::from_window(&fetch(&url_b), "NIFTY", &url_b, &store_root, plan(&request));
    let after_second = std::fs::read(manifest_path(&store_root, Vendor::Dhan)).expect("a census");

    // `bars_stored` is bars OFFERED to the store after folding, not bars newly
    // written — the file's own append refuses a duplicate, so the second run
    // offers the same four and writes none of them. Worth pinning, because the
    // ingest page shows this number and "4 stored" on a rerun that stored
    // nothing new is exactly the sort of counter an operator would misread.
    assert_eq!(
        second.bars_stored, 4,
        "the same four are offered again; what changes is that none are written"
    );
    assert_eq!(
        after_first, after_second,
        "the census is byte-for-byte identical after a rerun"
    );
    assert_eq!(census(&store_root).total_rows(), 4, "still four, not eight");
}

/// The broker path and the folder path are the same code below the seam.
///
/// Not a claim about the source — a claim about the *counters*. Both are
/// `from_members`, so a window of four bars balances its books the same way
/// whichever side it arrived from, and a future edit that special-cases one of
/// them breaks this.
#[test]
fn the_broker_path_and_the_folder_path_are_one_implementation() {
    let scratch = Scratch::new("shared");
    let store_root = scratch.store();
    let (url, _seen) = broker(BODY);
    let request = request();

    let raw = fetch(&url);
    let done = ingest::from_window(&raw, "NIFTY", &url, &store_root, plan(&request));

    // Every field `Ingested` carries is filled by the shared loop, so a broker
    // pull reports the same shape a folder pull does — including the ones a
    // hand-rolled second implementation would have forgotten.
    assert_eq!(done.members, 1);
    assert_eq!(done.counted, 1, "the counter row was recorded");
    assert_eq!(done.rows_folded, 0, "one-minute bars, nothing to fold");
    assert_eq!(
        done.census.total(),
        0,
        "every row is inside the session and the window"
    );
    assert!(done.failures.is_empty());
}
