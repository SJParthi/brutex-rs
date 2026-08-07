//! The socket. Dhan and Groww, over HTTPS.
//!
//! # Why this file did not exist until now
//!
//! [`crate::fetch::BarSource`] has been the transport seam since it was
//! written, and until this change it had exactly **one** implementor —
//! [`crate::fetch::FakeSource`], which answers from memory. Everything below
//! the seam was built and proved against real data through the local-archive
//! path: the seven-array length check, the paisa conversion, the session
//! filter, the drop census, the fold, the store write, the locked census.
//! 62,978 real bars went through it.
//!
//! What was missing was the one thing that reaches a broker.
//!
//! # Nothing here decides anything
//!
//! Every difference between one vendor and another is a **field on the
//! descriptor**: the base URL, the path, the method, the auth header and
//! scheme, the date format on the wire, whether the range end is inclusive,
//! the response shape, the field names, the timestamp encoding, the price
//! scale, the rate budget. This module reads them. It contains no `if vendor
//! is Dhan`, and adding a broker is a row in [`crate::vendor`], not an edit
//! here.
//!
//! # The credential never leaves this process
//!
//! It arrives through [`crate::secret::SecretSource`], goes into one header,
//! and is never logged, never formatted, never put in an error message. The
//! [`std::fmt::Debug`] impl below redacts it, because a `#[derive(Debug)]` on a
//! struct holding a token is how a token reaches a log file.
//!
//! **This repository never mints a token.** `CLAUDE.md` §8: a stale token is
//! re-read, and if the re-read returns the same dead value the pull halts
//! loudly. There is no refresh call here and there is no code path that could
//! create one.
//!
//! # What a refusal must carry
//!
//! A 429 is the governor's business and a 500 is not, so the status reaches
//! the caller rather than being flattened into "it failed". That distinction
//! is the whole reason [`crate::fetch::FetchError::VendorRefused`] carries a
//! number.

use crate::fetch::{BarRequest, BarSource, FetchError, ParallelArrays, RawWindow};
use crate::vendor::{Auth, AuthScheme, DateFormat, HttpSpec, Method, RangeEnd, ResponseShape};

/// The most bytes a vendor answer may occupy.
///
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary, and
/// unbounded input always arrives from outside. A one-second feed over a full
/// session is a few megabytes; this is generous for one window and still
/// refuses a vendor that answers a one-day request with a decade.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// How long one request may take before it is abandoned.
///
/// A hung socket with no timeout is a pull that never finishes and never says
/// why, which is worse than a refusal: the operator has nothing to act on.
pub const REQUEST_TIMEOUT_SECS: u64 = 30;

/// A vendor reached over HTTPS, driven entirely by its descriptor.
pub struct HttpSource {
    spec: HttpSpec,
    token: String,
    client: reqwest::Client,
}

// The token is the reason this is hand-written. A derived `Debug` prints every
// field, so a struct holding a credential and deriving Debug is one `dbg!`
// away from a token in a log file.
//
// `missing_fields_in_debug` fires here and is allowed ON PURPOSE: the omission
// is the feature. The lint exists to catch a field forgotten by accident, and
// this one is left out deliberately and replaced by `<redacted>`, so the reader
// can still see that a token is held without seeing its value. `client` is
// omitted too — a `reqwest::Client` prints nothing an operator can act on.
#[allow(clippy::missing_fields_in_debug)]
impl core::fmt::Debug for HttpSource {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HttpSource")
            .field("base_url", &self.spec.base_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl HttpSource {
    /// Builds a source for one vendor.
    ///
    /// # Errors
    ///
    /// [`FetchError::TransportFailed`] if the client cannot be constructed —
    /// which on this path means the TLS backend is unavailable, and is a
    /// deployment fault rather than a vendor one.
    pub fn new(spec: HttpSpec, token: String) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .timeout(core::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|why| FetchError::TransportFailed {
                detail: format!("the HTTPS client could not be built: {why}"),
            })?;
        Ok(Self {
            spec,
            token,
            client,
        })
    }

    /// The URL one window is fetched from.
    #[must_use]
    pub fn url(&self) -> String {
        format!("{}{}", self.spec.base_url, self.spec.bars_path)
    }

    /// A date as this vendor writes it on the wire.
    ///
    /// The same four formats [`crate::csv`] reads, written rather than parsed.
    /// One function per direction and one table of formats, so a vendor cannot
    /// be read one way and written another.
    #[must_use]
    pub fn on_the_wire(day: crate::session::Day, format: DateFormat) -> String {
        let (y, m, d) = (day.year(), day.month(), day.day());
        match format {
            DateFormat::DashedYmd => format!("{y:04}-{m:02}-{d:02}"),
            DateFormat::CompactYmd => format!("{y:04}{m:02}{d:02}"),
            DateFormat::SlashedDmy => format!("{d:02}/{m:02}/{y:04}"),
            DateFormat::CompactDmy => format!("{d:02}{m:02}{y:04}"),
        }
    }

    /// The end date this vendor's range takes, honouring its inclusivity.
    ///
    /// **One conversion site.** Dhan's `toDate` is exclusive, so the wire value
    /// is the day *after* the operator's last day; a vendor whose end is
    /// inclusive takes it unchanged. Two sites would be two answers and the
    /// off-by-one would return the first time either was edited.
    ///
    /// # Errors
    ///
    /// [`FetchError::TransportFailed`] past 9999-12-31 for an exclusive vendor,
    /// which has no successor to take.
    pub fn wire_end(
        last: crate::session::Day,
        end: RangeEnd,
        format: DateFormat,
    ) -> Result<String, FetchError> {
        let day = match end {
            RangeEnd::Inclusive => last,
            RangeEnd::Exclusive => last.succ().map_err(|why| FetchError::TransportFailed {
                detail: format!("{last} has no successor to put on the wire: {why}"),
            })?,
        };
        Ok(Self::on_the_wire(day, format))
    }

    /// The auth header this vendor takes, name and value.
    ///
    /// Returned as a pair rather than applied inside, so a test can assert the
    /// NAME without ever seeing the value.
    fn header(&self) -> (&'static str, String) {
        let Auth { header, scheme } = self.spec.auth;
        let value = match scheme {
            AuthScheme::Raw => self.token.clone(),
            AuthScheme::Bearer => format!("Bearer {}", self.token),
        };
        (header, value)
    }
}

/// Turns a vendor body into rows, using only what the descriptor declares.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] for a body that is not JSON or not the
/// declared shape, and whatever [`RawWindow::decode`] refuses — which includes
/// **the seven arrays disagreeing in length**, the trap that would otherwise
/// yield a short window filed as complete.
pub fn decode_body(body: &str, spec: &HttpSpec) -> Result<RawWindow, FetchError> {
    let root: serde_json::Value =
        serde_json::from_str(body).map_err(|why| FetchError::TransportFailed {
            detail: format!("the vendor's answer is not JSON: {why}"),
        })?;

    match spec.response {
        ResponseShape::ParallelArrays { .. } => {
            let f = spec.fields;
            let arrays = ParallelArrays {
                open: numbers(&root, f.open)?,
                high: numbers(&root, f.high)?,
                low: numbers(&root, f.low)?,
                close: numbers(&root, f.close)?,
                volume: numbers(&root, f.volume)?,
                timestamp: numbers(&root, f.timestamp)?,
                // OPEN INTEREST IS OPTIONAL AND ITS ABSENCE IS NOT A ZERO.
                // A spot index has none, so the descriptor leaves the name
                // `None` and no array is looked for. When the descriptor DOES
                // name one and the vendor omits it, that is a shape the
                // descriptor got wrong and it must be refused rather than
                // filled in — `CLAUDE.md` §7: `i64::MIN` is the null and zero
                // means zero, so a silent `Vec::new()` here would later read
                // back as real open interest of nothing.
                open_interest: match f.open_interest {
                    Some(name) => numbers(&root, name)?,
                    None => Vec::new(),
                },
            };
            RawWindow::decode(&arrays)
        }
        ResponseShape::ArrayOfObjects { .. } => Err(FetchError::TransportFailed {
            detail: "array-of-objects responses are declared in the descriptor \
                     but no vendor in this build uses one, so no decoder was \
                     written. Refused rather than guessed at: a decoder nobody \
                     has run against a real body is a decoder that is wrong."
                .to_owned(),
        }),
    }
}

/// One named array of numbers out of a JSON body, wherever it sits.
///
/// Searches the top level first, then one level down, because vendors wrap
/// their payload in a `data` object about as often as they do not. Two levels
/// and no further: an unbounded search would find a field of the right name in
/// the wrong place and report success.
fn numbers(root: &serde_json::Value, name: &str) -> Result<Vec<i64>, FetchError> {
    let found = root.get(name).or_else(|| {
        root.as_object()
            .and_then(|o| o.values().find_map(|v| v.get(name)))
    });
    let Some(array) = found.and_then(serde_json::Value::as_array) else {
        return Err(FetchError::TransportFailed {
            detail: format!(
                "the vendor's answer has no array named {name:?} at the top \
                 level or one below it"
            ),
        });
    };
    array.iter().map(|v| one_number(v, name)).collect()
}

/// One JSON number as an exact `i64`, or a refusal.
///
/// # `as` was wrong here and clippy is the reason it did not ship
///
/// This was `v.as_f64().map(|f| f.round() as i64)`. An `as` cast from `f64` to
/// `i64` **saturates**: `1e300 as i64` is `i64::MAX`, and `f64::NAN as i64` is
/// `0`. Both are silent. A vendor answering `1e300` for a price would have been
/// stored as the largest paisa value that exists, and a `NaN` would have been
/// stored as **zero** — and `CLAUDE.md` §7 is explicit that `i64::MIN` is the
/// only null and *zero means zero*. A NaN quietly becoming a real price of ₹0.00
/// is exactly the class of defect §4 forbids: a fallback that hides a failure.
///
/// So the float path refuses anything it cannot represent exactly rather than
/// coercing it. The integer path is tried first and is what every well-behaved
/// vendor hits; the float path exists because JSON has one number type and a
/// vendor may write `1234.0`.
fn one_number(v: &serde_json::Value, name: &str) -> Result<i64, FetchError> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let refuse = |why: &str| FetchError::TransportFailed {
        detail: format!("{name:?} holds {v}, which {why}"),
    };
    let Some(f) = v.as_f64() else {
        return Err(refuse("is not a number"));
    };
    if !f.is_finite() {
        return Err(refuse("is not finite"));
    }
    let rounded = f.round();
    // The bound is stated as f64 literals rather than `i64::MAX as f64`, because
    // `i64::MAX` is not representable in f64 — it rounds UP to 2^63, so a naive
    // `<= i64::MAX as f64` comparison admits a value one past the end. 2^63 and
    // -2^63 are both exact in f64, so this comparison is exact.
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&rounded) {
        return Err(refuse("is outside the range an i64 can hold"));
    }
    // Every branch above has proved this cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    Ok(rounded as i64)
}

impl BarSource for HttpSource {
    fn window(&self, _request: &BarRequest) -> Result<RawWindow, FetchError> {
        // A blocking `window` over an async client needs a runtime, and this
        // build has no async ingest path to hand one down. Rather than spin a
        // runtime per call — which would be a new thread pool per instrument —
        // the synchronous entry point is refused by name and `window_async` is
        // the one that works. Stated rather than silently blocking.
        Err(FetchError::TransportFailed {
            detail: "HttpSource is asynchronous: call `window_async` from a \
                     runtime. The blocking seam would need a runtime per call, \
                     which is a thread pool per instrument."
                .to_owned(),
        })
    }
}

impl HttpSource {
    /// Fetches one window from the vendor.
    ///
    /// # Errors
    ///
    /// [`FetchError::VendorRefused`] carrying the HTTP status — so a 429, which
    /// is the rate governor's business, is distinguishable from a 5xx, which is
    /// not. [`FetchError::TransportFailed`] if the socket never produced an
    /// answer, or the answer was too large, or it did not decode.
    pub async fn window_async(&self, request: &BarRequest) -> Result<RawWindow, FetchError> {
        let (name, value) = self.header();
        let from = Self::on_the_wire(request.window.from(), self.spec.date_format);
        let to = Self::wire_end(
            request.window.to(),
            self.spec.range_end,
            self.spec.date_format,
        )?;

        let url = self.url();
        let body = serde_json::json!({ "fromDate": from, "toDate": to });
        let builder = match self.spec.method {
            Method::Post => self.client.post(&url).json(&body),
            Method::Get => self.client.get(&url).query(&[("from", &from), ("to", &to)]),
        };

        let answer = builder.header(name, value).send().await.map_err(|why| {
            FetchError::TransportFailed {
                // `why` is reqwest's own words and never carries the header we
                // set, so the token cannot reach this string.
                detail: format!("{url} was not reached: {why}"),
            }
        })?;

        let status = answer.status().as_u16();
        if !answer.status().is_success() {
            let detail = answer.text().await.unwrap_or_default();
            return Err(FetchError::VendorRefused {
                status,
                detail: detail.chars().take(500).collect(),
            });
        }

        let text = answer
            .text()
            .await
            .map_err(|why| FetchError::TransportFailed {
                detail: format!("the answer could not be read: {why}"),
            })?;
        if text.len() > MAX_RESPONSE_BYTES {
            return Err(FetchError::TransportFailed {
                detail: format!(
                    "the vendor answered with {} bytes; this build accepts at \
                     most {MAX_RESPONSE_BYTES}",
                    text.len()
                ),
            });
        }

        decode_body(&text, &self.spec)
    }
}
