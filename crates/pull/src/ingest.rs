//! The join: a vendor's files on one side, bars on disk on the other, and the
//! counter that says what landed.
//!
//! Every piece this calls already existed and was tested on its own —
//! [`crate::archive`] walks a folder, [`crate::csv`] decodes a row,
//! [`crate::fetch`] filters and converts, [`store::file`] writes,
//! [`crate::manifest`] counts. What did not exist was anything that ran them in
//! order, so nothing could actually be ingested — and then nothing that ran the
//! counter at all, so `/store` reported an em-dash over a store holding 194 bar
//! files.
//!
//! # The census is opened before the first bar is written, and committed once
//!
//! **One load before the member loop, one install after it.** A commit per
//! member would rewrite the whole file twelve thousand times for one folder,
//! and the counter's whole value is that reading it is one operation; paying
//! `O(census)` twelve thousand times to maintain an `O(1)` read is the shape
//! this layer exists to remove.
//!
//! The load happens **first**, before a single bar reaches the disk, because a
//! census this build cannot read is a run whose bars would land uncounted. That
//! is the one outcome worse than a run that refused outright: the bars are
//! real, the census says they are not, and the next run refetches them into a
//! store that correctly refuses the append. So a census that will not open
//! stops the run before it writes, and says which path and which refusal.
//!
//! # A counter row and a bar file describe the same slice, or neither does
//!
//! The [`crate::manifest::EntryKey`] is built from the same `(exchange,
//! segment, symbol, timeframe, month)` the [`StorePath`] is built from, in the
//! same function, from the same values — and the venue is parsed into
//! [`Exchange`] and [`Segment`] **before** the bar file is opened. A plan whose
//! segment is not one the census can key therefore stores nothing for that
//! member rather than storing bars nobody counts.
//!
//! `rows`, `first_ts_micros` and `last_ts_micros` are read back off the bar
//! file's own committed header after the append, not from the batch that was
//! offered. On a second window into a month that already holds bars the batch
//! is a suffix and the header is the whole file, and it is the file the census
//! is a census of.
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
//! **A run-level refusal is a failure too, and not an `Err`.** The error type
//! here belongs to [`crate::archive`] and every variant of it is about a
//! folder; a census that will not open is not about the folder, and dressing
//! it as one would name the wrong layer. Such a refusal is one entry in
//! [`Ingested::failures`] naming the census path, with nothing stored — loud,
//! and `Ingested::balances` is false for it.
//!
//! # Cost
//!
//! One pass per member, one append per member, one hash probe and one 64-byte
//! entry per member that stored anything. The append is
//! `base + header + index·stride` — arithmetic, not a search. Enumerating the
//! folder is O(members), which is inherent to a bulk import and is stated in
//! [`crate::archive`] rather than dressed up.
//!
//! The census install is `O(entries)` **once per run**: it re-images every
//! committed entry, including the ones this run did not touch. That is the
//! documented cost of [`Manifest::image`], taken deliberately in exchange for
//! an install that is one `rename`. The incremental alternative —
//! [`Manifest::record`]'s [`crate::manifest::Append`], one 64-byte positional
//! write per member and one commit — is what a per-member publish would use,
//! and this deliberately does not publish per member.
//!
//! A run that changed nothing installs nothing, so a re-run leaves the census
//! byte for byte as it was. `CLAUDE.md` §3 rule 5 covers the counter as well as
//! the bars.

use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use brutex_core::instrument::{Exchange, Segment};
use brutex_core::vendor::Vendor;
use store::file::BarFile;
use store::path::{FileKind, PathParts, StorePath, Timeframe};

use crate::archive::{self, Member};
use crate::csv::Columns;
use crate::fetch::{self, BarRequest};
use crate::manifest::{ENTRY_STRIDE, Entry, EntryKey, HEADER_LEN, MAX_ENTRIES, Manifest};
use crate::session::DropCensus;
use crate::vendor::{PriceScale, TimestampEncoding};

/// The largest census this reader will hold in memory.
///
/// Derived, not chosen: `HEADER_LEN + MAX_ENTRIES × ENTRY_STRIDE` is the
/// largest file [`Manifest::image`] can produce, because
/// `ManifestHeader::advance` refuses a counter past
/// [`MAX_ENTRIES`] and every entry occupies exactly [`ENTRY_STRIDE`] bytes. A
/// file larger than that is not a census this build could have written, and
/// reading it to find that out is an allocation the size of whatever somebody
/// left at that path — which is not a bound, it is the absence of one.
const MAX_CENSUS_BYTES: u64 = HEADER_LEN + MAX_ENTRIES * ENTRY_STRIDE;

/// The derivation above, checked at compile time rather than in a comment.
const _: () = assert!(MAX_CENSUS_BYTES == 134_250_496);

/// What one member could not do, kept so the run can continue past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Which instrument.
    ///
    /// Or, for the two refusals that are the **run's** rather than one
    /// member's — a census that will not open, and one that will not install —
    /// the census path, because that is the thing an operator has to go and
    /// look at. Both are named in the same list so a caller cannot count
    /// member failures and believe it has counted every failure.
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
    /// Rows folded into a bar that was already open.
    ///
    /// **Consumed, not discarded, and not bars.** Both archive vendors ship
    /// two to four snapshots per second with no sub-second field, so a
    /// one-minute bucket swallows up to 240 rows and emits one bar. Every one
    /// of those rows is *in* the bar — its volume is summed into it, its high
    /// and low widen it, the last one is its close — so counting them as drops
    /// would say they were declined, and leaving them uncounted was what made
    /// [`Ingested::balances`] answer NO on a run where nothing was lost:
    /// 354,675 rows read, 62,978 bars stored, 170 dropped, and 291,527 rows
    /// with nowhere to be.
    ///
    /// It is a **measured** count taken at the fold — the length before minus
    /// the length after — and not `rows_read - bars_stored - drops`. Derived
    /// from the other three it would balance by construction and
    /// [`Ingested::balances`] would be a tautology rather than a check.
    pub rows_folded: usize,
    /// Members whose slice the census holds after this run.
    ///
    /// Recorded by this run, or already recorded with exactly these numbers.
    /// A member that stored bars is either in this count or named in
    /// [`Ingested::failures`]; there is no third place for it to be.
    pub counted: usize,
    /// Every row that did not become a bar, by reason.
    pub census: DropCensus,
    /// Members that failed, named. The run continued past each.
    pub failures: Vec<Failure>,
}

impl Ingested {
    /// Whether every row is accounted for: stored, folded into a bar that was
    /// already open, dropped, or in a member that failed.
    ///
    /// A row that vanished without landing in one of those four is
    /// indistinguishable from a row the vendor never sent, which is the
    /// failure this whole pipeline is shaped to prevent.
    #[must_use]
    pub fn balances(&self) -> bool {
        self.failures.is_empty()
            && self.rows_read == self.bars_stored + self.rows_folded + self.census.total() as usize
    }
}

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
    ///
    /// Also parsed into an [`Exchange`] for the census key, so `NSE` and `BSE`
    /// are the only two this build stores under — a directory the census
    /// cannot name is a directory `/store` can never report.
    pub exchange: &'a str,
    /// The instrument segment of the path.
    ///
    /// Also parsed into a [`Segment`], so `INDEX`, `CASH` and `FNO` are the
    /// only three this build stores under, for the same reason.
    pub segment: &'a str,
}

/// Reads every CSV in `dir`, writes the bars that survive into the store, and
/// publishes one census of what landed.
///
/// # Errors
///
/// Whatever [`archive::read_dir`] refuses — a missing folder, a wrong column
/// shape, a member that is not text. Those are **run-level**: the folder or the
/// declared shape is wrong, and every member shares both.
///
/// A failure on **one** member is counted in [`Ingested::failures`] and the run
/// continues. One malformed contract out of twelve thousand must not discard
/// the other eleven thousand nine hundred and ninety-nine. A census that will
/// not open or will not install is counted there too, named by its path — see
/// the module header for why it is not an `Err`.
pub fn from_dir(
    dir: &Path,
    store_root: &Path,
    plan: Plan<'_>,
) -> Result<Ingested, archive::ArchiveError> {
    // Only the column layout and the vendor are needed here; `one`
    // destructures the rest.
    let Plan {
        columns, vendor, ..
    } = plan;
    let members = archive::read_dir(dir, columns)?;
    let mut done = Ingested {
        members: members.len(),
        ..Ingested::default()
    };

    // THE CENSUS FIRST, BEFORE ANY BAR IS WRITTEN. A bar that cannot be
    // counted must not be stored: the store would then hold rows the census
    // denies, a later run would refetch them, and the append would be refused
    // for bars that are already there.
    let census_path = crate::manifest::manifest_path(store_root, vendor);
    let mut census = match read_census(&census_path, vendor) {
        Ok(census) => census,
        Err(why) => {
            // The folder was read, so say how much was in it. Reporting zero
            // rows would blame the vendor for a refusal that is ours.
            done.rows_read = members.iter().map(|member| member.rows.len()).sum();
            done.failures.push(Failure {
                instrument: census_path.display().to_string(),
                why,
            });
            return Ok(done);
        }
    };

    // A census that loaded degraded may still be appended to — recovering from
    // a torn commit *is* writing the next generation — but never silently.
    // What it recovered is a prefix: months a newer generation had committed
    // are not in it and cannot be got back from it, only from the bar files.
    let repairing = census.degraded_reason().is_some();
    if let Some(why) = census.degraded_reason() {
        done.failures.push(Failure {
            instrument: census_path.display().to_string(),
            why: format!(
                "the census loaded DEGRADED at generation {} after stepping \
                 over: {why}. This run's install is the repair, and any month \
                 a newer generation had committed is not in it — that is \
                 recoverable only by walking bars/",
                census.header().generation
            ),
        });
    }
    let mut publish = repairing;

    for member in &members {
        done.rows_read += member.rows.len();
        match one(member, store_root, plan) {
            Ok(landed) => {
                done.bars_stored += landed.bars;
                done.rows_folded += landed.folded;
                // The census is folded so the totals describe the RUN. A
                // per-member census would answer "why did this contract drop
                // rows" and the operator is asking "why did this window".
                for reason in [
                    crate::session::DropReason::BeforeSessionOpen,
                    crate::session::DropReason::AtOrAfterSessionClose,
                    crate::session::DropReason::BeforeWindow,
                    crate::session::DropReason::AfterWindow,
                ] {
                    for _ in 0..landed.census.of(reason) {
                        done.census.count(reason);
                    }
                }
                if let Some(entry) = landed.entry {
                    match count(&mut census, entry) {
                        Ok(changed) => {
                            done.counted += 1;
                            publish |= changed;
                        }
                        // THE WORST OUTCOME THERE IS, AND IT IS NAMED. The
                        // bars are on the disk and the census does not count
                        // them, so a later run refetches a month the store
                        // already holds and the append refuses it. Swallowing
                        // this would leave a store that disagrees with its own
                        // counter and no record of when it started.
                        Err(why) => done.failures.push(Failure {
                            instrument: member.instrument.clone(),
                            why: format!(
                                "{} holds {} bar(s) the census does not count: {why}",
                                member.instrument, entry.rows
                            ),
                        }),
                    }
                }
            }
            Err(why) => done.failures.push(Failure {
                instrument: member.instrument.clone(),
                why,
            }),
        }
    }

    // ONE INSTALL, AFTER THE LOOP — and none at all when nothing changed, so
    // a re-run of the same folder leaves the census byte for byte as it was.
    if publish && let Err(why) = install(&census_path, &census.image()) {
        done.failures.push(Failure {
            instrument: census_path.display().to_string(),
            why: format!(
                "{} slice(s) are on disk and the census that counts them was \
                 not published: {why}",
                done.counted
            ),
        });
    }

    Ok(done)
}

/// What one member put on disk, and the counter row that describes it.
struct Landed {
    /// Bars the member offered to the store.
    bars: usize,
    /// Snapshots that merged into a bar which was already open.
    folded: usize,
    /// Why rows were declined.
    census: DropCensus,
    /// The counter row for the month file, `None` when nothing was stored.
    entry: Option<Entry>,
}

/// One member: convert, fold, append, and describe the month file that
/// resulted.
fn one(member: &Member, store_root: &Path, plan: Plan<'_>) -> Result<Landed, String> {
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
    let mut landed = fetch::land(&raw, request, encoding, scale).map_err(|why| why.to_string())?;
    if landed.bars.is_empty() {
        return Ok(Landed {
            bars: 0,
            folded: 0,
            census: landed.census,
            entry: None,
        });
    }

    // THE FOLD. Without it a real run produced 354,675 rows and ZERO bars:
    // both archive vendors ship one-second snapshots with two to four rows per
    // second and no sub-second field, and the store correctly refuses two
    // records claiming the same instant.
    //
    // The bucket width comes from the TIMEFRAME the caller is filing under, so
    // a one-second feed, a one-minute feed and a daily feed all fold through
    // this same line. Nothing here knows which vendor it came from.
    let bucket = crate::fold::Bucket::of_secs(timeframe.secs())
        .ok_or("a timeframe of zero seconds has no bucket to fold into")?;
    let snapshots = landed.bars.len();
    landed.bars = crate::fold::fold(&landed.bars, bucket).map_err(|why| why.to_string())?;
    // Measured at the fold, where the two lengths are both in hand. `fold`
    // emits one bar per bucket that held anything and never invents one, so
    // this subtraction cannot go negative.
    let folded = snapshots.saturating_sub(landed.bars.len());

    // The month comes from the FIRST surviving bar. A member whose bars cross a
    // month boundary would need two files, and that split is a decision about
    // paths rather than about bars — so it is refused here by name rather than
    // silently filing December into November.
    //
    // `fold` returns at least one bar for an input that had at least one, and
    // the empty input already returned above — but the type does not say so,
    // and the arm that says "this cannot happen" is a `Landed` with nothing in
    // it rather than a panic.
    let (Some(first), Some(last)) = (landed.bars.first(), landed.bars.last()) else {
        return Ok(Landed {
            bars: 0,
            folded,
            census: landed.census,
            entry: None,
        });
    };
    let at = crate::session::IstMoment::from_epoch_secs(first.ts_micros.div_euclid(1_000_000))
        .map_err(|why| why.to_string())?;
    let ym = at.day().year_month().map_err(|why| why.to_string())?;
    let end = crate::session::IstMoment::from_epoch_secs(last.ts_micros.div_euclid(1_000_000))
        .map_err(|why| why.to_string())?;
    let end_ym = end.day().year_month().map_err(|why| why.to_string())?;
    if end_ym != ym {
        return Err(format!(
            "bars span {ym} to {end_ym}; the store addresses one month per \
             file and splitting is the caller's decision, not this one's"
        ));
    }

    let symbol = brutex_core::symbol::Symbol::new(&member.instrument)
        .map_err(|why| format!("{}: {why}", member.instrument))?;

    // THE VENUE IS PARSED BEFORE THE BAR FILE IS OPENED, and that order is the
    // point. `StorePath` accepts any upper-case segment, so a plan naming one
    // the census cannot key would write bars under a directory `/store` can
    // never report on. Refusing here means such a member stores nothing,
    // rather than storing bars nobody counts.
    let exchange = Exchange::parse(exchange)
        .map_err(|why| format!("{}: exchange {exchange:?}: {why}", member.instrument))?;
    let segment = Segment::parse(segment)
        .map_err(|why| format!("{}: segment {segment:?}: {why}", member.instrument))?;

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

    // THE PATH IS RENDERED FROM THE VALUES THE KEY IS BUILT FROM, not from the
    // plan's strings a second time. `exchange.as_str()` is the parsed
    // `Exchange` speaking, so the directory a bar lands in and the code the
    // census records cannot disagree — there is only one value.
    //
    // A consequence worth naming: with the venue a closed enum, the symbol a
    // `Symbol` (whose byte set is a subset of `store::path::check_segment`'s
    // and whose capacity is the same 24), the vendor a `Vendor`, the timeframe
    // one of `Timeframe::KNOWN` and the month a `YearMonth`, this refusal is
    // no longer reachable from here. It stays because `StorePath::new` is the
    // only thing that renders a path and a caller that skipped it would be
    // asserting the above rather than checking it — but it is a backstop now,
    // and `docs/06-limits.md` should not be told it is exercised.
    let parts = PathParts {
        vendor,
        exchange: exchange.as_str(),
        segment: segment.as_str(),
        symbol: symbol.as_str(),
        timeframe,
        month: ym,
        file: FileKind::Bars,
    };
    let path = StorePath::new(parts).map_err(|why| format!("{}: {why}", member.instrument))?;

    let mut file =
        BarFile::open_or_create(store_root, path, symbol_id).map_err(|why| why.to_string())?;
    file.append(&landed.bars).map_err(|why| why.to_string())?;

    // THE COUNTER ROW DESCRIBES THE FILE, NOT THE BATCH. Read back off the
    // header the append just committed, so a second window into a month that
    // already held bars records the whole month rather than the suffix that
    // was offered — and an `AlreadyPresent` append records what is there
    // rather than counting it twice.
    let header = file.header();
    Ok(Landed {
        bars: landed.bars.len(),
        folded,
        census: landed.census,
        entry: Some(Entry {
            key: EntryKey {
                exchange,
                segment,
                symbol,
                timeframe,
                month: ym,
            },
            rows: header.n_valid,
            first_ts_micros: header.first_ts_micros,
            last_ts_micros: header.last_ts_micros,
        }),
    })
}

/// Records one month in the census, unless it is already recorded exactly.
///
/// Returns whether anything changed, which is what decides whether the file is
/// rewritten at all.
///
/// # The probe is not an optimisation
///
/// The manifest's entry region is an append-only log, so re-recording a month
/// whose numbers have not moved appends a second row saying what the first row
/// already said. Two identical runs would then leave two different files, and
/// `CLAUDE.md` §3 rule 5 is about the bytes. One hash probe answers "is this
/// already exactly what is recorded", and equality is over the whole entry —
/// key, rows and both timestamps — so a month that grew by one bar is a
/// change and is recorded.
///
/// # Errors
///
/// Whatever [`Manifest::record`] refuses, in its own words: a row count that
/// went backwards, timestamps that did, or a census at its ceiling.
fn count(census: &mut Manifest, entry: Entry) -> Result<bool, String> {
    if census.entry(&entry.key) == Some(entry) {
        return Ok(false);
    }
    census.record(entry).map_err(|why| why.to_string())?;
    Ok(true)
}

/// Whether a file of `len` bytes is larger than this build could have written.
///
/// A named predicate rather than the comparison written inline, because the
/// boundary is the whole content of the check and **a file of exactly
/// [`MAX_CENSUS_BYTES`] is legal**: it is a census at [`MAX_ENTRIES`], which
/// `ManifestHeader::advance` accepts and refuses only one past. Written as
/// `>=` it would refuse the largest legal census, and no test that could
/// afford to build one would ever notice — materialising that file means a
/// 134 MB allocation, which is not a thing a unit test should do. So the
/// ceiling is verified as **arithmetic** here, exactly as `pull::manifest`
/// verifies its own, and `pull::ingest::tests` is the only place that can
/// reach it without one.
const fn beyond_ceiling(len: u64) -> bool {
    len > MAX_CENSUS_BYTES
}

/// The vendor's census, or why this run must not write.
///
/// # An absent file is a first ingest, and only an absent file
///
/// [`ErrorKind::NotFound`] and [`ErrorKind::NotADirectory`] are the two shapes
/// of *there is nothing at that path*: no file, or no directory to hold one.
/// Both are the ordinary state before a vendor's first ingest, and both are
/// answered with the empty slice, which [`Manifest::open_image`] turns into a
/// genesis census.
///
/// **Every other refusal stops the run.** A census that exists and cannot be
/// read must never be treated as one that does not exist: starting a fresh
/// census over a file that already holds one is the defect D-0036 closed — the
/// stale slot wins on generation, the new commit lands in the other, and the
/// result loads clean while being wrong.
///
/// # Errors
///
/// The path and the refusal, in one sentence, for a file that cannot be
/// measured, is larger than this build could have written, cannot be read, or
/// is not a census this build accepts.
fn read_census(path: &Path, vendor: Vendor) -> Result<Manifest, String> {
    let bytes = match fs::metadata(path) {
        // THE SIZE IS CHECKED BEFORE THE READ. `read` on a file this process
        // cannot hold is not an error it can report — it is an allocator
        // failure or an OOM kill, and neither reaches the operator as "that
        // census is too big".
        Ok(found) if beyond_ceiling(found.len()) => {
            return Err(format!(
                "{} is {} bytes; the largest census this build can write is \
                 {MAX_CENSUS_BYTES} and this reader refuses more",
                path.display(),
                found.len()
            ));
        }
        Ok(_) => {
            fs::read(path).map_err(|why| format!("{} could not be read: {why}", path.display()))?
        }
        Err(ref absent)
            if matches!(
                absent.kind(),
                ErrorKind::NotFound | ErrorKind::NotADirectory
            ) =>
        {
            Vec::new()
        }
        Err(why) => {
            return Err(format!(
                "{} could not be measured: {why}. A census that exists and \
                 cannot be read is not one that does not exist, and this run \
                 will not start a second census over it",
                path.display()
            ));
        }
    };
    Manifest::open_image(vendor, &bytes).map_err(|why| format!("{}: {why}", path.display()))
}

/// Publishes a whole census: write a temporary, flush it, rename it into place.
///
/// # Why a rename and not a write
///
/// [`Manifest::image`] is atomic as a buffer and says so; its **installation**
/// is this function's problem and a rename within one filesystem is the only
/// step that is atomic against a crash. Overwriting the live file in place
/// publishes a prefix — a header counting entries that are not there yet — and
/// while that prefix is *detected* on the next load
/// (`CounterExceedsRegion`, or a slot that fails its own CRC-32C), being
/// detectably broken is worse than never being broken.
///
/// The temporary sits in the census's own directory so the rename cannot cross
/// a filesystem, and the directory is flushed afterwards so the rename itself
/// survives the power going out.
///
/// # Errors
///
/// The path and the host's own words, for a directory that cannot be made, a
/// temporary that cannot be written or flushed, or a rename that is refused.
fn install(path: &Path, image: &[u8]) -> Result<(), String> {
    // THE CENSUS LOCK, and the reason it exists is measured rather than
    // imagined. Two POSTs fired at the same instant — one person, two browser
    // tabs — over two folders sharing no files at all:
    //
    //   both receipts said STORED
    //   both said "balances: yes — every row accounted for"
    //   40 bar files landed, all of run A and all of run B
    //   the census held 20 entries: RUN B ONLY
    //   6,433 bars, 48.3% of everything written and fsync-ed, invisible
    //
    // The install is a read-modify-write of one whole file, so the loser's
    // whole census is overwritten by a rename that never saw it. Each run's
    // books were internally perfect and both were wrong about the disk: a
    // correctness check that only inspects its own run cannot see a concurrent
    // one.
    //
    // `store::file` has taken an advisory `try_lock` on every INDIVIDUAL bar
    // file since it was written. The census — the one file two runs actually
    // contend over — was the only one nothing locked. This is that same call,
    // in the one place it mattered most.
    //
    // `try_lock` rather than a blocking lock: a second pull should be told it
    // is second, loudly, and not wait an unbounded time behind a run that may
    // be importing eleven thousand contracts.
    // The directory first: the lock lives beside the census, and on a first
    // ever run neither exists. `install_locked` creates it too, but that is
    // after this point and the lock cannot wait for it.
    // BEST EFFORT, and deliberately not `?`. The lock lives beside the census
    // and on a first ever run neither exists, so the directory is attempted
    // here — but a failure to create it is NOT this function's to report.
    // `install_locked` creates it too and names the fault properly, and a test
    // that puts a *file* where the directory belongs expects that sentence, not
    // this one. Swallowing here means the real refusal still reaches the
    // operator; refusing here would replace it with a worse-worded twin.
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let lock_path = path.with_extension("man.lock");
    let Ok(lock) = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    else {
        // The lock could not even be opened, which means the directory is not
        // one. Fall through: the install below fails on the same cause and says
        // so in the words the operator needs.
        return install_locked(path, image);
    };
    if lock.try_lock().is_err() {
        return Err(format!(
            "another pull holds the census lock at {}. Refused rather than \
             queued: two runs installing at once silently discard one, and the \
             loser's receipt still reads 'every row accounted for' because its \
             own books balanced. Wait for the other run and try again.",
            lock_path.display()
        ));
    }
    install_locked(path, image)
}

/// The install itself, once the census lock is held.
fn install_locked(path: &Path, image: &[u8]) -> Result<(), String> {
    // A rendered census path is `<root>/manifest/<vendor>.man`, so the parent
    // is always there; the fallback is the path itself, which cannot be
    // created as a directory and therefore fails loudly on the next line
    // rather than writing somewhere unexpected.
    let dir: &Path = path.parent().unwrap_or(path);
    let tmp: PathBuf = path.with_extension("man.writing");
    publish(dir, &tmp, path, image).map_err(|why| {
        format!(
            "{} could not be published through {}: {why}",
            path.display(),
            tmp.display()
        )
    })
}

/// The five calls [`install`] is, kept apart from the sentence it fails with.
fn publish(dir: &Path, tmp: &Path, path: &Path, image: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut file = fs::File::create(tmp)?;
    file.write_all(image)?;
    // The bytes before the rename, always. A rename that beats its own
    // contents to the disk publishes a name over nothing.
    file.sync_all()?;
    drop(file);
    fs::rename(tmp, path)?;
    fs::File::open(dir)?.sync_all()
}

/// The one fact about this module no external test can reach.
///
/// Everything else here is proved from outside, in
/// `crates/pull/tests/census.rs`, because that is where a caller stands — a
/// folder goes in, bars and a counter come out, and the two are checked
/// against each other. This one is here because reaching it from outside means
/// putting a 134,250,496-byte file on the disk and reading it into memory, and
/// a boundary that is only ever tested one side of is a boundary nobody has
/// checked. `pull::manifest` makes the same argument about [`MAX_ENTRIES`] and
/// verifies it the same way.
#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "the same exception every test module in this workspace takes: a \
              test that cannot panic cannot fail."
)]
mod tests {
    use super::{MAX_CENSUS_BYTES, beyond_ceiling};

    /// The largest census this build can write is accepted; one byte more is
    /// not.
    #[test]
    fn the_ceiling_admits_the_largest_census_this_build_can_write() {
        assert_eq!(
            MAX_CENSUS_BYTES, 134_250_496,
            "32,768 bytes of header region and 2,097,152 entries of 64 bytes"
        );
        assert!(
            !beyond_ceiling(MAX_CENSUS_BYTES),
            "a census at exactly MAX_ENTRIES is one this build writes, and \
             refusing it would refuse a legal file"
        );
        assert!(!beyond_ceiling(MAX_CENSUS_BYTES - 1));
        assert!(
            beyond_ceiling(MAX_CENSUS_BYTES + 1),
            "and one byte past it is not a census this build could have made"
        );
        assert!(beyond_ceiling(u64::MAX));
        assert!(!beyond_ceiling(0), "an empty file is a first ingest");
    }
}
