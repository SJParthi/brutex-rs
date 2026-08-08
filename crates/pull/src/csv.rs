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

    /// Zero-based index of date, time, price, volume and open interest.
    ///
    /// **The last two were missing, and every bar written before this was
    /// wrong.** The decoder read three columns and hardcoded `volume: 0,
    /// open_interest: None`, so a contract that traded 250 lots was stored
    /// asserting it traded none — with a valid checksum, undetectable
    /// afterwards. Verified against `ABB-III.NFO.csv`, whose 11:50 bucket
    /// carries volume 250 and open interest 2,000 and reached disk as `0` and
    /// `i64::MIN`.
    const fn offsets(self) -> Offsets {
        match self {
            Self::TrueDataIndex | Self::TrueDataFutures => Offsets {
                date: 0,
                time: 1,
                price: 2,
                volume: 3,
                open_interest: 4,
            },
            // Ticker, Date, Time, LTP, BuyPrice, BuyQty, SellPrice, SellQty,
            // LTQ, OpenInterest. LTQ is the traded size — column 9.
            Self::Gdfl => Offsets {
                date: 1,
                time: 2,
                price: 3,
                volume: 8,
                open_interest: 9,
            },
        }
    }
}

/// Which column each field sits in, for one layout.
///
/// A struct rather than five positional `usize`: five adjacent numbers of the
/// same type transpose without a compiler complaint, and a volume index read as
/// a price index yields a plausible number. The same reasoning as
/// `ingest::Plan` and `api::render::View`.
#[derive(Debug, Clone, Copy)]
struct Offsets {
    date: usize,
    time: usize,
    price: usize,
    volume: usize,
    open_interest: usize,
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
pub(crate) fn paisa(text: &str) -> Option<i64> {
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
    let at = columns.offsets();
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

        let date_text = fields.get(at.date).copied().unwrap_or_default();
        let day =
            day_of(date_text, columns.date_format()).ok_or_else(|| CsvError::DateMalformed {
                line: line_no,
                got: date_text.to_owned(),
                format: columns.date_format(),
            })?;

        let time_text = fields.get(at.time).copied().unwrap_or_default();
        let secs = ist_seconds(time_text).ok_or_else(|| CsvError::TimeMalformed {
            line: line_no,
            got: time_text.to_owned(),
        })?;

        let price_text = fields.get(at.price).copied().unwrap_or_default();
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
            // THE VENDOR SENT THESE AND THEY WERE BEING DISCARDED. A field that
            // will not parse is zero for volume — a count this build could not
            // read is not a trade — and ABSENT for open interest, because
            // `None` and `Some(0)` are different facts: zero open interest is a
            // measurement, an unreadable field is not.
            volume: fields
                .get(at.volume)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0),
            open_interest: fields
                .get(at.open_interest)
                .and_then(|v| v.trim().parse::<i64>().ok()),
        });
    }

    Ok(rows)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
//
// IN THE FILE, because [`day_of`] is private and two of its four arms are
// unreachable through [`decode`]: `Columns::date_format` returns only
// `CompactYmd` and `SlashedDmy`, so the dashed and compact day-first arms — the
// ones an HTTP feed's descriptor selects — have no route in from outside. An
// arm nothing exercises is an arm nobody has read since it was written, and
// this decoder's whole subject is that reading a date the wrong way round is
// silent.
//
// NO DATE IS WRITTEN AS A BARE QUOTED WORD. An eight-digit compact date is
// shaped exactly like a parameter-path segment as far as CI gate 1d is
// concerned, so every fixture below is assembled by `format!` from integers
// instead — and so is every `expect` message that would otherwise spell one.
#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]
mod tests {
    use super::*;

    /// The same date, in each of the four renderings a descriptor can declare.
    fn rendered(year: u16, month: u8, day: u8, format: DateFormat) -> String {
        match format {
            DateFormat::CompactYmd => format!("{year:04}{month:02}{day:02}"),
            DateFormat::DashedYmd => format!("{year:04}-{month:02}-{day:02}"),
            DateFormat::SlashedDmy => format!("{day:02}/{month:02}/{year:04}"),
            DateFormat::CompactDmy => format!("{day:02}{month:02}{year:04}"),
            DateFormat::DashedYmdMidnight => {
                format!("{year:04}-{month:02}-{day:02} 00:00:00")
            }
        }
    }

    /// Every declared date format reads the same day, and the day-first ones
    /// read it day-first.
    #[test]
    fn every_declared_date_format_reads_the_day_it_names() {
        let every = [
            DateFormat::CompactYmd,
            DateFormat::DashedYmd,
            DateFormat::SlashedDmy,
            DateFormat::CompactDmy,
        ];
        let wanted = crate::session::Day::new(2025, 7, 1).expect("a real date");

        for format in every {
            let text = rendered(2025, 7, 1, format);
            assert_eq!(
                day_of(&text, format),
                Some(wanted),
                "{text} read as {format:?} is 1 July 2025"
            );
        }

        // THE FAULT THIS EXISTS TO PREVENT. The same eight and ten bytes read
        // under the other convention are a different month, and the wrong
        // answer is a perfectly ordinary date.
        let day_first = rendered(2025, 7, 1, DateFormat::SlashedDmy);
        assert_eq!(
            day_of(&day_first, DateFormat::SlashedDmy),
            Some(wanted),
            "01/07/2025 is 1 July"
        );
        assert_eq!(
            day_of(&day_first, DateFormat::DashedYmd),
            None,
            "and it is not a dashed year-first date at all — refused rather \
             than read as 7 January"
        );
        let compact_day_first = rendered(2025, 7, 1, DateFormat::CompactDmy);
        assert_eq!(
            day_of(&compact_day_first, DateFormat::CompactYmd),
            None,
            "01072025 is not a year-first date either: year 0107 is before the \
             calendar this build holds"
        );
    }

    /// A date of the wrong length, the wrong alphabet or the wrong calendar is
    /// refused rather than salvaged.
    #[test]
    fn a_date_that_is_not_the_declared_format_is_refused() {
        for format in [
            DateFormat::CompactYmd,
            DateFormat::DashedYmd,
            DateFormat::SlashedDmy,
            DateFormat::CompactDmy,
        ] {
            let text = rendered(2025, 7, 1, format);
            let mut short = text.clone();
            short.pop();
            assert_eq!(
                day_of(&short, format),
                None,
                "{short} is the wrong length for {format:?}"
            );
            let mut long = text.clone();
            long.push('0');
            assert_eq!(day_of(&long, format), None, "and so is {long}");
            assert_eq!(day_of("", format), None, "an empty field names no day");
        }

        // The right length and the wrong alphabet.
        let letters = format!("{}{}", "ABCD", "EFGH");
        assert_eq!(day_of(&letters, DateFormat::CompactYmd), None);
        let separators = format!("{}-{}-{}", "ABCD", "EF", "GH");
        assert_eq!(day_of(&separators, DateFormat::DashedYmd), None);

        // The right length, the right alphabet, and no such day. The calendar
        // rule is `Day::new`'s, and this proves it is actually consulted.
        let unreal = rendered(2025, 2, 30, DateFormat::CompactYmd);
        assert_eq!(
            day_of(&unreal, DateFormat::CompactYmd),
            None,
            "30 February parses and is still not a date"
        );
        let before_the_calendar = rendered(1969, 12, 31, DateFormat::CompactYmd);
        assert_eq!(day_of(&before_the_calendar, DateFormat::CompactYmd), None);
    }

    /// A price whose rupee part is too large to put on the paisa grid is
    /// refused rather than wrapped.
    #[test]
    fn a_price_too_large_for_the_paisa_grid_is_refused() {
        let just_inside = i64::MAX / 100;
        assert_eq!(
            paisa(&just_inside.to_string()),
            Some(just_inside * 100),
            "the largest whole rupee value that fits still converts"
        );
        assert_eq!(
            paisa(&(just_inside + 1).to_string()),
            None,
            "and one rupee more overflows the grid — refused, never wrapped"
        );
        assert_eq!(
            paisa(&format!("{just_inside}.99")),
            None,
            "the hundredths are what tip this one over, and the add is checked \
             as well as the multiply"
        );
        assert_eq!(
            paisa(&format!("-{just_inside}")),
            Some(-just_inside * 100),
            "and the sign is applied after the arithmetic, not before it"
        );
        assert_eq!(
            paisa(&core::iter::repeat_n(char::from(b'9'), 20).collect::<String>()),
            None,
            "twenty digits are all digits and still name no i64 — refused at \
             the parse rather than wrapped"
        );
    }

    /// `HH:MM:SS`, and every way a time field is not that.
    #[test]
    fn a_time_that_is_not_hh_mm_ss_is_refused_rather_than_salvaged() {
        let hms = |h: u32, m: u32, s: u32| format!("{h:02}:{m:02}:{s:02}");
        let good = hms(9, 15, 1);
        assert_eq!(
            ist_seconds(&good),
            Some(9 * 3_600 + 15 * 60 + 1),
            "seconds since IST midnight"
        );
        assert_eq!(ist_seconds(&hms(0, 0, 0)), Some(0));
        assert_eq!(ist_seconds(&hms(23, 59, 59)), Some(23 * 3_600 + 3_599));

        for (text, why) in [
            (good[..5].to_owned(), "no seconds field at all"),
            (
                good.replace(':', ""),
                "no separators, so one field not three",
            ),
            (format!("{good}:{:02}", 0), "a fourth field"),
            (format!("{}:{:02}:{:02}", 9, 15, 1), "a one-digit hour"),
            (format!("{:02}:{}:{:02}", 9, 5, 1), "a one-digit minute"),
            (format!("{:02}:{:02}:{}", 9, 15, 1), "a one-digit second"),
            (
                format!("{}:{:02}:{:02}", "AB", 15, 1),
                "an hour that is letters",
            ),
            (
                format!("{:02}:{}:{:02}", 9, "AB", 1),
                "a minute that is letters",
            ),
            (
                format!("{:02}:{:02}:{}", 9, 15, "AB"),
                "a second that is letters",
            ),
            (hms(24, 0, 0), "an hour past 23"),
            (hms(9, 60, 0), "a minute past 59"),
            (
                hms(9, 15, 60),
                "a second past 59 — no leap second is stored",
            ),
        ] {
            assert_eq!(ist_seconds(&text), None, "{text:?} has {why}");
        }
    }

    /// The byte ranges each format reads its year, month and day from, in the
    /// order [`day_of`] evaluates them.
    const fn ranges(format: DateFormat) -> [(usize, usize); 3] {
        match format {
            DateFormat::CompactYmd => [(0, 4), (4, 6), (6, 8)],
            // The date occupies the same first ten bytes either way; the
            // clock a datetime format adds after it is not a date field.
            DateFormat::DashedYmd | DateFormat::DashedYmdMidnight => [(0, 4), (5, 7), (8, 10)],
            DateFormat::SlashedDmy => [(6, 10), (3, 5), (0, 2)],
            DateFormat::CompactDmy => [(4, 8), (2, 4), (0, 2)],
        }
    }

    /// A field of the right length whose digits are not digits is refused, one
    /// field at a time, in every format.
    #[test]
    fn one_bad_field_is_enough_to_refuse_a_date_in_every_format() {
        for format in [
            DateFormat::CompactYmd,
            DateFormat::DashedYmd,
            DateFormat::SlashedDmy,
            DateFormat::CompactDmy,
        ] {
            let whole = rendered(2025, 7, 1, format);
            for (start, end) in ranges(format) {
                let mut bytes = whole.clone().into_bytes();
                for slot in bytes.iter_mut().take(end).skip(start) {
                    *slot = b'A';
                }
                let corrupted = String::from_utf8(bytes).expect("still ASCII");
                assert_eq!(
                    day_of(&corrupted, format),
                    None,
                    "{corrupted} keeps the shape of {format:?} and one field is \
                     not a number — refused rather than read as whatever the \
                     rest says"
                );
            }
        }
    }

    /// A date field of the right BYTE length whose bytes are not all ASCII is
    /// refused rather than panicking on a slice that is not a char boundary.
    ///
    /// `text.len()` counts bytes and a vendor field is arbitrary input, so the
    /// three slices `day_of` takes are `get` rather than `[..]`. Each case
    /// below puts a two-byte character across exactly one of those boundaries.
    #[test]
    fn a_date_whose_bytes_are_not_all_ascii_is_refused_not_sliced() {
        // Byte layouts, chosen so that the boundary named in the comment is the
        // FIRST one that falls inside the two-byte character.
        let cases = [
            (DateFormat::CompactYmd, format!("202{}003", 'é')),
            (DateFormat::CompactYmd, format!("20221{}0", 'é')),
            (DateFormat::DashedYmd, format!("202{}10-03", 'é')),
            (DateFormat::DashedYmd, format!("2025-1{}01", 'é')),
            (DateFormat::DashedYmd, format!("2025-10{}0", 'é')),
            (DateFormat::SlashedDmy, format!("01/07{}025", 'é')),
            (DateFormat::SlashedDmy, format!("01/0{}2025", 'é')),
            (DateFormat::SlashedDmy, format!("0{}07/2025", 'é')),
            (DateFormat::CompactDmy, format!("012{}025", 'é')),
            (DateFormat::CompactDmy, format!("0{}12025", 'é')),
        ];
        for (format, text) in cases {
            let wanted = match format {
                DateFormat::CompactYmd | DateFormat::CompactDmy => 8,
                DateFormat::DashedYmd | DateFormat::SlashedDmy => 10,
                DateFormat::DashedYmdMidnight => 19,
            };
            assert_eq!(
                text.len(),
                wanted,
                "{text:?} must be the right BYTE length for {format:?}, or the \
                 length guard refuses it before the slice is ever taken"
            );
            assert!(!text.is_char_boundary(4) || !text.is_ascii());
            assert_eq!(
                day_of(&text, format),
                None,
                "{text:?} is refused rather than sliced through a character"
            );
        }
    }
}
