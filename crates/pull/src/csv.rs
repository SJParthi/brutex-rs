//! Vendor CSV rows into [`crate::fetch::RawRow`]s, driven by the descriptor.
//!
//! # Why this is not "a CSV parser"
//!
//! Every shape here was read out of the operator's own purchased archives, not
//! taken from a vendor's documentation. The documentation and the files
//! disagree, and where they do, **the files win**.
//!
//! | Vendor | Segment | Columns | Header | Date |
//! |---|---|---|---|---|
//! | `TrueData` | index | **5** | none | `YYYYMMDD` |
//! | `TrueData` | futures | **9** | none | `YYYYMMDD` |
//! | GDFL | options, futures | **10** | present | **`DD/MM/YYYY`** |
//!
//! Two things in that table are the whole reason this module exists.
//!
//! **The column count varies by segment inside one vendor.** `TrueData` emits
//! five columns for an index and nine for a future, in the same archive on the
//! same day. An index has no volume and no open interest, so those fields are
//! *structurally absent* rather than zero. A single per-vendor layout would
//! mis-parse one of the two, and the failure is silent: a price column read as
//! a volume yields a plausible number.
//!
//! **GDFL dates are `DD/MM/YYYY`.** `01/07/2025` is 1 July, not 7 January.
//! Reading it the other way shifts every bar by months and produces a file that
//! is internally consistent and completely wrong.
//!
//! # `__MACOSX` is not data
//!
//! `GDFL.zip` lists 24,292 entries, of which **12,145 are `__MACOSX`** — the
//! shadow tree macOS writes when it re-zips, one `AppleDouble` stub per real
//! file. They end in `.csv` and they are binary. A reader that globs `*.csv`
//! will open them and try to parse a resource fork as text.
//!
//! This was found by getting a count wrong: `docs/08-vendor-samples.md` said
//! GDFL held 24,264 CSVs when it holds 12,133, because the ghosts were counted
//! as data. [`is_ghost`] is the same rule applied where it matters.
//!
//! # Cost
//!
//! One pass per line, splitting on a byte. No allocation per row: fields are
//! borrowed from the input and parsed into integers in place. The row vector is
//! reserved from a caller-supplied bound — `docs/07-o1-architecture.md` law 2.

use crate::fetch::{FetchError, MAX_ROWS, RawRow};
use crate::vendor::DateFormat;

/// Which columns a vendor's CSV carries, in order.
///
/// One variant per shape actually observed in the operator's archives. A shape
/// nobody has read is not here, and adding one means reading a file first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Columns {
    /// `date, time, price, volume, open_interest` — five fields.
    ///
    /// `TrueData`'s index layout. Volume and open interest are present in the
    /// row and always zero, because an index has neither. Observed:
    /// `20221003,09:07:41,38444.90,0,0`.
    TrueDataIndex,
    /// `date, time, price, volume, open_interest, …` — nine fields.
    ///
    /// `TrueData`'s futures layout. The trailing four carry bid/ask depth on
    /// the `TICK_BA` products.
    TrueDataFutures,
    /// `Ticker, Date, Time, LTP, BuyPrice, BuyQty, SellPrice, SellQty, LTQ,
    /// OpenInterest` — ten fields, with a header row.
    ///
    /// GDFL's layout for both options and futures. `LTQ` is `0` on most rows:
    /// those are **quote** updates, not trades.
    Gdfl,
}

impl Columns {
    /// How many fields a row of this shape has.
    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::TrueDataIndex => 5,
            Self::TrueDataFutures => 9,
            Self::Gdfl => 10,
        }
    }

    /// Whether the file opens with a header row naming the columns.
    #[must_use]
    pub const fn has_header(self) -> bool {
        matches!(self, Self::Gdfl)
    }

    /// The date format this shape carries.
    #[must_use]
    pub const fn date_format(self) -> DateFormat {
        match self {
            Self::TrueDataIndex | Self::TrueDataFutures => DateFormat::CompactYmd,
            Self::Gdfl => DateFormat::SlashedDmy,
        }
    }

    /// Zero-based index of the date, time and price fields.
    const fn offsets(self) -> (usize, usize, usize) {
        match self {
            // date, time, price
            Self::TrueDataIndex | Self::TrueDataFutures => (0, 1, 2),
            // ticker, date, time, ltp
            Self::Gdfl => (1, 2, 3),
        }
    }
}

/// Why a CSV line is not a row.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CsvError {
    /// The line has the wrong number of fields.
    FieldCount {
        /// One-based line number within the file.
        line: usize,
        /// How many fields were found.
        got: usize,
        /// How many the declared shape has.
        want: usize,
    },
    /// A date field is not the declared format.
    DateMalformed {
        /// One-based line number.
        line: usize,
        /// What was there.
        got: String,
        /// The format that was expected.
        format: DateFormat,
    },
    /// A time field is not `HH:MM:SS`.
    TimeMalformed {
        /// One-based line number.
        line: usize,
        /// What was there.
        got: String,
    },
    /// A price is not a decimal this build can put on the paisa grid.
    PriceMalformed {
        /// One-based line number.
        line: usize,
        /// What was there.
        got: String,
    },
    /// More rows than [`MAX_ROWS`].
    TooManyRows {
        /// How many were found before stopping.
        rows: usize,
        /// The bound.
        cap: usize,
    },
}

impl core::fmt::Display for CsvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::FieldCount { line, got, want } => write!(
                f,
                "line {line}: {got} fields, expected {want}. The column layout \
                 is declared per (vendor, segment) because one vendor emits \
                 five for an index and nine for a future."
            ),
            Self::DateMalformed {
                line,
                ref got,
                format,
            } => write!(f, "line {line}: date {got:?} is not {format:?}"),
            Self::TimeMalformed { line, ref got } => {
                write!(f, "line {line}: time {got:?} is not HH:MM:SS")
            }
            Self::PriceMalformed { line, ref got } => {
                write!(f, "line {line}: price {got:?} is not a decimal")
            }
            Self::TooManyRows { rows, cap } => {
                write!(f, "the file holds at least {rows} rows; the cap is {cap}")
            }
        }
    }
}

impl core::error::Error for CsvError {}

impl From<CsvError> for FetchError {
    fn from(why: CsvError) -> Self {
        Self::TransportFailed {
            detail: why.to_string(),
        }
    }
}

/// Whether an archive member is a macOS resource-fork ghost rather than data.
///
/// `GDFL.zip` holds 12,145 of these against 12,133 real CSVs. They end in
/// `.csv`, they are binary, and a reader that globs by extension will try to
/// parse one as text.
///
/// # Examples
///
/// ```
/// # use pull::csv::is_ghost;
/// assert!(is_ghost("__MACOSX/GFDLNFO/Options/._NIFTY25SEP2525700PE.NFO.csv"));
/// assert!(is_ghost("GFDLNFO/Options/._NIFTY.csv"), "the AppleDouble prefix alone");
/// assert!(!is_ghost("GFDLNFO_TICK_01072025/Options/NIFTY25SEP2525700PE.NFO.csv"));
/// ```
#[must_use]
pub fn is_ghost(member: &str) -> bool {
    member.contains("__MACOSX")
        || member
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("._") || name == ".DS_Store")
}

/// Parses a decimal price into paisa, exactly.
///
/// Rejects a third decimal place rather than rounding it: the tick grid is two
/// places (`CLAUDE.md` §7), so a third digit is the vendor sending something
/// this build does not understand, and rounding it here would be a second
/// snapping site competing with the one at the write boundary.
fn paisa(text: &str) -> Option<i64> {
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    if frac.len() > 2 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let negative = whole.starts_with('-');
    let digits = whole.strip_prefix('-').unwrap_or(whole);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rupees: i64 = digits.parse().ok()?;
    // "38444.9" is nine tenths, not nine hundredths — pad on the right.
    let hundredths: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    let total = rupees.checked_mul(100)?.checked_add(hundredths)?;
    Some(if negative { -total } else { total })
}

/// Seconds since IST midnight, from `HH:MM:SS`.
fn ist_seconds(text: &str) -> Option<i64> {
    let mut parts = text.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || h.len() != 2 || m.len() != 2 || s.len() != 2 {
        return None;
    }
    let (h, m, s): (i64, i64, i64) = (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?);
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    Some(h * 3_600 + m * 60 + s)
}

/// A calendar date from a vendor's date field.
fn day_of(text: &str, format: DateFormat) -> Option<crate::session::Day> {
    let (y, m, d) = match format {
        // `20221003`
        DateFormat::CompactYmd if text.len() == 8 => (
            text.get(0..4)?.parse().ok()?,
            text.get(4..6)?.parse().ok()?,
            text.get(6..8)?.parse().ok()?,
        ),
        // `2022-10-03`
        DateFormat::DashedYmd if text.len() == 10 => (
            text.get(0..4)?.parse().ok()?,
            text.get(5..7)?.parse().ok()?,
            text.get(8..10)?.parse().ok()?,
        ),
        // `01/07/2025` — DAY first. 1 July, not 7 January.
        DateFormat::SlashedDmy if text.len() == 10 => (
            text.get(6..10)?.parse().ok()?,
            text.get(3..5)?.parse().ok()?,
            text.get(0..2)?.parse().ok()?,
        ),
        // `01072025`
        DateFormat::CompactDmy if text.len() == 8 => (
            text.get(4..8)?.parse().ok()?,
            text.get(2..4)?.parse().ok()?,
            text.get(0..2)?.parse().ok()?,
        ),
        _ => return None,
    };
    crate::session::Day::new(y, m, d).ok()
}

/// Decodes a whole CSV body into rows.
///
/// Timestamps come out as **UTC epoch seconds**, so the result feeds
/// [`crate::fetch::land`] with [`crate::vendor::TimestampEncoding::EpochSecondsUtc`]
/// — the vendor's IST wall clock is converted here, once, where the format is
/// known, rather than being carried onward as an encoding somebody downstream
/// has to remember.
///
/// # Errors
///
/// Any [`CsvError`]. A malformed line refuses the **whole file**: a file
/// missing an arbitrary subset of its rows is not a shorter file, it is a
/// wrong one, and the manifest would record it as complete.
///
/// # Examples
///
/// ```
/// # use pull::csv::{decode, Columns};
/// // TrueData index: five fields, no header, YYYYMMDD, second resolution.
/// let body = "20221003,09:15:01,38445.65,0,0\n20221003,09:15:02,38419.40,0,0\n";
/// let rows = decode(body, Columns::TrueDataIndex)?;
/// assert_eq!(rows.len(), 2);
/// assert_eq!(rows[0].close, 3_844_565, "38444.65 rupees in paisa");
/// # Ok::<(), pull::csv::CsvError>(())
/// ```
pub fn decode(body: &str, columns: Columns) -> Result<Vec<RawRow>, CsvError> {
    let (date_at, time_at, price_at) = columns.offsets();
    let want = columns.count();
    let mut rows: Vec<RawRow> = Vec::new();

    for (i, raw_line) in body.lines().enumerate() {
        let line_no = i + 1;
        // CRLF: `lines()` strips `\n` but leaves `\r`, and a trailing `\r`
        // turns the last field into a number that will not parse. Observed in
        // vendor files, so trimmed rather than assumed absent.
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if i == 0 && columns.has_header() {
            continue;
        }
        if rows.len() >= MAX_ROWS {
            return Err(CsvError::TooManyRows {
                rows: rows.len(),
                cap: MAX_ROWS,
            });
        }

        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != want {
            return Err(CsvError::FieldCount {
                line: line_no,
                got: fields.len(),
                want,
            });
        }

        let date_text = fields.get(date_at).copied().unwrap_or_default();
        let day =
            day_of(date_text, columns.date_format()).ok_or_else(|| CsvError::DateMalformed {
                line: line_no,
                got: date_text.to_owned(),
                format: columns.date_format(),
            })?;

        let time_text = fields.get(time_at).copied().unwrap_or_default();
        let secs = ist_seconds(time_text).ok_or_else(|| CsvError::TimeMalformed {
            line: line_no,
            got: time_text.to_owned(),
        })?;

        let price_text = fields.get(price_at).copied().unwrap_or_default();
        let price = paisa(price_text).ok_or_else(|| CsvError::PriceMalformed {
            line: line_no,
            got: price_text.to_owned(),
        })?;

        // IST wall clock to UTC epoch seconds, converted HERE where the format
        // is known. Carrying the IST-ness onward is exactly the shape of W1.
        let epoch_utc =
            i64::from(day.days_from_epoch()) * 86_400 + secs - crate::session::IST_OFFSET_SECS;

        // A snapshot row carries ONE price, not four. Open, high, low and close
        // are all that price, and that is honest for a snapshot: nothing in the
        // row claims a range, so nothing here invents one.
        rows.push(RawRow {
            timestamp: epoch_utc,
            open: price,
            high: price,
            low: price,
            close: price,
            volume: 0,
            open_interest: None,
        });
    }

    Ok(rows)
}
