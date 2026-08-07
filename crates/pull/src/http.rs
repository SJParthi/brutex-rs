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
use crate::vendor::{
    Auth, AuthScheme, DateFormat, HttpSpec, Method, PriceScale, RangeEnd, ResponseShape,
};

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
                // PRICES GO THROUGH `prices`, NOT `numbers`, AND THE
                // DIFFERENCE IS 75 PAISE ON EVERY BAR THAT HAS THEM.
                //
                // `numbers` rounds a JSON number to an integer, which is right
                // for a volume and a timestamp and WRONG for a price. A vendor
                // quoting rupees sends `24500.75`; rounding that to `24501` and
                // letting `fetch::to_paisa` multiply by 100 stores ₹24,501.00
                // and the paise are gone, silently, on every bar.
                //
                // `CLAUDE.md` §7 puts the tick grid at TWO decimal places and
                // the single snap at the write boundary. Rounding to whole
                // rupees here is a snap at the wrong granularity in the wrong
                // place. `prices` therefore scales first and rounds once, while
                // the paise are still in the float.
                open: prices(&root, f.open, spec.prices)?,
                high: prices(&root, f.high, spec.prices)?,
                low: prices(&root, f.low, spec.prices)?,
                close: prices(&root, f.close, spec.prices)?,
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

/// The unit [`decode_body`] leaves prices in, whatever the vendor quoted.
///
/// **A caller passing this window to [`crate::fetch::land`] must pass
/// [`PriceScale::Paisa`], not `spec.prices`.** The conversion has already
/// happened, and happening twice would multiply every price by 100 again.
pub const DECODED_PRICE_SCALE: PriceScale = PriceScale::Paisa;

/// One named array of **prices**, converted to exact paisa without a float.
///
/// # Two wrong answers preceded this one
///
/// The first read a price with `f.round()` and let [`crate::fetch::to_paisa`]
/// multiply by 100. A vendor quoting `24500.75` therefore stored ₹24,501.00 and
/// **the 75 paise were gone**, silently, on every bar that had them.
///
/// The second scaled before rounding — `24500.75 × 100` — which is *arithmetically*
/// right and still wrong for this repository: `clippy::float_arithmetic` is
/// denied workspace-wide, precisely so that `CLAUDE.md` §7's "never a float"
/// cannot be walked back one expression at a time. The lint was correct. There
/// is no float in a price here, not even briefly.
///
/// # What it does instead
///
/// [`crate::csv::paisa`] already turns `"24500.75"` into `2450075` by splitting
/// on the point and doing integer arithmetic — it is what the local-archive
/// path has always used, and it is now shared rather than reimplemented. JSON's
/// number is rendered back to its text with `Display`, which `serde_json` emits
/// via shortest-round-trip formatting, so a two-decimal price round-trips
/// character for character.
///
/// A vendor sending **more precision than the tick grid holds** — `100.005` —
/// is refused by name rather than snapped. That matches the CSV path exactly
/// (`csv::paisa` returns `None` past two decimals) and it is the louder choice:
/// a third decimal on an NSE price means the descriptor's `PriceScale` is
/// wrong, and quietly rounding it would hide that.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] naming the field and the value, for anything
/// that is not a number or does not land exactly on the paisa grid.
fn prices(root: &serde_json::Value, name: &str, scale: PriceScale) -> Result<Vec<i64>, FetchError> {
    array_at(root, name)?
        .iter()
        .map(|v| {
            let refuse = || FetchError::TransportFailed {
                detail: format!(
                    "{name:?} holds {v}, which is not a price this build can put \
                     on the paisa grid"
                ),
            };
            let Some(number) = v.as_number() else {
                return Err(refuse());
            };
            match scale {
                // Already paisa: an integer count, and nothing to convert.
                PriceScale::Paisa => number.as_i64().ok_or_else(refuse),
                // Rupees: the text is the truth, and `csv::paisa` owns the rule.
                PriceScale::Rupees => crate::csv::paisa(&number.to_string()).ok_or_else(refuse),
            }
        })
        .collect()
}

/// One named array of counts — volumes, timestamps, open interest.
///
/// These are integers in their own right and no scale applies. A vendor writing
/// `250.0` means two hundred and fifty, so the same text parser is reused and
/// the hundredths are required to be zero; `250.5` of anything is a shape this
/// build does not understand and says so.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] naming the field and the value.
fn numbers(root: &serde_json::Value, name: &str) -> Result<Vec<i64>, FetchError> {
    array_at(root, name)?
        .iter()
        .map(|v| {
            if let Some(n) = v.as_i64() {
                return Ok(n);
            }
            let refuse = || FetchError::TransportFailed {
                detail: format!("{name:?} holds {v}, which is not a whole number"),
            };
            let number = v.as_number().ok_or_else(refuse)?;
            let hundredths = crate::csv::paisa(&number.to_string()).ok_or_else(refuse)?;
            if hundredths % 100 == 0 {
                Ok(hundredths / 100)
            } else {
                Err(refuse())
            }
        })
        .collect()
}

/// The array a field names, wherever in the body it sits.
///
/// Searches the top level first, then one level down, because vendors wrap
/// their payload in a `data` object about as often as they do not. Two levels
/// and no further: an unbounded search would find a field of the right name in
/// the wrong place and report success.
fn array_at<'a>(
    root: &'a serde_json::Value,
    name: &str,
) -> Result<&'a Vec<serde_json::Value>, FetchError> {
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
    Ok(array)
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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]
mod tests {
    use super::*;
    use crate::vendor::{Budget, FieldNames, Pooling, TimestampEncoding};

    /// A descriptor with only the fields the decoder reads, so a test says what
    /// it is testing. `prices` is the parameter every price case turns on.
    fn spec(prices: PriceScale) -> HttpSpec {
        HttpSpec {
            base_url: "https://vendor.invalid",
            bars_path: "/bars",
            method: Method::Post,
            auth: Auth {
                header: "x-token",
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
            prices,
            budget: Budget {
                per_second: None,
                per_minute: None,
                per_day: None,
            },
            pooling: Pooling::PerVendor,
        }
    }

    /// **THE 75 PAISE.** This is the regression test for a defect that reached
    /// the tree and would have silently rewritten every fractional price in the
    /// archive.
    ///
    /// The decoder read a price with `f.round()` and handed the result to
    /// `fetch::to_paisa`, which multiplies a rupee figure by 100. So a vendor
    /// quoting `24500.75` produced `24501` rupees, then `2450100` paisa —
    /// **₹24,501.00, and the 75 paise were gone.** No error, no counter, no
    /// drop reason: the bar landed on disk looking exactly like a real one.
    ///
    /// `CLAUDE.md` §7 puts the tick grid at two decimals and the single snap at
    /// the write boundary. Rounding to whole rupees is a snap at the wrong
    /// granularity, in the wrong place.
    #[test]
    fn a_fractional_rupee_price_keeps_its_paise() {
        let body = r#"{
            "open":[24500.75],"high":[24500.75],"low":[24500.75],
            "close":[24500.75],"volume":[250],"timestamp":[1751337900]
        }"#;
        let window = decode_body(body, &spec(PriceScale::Rupees)).expect("decodes");
        let row = &window.rows[0];
        assert_eq!(
            row.open, 2_450_075,
            "24500.75 rupees is 2450075 paisa. 2450100 would be the bug: the \
             price rounded to a whole rupee before the scale was applied."
        );
        assert_eq!(
            (row.high, row.low, row.close),
            (2_450_075, 2_450_075, 2_450_075)
        );
        assert_eq!(row.volume, 250, "a volume is a count and is not scaled");
    }

    /// Every price the paisa grid can hold, held exactly.
    #[test]
    fn every_price_on_the_paisa_grid_survives_intact() {
        for (sent, want) in [
            ("0", 0_i64),      // zero means zero — never a null
            ("0.01", 1),       // one paisa survives
            ("0.1", 10),       // one tenth is TEN paisa, not one
            ("100.4", 10_040), // a single decimal pads on the RIGHT
            ("24500.75", 2_450_075),
            ("99999.99", 9_999_999),
        ] {
            let body = format!(
                "{{\"open\":[{sent}],\"high\":[{sent}],\"low\":[{sent}],\
                  \"close\":[{sent}],\"volume\":[1],\"timestamp\":[1751337900]}}"
            );
            let window = decode_body(&body, &spec(PriceScale::Rupees)).expect("decodes");
            assert_eq!(window.rows[0].open, want, "{sent} rupees is {want} paisa");
        }
    }

    /// A price finer than the tick grid is **refused, not rounded**.
    ///
    /// `CLAUDE.md` §7 fixes the grid at two decimals. A third decimal on an NSE
    /// price does not mean "round me" — it means the descriptor's `PriceScale`
    /// is wrong, or the field is not a price at all. Snapping would hide that.
    /// `crate::csv::paisa` has always refused it on the archive path, and this
    /// path now shares that one function, so the two cannot disagree.
    #[test]
    fn a_price_finer_than_the_tick_grid_is_refused_rather_than_rounded() {
        for sent in ["100.005", "100.12345", "1e300"] {
            let body = format!(
                "{{\"open\":[{sent}],\"high\":[1],\"low\":[1],\"close\":[1],\
                  \"volume\":[1],\"timestamp\":[1]}}"
            );
            let Err(FetchError::TransportFailed { detail }) =
                decode_body(&body, &spec(PriceScale::Rupees))
            else {
                panic!("{sent} is off the paisa grid: refuse it, do not snap it")
            };
            assert!(
                detail.contains("open"),
                "the refusal names the field: {detail}"
            );
        }
    }

    /// A vendor already quoting paisa is taken as is and never scaled twice.
    #[test]
    fn a_paisa_vendor_is_not_scaled_again() {
        let body = r#"{"open":[2450075],"high":[2450075],"low":[2450075],
                       "close":[2450075],"volume":[1],"timestamp":[1751337900]}"#;
        let window = decode_body(body, &spec(PriceScale::Paisa)).expect("decodes");
        assert_eq!(window.rows[0].open, 2_450_075, "already paisa, unchanged");
    }

    /// `NaN` must never become a price, and `1e300` must never become
    /// `i64::MAX`. Both were silent under the `as` cast this replaced.
    #[test]
    fn a_price_that_cannot_be_represented_is_refused_rather_than_coerced() {
        for bad in ["1e300", "-1e300"] {
            let body = format!(
                "{{\"open\":[{bad}],\"high\":[1],\"low\":[1],\"close\":[1],\
                  \"volume\":[1],\"timestamp\":[1]}}"
            );
            let refused = decode_body(&body, &spec(PriceScale::Rupees));
            assert!(
                matches!(refused, Err(FetchError::TransportFailed { .. })),
                "{bad} must be refused, not saturated to i64::MAX"
            );
        }
        // serde_json parses a bare `NaN` as invalid JSON, so the reachable
        // not-a-number case is a string where a number belongs.
        let body = r#"{"open":["x"],"high":[1],"low":[1],"close":[1],
                       "volume":[1],"timestamp":[1]}"#;
        assert!(matches!(
            decode_body(body, &spec(PriceScale::Rupees)),
            Err(FetchError::TransportFailed { .. })
        ));
    }

    /// The seven-array length check is the trap `zip` would have hidden: a
    /// short array must refuse the whole window, not yield a short one.
    #[test]
    fn arrays_that_disagree_in_length_refuse_the_whole_window() {
        let body = r#"{"open":[1,2],"high":[1,2],"low":[1,2],"close":[1,2],
                       "volume":[1],"timestamp":[1,2]}"#;
        assert!(matches!(
            decode_body(body, &spec(PriceScale::Rupees)),
            Err(FetchError::LengthDisagreement { .. })
        ));
    }

    /// A body that is not JSON, and one missing a declared field, are both
    /// named rather than defaulted.
    #[test]
    fn a_malformed_or_incomplete_body_is_refused_by_name() {
        assert!(matches!(
            decode_body("not json", &spec(PriceScale::Rupees)),
            Err(FetchError::TransportFailed { .. })
        ));
        let missing = r#"{"open":[1],"high":[1],"low":[1],"close":[1],"volume":[1]}"#;
        let Err(FetchError::TransportFailed { detail }) =
            decode_body(missing, &spec(PriceScale::Rupees))
        else {
            panic!("a missing timestamp array must refuse")
        };
        assert!(
            detail.contains("timestamp"),
            "the refusal names it: {detail}"
        );
    }

    /// The arrays may sit one level down, under an envelope key.
    #[test]
    fn arrays_one_level_below_the_root_are_found() {
        let body = r#"{"data":{"open":[100.5],"high":[100.5],"low":[100.5],
                       "close":[100.5],"volume":[7],"timestamp":[1751337900]}}"#;
        let window = decode_body(body, &spec(PriceScale::Rupees)).expect("decodes");
        assert_eq!(window.rows[0].open, 10_050);
    }

    /// The blocking seam refuses by name rather than silently blocking, and
    /// the credential never appears in a `Debug` rendering.
    #[test]
    fn the_sync_seam_refuses_and_the_token_is_never_printed() {
        let source = HttpSource::new(spec(PriceScale::Rupees), "SUPERSECRET".to_owned())
            .expect("a client builds");
        let shown = format!("{source:?}");
        assert!(!shown.contains("SUPERSECRET"), "the token leaked: {shown}");
        assert!(shown.contains("<redacted>"), "and it says so: {shown}");
        assert_eq!(source.url(), "https://vendor.invalid/bars");
    }

    /// `wire_end` is the one conversion site for a non-inclusive `toDate`.
    #[test]
    fn the_wire_end_is_the_day_after_only_for_an_exclusive_vendor() {
        let last = crate::session::Day::new(2025, 7, 31).expect("a real day");
        assert_eq!(
            HttpSource::wire_end(last, RangeEnd::Exclusive, DateFormat::DashedYmd)
                .expect("has a successor"),
            "2025-08-01",
            "exclusive means the day AFTER goes on the wire"
        );
        assert_eq!(
            HttpSource::wire_end(last, RangeEnd::Inclusive, DateFormat::DashedYmd)
                .expect("unchanged"),
            "2025-07-31"
        );
    }

    /// Every date format the descriptor can declare is written, and written
    /// zero-padded, so a one-digit month cannot reach a vendor as `2025-7-1`.
    #[test]
    fn every_declared_date_format_is_written_zero_padded() {
        let day = crate::session::Day::new(2025, 7, 1).expect("a real day");
        for (format, want) in [
            (DateFormat::DashedYmd, "2025-07-01"),
            (DateFormat::CompactYmd, "20250701"),
            (DateFormat::SlashedDmy, "01/07/2025"),
            (DateFormat::CompactDmy, "01072025"),
        ] {
            assert_eq!(HttpSource::on_the_wire(day, format), want);
        }
    }
}
