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
            // ── REDIRECTS ARE NOT FOLLOWED, AND THE REASON IS THE CREDENTIAL ──
            //
            // `reqwest` follows up to ten redirects by default, and on a
            // cross-origin hop it strips the headers it considers sensitive:
            // `Authorization`, `Cookie`, `Proxy-Authorization`,
            // `WWW-Authenticate`. **It has no way to know that this vendor's
            // credential is not in one of those.** Dhan's descriptor names its
            // header `access-token` (`crate::vendor`, `AuthScheme::Raw`), which
            // is a custom header like any other, so a 302 from the bars
            // endpoint would put a live broker token on a socket to whatever
            // host the `Location` named — and the strip list would not fire,
            // because the name is not on it. Groww's is `Authorization` and is
            // stripped, but only cross-origin: a redirect to another path on a
            // host that has been taken over still carries it.
            //
            // Nothing legitimate is lost. `bars_path` is a fixed path on a
            // fixed `base_url` in the descriptor; a broker's historical-bars
            // endpoint answering 3xx is not a route change this build should
            // silently chase. With `Policy::none` the 3xx comes back as a
            // response, `is_success` is false, and `window_async` turns it into
            // `VendorRefused` carrying the status — so the operator sees `302`
            // and the `Location`, and decides. `CLAUDE.md` §4: degrade loudly
            // and name the reason, never both silently.
            .redirect(reqwest::redirect::Policy::none())
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
            DateFormat::DashedYmdMidnight => format!("{y:04}-{m:02}-{d:02} 00:00:00"),
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
        // A DATETIME FORMAT MUST NOT COLLAPSE A DAY TO A POINT.
        //
        // `on_the_wire` renders midnight, which is right for the START of a
        // window and wrong for the end of an INCLUSIVE one: a single-day pull
        // then sends `2026-08-04 00:00:00` for both ends, and Groww refuses
        // with `GA001 Start time should be less than end time`. Measured, not
        // reasoned about.
        //
        // An exclusive end is already the following day, so midnight there is
        // exactly the boundary and must stay midnight.
        if format == DateFormat::DashedYmdMidnight && end == RangeEnd::Inclusive {
            let (y, m, d) = (day.year(), day.month(), day.day());
            return Ok(format!("{y:04}-{m:02}-{d:02} 23:59:59"));
        }
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
        ResponseShape::ParallelArrays { envelope } => {
            // EVERY FIELD IS READ FROM ONE OBJECT, RESOLVED ONCE.
            //
            // Each of the seven used to be looked up on its own, and each
            // lookup fell back to searching *every* value one level below the
            // root for a key of that name. So a body holding two objects that
            // both carry bar fields — a payload beside a cached copy, a primary
            // beside a fallback, two exchanges in one answer — could have its
            // `open` taken from the first and its `close` from the second, and
            // the bar assembled from them never existed.
            //
            // Nothing downstream could catch it. The seven-array length check
            // passes when both objects hold the same number of bars, which is
            // exactly when two such objects would appear together, and the
            // result is a window of well-formed bars that no vendor ever sent.
            //
            // The descriptor has always carried `envelope` and this decoder
            // ignored it. It is now the answer: one container, named by the
            // row in `crate::vendor`, and all seven fields come out of it.
            let root = container(&root, envelope)?;
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
                open: prices(root, f.open, spec.prices)?,
                high: prices(root, f.high, spec.prices)?,
                low: prices(root, f.low, spec.prices)?,
                close: prices(root, f.close, spec.prices)?,
                volume: numbers(root, f.volume)?,
                timestamp: numbers(root, f.timestamp)?,
                // OPEN INTEREST IS OPTIONAL AND ITS ABSENCE IS NOT A ZERO.
                // A spot index has none, so the descriptor leaves the name
                // `None` and no array is looked for. When the descriptor DOES
                // name one and the vendor omits it, that is a shape the
                // descriptor got wrong and it must be refused rather than
                // filled in — `CLAUDE.md` §7: `i64::MIN` is the null and zero
                // means zero, so a silent `Vec::new()` here would later read
                // back as real open interest of nothing.
                open_interest: match f.open_interest {
                    Some(name) => numbers(root, name)?,
                    None => Vec::new(),
                },
            };
            RawWindow::decode(&arrays)
        }
        // ONE OBJECT PER BAR — the shape `crate::vendor`'s Groww row declares.
        //
        // This arm refused for as long as no vendor in this build used it, and
        // the refusal said so in as many words: *a decoder nobody has run
        // against a real body is a decoder that is wrong.* That is still true,
        // and it is why the fields below are read strictly by the names the
        // descriptor gives rather than by position — a positional reader would
        // silently file a high as a low the first time a vendor reordered.
        //
        // Each element carries the seven fields the parallel-array shape
        // spreads across seven arrays, so the SAME `prices` and `numbers`
        // conversions run per object: rupees to paisa through `csv::paisa`, no
        // float, and a value off the tick grid refused by name.
        ResponseShape::ArrayOfObjects { envelope } => decode_objects(&root, spec, envelope),
        ResponseShape::PositionalRows { envelope, array } => {
            decode_positional(&root, spec, envelope, array)
        }
    }
}

/// One object per bar, into the same seven arrays the other shape arrives as.
///
/// # Why this transposes rather than adding a second pipeline
///
/// [`RawWindow::decode`] already owns the length check, the row assembly and
/// every refusal below it. A decoder that built rows directly would be a second
/// answer to "what is a bar", and the two would drift — the same argument
/// `crate::ingest::from_members` makes about the two ingest paths. So this
/// collects the objects' fields into columns and hands them to the one decoder.
///
/// # The length check still means something here
///
/// In the parallel-array shape the seven arrays can disagree; here they cannot,
/// because they are built by walking one list. What CAN differ is an object
/// missing a field, and that is refused **by name and by index** rather than
/// filled in — a bar with a defaulted close is a bar that looks real.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] naming the field and the element for
/// anything that is not the declared shape, and whatever [`RawWindow::decode`]
/// refuses.
fn decode_objects(
    root: &serde_json::Value,
    spec: &HttpSpec,
    envelope: Option<&'static str>,
) -> Result<RawWindow, FetchError> {
    let container = container(root, envelope)?;
    let f = spec.fields;

    // The array itself is found the same way a parallel array is: by the name
    // the descriptor gives, inside the one container. `crate::vendor` names it
    // through the same `FieldNames` row, so there is no second convention.
    let items = array_at(container, f.timestamp)
        .or_else(|_| array_at(container, "candles"))
        .or_else(|_| {
            container
                .as_array()
                .ok_or_else(|| FetchError::TransportFailed {
                    detail: format!(
                        "this vendor declares one object per bar, and the \
                         container is neither an array nor holds one named \
                         {:?}. It has: {}",
                        f.timestamp,
                        keys_of(container)
                    ),
                })
        })?;

    let mut arrays = ParallelArrays {
        open: Vec::with_capacity(items.len()),
        high: Vec::with_capacity(items.len()),
        low: Vec::with_capacity(items.len()),
        close: Vec::with_capacity(items.len()),
        volume: Vec::with_capacity(items.len()),
        timestamp: Vec::with_capacity(items.len()),
        open_interest: Vec::new(),
    };

    for (i, item) in items.iter().enumerate() {
        // A field missing from ONE object is refused naming both the field and
        // which bar it was, because "the vendor sent 400 bars and one of them
        // has no close" is a different fault from "the shape is wrong" and
        // sends an operator somewhere different.
        let one = |name: &str| -> Result<&serde_json::Value, FetchError> {
            item.get(name).ok_or_else(|| FetchError::TransportFailed {
                detail: format!("bar {i} carries no {name:?}. It has: {}", keys_of(item)),
            })
        };
        arrays
            .open
            .push(one_price(one(f.open)?, f.open, spec.prices)?);
        arrays
            .high
            .push(one_price(one(f.high)?, f.high, spec.prices)?);
        arrays.low.push(one_price(one(f.low)?, f.low, spec.prices)?);
        arrays
            .close
            .push(one_price(one(f.close)?, f.close, spec.prices)?);
        arrays.volume.push(one_number(one(f.volume)?, f.volume)?);
        arrays
            .timestamp
            .push(one_number(one(f.timestamp)?, f.timestamp)?);
        if let Some(name) = f.open_interest {
            arrays.open_interest.push(one_number(one(name)?, name)?);
        }
    }

    RawWindow::decode(&arrays)
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
        .map(|v| one_price(v, name, scale))
        .collect()
}

/// One price value, whatever shape carried it.
///
/// Shared by both response shapes so a rupee is converted the same way whether
/// it arrived in a column or in an object — two conversions would be two
/// answers, and the second one would lose the paise first.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] naming the field and the value, for anything
/// that is not a number or does not land exactly on the paisa grid.
fn one_price(v: &serde_json::Value, name: &str, scale: PriceScale) -> Result<i64, FetchError> {
    let refuse = || FetchError::TransportFailed {
        detail: format!(
            "{name:?} holds {v}, which is not a price this build can put on the \
             paisa grid"
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
        .map(|v| one_number(v, name))
        .collect()
}

/// One count, whatever shape carried it.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] naming the field and the value.
fn one_number(v: &serde_json::Value, name: &str) -> Result<i64, FetchError> {
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
}

/// The one object this vendor's bar fields are read from.
///
/// # The search this replaces could build a bar out of two objects
///
/// Every field used to resolve itself: look at the top level, and failing that
/// look inside **each** value one level down for a key of that name, taking the
/// first hit. The intent was to tolerate a vendor that wraps its payload in a
/// `data` object, and in a body with exactly one such object it did.
///
/// In a body with two it silently spliced them. `{"live":{...},"cached":{...}}`
/// — or a primary beside a fallback, or two exchanges in one answer — resolves
/// `open` against whichever object `serde_json` yields first and `close` against
/// whichever holds a key of that name, and those need not be the same object.
/// The seven-array length check downstream cannot see it: two objects describing
/// the same window hold the same number of bars, so the lengths agree and a
/// window of bars that were never quoted together lands on disk looking exactly
/// like a real one. `serde_json`'s default map is sorted, so which object won
/// was decided by *alphabetical order of the wrapper keys* — stable, and
/// stably wrong.
///
/// So there is no search. [`crate::vendor`]'s `envelope` says where the bars
/// are, one container is resolved from it here, and all seven fields come out of
/// that container.
///
/// # A descriptor that is wrong says so
///
/// `envelope` for the brokers in this build is **UNVERIFIED against a live
/// body** — no vendor has been reached from this process yet (see the module
/// header). If it is wrong, the refusal below names the key that was expected
/// and lists the keys that were actually there, which is a one-row diff to fix.
/// Guessing instead is what produced the splice.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] when the declared envelope is absent.
fn container<'a>(
    root: &'a serde_json::Value,
    envelope: Option<&'static str>,
) -> Result<&'a serde_json::Value, FetchError> {
    let Some(key) = envelope else {
        return Ok(root);
    };
    root.get(key).ok_or_else(|| FetchError::TransportFailed {
        detail: format!(
            "the descriptor says this vendor hangs its bars under {key:?}, and \
             the answer has no such key. It has: {}",
            keys_of(root)
        ),
    })
}

/// Whether an answer of this many bytes is more than this build will hold.
///
/// **A function rather than an inline comparison, so the boundary is testable.**
/// Written in place, `text.len() > MAX_RESPONSE_BYTES` is a comparison whose
/// `>=` and `==` mutants can only be killed by a test that allocates 64 MiB —
/// which is a test nobody should write and which therefore never got written.
/// Split out, the same boundary is three assertions and no allocation at all.
#[must_use]
const fn too_large(len: usize) -> bool {
    len > MAX_RESPONSE_BYTES
}

/// As much of a vendor's refusal as belongs in an error.
///
/// A refusal body is unbounded input from outside, and an error string is a
/// thing that reaches a log — so it is cut, at characters rather than bytes so
/// the cut cannot land inside one.
fn trim(body: &str) -> String {
    body.chars().take(500).collect()
}

/// The keys of a JSON object, for a refusal that tells an operator what to fix.
///
/// Names only — **never a value**, because a value here is vendor data and a
/// refusal is a string that reaches a log.
fn keys_of(value: &serde_json::Value) -> String {
    value.as_object().map_or_else(
        || "nothing — the answer is not an object".to_owned(),
        |o| {
            let names: Vec<&str> = o.keys().map(String::as_str).collect();
            if names.is_empty() {
                "no keys at all".to_owned()
            } else {
                names.join(", ")
            }
        },
    )
}

/// The array a field names, **in the one container the descriptor chose**.
///
/// No fallback and no second place to look: see [`container`] for what looking
/// in a second place cost.
fn array_at<'a>(
    root: &'a serde_json::Value,
    name: &str,
) -> Result<&'a Vec<serde_json::Value>, FetchError> {
    let Some(array) = root.get(name).and_then(serde_json::Value::as_array) else {
        return Err(FetchError::TransportFailed {
            detail: format!(
                "the vendor's answer has no array named {name:?} where the \
                 descriptor says the bars are. It has: {}",
                keys_of(root)
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

        // THE REQUEST IS BUILT FROM THE DESCRIPTOR ROW, NOT WRITTEN HERE.
        //
        // This was `json!({ "fromDate": from, "toDate": to })` and a two-pair
        // query — a window and nothing else. No instrument, no segment, no
        // interval. That is why Dhan answered `DH-905 securityId is required`
        // and why no broker has ever returned a bar to this build.
        //
        // Every field now comes from `spec.params`, so adding a vendor stays a
        // row in `crate::vendor` and this function never learns either broker's
        // spelling. An empty `params` still sends the window alone, which is
        // what a feed that needs nothing else would want and is also exactly
        // the old behaviour — so the shape did not change, only where it is
        // decided.
        let pairs: Vec<(&'static str, String)> = self
            .spec
            .params
            .iter()
            .map(|p| {
                let v = match p.value {
                    crate::vendor::ParamValue::From => from.clone(),
                    crate::vendor::ParamValue::To => to.clone(),
                    crate::vendor::ParamValue::InstrumentId => request.instrument_id.clone(),
                    crate::vendor::ParamValue::Fixed(word) => word.to_owned(),
                };
                (p.name, v)
            })
            .collect();

        let mut builder = match self.spec.method {
            Method::Post => {
                let body: serde_json::Map<String, serde_json::Value> = pairs
                    .iter()
                    .map(|(n, v)| ((*n).to_owned(), serde_json::Value::String(v.clone())))
                    .collect();
                self.client
                    .post(&url)
                    .json(&serde_json::Value::Object(body))
            }
            Method::Get => self.client.get(&url).query(&pairs),
        };
        // Headers this feed requires beyond the credential — Groww's
        // `X-API-VERSION`, which its own page shows on every historical call.
        for (header, word) in self.spec.extra_headers {
            builder = builder.header(*header, *word);
        }

        let answer = builder.header(name, value).send().await.map_err(|why| {
            FetchError::TransportFailed {
                // `why` is reqwest's own words and never carries the header we
                // set, so the token cannot reach this string.
                detail: format!("{url} was not reached: {why}"),
            }
        })?;

        let status = answer.status().as_u16();
        if !answer.status().is_success() {
            // A REDIRECT IS NOW A REFUSAL, SO IT HAS TO SAY SO IN WORDS.
            //
            // The client does not follow one (see `new`), which means a 3xx
            // arrives here instead of silently becoming a request to another
            // host carrying the credential. Its body is almost always empty, so
            // without this the operator would get `302` and nothing else. The
            // `Location` is named — it is the vendor's own routing, not a
            // secret — and the credential is not, because it never appears in
            // anything this function can reach.
            let hint = answer.status().is_redirection().then(|| {
                let target = answer
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map_or_else(
                        || "no Location header".to_owned(),
                        |v| v.chars().take(200).collect(),
                    );
                format!(
                    "this build does not follow redirects, because the \
                     credential travels in a header no HTTP client knows to \
                     strip. The vendor wanted to send this request to: {target}"
                )
            });
            let body: String = answer.text().await.unwrap_or_default();
            let detail = match hint {
                Some(why) if body.is_empty() => why,
                Some(why) => format!("{why} — and it said: {}", trim(&body)),
                None => trim(&body),
            };
            return Err(FetchError::VendorRefused { status, detail });
        }

        let text = answer
            .text()
            .await
            .map_err(|why| FetchError::TransportFailed {
                detail: format!("the answer could not be read: {why}"),
            })?;
        if too_large(text.len()) {
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

/// One array per bar, read by POSITION: `[ts, open, high, low, close, volume]`
/// and optionally open interest seventh.
///
/// # There are no names here, so the width is the contract
///
/// `decode_objects` reads each field by the name the descriptor gives. This
/// vendor sends no names at all — Groww's page says "in that order" — so a row
/// that is the wrong length is refused naming BOTH lengths rather than read
/// short. A six-element row read as seven would take `null` for volume; a
/// seven read as six would silently drop open interest. Neither is a thing to
/// discover later from a chart that looks nearly right.
///
/// # Errors
///
/// [`FetchError::TransportFailed`] naming the bar index and what was found,
/// for a container that is not the named array, a row that is not an array, a
/// row of an unusable width, or a cell that is not the number it must be.
fn decode_positional(
    root: &serde_json::Value,
    spec: &HttpSpec,
    envelope: Option<&'static str>,
    array: &'static str,
) -> Result<RawWindow, FetchError> {
    let container = container(root, envelope)?;
    let rows = array_at(container, array)?;

    let mut arrays = ParallelArrays {
        open: Vec::with_capacity(rows.len()),
        high: Vec::with_capacity(rows.len()),
        low: Vec::with_capacity(rows.len()),
        close: Vec::with_capacity(rows.len()),
        volume: Vec::with_capacity(rows.len()),
        timestamp: Vec::with_capacity(rows.len()),
        open_interest: Vec::new(),
    };

    for (i, row) in rows.iter().enumerate() {
        let cells = row.as_array().ok_or_else(|| FetchError::TransportFailed {
            detail: format!("bar {i} is {row}, and this vendor sends one ARRAY per bar"),
        })?;
        // Six is the deprecated endpoint, seven the live one, and the seventh
        // is open interest. Any other width is a shape this build has not seen.
        if cells.len() != 6 && cells.len() != 7 {
            return Err(FetchError::TransportFailed {
                detail: format!(
                    "bar {i} carries {} cell(s); this vendor sends six \
                     (timestamp, open, high, low, close, volume) or seven with \
                     open interest last",
                    cells.len()
                ),
            });
        }
        let cell = |at: usize| -> Result<&serde_json::Value, FetchError> {
            cells.get(at).ok_or_else(|| FetchError::TransportFailed {
                detail: format!("bar {i} has no cell {at}"),
            })
        };
        arrays
            .timestamp
            .push(one_stamp(cell(0)?, spec.timestamps, i)?);
        arrays.open.push(one_price(cell(1)?, "open", spec.prices)?);
        arrays.high.push(one_price(cell(2)?, "high", spec.prices)?);
        arrays.low.push(one_price(cell(3)?, "low", spec.prices)?);
        arrays
            .close
            .push(one_price(cell(4)?, "close", spec.prices)?);
        // Volume is `null` on an index, which has none. Zero means zero and the
        // charter's null sentinel is for OPEN INTEREST, not volume, so a null
        // volume becomes 0 rather than i64::MIN.
        arrays.volume.push(match cell(5)? {
            serde_json::Value::Null => 0,
            given => one_number(given, "volume")?,
        });
    }

    RawWindow::decode(&arrays)
}

/// One timestamp cell, in whichever spelling this feed uses.
///
/// A number is taken as it stands; a string is parsed as a local date and time.
/// `fetch::land` then shifts an IST-based value back to UTC — this function's
/// only job is to turn the wire into seconds, and it never guesses which.
fn one_stamp(
    v: &serde_json::Value,
    encoding: crate::vendor::TimestampEncoding,
    at: usize,
) -> Result<i64, FetchError> {
    use crate::vendor::TimestampEncoding as T;
    match encoding {
        T::EpochSecondsUtc | T::EpochMillisUtc => one_number(v, "timestamp"),
        T::IstDateTimeText | T::IsoDateTimeText => {
            let text = v.as_str().ok_or_else(|| FetchError::TransportFailed {
                detail: format!("bar {at} stamps {v}, and this feed spells its timestamps as text"),
            })?;
            local_seconds(text).ok_or_else(|| FetchError::TransportFailed {
                detail: format!(
                    "bar {at} stamps {text:?}, which is not YYYY-MM-DD followed by HH:MM:SS"
                ),
            })
        }
    }
}

/// `YYYY-MM-DD?HH:MM:SS` to seconds since the epoch, treating the value as
/// local. The separator is a `T` on Groww's live endpoint and a space on its
/// deprecated one — and its documentation says space for both — so this accepts
/// either rather than believing the annotation.
fn local_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let day = crate::session::Day::new(
        u16::try_from(y).ok()?,
        u8::try_from(mo).ok()?,
        u8::try_from(d).ok()?,
    )
    .ok()?;
    let days = i64::from(day.days_from_epoch());
    Some(days * 86_400 + h * 3_600 + mi * 60 + sec)
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
        spec_under(prices, None)
    }

    /// The same descriptor, declaring where its bars hang.
    fn spec_under(prices: PriceScale, envelope: Option<&'static str>) -> HttpSpec {
        HttpSpec {
            response: ResponseShape::ParallelArrays { envelope },
            ..spec_top(prices)
        }
    }

    fn spec_top(prices: PriceScale) -> HttpSpec {
        HttpSpec {
            // The window, named — which is what these tests assert and what
            // the request used to hardcode. A real vendor row carries more
            // (`securityId`, `exchangeSegment`); the fixtures that care about
            // those name them for themselves.
            params: &[
                crate::vendor::Param {
                    name: "from",
                    value: crate::vendor::ParamValue::From,
                },
                crate::vendor::Param {
                    name: "to",
                    value: crate::vendor::ParamValue::To,
                },
            ],
            extra_headers: &[],
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

    /// The arrays may sit one level down — **when the descriptor says so.**
    #[test]
    fn arrays_under_the_declared_envelope_are_found() {
        let body = r#"{"data":{"open":[100.5],"high":[100.5],"low":[100.5],
                       "close":[100.5],"volume":[7],"timestamp":[1751337900]}}"#;
        let window =
            decode_body(body, &spec_under(PriceScale::Rupees, Some("data"))).expect("decodes");
        assert_eq!(window.rows[0].open, 10_050);
    }

    /// **ONE BAR, TWO OBJECTS.** The defect this envelope exists to close.
    ///
    /// Each field used to resolve itself: top level first, then the first value
    /// one level down holding a key of that name. With two such objects in one
    /// body the seven fields could come from either, and `serde_json`'s map is
    /// sorted — so `cached` won over `live` by alphabet, on every field that
    /// `live` did not also have at the same place.
    ///
    /// Nothing downstream catches it. Two objects describing the same window
    /// hold the same number of bars, so the seven-array length check passes and
    /// the spliced bar reaches the store looking exactly like a real one.
    ///
    /// Here `cached` quotes a stale 999 and `live` quotes 100.5. A decoder that
    /// searches returns a bar built from both. A decoder told where to look
    /// returns `live`'s bar, whole.
    #[test]
    fn one_bar_can_never_be_assembled_from_two_different_objects() {
        let body = r#"{
            "cached":{"open":[999],"high":[999],"low":[999],"close":[999],
                      "volume":[1],"timestamp":[1]},
            "live":{"open":[100.5],"high":[100.5],"low":[100.5],"close":[100.5],
                    "volume":[7],"timestamp":[1751337900]}
        }"#;
        let window =
            decode_body(body, &spec_under(PriceScale::Rupees, Some("live"))).expect("decodes");
        let row = &window.rows[0];
        assert_eq!(
            (row.open, row.high, row.low, row.close),
            (10_050, 10_050, 10_050, 10_050),
            "every price comes from `live`; a 99900 anywhere here is `cached` \
             leaking into a bar that was never quoted"
        );
        assert_eq!(row.volume, 7, "and so does the volume");
        assert_eq!(row.timestamp, 1_751_337_900, "and the timestamp");

        // The other object is reachable only by naming it, which is the point:
        // which bar you get is the descriptor's decision, never the alphabet's.
        let stale =
            decode_body(body, &spec_under(PriceScale::Rupees, Some("cached"))).expect("decodes");
        assert_eq!(stale.rows[0].open, 99_900);
    }

    /// With no envelope declared, the top level is the only place looked.
    ///
    /// A body that wraps its bars is then refused by name rather than
    /// rummaged through — the refusal is what tells an operator to add the
    /// envelope to the descriptor's row.
    #[test]
    fn no_envelope_means_the_top_level_and_nowhere_else() {
        let wrapped = r#"{"data":{"open":[1],"high":[1],"low":[1],"close":[1],
                          "volume":[1],"timestamp":[1]}}"#;
        let Err(FetchError::TransportFailed { detail }) =
            decode_body(wrapped, &spec(PriceScale::Rupees))
        else {
            panic!("the descriptor says top level, so `data` is not searched")
        };
        assert!(
            detail.contains("open"),
            "the refusal names the field: {detail}"
        );
        assert!(
            detail.contains("data"),
            "and lists what the answer does have, so the descriptor can be \
             corrected: {detail}"
        );
    }

    /// A declared envelope the answer does not carry is refused, and the
    /// refusal carries the two things needed to fix the descriptor.
    #[test]
    fn a_missing_envelope_names_the_key_expected_and_the_keys_present() {
        let body = r#"{"payload":{"open":[1],"high":[1],"low":[1],"close":[1],
                       "volume":[1],"timestamp":[1]}}"#;
        let Err(FetchError::TransportFailed { detail }) =
            decode_body(body, &spec_under(PriceScale::Rupees, Some("data")))
        else {
            panic!("the declared envelope is absent and must be named")
        };
        assert!(detail.contains("\"data\""), "the key expected: {detail}");
        assert!(detail.contains("payload"), "the key present: {detail}");

        // A body that is not an object at all says that rather than listing
        // keys it does not have.
        let Err(FetchError::TransportFailed { detail }) =
            decode_body("[1,2,3]", &spec_under(PriceScale::Rupees, Some("data")))
        else {
            panic!("an array is not an envelope")
        };
        assert!(detail.contains("not an object"), "{detail}");

        // And an object with no keys at all.
        let Err(FetchError::TransportFailed { detail }) =
            decode_body("{}", &spec_under(PriceScale::Rupees, Some("data")))
        else {
            panic!("an empty object holds no envelope")
        };
        assert!(detail.contains("no keys at all"), "{detail}");
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

    /// A server on loopback that answers once and reports what it was sent.
    ///
    /// Raw sockets and hand-written HTTP, because `crates/pull` takes `tokio`
    /// without the `net` feature and a test is not a reason to widen a
    /// dependency. `std::net` in a plain thread is enough to prove where a
    /// header did and did not go.
    ///
    /// Returns the base URL to point a descriptor at, and a handle that yields
    /// the request line and headers of whatever arrived — or `None` if nothing
    /// ever connected, which is the assertion that matters below.
    fn listener(
        answer: Option<String>,
    ) -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::net::SocketAddr,
    ) {
        use std::io::{Read as _, Write as _};
        let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let addr = socket.local_addr().expect("an address");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = socket.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let seen = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned();
            let _ = tx.send(seen);
            if let Some(ref body) = answer {
                let _ = stream.write_all(body.as_bytes());
            }
            let _ = stream.flush();
        });
        (format!("http://{addr}"), rx, addr)
    }

    /// **THE CREDENTIAL MUST NOT FOLLOW A REDIRECT.**
    ///
    /// `reqwest` follows up to ten by default and strips only the headers it
    /// knows are sensitive — `Authorization`, `Cookie`, `Proxy-Authorization`,
    /// `WWW-Authenticate`. Dhan's descriptor calls its credential header
    /// `access-token`, which is on none of those lists, so a 302 from the bars
    /// endpoint would have carried a live broker token to whatever host the
    /// `Location` named. Nothing would have logged it and nothing would have
    /// failed.
    ///
    /// This drives it over two real loopback sockets: the origin answers `302`
    /// pointing at the second, and the second must never be connected to at all.
    #[test]
    fn a_redirect_is_refused_and_the_token_never_reaches_its_target() {
        // The hop answers nothing, because nothing must ever ask it.
        let (hop_url, hop_seen, _) = listener(None);
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: {hop_url}/stolen\r\nContent-Length: 0\r\n\
             Connection: close\r\n\r\n"
        );
        let (origin_url, origin_seen, _) = listener(Some(redirect));

        let spec = HttpSpec {
            base_url: Box::leak(origin_url.into_boxed_str()),
            ..spec(PriceScale::Rupees)
        };
        let source = HttpSource::new(spec, "SUPERSECRET".to_owned()).expect("a client builds");
        let request = BarRequest {
            instrument_id: String::new(),
            window: crate::session::Window::new(
                crate::session::Day::new(2025, 7, 1).expect("a real day"),
                crate::session::Day::new(2025, 7, 1).expect("a real day"),
            )
            .expect("a real window"),
            cadence: crate::session::Cadence::Minute,
        };
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(source.window_async(&request));

        // The origin was reached, and it WAS sent the credential — otherwise
        // this test would pass by never authenticating at all.
        let sent = origin_seen
            .recv_timeout(core::time::Duration::from_secs(5))
            .expect("the origin was contacted");
        assert!(
            sent.contains("SUPERSECRET"),
            "the descriptor's own header must carry the token to the vendor, \
             or this test proves nothing: {sent}"
        );

        // THE HOP WAS NEVER CONTACTED. This is the whole assertion.
        assert!(
            hop_seen
                .recv_timeout(core::time::Duration::from_secs(1))
                .is_err(),
            "the redirect was followed and the credential left for a host the \
             descriptor never named"
        );

        // And the 302 came back as a refusal that says what happened.
        let Err(FetchError::VendorRefused { status, detail }) = outcome else {
            panic!("a redirect is a refusal this build reports, not one it hides")
        };
        assert_eq!(status, 302, "the status reaches the caller");
        assert!(detail.contains("stolen"), "the Location is named: {detail}");
        assert!(
            detail.contains("does not follow redirects"),
            "and the reason is named: {detail}"
        );
        assert!(
            !detail.contains("SUPERSECRET"),
            "the refusal must never carry the credential: {detail}"
        );
    }

    /// An ordinary refusal still carries the vendor's own words, and a 429 is
    /// still distinguishable from a 500 — the redirect arm did not swallow them.
    #[test]
    fn a_non_redirect_refusal_carries_the_body_and_its_status() {
        let body = "rate limited, try later";
        let (url, _seen, _) = listener(Some(format!(
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        )));
        let spec = HttpSpec {
            base_url: Box::leak(url.into_boxed_str()),
            ..spec(PriceScale::Rupees)
        };
        let source = HttpSource::new(spec, "SUPERSECRET".to_owned()).expect("a client builds");
        let request = BarRequest {
            instrument_id: String::new(),
            window: crate::session::Window::new(
                crate::session::Day::new(2025, 7, 1).expect("a real day"),
                crate::session::Day::new(2025, 7, 1).expect("a real day"),
            )
            .expect("a real window"),
            cadence: crate::session::Cadence::Minute,
        };
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(source.window_async(&request));

        let Err(FetchError::VendorRefused { status, detail }) = outcome else {
            panic!("429 is a refusal the governor needs to see")
        };
        assert_eq!(
            status, 429,
            "the governor's business, not a flattened 'failed'"
        );
        assert_eq!(detail, body, "the vendor's own words, unchanged");
        assert!(
            !detail.contains("redirect"),
            "and no redirect wording: {detail}"
        );
    }

    /// One window, fetched over a real socket, end to end.
    ///
    /// Everything else here drives one seam. This drives all of them at once —
    /// the header, the wire dates, the status check, the size bound and the
    /// decode — because until this test existed `window_async`'s success path
    /// was the one part of the vendor client no test had ever entered.
    #[test]
    fn a_window_is_fetched_decoded_and_returned_over_a_real_socket() {
        let body = r#"{"open":[24500.75],"high":[24500.75],"low":[24500.75],
                       "close":[24500.75],"volume":[250],"timestamp":[1751337900],
                       "open_interest":[41]}"#;
        let (url, seen, _) = listener(Some(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )));
        let spec = HttpSpec {
            base_url: Box::leak(url.into_boxed_str()),
            fields: FieldNames {
                open_interest: Some("open_interest"),
                ..spec(PriceScale::Rupees).fields
            },
            ..spec(PriceScale::Rupees)
        };
        let window = source_of(spec, "SUPERSECRET")
            .block_on_window()
            .expect("a window comes back");
        let row = &window.rows[0];
        assert_eq!(row.open, 2_450_075, "the paise survive the whole path");
        assert_eq!(row.volume, 250);
        assert_eq!(
            row.open_interest,
            Some(41),
            "a declared field is read, not zeroed"
        );

        // And the request that produced it carried the descriptor's own header
        // and the descriptor's own wire dates — `toDate` exclusive, so the day
        // AFTER the operator's last day.
        let sent = seen
            .recv_timeout(core::time::Duration::from_secs(5))
            .expect("the vendor was contacted");
        assert!(
            sent.contains("access-token: SUPERSECRET") || sent.contains("x-token: SUPERSECRET")
        );
        assert!(sent.contains("2025-07-01"), "fromDate: {sent}");
        assert!(sent.contains("2025-07-02"), "toDate is exclusive: {sent}");
    }

    /// A `GET` vendor puts its window in the query string, not in a body.
    #[test]
    fn a_get_vendor_carries_its_window_in_the_query_string() {
        let (url, seen, _) = listener(Some(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\
             Connection: close\r\n\r\n"
                .to_owned(),
        ));
        let spec = HttpSpec {
            base_url: Box::leak(url.into_boxed_str()),
            method: Method::Get,
            range_end: RangeEnd::Inclusive,
            auth: Auth {
                // Spelled as `crate::vendor` spells Groww's, capital and all:
                // this fixture is meant to be that descriptor's shape, and a
                // lowercase copy would be a second spelling of one name.
                header: "Authorization",
                scheme: AuthScheme::Bearer,
            },
            ..spec(PriceScale::Rupees)
        };
        let outcome = source_of(spec, "SUPERSECRET").block_on_window();
        let sent = seen
            .recv_timeout(core::time::Duration::from_secs(5))
            .expect("the vendor was contacted");
        assert!(sent.starts_with("GET /bars?"), "a GET with a query: {sent}");
        assert!(sent.contains("from=2025-07-01"), "{sent}");
        assert!(
            sent.contains("to=2025-07-01"),
            "an inclusive vendor takes the day unchanged: {sent}"
        );
        // Header names are case-insensitive and the client writes them
        // lowercased on the wire, so the haystack is folded rather than the
        // needle being written out in the wire's own casing.
        assert!(
            sent.to_lowercase()
                .contains("authorization: bearer supersecret"),
            "the Bearer scheme is a prefix on the value, not a second header: {sent}"
        );
        // A 5xx is not the governor's business and is not a redirect either, so
        // it carries neither redirect wording nor a body it does not have.
        let Err(FetchError::VendorRefused { status, detail }) = outcome else {
            panic!("500 is a refusal")
        };
        assert_eq!(status, 500);
        assert!(
            detail.is_empty(),
            "no body, and nothing invented: {detail:?}"
        );
    }

    /// A redirect with no `Location`, and a redirect that also says something.
    ///
    /// Both arms of the refusal's wording, which the two-socket test above does
    /// not reach: it always sends a `Location` and never a body.
    #[test]
    fn a_redirect_says_so_with_or_without_a_location_and_with_or_without_a_body() {
        let bare = "HTTP/1.1 307 Temporary Redirect\r\nContent-Length: 0\r\n\
                    Connection: close\r\n\r\n";
        let (url, _seen, _) = listener(Some(bare.to_owned()));
        let no_location = HttpSpec {
            base_url: Box::leak(url.into_boxed_str()),
            ..spec(PriceScale::Rupees)
        };
        let Err(FetchError::VendorRefused { status, detail }) =
            source_of(no_location, "SUPERSECRET").block_on_window()
        else {
            panic!("a 307 is still a redirect this build refuses")
        };
        assert_eq!(status, 307);
        assert!(detail.contains("no Location header"), "{detail}");
        assert!(detail.contains("does not follow redirects"), "{detail}");
        // AN EMPTY BODY ADDS NOTHING. Without the `body.is_empty()` guard the
        // refusal ends `— and it said: ` with nothing after it, which reads as
        // a vendor message that was lost rather than one that never existed.
        assert!(
            !detail.contains("and it said"),
            "a 307 with no body must not claim the vendor said something: {detail}"
        );
        assert!(
            detail.ends_with("no Location header"),
            "the refusal ends where the facts do: {detail}"
        );

        // And one that redirects AND explains itself.
        let chatty = "moved, ask elsewhere";
        let (url, _seen, _) = listener(Some(format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: https://elsewhere.invalid/x\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{chatty}",
            chatty.len()
        )));
        let with_body = HttpSpec {
            base_url: Box::leak(url.into_boxed_str()),
            ..spec(PriceScale::Rupees)
        };
        let Err(FetchError::VendorRefused { status, detail }) =
            source_of(with_body, "SUPERSECRET").block_on_window()
        else {
            panic!("a 301 is still a redirect this build refuses")
        };
        assert_eq!(status, 301);
        assert!(detail.contains("elsewhere.invalid"), "the target: {detail}");
        assert!(detail.contains(chatty), "and the vendor's words: {detail}");
        assert!(!detail.contains("SUPERSECRET"), "never the token: {detail}");
    }

    /// The object shape no longer refuses on principle — it refuses on SHAPE.
    ///
    /// This test used to assert "no decoder was written", which was true and is
    /// not any more: Groww's descriptor declares one object per bar, so the
    /// decoder exists. What must still hold is that a body which is *not* that
    /// shape is refused by name rather than guessed at, and that the refusal
    /// says what it did find.
    #[test]
    fn an_object_shape_body_that_is_not_a_list_of_bars_is_refused_by_name() {
        let shape = HttpSpec {
            response: ResponseShape::ArrayOfObjects { envelope: None },
            ..spec(PriceScale::Rupees)
        };
        let Err(FetchError::TransportFailed { detail }) = decode_body("{}", &shape) else {
            panic!("an empty object holds no bars and must be refused")
        };
        assert!(
            detail.contains("one object per bar"),
            "the refusal names the shape it expected: {detail}"
        );
        assert!(
            detail.contains("no keys at all"),
            "and what it actually found: {detail}"
        );
    }

    /// The blocking seam refuses by name rather than spinning a runtime per call.
    #[test]
    fn the_blocking_seam_refuses_and_names_the_asynchronous_one() {
        let source = HttpSource::new(spec(PriceScale::Rupees), "SUPERSECRET".to_owned())
            .expect("a client builds");
        let Err(FetchError::TransportFailed { detail }) = source.window(&one_day()) else {
            panic!("the sync seam cannot work and must say so")
        };
        assert!(
            detail.contains("window_async"),
            "it names the one that does: {detail}"
        );
        assert!(!detail.contains("SUPERSECRET"), "{detail}");
    }

    /// A count written as a decimal means the count, and a fractional one does
    /// not mean anything.
    ///
    /// `250.0` of something is two hundred and fifty; `250.5` of something is a
    /// shape this build does not understand, and it says so rather than
    /// truncating. Same text parser as a price, with the hundredths required to
    /// be zero.
    #[test]
    fn a_count_written_as_a_decimal_is_read_and_a_fractional_one_is_refused() {
        let whole = r#"{"open":[1],"high":[1],"low":[1],"close":[1],
                        "volume":[250.0],"timestamp":[1751337900.0]}"#;
        let window = decode_body(whole, &spec(PriceScale::Paisa)).expect("decodes");
        assert_eq!(window.rows[0].volume, 250, "250.0 of anything is 250");
        assert_eq!(window.rows[0].timestamp, 1_751_337_900);

        for bad in ["250.5", "\"250\"", "250.005"] {
            let body = format!(
                "{{\"open\":[1],\"high\":[1],\"low\":[1],\"close\":[1],\
                  \"volume\":[{bad}],\"timestamp\":[1]}}"
            );
            let Err(FetchError::TransportFailed { detail }) =
                decode_body(&body, &spec(PriceScale::Paisa))
            else {
                panic!("{bad} is not a whole count and must be refused, not truncated")
            };
            assert!(detail.contains("volume"), "the refusal names it: {detail}");
        }
    }

    /// A socket that never answers is a named transport failure, and the
    /// refusal cannot carry the credential — `reqwest`'s own words never
    /// include a header this code set.
    #[test]
    fn a_host_that_cannot_be_reached_is_named_and_never_carries_the_token() {
        // Port 1 on loopback: bound by nothing, and refused immediately rather
        // than left to the 30-second timeout.
        let spec = HttpSpec {
            base_url: "http://127.0.0.1:1",
            ..spec(PriceScale::Rupees)
        };
        let Err(FetchError::TransportFailed { detail }) =
            source_of(spec, "SUPERSECRET").block_on_window()
        else {
            panic!("an unreachable host is a transport failure, not a window")
        };
        assert!(detail.contains("127.0.0.1:1"), "the URL is named: {detail}");
        assert!(detail.contains("was not reached"), "{detail}");
        assert!(
            !detail.contains("SUPERSECRET"),
            "the token leaked: {detail}"
        );
    }

    /// The last day this calendar can name has no successor to put on the wire,
    /// so an exclusive vendor is refused there rather than wrapping.
    #[test]
    fn the_last_nameable_day_has_no_exclusive_successor_and_says_so() {
        let last = crate::session::Day::new(9999, 12, 31).expect("the last day");
        let Err(FetchError::TransportFailed { detail }) =
            HttpSource::wire_end(last, RangeEnd::Exclusive, DateFormat::DashedYmd)
        else {
            panic!("9999-12-31 has no day after it")
        };
        assert!(detail.contains("9999-12-31"), "the day is named: {detail}");
        // An inclusive vendor takes it unchanged, because it needs no successor.
        assert_eq!(
            HttpSource::wire_end(last, RangeEnd::Inclusive, DateFormat::DashedYmd)
                .expect("unchanged"),
            "9999-12-31"
        );
    }

    /// The two bounds this module states as numbers, asserted as numbers.
    ///
    /// `64 * 1024 * 1024` is a product, and `64 + 1024 + 1024` is 2,112 — a
    /// bound that would refuse every real answer while still reading like a
    /// generous one. Nothing else in the suite looks at the value.
    #[test]
    fn the_stated_bounds_are_the_numbers_they_are_written_as() {
        assert_eq!(MAX_RESPONSE_BYTES, 67_108_864, "64 MiB, not 64+1024+1024");
        assert_eq!(REQUEST_TIMEOUT_SECS, 30);

        // And the size bound is tight: the last acceptable answer is exactly
        // MAX_RESPONSE_BYTES, and one byte more is refused. Asserted here
        // rather than over a real body, because a test that allocates 64 MiB to
        // check a `>` is a test nobody runs.
        assert!(!too_large(0), "an empty answer is not too large");
        assert!(!too_large(MAX_RESPONSE_BYTES - 1));
        assert!(
            !too_large(MAX_RESPONSE_BYTES),
            "the bound is inclusive: exactly the maximum is accepted"
        );
        assert!(
            too_large(MAX_RESPONSE_BYTES + 1),
            "and one byte past it is refused"
        );
    }

    /// A one-day request, which is every socket test's window.
    fn one_day() -> BarRequest {
        BarRequest {
            instrument_id: String::new(),
            window: crate::session::Window::new(
                crate::session::Day::new(2025, 7, 1).expect("a real day"),
                crate::session::Day::new(2025, 7, 1).expect("a real day"),
            )
            .expect("a real window"),
            cadence: crate::session::Cadence::Minute,
        }
    }

    /// A source, and a runtime to drive its one asynchronous method.
    // `HttpSpec` passed by value here rather than by reference, and clippy is
    // right that it is now over the 256-byte threshold: it grew a parameter map
    // and a header list. It is `Copy`, this is a test helper called a handful
    // of times, and taking it by reference would make every fixture write `&`
    // for no gain a profile could measure.
    #[allow(
        clippy::large_types_passed_by_value,
        reason = "a Copy descriptor row in a test helper; the copy is the point"
    )]
    fn source_of(spec: HttpSpec, token: &str) -> Driven {
        Driven(HttpSource::new(spec, token.to_owned()).expect("a client builds"))
    }

    struct Driven(HttpSource);

    impl Driven {
        fn block_on_window(&self) -> Result<RawWindow, FetchError> {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(self.0.window_async(&one_day()))
        }
    }

    /// Groww's declared shape, decoded: one object per bar under `payload`.
    ///
    /// This arm refused for as long as no vendor used it. Groww's descriptor
    /// declares it, so it is written — and written to read fields **by the
    /// names the descriptor gives**, never by position. A positional reader
    /// would file a high as a low the first time a vendor reordered its keys,
    /// and every value would still be a plausible price.
    #[test]
    fn one_object_per_bar_decodes_through_the_same_conversions() {
        let body = r#"{"payload":[
            {"open":24500.75,"high":24512.00,"low":24498.50,"close":24510.25,
             "volume":1200,"timestamp":1751341500},
            {"open":24510.25,"high":24518.75,"low":24505.00,"close":24515.00,
             "volume":980,"timestamp":1751341560}
        ]}"#;
        let spec = HttpSpec {
            response: ResponseShape::ArrayOfObjects {
                envelope: Some("payload"),
            },
            ..spec(PriceScale::Rupees)
        };
        let window = decode_body(body, &spec).expect("decodes");
        assert_eq!(window.rows.len(), 2);
        // THE SAME PAISA CONVERSION as the column shape — one implementation,
        // so a rupee cannot be worth two different things depending on which
        // way the vendor happened to send it.
        assert_eq!(window.rows[0].open, 2_450_075);
        assert_eq!(window.rows[0].close, 2_451_025);
        assert_eq!(window.rows[1].high, 2_451_875);
        assert_eq!(window.rows[0].volume, 1200);
        assert_eq!(window.rows[1].timestamp, 1_751_341_560);
    }

    /// A field missing from ONE bar names the field **and which bar**.
    ///
    /// "the vendor sent 400 bars and one has no close" is a different fault
    /// from "the shape is wrong", and it sends an operator somewhere different.
    /// Filling in a default would put a bar on disk that looks entirely real.
    #[test]
    fn a_bar_missing_one_field_is_refused_by_field_and_by_index() {
        let body = r#"{"payload":[
            {"open":1,"high":1,"low":1,"close":1,"volume":1,"timestamp":1},
            {"open":1,"high":1,"low":1,"volume":1,"timestamp":2}
        ]}"#;
        let spec = HttpSpec {
            response: ResponseShape::ArrayOfObjects {
                envelope: Some("payload"),
            },
            ..spec(PriceScale::Paisa)
        };
        let Err(FetchError::TransportFailed { detail }) = decode_body(body, &spec) else {
            panic!("a bar with no close must be refused, not defaulted")
        };
        assert!(detail.contains("close"), "the field: {detail}");
        assert!(detail.contains("bar 1"), "and which bar: {detail}");
        assert!(detail.contains("open"), "and what it did have: {detail}");
    }

    /// The declared envelope is honoured here too — D-0049 applies to both
    /// shapes, so one bar can never be spliced out of two objects.
    #[test]
    fn the_object_shape_reads_only_the_declared_envelope() {
        let body = r#"{
            "cached":[{"open":999,"high":999,"low":999,"close":999,"volume":1,"timestamp":1}],
            "payload":[{"open":100,"high":100,"low":100,"close":100,"volume":7,"timestamp":2}]
        }"#;
        let spec = HttpSpec {
            response: ResponseShape::ArrayOfObjects {
                envelope: Some("payload"),
            },
            ..spec(PriceScale::Paisa)
        };
        let window = decode_body(body, &spec).expect("decodes");
        assert_eq!(window.rows.len(), 1, "only the named envelope is read");
        assert_eq!(window.rows[0].open, 100, "99900 would be `cached` leaking");
        assert_eq!(window.rows[0].volume, 7);
    }

    /// An envelope the answer does not carry is refused, naming what it has.
    #[test]
    fn the_object_shape_refuses_a_missing_envelope_by_name() {
        let spec = HttpSpec {
            response: ResponseShape::ArrayOfObjects {
                envelope: Some("payload"),
            },
            ..spec(PriceScale::Paisa)
        };
        let Err(FetchError::TransportFailed { detail }) = decode_body(r#"{"data":[]}"#, &spec)
        else {
            panic!("the declared envelope is absent and must be named")
        };
        assert!(detail.contains("payload"), "expected: {detail}");
        assert!(detail.contains("data"), "present: {detail}");
    }

    /// An empty list is a real answer — no bars, and no refusal.
    #[test]
    fn an_empty_object_list_is_a_window_of_no_bars_and_not_a_failure() {
        let spec = HttpSpec {
            response: ResponseShape::ArrayOfObjects {
                envelope: Some("payload"),
            },
            ..spec(PriceScale::Paisa)
        };
        let window = decode_body(r#"{"payload":[]}"#, &spec).expect("decodes");
        assert!(window.rows.is_empty(), "no bars is not an error");
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
