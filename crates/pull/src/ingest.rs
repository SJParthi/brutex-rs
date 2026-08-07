//! The join: a vendor's files on one side, bars on disk on the other.
//!
//! Every piece this calls already existed and was tested on its own —
//! [`crate::archive`] walks a folder, [`crate::csv`] decodes a row,
//! [`crate::fetch`] filters and converts, [`store::file`] writes. What did not
//! exist was anything that ran them in order, so nothing could actually be
//! ingested.
//!
//! # What this refuses to do
//!
//! **It does not decide which bars are wanted.** The window and the cadence
//! arrive from the caller and go straight to [`crate::fetch::land`], so the
//! session rule lives in one place. A second filter here would be a second
//! answer to "is this bar in the session", and the two would disagree the
//! first time either changed.
//!
//! **It does not group rows into a file.** One member becomes one append. A
//! member that spans a month boundary is the caller's problem, because the
//! store addresses per (instrument, timeframe, month) and splitting is a
//! decision about paths rather than about bars.
//!
//! # A member that fails does not stop the run
//!
//! One malformed contract out of twelve thousand should not discard the other
//! eleven thousand nine hundred and ninety-nine. So a member-level failure is
//! **counted and named** in [`Ingested::failures`] rather than returned, and
//! the run continues. That is the opposite of [`crate::archive::read_dir`]'s
//! rule, deliberately: a *decode* failure means the shape is wrong and every
//! member shares the shape, whereas a *write* failure is usually about one
//! path. The run reports both totals so neither hides in the other.
//!
//! # Cost
//!
//! One pass per member, one append per member. The append is
//! `base + header + index·stride` — arithmetic, not a search. Enumerating the
//! folder is O(members), which is inherent to a bulk import and is stated in
//! [`crate::archive`] rather than dressed up.

use std::path::Path;

use brutex_core::vendor::Vendor;
use store::file::BarFile;
use store::path::{FileKind, PathParts, StorePath, Timeframe};

use crate::archive::{self, Member};
use crate::csv::Columns;
use crate::fetch::{self, BarRequest};
use crate::session::DropCensus;
use crate::vendor::{PriceScale, TimestampEncoding};

/// What one member could not do, kept so the run can continue past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Which instrument.
    pub instrument: String,
    /// Why, in its own words.
    pub why: String,
}

/// What a run put on disk, and what it did not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ingested {
    /// Members read from the folder.
    pub members: usize,
    /// Rows the vendor's files held, before any filtering.
    pub rows_read: usize,
    /// Bars written to the store.
    pub bars_stored: usize,
    /// Every row that did not become a bar, by reason.
    pub census: DropCensus,
    /// Members that failed, named. The run continued past each.
    pub failures: Vec<Failure>,
}

impl Ingested {
    /// Whether every row is accounted for: stored, dropped, or in a member
    /// that failed.
    ///
    /// A row that vanished without landing in one of those three is
    /// indistinguishable from a row the vendor never sent, which is the
    /// failure this whole pipeline is shaped to prevent.
    #[must_use]
    pub fn balances(&self) -> bool {
        self.failures.is_empty()
            && self.rows_read == self.bars_stored + self.census.total() as usize
    }
}

/// Reads every CSV in `dir` and writes the bars that survive into the store.
///
/// # Errors
///
/// Whatever [`archive::read_dir`] refuses — a missing folder, a wrong column
/// shape, a member that is not text. Those are **run-level**: they mean the
/// folder or the declared shape is wrong, and every member shares both.
///
/// A failure on one member — a bad timestamp, a store refusal — is counted in
/// [`Ingested::failures`] and the run continues.
///
/// # Examples
///
/// ```no_run
/// # use std::path::Path;
/// # use pull::{csv::Columns, ingest, session::{Cadence, Day, Window}, fetch::BarRequest};
/// # use pull::vendor::{PriceScale, TimestampEncoding};
/// # use store::path::Timeframe;
/// let day = Day::new(2025, 7, 1)?;
/// let done = ingest::from_dir(
///     Path::new("/data/GFDLNFO_TICK_01072025/Futures/-III"),
///     Path::new("/store"),
///     Columns::Gdfl,
///     &BarRequest { window: Window::new(day, day)?, cadence: Cadence::Minute },
///     TimestampEncoding::EpochSecondsUtc,
///     PriceScale::Paisa,
///     Timeframe::MINUTE_1,
/// )?;
/// println!("{} bars from {} members", done.bars_stored, done.members);
/// # Ok::<(), Box<dyn core::error::Error>>(())
/// ```
/// Everything one run needs, as one value.
///
/// A struct rather than ten positional arguments, and not only to satisfy a
/// lint: THREE OF THEM ARE `&str` AND ADJACENT. `exchange` and `segment`
/// would transpose without a compiler complaint and the bars would land under
/// a path that looks plausible and is wrong. The same reasoning produced
/// `api::render::View` earlier in this codebase, for the same reason.
#[derive(Debug, Clone, Copy)]
pub struct Plan<'a> {
    /// Which column layout the files carry.
    pub columns: Columns,
    /// The window and cadence, passed straight to the filter.
    pub request: &'a BarRequest,
    /// How the vendor encodes its timestamps.
    pub encoding: TimestampEncoding,
    /// Whether prices arrive in rupees or paisa.
    pub scale: PriceScale,
    /// The timeframe the bars are filed under.
    pub timeframe: Timeframe,
    /// Which vendor prefix to write beneath.
    pub vendor: Vendor,
    /// The exchange segment of the path.
    pub exchange: &'a str,
    /// The instrument segment of the path.
    pub segment: &'a str,
}

/// Reads every CSV in `dir` and writes the bars that survive into the store.
///
/// # Errors
///
/// Whatever [`archive::read_dir`] refuses — a missing folder, a wrong column
/// shape, a member that is not text. Those are **run-level**: the folder or the
/// declared shape is wrong, and every member shares both.
///
/// A failure on **one** member is counted in [`Ingested::failures`] and the run
/// continues. One malformed contract out of twelve thousand must not discard
/// the other eleven thousand nine hundred and ninety-nine.
pub fn from_dir(
    dir: &Path,
    store_root: &Path,
    plan: Plan<'_>,
) -> Result<Ingested, archive::ArchiveError> {
    // Only the column layout is needed here; `one` destructures the rest.
    let Plan { columns, .. } = plan;
    let members = archive::read_dir(dir, columns)?;
    let mut done = Ingested {
        members: members.len(),
        ..Ingested::default()
    };

    for member in &members {
        done.rows_read += member.rows.len();
        match one(member, store_root, plan) {
            Ok((stored, census)) => {
                done.bars_stored += stored;
                // The census is folded so the totals describe the RUN. A
                // per-member census would answer "why did this contract drop
                // rows" and the operator is asking "why did this window".
                for reason in [
                    crate::session::DropReason::BeforeSessionOpen,
                    crate::session::DropReason::AtOrAfterSessionClose,
                    crate::session::DropReason::BeforeWindow,
                    crate::session::DropReason::AfterWindow,
                ] {
                    for _ in 0..census.of(reason) {
                        done.census.count(reason);
                    }
                }
            }
            Err(why) => done.failures.push(Failure {
                instrument: member.instrument.clone(),
                why,
            }),
        }
    }

    Ok(done)
}

/// One member: convert, then append. Returns bars written and what dropped.
fn one(member: &Member, store_root: &Path, plan: Plan<'_>) -> Result<(usize, DropCensus), String> {
    let Plan {
        request,
        encoding,
        scale,
        timeframe,
        vendor,
        exchange,
        segment,
        ..
    } = plan;
    let raw = fetch::RawWindow {
        rows: member.rows.clone(),
    };
    let landed = fetch::land(&raw, request, encoding, scale).map_err(|why| why.to_string())?;
    if landed.bars.is_empty() {
        return Ok((0, landed.census));
    }

    // The month comes from the FIRST surviving bar. A member whose bars cross a
    // month boundary would need two files, and that split is a decision about
    // paths rather than about bars — so it is refused here by name rather than
    // silently filing December into November.
    let first = landed.bars.first().ok_or("no bars")?;
    let at = crate::session::IstMoment::from_epoch_secs(first.ts_micros.div_euclid(1_000_000))
        .map_err(|why| why.to_string())?;
    let ym = at.day().year_month().map_err(|why| why.to_string())?;

    if let Some(last) = landed.bars.last() {
        let end = crate::session::IstMoment::from_epoch_secs(last.ts_micros.div_euclid(1_000_000))
            .map_err(|why| why.to_string())?;
        let end_ym = end.day().year_month().map_err(|why| why.to_string())?;
        if end_ym != ym {
            return Err(format!(
                "bars span {ym} to {end_ym}; the store addresses one month per \
                 file and splitting is the caller's decision, not this one's"
            ));
        }
    }

    let symbol = brutex_core::symbol::Symbol::new(&member.instrument)
        .map_err(|why| format!("{}: {why}", member.instrument))?;

    // The symbol id is a CROSS-CHECK the store stamps into the header and
    // verifies on every reopen -- never the index, which is arithmetic. Derived
    // from the name so the same instrument always yields the same id, because a
    // counter would give a different one on a rerun and CLAUDE.md §3 rule 5
    // requires the same inputs to give the same bytes.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the id is a CROSS-CHECK the store stamps in the header and \
                  verifies on reopen, never an index — any 32 bits of the hash \
                  serve, and taking the low half of a 64-bit FNV-1a is the \
                  standard folding. Derived from the name so a rerun yields the \
                  same id; a counter would not, and CLAUDE.md §3 rule 5 requires \
                  the same inputs to give the same bytes."
    )]
    let symbol_id = brutex_core::universe::fnv1a(symbol.as_str()) as u32;

    let parts = PathParts {
        vendor,
        exchange,
        segment,
        symbol: symbol.as_str(),
        timeframe,
        month: ym,
        file: FileKind::Bars,
    };
    let path = StorePath::new(parts).map_err(|why| format!("{}: {why}", member.instrument))?;

    let mut file =
        BarFile::open_or_create(store_root, path, symbol_id).map_err(|why| why.to_string())?;
    file.append(&landed.bars).map_err(|why| why.to_string())?;

    Ok((landed.bars.len(), landed.census))
}
