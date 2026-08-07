//! Vendor rows in, [`store::format::Bar`]s out — with every discarded row
//! counted and every refusal named.
//!
//! # The seam, and why it is where it is
//!
//! `CLAUDE.md` requires this crate to build and test against **fakes, with no
//! live vendor call**. So the transport sits behind [`BarSource`], exactly as
//! the credential store sits behind [`crate::secret::SecretSource`]:
//!
//! - [`FakeSource`] is what every test in this crate uses.
//! - An HTTP or archive implementation is the *only* place a socket or a zip
//!   appears, and it produces the same [`RawWindow`] either way.
//!
//! Everything below [`BarSource`] — the length agreement, the paisa
//! conversion, the session filter, the census — is transport-blind. If the two
//! transports ever needed different downstream code, the seam would be in the
//! wrong place.
//!
//! # The trap that silently loses data
//!
//! A parallel-arrays response is **seven arrays that can disagree in length**.
//! The obvious Rust is `zip`, and `zip` stops at the shortest. A vendor
//! returning 375 opens and 374 volumes would yield 374 perfectly valid-looking
//! bars, and the manifest would record a complete window: data loss with a
//! green checkmark.
//!
//! [`RawWindow::decode`] compares all seven lengths as **one refusal, before
//! any iterator exists**. Once you are zipping, the bug is unobservable —
//! there is no later point at which the missing row can be noticed.
//!
//! # W1: the fault that *stored* a wrong answer
//!
//! A permutation audit found that a vendor stamping its **local IST wall
//! clock** into a field this code read as UTC epoch seconds produced 45 bars at
//! wrong timestamps that passed every validation and were written to disk.
//! Every *other* timestamp fault refuses; that one produced storable data, and
//! once stored there is nothing left to detect it — the file is well formed,
//! the checksum is right, the counter is accurate.
//!
//! The fix is that the encoding is **not assumed**. It is a field on the vendor
//! descriptor ([`vendor::TimestampEncoding`]) and this module dispatches on it.
//! A vendor whose encoding has never been established cannot be added without
//! someone choosing a variant, which is the point.
//!
//! # Cost
//!
//! One pass over the rows. Per row: a fixed number of integer operations, one
//! IST conversion (an add and two divisions) and one census
//! increment. No allocation per row beyond the output vector, which is reserved
//! from the row count before the loop — `docs/07-o1-architecture.md` law 2.

use store::format::Bar;

use crate::session::{Cadence, Day, DropCensus, SessionError, Window};
use crate::vendor::{PriceScale, TimestampEncoding};

/// The largest number of rows one window may return.
///
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary, and
/// unbounded input always arrives from outside. A one-second feed produces
/// 22,500 rows in a session and a snapshot feed a few multiples of that, so
/// this is generous for a single window and still refuses a vendor that
/// answers a one-day request with a decade.
pub const MAX_ROWS: usize = 1_000_000;

/// One row as the vendor sent it, before any interpretation.
///
/// Prices are still in the vendor's own scale and the timestamp is still in the
/// vendor's own encoding. Nothing here has been trusted yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawRow {
    /// Timestamp in whatever [`TimestampEncoding`] the descriptor declares.
    pub timestamp: i64,
    /// Open, in the descriptor's [`PriceScale`].
    pub open: i64,
    /// High.
    pub high: i64,
    /// Low.
    pub low: i64,
    /// Close.
    pub close: i64,
    /// Volume. Always a count, never scaled.
    pub volume: i64,
    /// Open interest, or [`None`] when the vendor omitted it.
    ///
    /// [`None`] and `Some(0)` are different facts and must stay different:
    /// zero open interest is a measurement, an absent field is not.
    pub open_interest: Option<i64>,
}

/// A decoded window: rows, and nothing decided about them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawWindow {
    /// The rows, in the order the vendor sent them.
    pub rows: Vec<RawRow>,
}

/// The seven parallel arrays, as a vendor sends them.
///
/// Taken as a struct rather than seven arguments so a caller cannot transpose
/// `high` and `low` silently — four of the seven are the same type and would
/// swap without complaint.
#[derive(Debug, Clone, Default)]
pub struct ParallelArrays {
    /// Opens.
    pub open: Vec<i64>,
    /// Highs.
    pub high: Vec<i64>,
    /// Lows.
    pub low: Vec<i64>,
    /// Closes.
    pub close: Vec<i64>,
    /// Volumes.
    pub volume: Vec<i64>,
    /// Timestamps.
    pub timestamp: Vec<i64>,
    /// Open interest. Empty means the vendor does not send it at all, which is
    /// different from sending zeros.
    pub open_interest: Vec<i64>,
}

/// Why a fetch produced no window.
///
/// Every variant carries the values it refused. `CLAUDE.md` §4 — degrade loudly
/// and name the reason, or refuse; never both silently.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchError {
    /// The seven arrays disagree in length.
    ///
    /// **This is the refusal that exists because of `zip`.** It names every
    /// length so an operator can see which field the vendor truncated, rather
    /// than being told only that something was wrong.
    LengthDisagreement {
        /// Opens.
        open: usize,
        /// Highs.
        high: usize,
        /// Lows.
        low: usize,
        /// Closes.
        close: usize,
        /// Volumes.
        volume: usize,
        /// Timestamps.
        timestamp: usize,
        /// Open interest, when present.
        open_interest: usize,
    },
    /// More rows than [`MAX_ROWS`].
    TooManyRows {
        /// How many arrived.
        rows: usize,
        /// The bound.
        cap: usize,
    },
    /// A timestamp names no instant this build can hold.
    TimestampRefused {
        /// The row's position, so it can be found in the response.
        row: usize,
        /// The value.
        raw: i64,
        /// The calendar's own refusal.
        why: SessionError,
    },
    /// A price does not fit the paisa grid after conversion.
    PriceRefused {
        /// The row's position.
        row: usize,
        /// Which field.
        field: &'static str,
        /// The value, in the vendor's scale.
        raw: i64,
    },
    /// The vendor was reached and answered that it would not serve this.
    ///
    /// Carries the status so a rate refusal (429) is distinguishable from a
    /// broken vendor (5xx) — the first is the governor's business, the second
    /// is not.
    VendorRefused {
        /// HTTP status, or the transport's equivalent.
        status: u16,
        /// Whatever the vendor said, truncated.
        detail: String,
    },
    /// The transport failed before an answer arrived.
    TransportFailed {
        /// What went wrong, in the transport's own words.
        detail: String,
    },
}

impl core::fmt::Display for FetchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::LengthDisagreement {
                open,
                high,
                low,
                close,
                volume,
                timestamp,
                open_interest,
            } => write!(
                f,
                "the vendor's arrays disagree in length — open {open}, high \
                 {high}, low {low}, close {close}, volume {volume}, timestamp \
                 {timestamp}, open interest {open_interest}. Refused whole \
                 rather than truncated to the shortest: a short window filed \
                 as complete is data loss nothing downstream can detect."
            ),
            Self::TooManyRows { rows, cap } => write!(
                f,
                "the vendor returned {rows} rows; this build accepts at most {cap}"
            ),
            Self::TimestampRefused { row, raw, why } => {
                write!(f, "row {row}: timestamp {raw} refused — {why}")
            }
            Self::PriceRefused { row, field, raw } => write!(
                f,
                "row {row}: {field} {raw} does not land on the paisa grid"
            ),
            Self::VendorRefused { status, ref detail } => {
                write!(f, "the vendor refused with status {status}: {detail}")
            }
            Self::TransportFailed { ref detail } => {
                write!(f, "the vendor was not reached: {detail}")
            }
        }
    }
}

impl core::error::Error for FetchError {}

impl RawWindow {
    /// Decodes seven parallel arrays into rows, or refuses the whole window.
    ///
    /// # Errors
    ///
    /// [`FetchError::LengthDisagreement`] when the arrays do not agree, and
    /// [`FetchError::TooManyRows`] past [`MAX_ROWS`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::fetch::{ParallelArrays, RawWindow, FetchError};
    /// // 3 opens, 2 volumes — `zip` would have produced two valid-looking bars.
    /// let short = ParallelArrays {
    ///     open: vec![1, 2, 3],
    ///     high: vec![1, 2, 3],
    ///     low: vec![1, 2, 3],
    ///     close: vec![1, 2, 3],
    ///     volume: vec![1, 2],
    ///     timestamp: vec![0, 60, 120],
    ///     open_interest: Vec::new(),
    /// };
    /// assert!(matches!(
    ///     RawWindow::decode(&short),
    ///     Err(FetchError::LengthDisagreement { volume: 2, open: 3, .. })
    /// ));
    /// ```
    pub fn decode(a: &ParallelArrays) -> Result<Self, FetchError> {
        // EVERY LENGTH, COMPARED BEFORE AN ITERATOR EXISTS. Open interest is
        // optional — a vendor that never sends it sends an empty vector — so it
        // agrees when it is either empty or the same length as the rest.
        let n = a.open.len();
        let oi = a.open_interest.len();
        let agree = a.high.len() == n
            && a.low.len() == n
            && a.close.len() == n
            && a.volume.len() == n
            && a.timestamp.len() == n
            && (oi == 0 || oi == n);
        if !agree {
            return Err(FetchError::LengthDisagreement {
                open: n,
                high: a.high.len(),
                low: a.low.len(),
                close: a.close.len(),
                volume: a.volume.len(),
                timestamp: a.timestamp.len(),
                open_interest: oi,
            });
        }
        if n > MAX_ROWS {
            return Err(FetchError::TooManyRows {
                rows: n,
                cap: MAX_ROWS,
            });
        }

        // Reserved from a count known before the loop — law 2, so the vector
        // never grows and the append stays worst-case constant.
        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            // `get` rather than indexing: the agreement above makes every index
            // in range, but an index that is correct *because of a check forty
            // lines up* is how a panic arrives during a later edit.
            let (Some(&open), Some(&high), Some(&low), Some(&close)) =
                (a.open.get(i), a.high.get(i), a.low.get(i), a.close.get(i))
            else {
                break;
            };
            let (Some(&volume), Some(&timestamp)) = (a.volume.get(i), a.timestamp.get(i)) else {
                break;
            };
            rows.push(RawRow {
                timestamp,
                open,
                high,
                low,
                close,
                volume,
                open_interest: a.open_interest.get(i).copied(),
            });
        }
        Ok(Self { rows })
    }
}

/// What one window of bars was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarRequest {
    /// The inclusive window the operator asked for.
    pub window: Window,
    /// Whether these are intraday bars or daily ones.
    ///
    /// An argument rather than an assumption: a daily bar is stamped at
    /// midnight and would be dropped by every intraday filter.
    pub cadence: Cadence,
}

/// Where rows come from. The only thing a transport must satisfy.
pub trait BarSource {
    /// Fetches one window.
    ///
    /// # Errors
    ///
    /// Whatever the transport refuses, as a named [`FetchError`].
    fn window(&self, request: &BarRequest) -> Result<RawWindow, FetchError>;
}

/// A source that answers from memory. What every test in this crate uses.
///
/// Exists so that no test needs a socket. A test that needed one would be
/// testing the network rather than this code, and would fail on a train.
#[derive(Debug, Clone)]
pub struct FakeSource {
    /// What to answer with.
    pub answer: Result<RawWindow, FetchError>,
}

impl Default for FakeSource {
    /// An empty window. A fake that refuses by default would make every
    /// test that forgot to configure it pass for the wrong reason.
    fn default() -> Self {
        Self {
            answer: Ok(RawWindow::default()),
        }
    }
}

impl FakeSource {
    /// A source that returns these rows.
    #[must_use]
    pub fn returning(rows: Vec<RawRow>) -> Self {
        Self {
            answer: Ok(RawWindow { rows }),
        }
    }

    /// A source that refuses.
    #[must_use]
    pub const fn refusing(why: FetchError) -> Self {
        Self { answer: Err(why) }
    }
}

impl BarSource for FakeSource {
    fn window(&self, _request: &BarRequest) -> Result<RawWindow, FetchError> {
        self.answer.clone()
    }
}

/// What one window produced: the bars that survived, and why the rest did not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Landed {
    /// Bars inside the window and the session, ready for the store.
    pub bars: Vec<Bar>,
    /// Every discarded row, by reason. The total equals rows in minus bars out.
    pub census: DropCensus,
}

/// Converts a vendor price to paisa.
///
/// A vendor quoting rupees is multiplied by 100; one already quoting paisa is
/// taken as is. **There is no rounding here and that is deliberate** —
/// `CLAUDE.md` §7 puts the single snap on the tick grid at the write boundary,
/// and a second rounding site is a second answer.
const fn to_paisa(raw: i64, scale: PriceScale) -> Option<i64> {
    match scale {
        PriceScale::Paisa => Some(raw),
        PriceScale::Rupees => raw.checked_mul(100),
    }
}

/// Turns a raw window into bars, filtering by the operator's window and the
/// session, and counting every drop by reason.
///
/// # Errors
///
/// [`FetchError::TimestampRefused`] for an instant this build cannot hold, and
/// [`FetchError::PriceRefused`] for a price that overflows on conversion. Both
/// refuse the **whole window**: a window missing an arbitrary subset of its
/// rows is not a shorter window, it is a wrong one.
///
/// # Cost
///
/// One pass. Per row: a fixed number of integer operations, one [`IstMoment`]
/// conversion and one census increment. The output vector is reserved from the
/// row count before the loop.
pub fn land(
    raw: &RawWindow,
    request: &BarRequest,
    encoding: TimestampEncoding,
    scale: PriceScale,
) -> Result<Landed, FetchError> {
    let mut bars = Vec::with_capacity(raw.rows.len());
    let mut census = DropCensus::default();

    for (i, row) in raw.rows.iter().enumerate() {
        // W1 LIVES HERE. The encoding is dispatched, never assumed. A vendor
        // stamping IST wall-clock seconds into a field read as UTC epoch
        // produced 45 bars at wrong timestamps that passed every check and
        // were written to disk — the only fault in this pipeline that STORED a
        // wrong answer instead of refusing.
        let epoch_utc = match encoding {
            TimestampEncoding::EpochSecondsUtc => row.timestamp,
            TimestampEncoding::EpochMillisUtc => row.timestamp.div_euclid(1_000),
            // The vendor's value is already IST-based, so the offset this
            // build would add has already been added by the vendor. Subtract it
            // back out before the shared conversion, rather than teaching that
            // conversion a second mode.
            TimestampEncoding::IstDateTimeText => row.timestamp - crate::session::IST_OFFSET_SECS,
        };

        // `verdict` does the conversion itself and refuses a timestamp that is
        // not one. A drop is a bar this engine DECLINED; a timestamp it could
        // not read is the vendor or the decoder being wrong, and those are
        // different answers.
        let verdict = request
            .window
            .verdict(epoch_utc, request.cadence)
            .map_err(|why| FetchError::TimestampRefused {
                row: i,
                raw: row.timestamp,
                why,
            })?;
        if let Some(reason) = verdict {
            census.count(reason);
            continue;
        }

        let price = |raw: i64, field: &'static str| {
            to_paisa(raw, scale).ok_or(FetchError::PriceRefused { row: i, field, raw })
        };
        let bar = Bar {
            ts_micros: epoch_utc
                .checked_mul(1_000_000)
                .ok_or(FetchError::TimestampRefused {
                    row: i,
                    raw: row.timestamp,
                    why: SessionError::TimestampOutOfRange { secs: epoch_utc },
                })?,
            open: price(row.open, "open")?,
            high: price(row.high, "high")?,
            low: price(row.low, "low")?,
            close: price(row.close, "close")?,
            volume: row.volume,
            // ABSENT IS NOT ZERO. `i64::MIN` is the null sentinel; a vendor
            // sending a literal 0 means zero open interest, which is a
            // measurement, and it must survive as one.
            open_interest: row.open_interest.unwrap_or(i64::MIN),
        };
        bars.push(bar);
    }

    Ok(Landed { bars, census })
}

/// Fetches one window and lands it, in one call.
///
/// # Errors
///
/// Whatever the source or [`land`] refuses.
pub fn fetch_and_land<S: BarSource>(
    source: &S,
    request: &BarRequest,
    encoding: TimestampEncoding,
    scale: PriceScale,
) -> Result<Landed, FetchError> {
    let raw = source.window(request)?;
    land(&raw, request, encoding, scale)
}

/// The wire value for a window's end, honouring the vendor's inclusivity.
///
/// **One conversion site.** Dhan's `toDate` is exclusive, so the wire value is
/// the day *after* the operator's last day; a vendor whose end is inclusive
/// takes it unchanged. Two sites would be two answers, and the off-by-one would
/// return the first time someone edited one of them.
///
/// # Errors
///
/// [`SessionError::NoNextDay`] past 9999-12-31, for an exclusive vendor.
pub fn wire_end(last: Day, end: crate::vendor::RangeEnd) -> Result<Day, SessionError> {
    match end {
        crate::vendor::RangeEnd::Inclusive => Ok(last),
        crate::vendor::RangeEnd::Exclusive => last.succ(),
    }
}
