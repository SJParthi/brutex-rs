//! Reading a vendor instrument master from disk.
//!
//! # Columns are found by NAME, never by position
//!
//! The primary broker publishes 19 columns and ships **21** — `internal_trading_symbol`
//! and `is_intraday` are undocumented — and the documented *order* is wrong:
//! the docs list `lot_size` before `expiry_date`, the file has
//! `expiry_date,strike_price,lot_size`.
//!
//! A positional reader would therefore put a lot size where a strike belongs
//! and never fail, because both are numbers. Every field here is located by
//! header name, and a missing header is a refusal rather than a default.
//!
//! # Which names, though, is the vendor's business
//!
//! The name table itself lives in [`brutex_core::vendor`], reached through
//! [`Vendor::master_columns`]. It was here, as a `match` on the vendor, and it
//! could not stay: [`Vendor`] is `#[non_exhaustive]`, so a match on it in this
//! crate requires a wildcard arm that no test can reach and the coverage gate
//! can never satisfy. Moving the table to the crate that owns the enum makes
//! that match exhaustive and provable, and it puts the column names beside the
//! segment and instrument-type alphabets they belong with — one place to look
//! when a vendor renames a column, rather than one per reader.

use brutex_core::instrument::InstrumentKey;
use brutex_core::isin::Isin;
use brutex_core::vendor::{Decoded, Listing, MasterRow, Skip, Vendor, decode_master_row};
use std::collections::{BTreeMap, HashMap};

/// What one master file produced.
#[derive(Debug, Default)]
pub struct Loaded {
    /// Rows that decoded into an instrument this engine stores, each with the
    /// vendor's ISIN beside its key.
    pub kept: Vec<Listing>,
    /// Where each key sits in [`Self::kept`], so resolving one is a probe.
    ///
    /// # Why this is not a nicety
    ///
    /// The only way to find a listing was to walk `kept`. On the real Dhan
    /// master that is a scan of every kept row, on a request path, to answer a
    /// question `InstrumentKey` was built to answer in one step — its own
    /// documentation says equality and hashing are structural "which is what
    /// makes duplicate rejection a single probe rather than a scan", and
    /// nothing was probing.
    ///
    /// `CLAUDE.md` §3 rule 4 is a PER-OPERATION bound, so it does not soften
    /// because the constant happens to be small on today's universe. The index
    /// costs one `usize` per listing and is built once at startup.
    ///
    /// A duplicate key keeps the FIRST listing and is counted in
    /// [`Self::duplicate_keys`] rather than silently overwriting: two rows that
    /// reduce to one key is a fact about the vendor's file, and picking one in
    /// silence is how the wrong `securityId` would reach a request.
    by_key: HashMap<InstrumentKey, usize>,
    /// How many rows reduced to a key another row already held.
    pub duplicate_keys: usize,
    /// Rows declined, counted by reason.
    pub skipped: HashMap<&'static str, usize>,
    /// Every declined row that carried a parseable ISIN, and why it was
    /// declined.
    ///
    /// This is what makes an **eligibility** disagreement visible. Before it
    /// existed a declined row was dropped right here, so the merge could only
    /// ever compare rows both vendors had already agreed to keep, and it
    /// printed `0 conflicts` while the two masters disagreed about 62
    /// instruments that carried the same ISIN in both files. See
    /// [`crate::merge::Merged::eligibility`].
    pub declined: Vec<(Isin, Skip)>,
    /// Rows that could not be understood, with the line number and the reason.
    ///
    /// Held rather than dropped: a row we failed to parse is how an instrument
    /// silently vanishes. [`Loaded::errors_by_reason`] is what actually puts
    /// them on the page — for a long time this vector was allocated, formatted
    /// and then read only for its `.len()`, so "104 unreadable" was the whole
    /// of what an operator was ever told and the reason had to be grepped out
    /// of the raw CSV by hand.
    pub errors: Vec<(usize, String)>,
    /// Every listing class this engine does not recognise, and how often.
    ///
    /// Separate from [`Loaded::skipped`] because the count alone does not name
    /// the code, and the code is the entire diagnostic: "2,438 rows declined
    /// under `EQX`" says an alphabet moved, while "2,438 not an equity
    /// listing" says nothing at all. See
    /// [`brutex_core::vendor::Skip::UnrecognisedListingClass`].
    pub unrecognised: BTreeMap<String, usize>,
}

impl Loaded {
    /// The listing under a key, in one probe.
    ///
    /// This is the whole point of [`Self::by_key`]. Before it existed the only
    /// way to answer "what is this instrument's vendor id" was to walk
    /// [`Self::kept`] — a scan on a request path for a question
    /// `InstrumentKey` was designed to answer in one step.
    #[must_use]
    pub fn listing(&self, key: &InstrumentKey) -> Option<&Listing> {
        self.by_key.get(key).and_then(|at| self.kept.get(*at))
    }

    /// The vendor's own id for an instrument, in one probe.
    ///
    /// What a request actually needs: `securityId` for Dhan, `groww_symbol`
    /// for Groww. Returns `None` when this vendor does not list it, which a
    /// caller must refuse on rather than substitute — filing one broker's bars
    /// under another's prefix is what `Feed::store_vendor` calls destroying
    /// D-0019 irreversibly.
    #[must_use]
    pub fn vendor_id(&self, key: &InstrumentKey) -> Option<&brutex_core::vendor::VendorId> {
        self.listing(key).map(|l| &l.vendor_id)
    }

    /// Total rows declined.
    #[must_use]
    pub fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }

    /// The decline reasons and their counts, ordered so a report is stable.
    #[must_use]
    pub fn skipped_by_reason(&self) -> Vec<(&'static str, usize)> {
        let mut v: Vec<(&'static str, usize)> =
            self.skipped.iter().map(|(&k, &n)| (k, n)).collect();
        v.sort_unstable();
        v
    }

    /// Each distinct parse failure, how many rows hit it, and the first line
    /// that did.
    ///
    /// Grouped rather than listed row by row so the output is bounded without
    /// being truncated: every *reason* is named, always, and the line number
    /// is where to look. A bare total is what this replaces.
    #[must_use]
    pub fn errors_by_reason(&self) -> Vec<(&str, usize, usize)> {
        let mut by: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (line, reason) in &self.errors {
            let e = by.entry(reason.as_str()).or_insert((0, *line));
            e.0 += 1;
            e.1 = e.1.min(*line);
        }
        by.into_iter()
            .map(|(reason, (n, first))| (reason, n, first))
            .collect()
    }
}

/// Locates the columns this decoder needs, by name.
#[derive(Debug, Clone, Copy)]
struct Columns {
    /// Where the vendor writes its own instrument id.
    vendor_id: usize,
    exchange: usize,
    segment: usize,
    underlying: usize,
    trading_symbol: usize,
    instrument_type: usize,
    listing_class: usize,
    isin: usize,
    expiry: usize,
    strike: usize,
    option_side: Option<usize>,
}

impl Columns {
    /// Finds each required column in the header row.
    ///
    /// # Errors
    ///
    /// The name of the first column that is absent. A missing column is a
    /// refusal and never a default: a defaulted column reads an empty string
    /// for every row, which the decoder would report as thousands of routine
    /// skips rather than as the mapping bug it is.
    fn locate(header: &str, vendor: Vendor) -> Result<Self, String> {
        let idx: HashMap<&str, usize> = header
            .trim_end()
            .split(',')
            .enumerate()
            .map(|(i, name)| (name.trim(), i))
            .collect();
        let need = |n: &str| -> Result<usize, String> {
            idx.get(n)
                .copied()
                .ok_or_else(|| format!("no column {n:?}"))
        };
        let names = vendor.master_columns();
        Ok(Self {
            vendor_id: need(names.vendor_id)?,
            exchange: need(names.exchange)?,
            segment: need(names.segment)?,
            underlying: need(names.underlying)?,
            trading_symbol: need(names.trading_symbol)?,
            instrument_type: need(names.instrument_type)?,
            listing_class: need(names.listing_class)?,
            isin: need(names.isin)?,
            expiry: need(names.expiry)?,
            strike: need(names.strike)?,
            option_side: names.option_side.map(need).transpose()?,
        })
    }

    /// The highest column index this decoder reads.
    ///
    /// A row with fewer fields than this cannot be decoded, and must not be
    /// *guessed* at — see the shortfall check in [`load`].
    fn widest(&self) -> usize {
        // Folded from `exchange` rather than `max()`ed over everything,
        // because `Iterator::max` returns an `Option` whose `None` arm cannot
        // happen here — and a branch no test can enter is a branch nobody has
        // checked. `option_side` chains in only for the vendor that has one.
        [
            self.segment,
            self.underlying,
            self.trading_symbol,
            self.instrument_type,
            self.listing_class,
            self.isin,
            self.expiry,
            self.strike,
        ]
        .into_iter()
        .chain(self.option_side)
        .fold(self.exchange, usize::max)
    }
}

/// The largest master file this reader will pull into memory.
///
/// [`load`] reads the whole file before it looks at a single row, which is a
/// deliberate choice — the masters are read once at startup and a streaming
/// reader would buy nothing — but it was previously *unbounded*, so the size of
/// the allocation was whatever the vendor happened to serve.
///
/// Measured 2026-08-01: the real files are 33,990,514 B (Dhan, 200,461 rows)
/// and 19,224,497 B (Groww, 133,379 rows). 256 MiB is 7.9× the larger, which
/// leaves room for years of listing growth and still refuses a file that is
/// not a master at all. The refusal names the size, so an operator who
/// legitimately outgrows this sees the number rather than an out-of-memory
/// kill. D-0033.
pub const MAX_MASTER_BYTES: u64 = 256 * 1024 * 1024;

/// The longest single row this reader will split.
///
/// A row is split into a `Vec<&str>` before any field is examined, so an
/// unbounded row is an unbounded allocation *and* an unbounded number of
/// fields, both paid before [`decode_master_row`]'s own width gate is reached.
///
/// Measured 2026-08-01 over both real masters: the longest row is 486 bytes
/// (Dhan, 33 columns) and 269 bytes (Groww, 21 columns). 4,096 is 8.4× the
/// larger. A row above it is reported at its line number like any other
/// unreadable row — never dropped, never truncated and read anyway.
pub const MAX_ROW_BYTES: usize = 4096;

/// Reads a vendor master and decodes every row.
///
/// # Errors
///
/// A message naming what was wrong with the file itself — unreadable, empty,
/// larger than [`MAX_MASTER_BYTES`], or missing a required column.
pub fn load(path: &std::path::Path, vendor: Vendor) -> Result<Loaded, String> {
    // THE SIZE IS CHECKED BEFORE THE READ, NOT AFTER.
    //
    // `read_to_string` on a file this process cannot hold is not an error it
    // can report -- it is an allocator failure or an OOM kill, and neither
    // reaches the operator as "the master is too big". One `metadata` call is
    // the difference between a named refusal and a dead process.
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size > MAX_MASTER_BYTES {
        return Err(format!(
            "{}: {size} bytes; this reader holds at most {MAX_MASTER_BYTES}",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| "file is empty".to_owned())?;
    let cols = Columns::locate(header, vendor)?;

    let widest = cols.widest();
    let mut out = Loaded::default();
    for (n, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        // BEFORE THE SPLIT, NOT AFTER. `split(',').collect()` allocates one
        // pointer pair per comma, and `decode_master_row`'s own width gate
        // cannot run until that vector exists. `str::len` is a field read.
        if line.len() > MAX_ROW_BYTES {
            out.errors.push((
                n + 2,
                format!(
                    "row is {} bytes; this reader splits at most {MAX_ROW_BYTES}",
                    line.len()
                ),
            ));
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        // A ROW TOO SHORT TO HOLD THE COLUMNS IS AN ERROR, NOT A DEFAULT.
        //
        // Defaulting a missing field to `""` put the empty string into the
        // listing class, where the gate read it as a series it does not
        // recognise and declined a genuine share as routine business, with
        // `0 unreadable` beside it. `Columns::locate` already refuses a
        // missing HEADER for exactly this hazard; this is the same hazard one
        // row at a time, and the real masters are uniform — every Dhan row has
        // 33 fields and every Groww row has 21 — so a short row is an anomaly
        // and never the normal case.
        if f.len() <= widest {
            out.errors.push((
                n + 2,
                format!(
                    "row has {} field(s); the columns this vendor needs run to {}",
                    f.len(),
                    widest + 1
                ),
            ));
            continue;
        }
        let get = |i: usize| -> &str { f.get(i).copied().unwrap_or("") };
        let row = MasterRow {
            vendor_id: get(cols.vendor_id),
            exchange: get(cols.exchange),
            segment: get(cols.segment),
            underlying: get(cols.underlying),
            trading_symbol: get(cols.trading_symbol),
            instrument_type: get(cols.instrument_type),
            listing_class: get(cols.listing_class),
            isin: get(cols.isin),
            expiry: get(cols.expiry),
            strike_rupees: get(cols.strike),
            option_side: cols.option_side.map_or("", get),
        };
        match decode_master_row(vendor, row) {
            Ok(Decoded::Keep(l)) => {
                // The index is built as rows arrive rather than in a second
                // pass, so `kept` and `by_key` cannot disagree about what is
                // present.
                match out.by_key.entry(l.key) {
                    std::collections::hash_map::Entry::Occupied(_) => out.duplicate_keys += 1,
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(out.kept.len());
                        out.kept.push(l);
                    }
                }
            }
            // The reason text belongs to the Skip variant, in the crate that
            // owns it. A `match` here would need a wildcard -- Skip is
            // `#[non_exhaustive]` -- and a wildcard silently files every
            // variant added later under whatever label it happens to name.
            Ok(Decoded::Skipped(d)) => {
                *out.skipped.entry(d.reason.reason()).or_insert(0) += 1;
                if let Some(isin) = d.isin {
                    out.declined.push((isin, d.reason));
                }
                // The COUNT says an alphabet moved; the CODE says which one.
                if d.reason == Skip::UnrecognisedListingClass {
                    *out.unrecognised
                        .entry(row.listing_class.trim().to_owned())
                        .or_insert(0) += 1;
                }
            }
            Err(e) => out.errors.push((n + 2, e.to_string())),
        }
    }
    Ok(out)
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

    fn tmp(name: &str, body: &str) -> std::path::PathBuf {
        let p = crate::scratch::path(&format!("master-{name}.csv"));
        let mut f = std::fs::File::create(&p).expect("create");
        f.write_all(body.as_bytes()).expect("write");
        p
    }

    #[test]
    fn columns_are_found_by_name_not_position() {
        // The columns are deliberately in a DIFFERENT order from the docs and
        // carry two undocumented trailing fields, exactly like the real file.
        let body = "is_intraday,segment,exchange,instrument_type,groww_symbol,\
                    trading_symbol,series,isin,expiry_date,strike_price,\
                    underlying_symbol,internal_trading_symbol\n\
                    0,CASH,NSE,IDX,NSE-NIFTY,NIFTY,,NIFTY,,,,x\n";
        let p = tmp("byname", body);
        let got = load(&p, Vendor::Groww).expect("loads");
        assert_eq!(got.kept.len(), 1);
        assert_eq!(got.kept[0].key.underlying.as_str(), "NIFTY");
        assert!(got.kept[0].key.is_sweepable());
        assert_eq!(got.kept[0].isin, None, "an index has no ISIN");
        assert!(got.errors.is_empty());
    }

    #[test]
    fn a_missing_column_is_refused_and_named() {
        let p = tmp("missing", "exchange,segment,groww_symbol\nNSE,CASH,NSE-X\n");
        let err = load(&p, Vendor::Groww).expect_err("must refuse");
        // The FIRST missing column is named, whichever it is -- the point is
        // that it refuses and says which, never that it defaults.
        assert!(err.starts_with("no column"), "got {err}");
        assert!(err.contains("underlying_symbol"), "got {err}");
    }

    #[test]
    fn every_column_a_vendor_needs_is_required_and_named_when_absent() {
        // One case per required column, built by REMOVING that column from an
        // otherwise complete header. A column that could go missing without a
        // refusal would be read as an empty string on every row, and the
        // decoder would report that as thousands of routine skips rather than
        // as the mapping bug it is.
        for vendor in [Vendor::Groww, Vendor::Dhan] {
            let c = vendor.master_columns();
            let mut all = vec![
                c.vendor_id,
                c.exchange,
                c.segment,
                c.underlying,
                c.trading_symbol,
                c.instrument_type,
                c.listing_class,
                c.isin,
                c.expiry,
                c.strike,
            ];
            all.extend(c.option_side);
            // The complete header is accepted, or the cases below prove
            // nothing about which column was missing.
            let full = format!("{}\n", all.join(","));
            assert!(
                load(&tmp("full", &full), vendor).is_ok(),
                "the complete header must load"
            );
            for missing in &all {
                let header: Vec<&str> = all.iter().copied().filter(|n| n != missing).collect();
                let body = format!("{}\n", header.join(","));
                let err = load(&tmp("dropped", &body), vendor).expect_err("must refuse");
                assert!(
                    err.contains(missing),
                    "dropping {missing} must name it, got {err}"
                );
            }
        }
    }

    #[test]
    fn the_new_class_and_isin_columns_are_required_by_their_own_names() {
        // Each vendor spells them differently, and a file missing either is a
        // refusal that NAMES the column -- never a silent empty string, which
        // the decoder would report as thousands of routine skips.
        let groww = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,expiry_date,strike_price,groww_symbol";
        let err = load(&tmp("noseries", &format!("{groww},isin\n")), Vendor::Groww)
            .expect_err("must refuse");
        assert!(err.contains("\"series\""), "got {err}");
        let err = load(&tmp("noisin", &format!("{groww},series\n")), Vendor::Groww)
            .expect_err("must refuse");
        assert!(err.contains("\"isin\""), "got {err}");

        let dhan = "EXCH_ID,SEGMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,INSTRUMENT,SM_EXPIRY_DATE,STRIKE_PRICE,OPTION_TYPE,SECURITY_ID";
        let err = load(&tmp("noclass", &format!("{dhan},ISIN\n")), Vendor::Dhan)
            .expect_err("must refuse");
        assert!(err.contains("\"SERIES\""), "got {err}");
        let err = load(
            &tmp("nodhanisin", &format!("{dhan},SERIES\n")),
            Vendor::Dhan,
        )
        .expect_err("must refuse");
        assert!(err.contains("\"ISIN\""), "got {err}");
        // The side column is Dhan's alone, and it is required there.
        let err = load(
            &tmp(
                "noside",
                "EXCH_ID,SEGMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,INSTRUMENT,SM_EXPIRY_DATE,STRIKE_PRICE,ISIN,SERIES,SECURITY_ID\n",
            ),
            Vendor::Dhan,
        )
        .expect_err("must refuse");
        assert!(err.contains("\"OPTION_TYPE\""), "got {err}");
        // And a file WITHOUT `INSTRUMENT_TYPE` loads: it is the measurably
        // wrong column, so it is neither read nor required. D-0025.
        assert!(
            load(
                &tmp(
                    "noinstrtype",
                    "EXCH_ID,SEGMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,INSTRUMENT,SM_EXPIRY_DATE,STRIKE_PRICE,ISIN,SERIES,OPTION_TYPE,SECURITY_ID\n",
                ),
                Vendor::Dhan,
            )
            .is_ok()
        );
    }

    #[test]
    fn the_equity_gate_runs_on_a_real_file_and_the_bond_loses() {
        // The CHOLAFIN pair, in the order the real Dhan master has them: the
        // 7.5% NCD FIRST, the share second. Before the gate, insert-if-absent
        // resolved the ticker to the bond. Both lines are verbatim, and the
        // column the gate reads is SERIES -- `D1` for the NCD, `EQ` for the
        // share -- not the INSTRUMENT_TYPE beside it.
        let body = "EXCH_ID,SEGMENT,ISIN,INSTRUMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,INSTRUMENT_TYPE,SERIES,SM_EXPIRY_DATE,STRIKE_PRICE,OPTION_TYPE,SECURITY_ID\nNSE,E,INE121A08PJ0,EQUITY,CHOLAFIN,CHOLAMANDALAM IN & FIN CO,DEB,D1,,,,1333\nNSE,E,INE121A01024,EQUITY,CHOLAFIN,CHOLAMANDALAM IN & FIN CO,ES,EQ,,,,1333\n";
        let got = load(&tmp("cholafin", body), Vendor::Dhan).expect("loads");
        assert_eq!(got.kept.len(), 1, "only the share survives");
        assert_eq!(got.kept[0].key.underlying.as_str(), "CHOLAFIN");
        assert_eq!(
            got.kept[0].isin.map(|i| i.to_string()).as_deref(),
            Some("INE121A01024"),
            "and it is the SHARE, identified by its own ISIN"
        );
        assert_eq!(got.skipped.get("not an equity listing"), Some(&1));
        assert!(got.errors.is_empty());
        // The bond's ISIN survives the decline, so another vendor keeping the
        // same paper can be recognised as a disagreement.
        assert_eq!(got.declined.len(), 1);
        assert_eq!(got.declined[0].0.as_str(), "INE121A08PJ0");
        assert_eq!(got.declined[0].1, Skip::NotEquityListing);
        assert!(got.unrecognised.is_empty(), "D1 is a series we know");
    }

    #[test]
    fn an_unrecognised_series_is_recorded_under_the_code_itself() {
        // A count under a shared label cannot distinguish "NSE minted a debt
        // series" from "the vendor renamed the equity series". The CODE can.
        let body = "EXCH_ID,SEGMENT,ISIN,INSTRUMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,INSTRUMENT_TYPE,SERIES,SM_EXPIRY_DATE,STRIKE_PRICE,OPTION_TYPE,SECURITY_ID\nNSE,E,INE002A01018,EQUITY,RELIANCE,RELIANCE INDUSTRIES,ES,  EQX  ,,,,1333\nNSE,E,INE121A01024,EQUITY,CHOLAFIN,CHOLA,ES,EQX,,,,1333\nNSE,E,INE775A08105,EQUITY,MOTHERSON,MOTHERSON NCD,DEB,D1,,,,1333\n";
        let got = load(&tmp("unrecognised", body), Vendor::Dhan).expect("loads");
        assert_eq!(got.kept.len(), 0);
        assert_eq!(got.skipped.get("unrecognised listing class"), Some(&2));
        assert_eq!(got.skipped.get("not an equity listing"), Some(&1));
        // Trimmed, so the padded and unpadded forms are ONE code and not two.
        assert_eq!(got.unrecognised.get("EQX"), Some(&2));
        assert_eq!(got.unrecognised.len(), 1);
    }

    #[test]
    fn a_row_with_too_few_fields_is_unreadable_and_names_the_shortfall() {
        // A defaulted field landed the empty string in the listing class,
        // where the gate declined a genuine share as routine business with
        // `0 unreadable` beside it. `Columns::locate` already refuses a
        // missing HEADER for this hazard; this is the same hazard per row.
        let body = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\nNSE,CASH,,RELIANCE,EQ\nNSE,CASH,,CHOLAFIN,EQ,EQ,INE121A01024,,,NSE-X\n";
        let got = load(&tmp("shortrow", body), Vendor::Groww).expect("loads");
        assert_eq!(got.kept.len(), 1, "only the intact row decodes");
        assert_eq!(got.errors.len(), 1);
        assert_eq!(got.errors[0].0, 2, "the line is named");
        // Bound rather than called inside the failure message: a call there is
        // a region that only runs when the assertion FAILS, so no passing test
        // can ever cover it.
        let reason = &got.errors[0].1;
        assert!(
            reason.contains("row has 5 field(s)") && reason.contains("run to 9"),
            "got {reason}"
        );
        assert!(
            got.skipped.is_empty(),
            "a truncated share is not a routine decline: {:?}",
            got.skipped
        );
    }

    #[test]
    fn unreadable_rows_are_grouped_by_reason_with_the_first_line_that_hit_it() {
        // `errors` was allocated, formatted and read only for `.len()`, so an
        // operator was told `104 unreadable` and nothing else. `NIFTY 100` is
        // a real Dhan index ticker; a space is not a legal Symbol, and 104
        // rows of the real master are exactly that shape.
        let body = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\nNSE,CASH,,NIFTY 100,IDX,,NIFTY,,,NSE-X\nNSE,CASH\nNSE,CASH,,NIFTY 200,IDX,,NIFTY,,,NSE-X\n";
        let got = load(&tmp("grouped", body), Vendor::Groww).expect("loads");
        let by = got.errors_by_reason();
        assert_eq!(by.len(), 2, "two distinct reasons: {by:?}");
        assert_eq!(
            by,
            vec![
                ("malformed instrument identifier", 2, 2),
                (
                    "row has 2 field(s); the columns this vendor needs run to 9",
                    1,
                    3
                ),
            ],
            "every reason named, with its count and the FIRST line it hit"
        );
        assert!(
            Loaded::default().errors_by_reason().is_empty(),
            "and nothing to say when nothing failed"
        );
    }

    #[test]
    fn an_unreadable_or_empty_file_is_refused() {
        let missing = crate::scratch::path("does-not-exist.csv");
        assert!(load(&missing, Vendor::Groww).is_err());
        let p = tmp("empty", "");
        assert!(
            load(&p, Vendor::Groww)
                .expect_err("empty")
                .contains("empty")
        );
        // A file whose SIZE is fine but whose BYTES are not text. `metadata`
        // succeeds and the read is what fails, so both refusal arms are real.
        let raw = crate::scratch::path("master-notutf8.csv");
        std::fs::File::create(&raw)
            .expect("create")
            .write_all(&[0xFF, 0xFE, 0x00, 0x41])
            .expect("write");
        assert!(load(&raw, Vendor::Groww).is_err());
    }

    #[test]
    fn a_master_larger_than_this_reader_holds_is_refused_before_it_is_read() {
        // `read_to_string` on a file this process cannot hold is not an error
        // it can report -- it is an OOM kill. The size is checked first, and
        // the refusal names the number so an operator who legitimately outgrows
        // the bound is told what to raise rather than losing the process.
        //
        // Sparse: `set_len` allocates no blocks, so this costs no disk. The
        // file is never read -- the refusal happens before `read_to_string`,
        // which is exactly the property under test.
        //
        // THE PATH CARRIES THIS PROCESS'S ID. It used to be one fixed name in
        // the shared temporary directory, and the `remove_file` below then
        // failed with NotFound about one run in three: a second process running
        // the same test deleted the fixture while this one was inside the
        // 256 MiB read. See `crate::scratch`.
        let p = crate::scratch::path("master-toobig.csv");
        let f = std::fs::File::create(&p).expect("create");
        f.set_len(MAX_MASTER_BYTES + 1).expect("set_len");
        drop(f);
        let err = load(&p, Vendor::Groww).expect_err("refused");
        assert!(
            err.contains(&(MAX_MASTER_BYTES + 1).to_string())
                && err.contains(&MAX_MASTER_BYTES.to_string()),
            "the refusal names both numbers: {err}"
        );
        // Exactly at the bound is not over it.
        f_at_bound(&p);
        // The cleanup is an ASSERTION, not housekeeping: this fixture is
        // 256 MiB and nothing else may have removed it.
        std::fs::remove_file(&p).expect("cleanup");
    }

    /// A file of exactly [`MAX_MASTER_BYTES`] is refused for being empty of
    /// columns, not for its size — the boundary is `>` and not `>=`.
    fn f_at_bound(p: &std::path::Path) {
        let f = std::fs::File::create(p).expect("create");
        f.set_len(MAX_MASTER_BYTES).expect("set_len");
        drop(f);
        let err = load(p, Vendor::Groww).expect_err("still refused, for another reason");
        assert!(
            !err.contains("this reader holds at most"),
            "the size arm fired at the bound itself: {err}"
        );
    }

    #[test]
    fn a_row_longer_than_this_reader_splits_is_named_and_never_split() {
        // `split(',').collect()` allocates one pointer pair per comma, and
        // `decode_master_row`'s own width gate cannot run until that vector
        // exists. So the row length is bounded BEFORE the split, and the row is
        // reported at its line number like any other unreadable row -- never
        // dropped, never truncated and read anyway.
        let long = "X".repeat(MAX_ROW_BYTES + 1);
        let body = format!(
            "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\nNSE,CASH,,{long},EQ,EQ,INE002A01018,,,NSE-X\nNSE,CASH,,RELIANCE,EQ,EQ,INE002A01018,,,NSE-X\n"
        );
        let got = load(&tmp("longrow", &body), Vendor::Groww).expect("loads");
        assert_eq!(got.kept.len(), 1, "the intact row still decodes");
        assert_eq!(got.errors.len(), 1);
        assert_eq!(got.errors[0].0, 2, "the line is named");
        let reason = &got.errors[0].1;
        assert!(
            reason.contains("row is") && reason.contains(&MAX_ROW_BYTES.to_string()),
            "got {reason}"
        );
    }

    #[test]
    fn a_field_wider_than_core_will_read_is_an_error_and_not_a_silent_keep() {
        // The row that shipped before D-0033: a legitimate `underlying_symbol`
        // and an enormous `trading_symbol`, which is scanned by TEST_MARKERS
        // and then never becomes the identity. It used to be KEPT.
        let wide = "X".repeat(brutex_core::vendor::MAX_FIELD_BYTES + 1);
        let body = format!(
            "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\nNSE,CASH,RELIANCE,{wide},EQ,EQ,INE002A01018,,,NSE-X\n"
        );
        let got = load(&tmp("widefield", &body), Vendor::Groww).expect("loads");
        assert!(got.kept.is_empty(), "an over-wide row is never stored");
        assert_eq!(got.errors.len(), 1);
        // Bound rather than called inside the failure message: a call there is
        // a region that only runs when the assertion FAILS, so no passing test
        // can ever cover it.
        let reason = &got.errors[0].1;
        assert!(
            reason.contains("trading_symbol"),
            "the offending field is named: {reason}"
        );
    }

    #[test]
    fn skips_are_counted_by_reason_and_errors_keep_their_line_number() {
        let body = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\nBSE,CASH,,SENSEX,IDX,,,,,NSE-X\nNSE,COMMODITY,GOLD,GOLD,FUT,,,2026-08-05,,NSE-X\nNSE,FNO,031NSETEST,X,FUT,,,2036-11-27,,NSE-X\nNSE,CASH,,SOMEBOND,EQ,N2,INE002A01018,,,NSE-X\nNSE,CASH,,SOMESME,EQ,SM,INE002A01018,,,NSE-X\nNSE,FNO,NIFTY,X,ZZ,,,2026-08-04,1,NSE-X\n";
        let p = tmp("skips", body);
        let got = load(&p, Vendor::Groww).expect("loads");
        assert_eq!(got.kept.len(), 0);
        assert_eq!(got.skipped.get("foreign exchange"), Some(&1));
        assert_eq!(got.skipped.get("segment not stored"), Some(&1));
        assert_eq!(got.skipped.get("exchange test instrument"), Some(&1));
        assert_eq!(got.skipped.get("not an equity listing"), Some(&1));
        assert_eq!(got.skipped.get("SME board"), Some(&1));
        assert_eq!(got.skipped_total(), 5);
        assert_eq!(
            got.skipped_by_reason(),
            vec![
                ("SME board", 1),
                ("exchange test instrument", 1),
                ("foreign exchange", 1),
                ("not an equity listing", 1),
                ("segment not stored", 1),
            ],
            "a report needs a stable order, not a hash order"
        );
        assert_eq!(got.errors.len(), 1, "the ZZ type is unreadable");
        assert_eq!(got.errors[0].0, 7, "line numbers count the header");
    }

    #[test]
    fn a_short_row_is_an_error_and_never_an_index_panic() {
        let body = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\nNSE,CASH\n";
        let p = tmp("short", body);
        let got = load(&p, Vendor::Groww).expect("loads");
        // Too short to hold the columns -> an error naming the shortfall,
        // never an index panic and never a defaulted empty field.
        assert_eq!(got.errors.len(), 1);
        assert!(got.kept.is_empty() && got.skipped.is_empty());
    }

    #[test]
    fn dhan_columns_use_their_own_names() {
        let body = "EXCH_ID,SEGMENT,SECURITY_ID,ISIN,INSTRUMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,SERIES,SM_EXPIRY_DATE,STRIKE_PRICE,OPTION_TYPE\nNSE,I,13,NA,INDEX,NIFTY,NIFTY,NA,0001-01-01,,\nNSE,D,1,,OPTIDX,NIFTY,NIFTY,,2026-08-04,19450.00000,CE\n";
        let p = tmp("dhan", body);
        let got = load(&p, Vendor::Dhan).expect("loads");
        // The INDEX row is kept. The OPTION row is a LIVE contract and is
        // skipped by design -- backtests run on expired contracts from the
        // historical endpoints and the lake, never on the live chain.
        assert_eq!(got.kept.len(), 1, "only the index is stored");
        assert!(got.errors.is_empty());
        assert!(
            got.kept[0].key.is_sweepable(),
            "NIFTY from Dhan is sweepable"
        );
        assert_eq!(
            got.kept[0].isin, None,
            "`NA` in the ISIN column is not an ISIN"
        );
        assert_eq!(got.skipped_total(), 1, "the live option was declined");
    }

    #[test]
    fn blank_lines_are_ignored() {
        let body = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,series,isin,expiry_date,strike_price,groww_symbol\n\nNSE,CASH,,NIFTY,IDX,,NIFTY,,,NSE-X\n\n";
        let p = tmp("blank", body);
        let got = load(&p, Vendor::Groww).expect("loads");
        assert_eq!(got.kept.len(), 1);
        assert_eq!(got.errors.len(), 0);
    }
}
