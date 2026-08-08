//! What the operator asked a pull to do, and every way it is refused **by
//! name**.
//!
//! # Two forms, not one form with a dropdown
//!
//! A spot pull and an expired-derivative pull do not take the same fields. Spot
//! takes a set of instruments and a window; F&O additionally takes an
//! underlying, a series and an expiry, and its expiry is the field that decides
//! whether the request is legal at all. Folding them into one form means every
//! field is optional, which means every field has to be checked for presence
//! twice — once for "did you fill it in" and once for "does this half of the
//! form need it" — and the second check is the one that gets forgotten. Two
//! forms, two parsers, two shapes.
//!
//! # The dates are `YYYY-MM-DD` because that is what the wire takes
//!
//! `<input type="date">` submits exactly that, and it is exactly what
//! [`pull::session::Day`]'s [`std::fmt::Display`] produces — the shape both
//! vendors take. There is no `DD/MM` versus `MM/DD` question anywhere on the
//! path, because no format that could be ambiguous is ever accepted.
//!
//! The text is parsed **here** and validated **there**: this module splits the
//! ten characters into three integers and hands them to [`Day::new`], which
//! owns every calendar rule including the Gregorian leap year. `session.rs`
//! deliberately has no constructor taking text, and this is not one — it is a
//! form field being turned into the three numbers that constructor already
//! refuses.
//!
//! # A live contract cannot be requested, structurally
//!
//! `CLAUDE.md` §1 permits futures and options to be **stored**; the operator's
//! rule is narrower — only expired series. [`parse_fno`] takes the current day
//! as an argument and refuses any expiry that is not strictly behind it, and
//! the form renders `max` on the expiry input from the same value. The form
//! makes it unofferable and the parser makes it unrequestable, so a hand-built
//! `POST` is refused by the same rule that greys the field out.
//!
//! # Cost
//!
//! Every function here is bounded arithmetic over a form body whose length the
//! server caps. Membership of the F&O universe is one probe of a compile-time
//! table (`brutex_core::universe::of_equity`), never a scan.

use std::fmt;
use std::time::SystemTime;

use brutex_core::symbol::Symbol;
use brutex_core::universe::{self, Universe};
use pull::session::{Day, IstMoment, SessionError, Window};

use crate::server::param;

/// The longest window one request may ask for, in days.
///
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary. Ten
/// years of calendar days, which is longer than either vendor's published
/// history and short enough that the day count cannot be mistaken for a
/// fat-fingered year. A request past it is refused naming both numbers, so an
/// operator who legitimately needs more is told what to raise.
pub const MAX_WINDOW_DAYS: u32 = 3_653;

/// Which instruments a spot pull covers.
///
/// Three fixed sets, not a free list. `CLAUDE.md` §1 fixes the engine surface
/// at two swept indices; the other two sets are stored and never swept, and
/// saying which is which on the form is the difference between an operator
/// knowing what a run will contain and guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpotTarget {
    /// `NSE-NIFTY` and `NSE-BANKNIFTY` — the only two the engine sweeps.
    Swept,
    /// Every NSE index series held for reference, including `NSE-INDIAVIX`.
    ///
    /// Stored and stamped onto trades; never in the condition vocabulary, never
    /// in ranking, never in run identity. `CLAUDE.md` §1.
    Indices,
    /// The NIFTY Total Market constituents. Stored, never swept.
    Equities,
}

impl SpotTarget {
    /// Every target, in the order the form lists them.
    pub const ALL: [Self; 3] = [Self::Swept, Self::Indices, Self::Equities];

    /// The value this target carries on the wire.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Swept => "swept",
            Self::Indices => "indices",
            Self::Equities => "equities",
        }
    }

    /// What the form calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Swept => "Swept indices",
            Self::Indices => "Reference indices",
            Self::Equities => "NIFTY Total Market equities",
        }
    }

    /// One line saying what the engine does with what this pulls.
    #[must_use]
    pub const fn note(self) -> &'static str {
        match self {
            Self::Swept => "NSE-NIFTY and NSE-BANKNIFTY — the only two swept",
            Self::Indices => "stored and stamped onto trades; never swept",
            Self::Equities => "stored, never swept",
        }
    }

    /// Which universe bit selects this target's members.
    #[must_use]
    pub const fn universe(self) -> Universe {
        match self {
            Self::Swept | Self::Indices => Universe::INDEX,
            Self::Equities => Universe::TOTAL_MARKET,
        }
    }

    /// The target a form field names.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.slug() == slug)
    }
}

/// Which derivative series an F&O request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Series {
    /// The future on that expiry.
    Futures,
    /// Every option strike on that expiry.
    Options,
}

impl Series {
    /// Both series, in the order the form lists them.
    pub const ALL: [Self; 2] = [Self::Futures, Self::Options];

    /// The value this series carries on the wire.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Futures => "fut",
            Self::Options => "opt",
        }
    }

    /// What the form calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Futures => "Future",
            Self::Options => "Option chain",
        }
    }

    /// The series a form field names.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.slug() == slug)
    }
}

/// Why a request was not accepted.
///
/// Every variant carries the value it refused. `CLAUDE.md` §4: degrade loudly
/// and name the reason. A form that answered "invalid input" would send an
/// operator back to guess which of five fields it meant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// A field the form always sends arrived empty or not at all.
    FieldMissing {
        /// Which field.
        field: &'static str,
    },
    /// A date field is not ten characters of `YYYY-MM-DD`.
    DateNotIso {
        /// Which field.
        field: &'static str,
        /// What arrived.
        got: String,
    },
    /// A date field is well shaped and names no day that exists.
    DateImpossible {
        /// Which field.
        field: &'static str,
        /// What arrived.
        got: String,
        /// The calendar's own refusal.
        why: SessionError,
    },
    /// The window's end is before its start.
    ///
    /// Refused rather than silently swapped, for the reason
    /// [`pull::session::SessionError::WindowRunsBackwards`] gives: a pull that
    /// quietly reversed them would fetch a range nobody asked for and report
    /// success.
    WindowBackwards {
        /// The calendar's own refusal, which names both ends.
        why: SessionError,
    },
    /// The window is longer than [`MAX_WINDOW_DAYS`].
    WindowTooLong {
        /// How many days were asked for.
        days: u32,
        /// The bound.
        cap: u32,
    },
    /// The window ends after today. No vendor holds a bar that has not traded.
    ///
    /// This variant exists because the spot form had only the HTML `max`
    /// attribute stopping it, while the F&O form one panel over carried a real
    /// parser gate and the sentence *"an attribute is a courtesy, a parser is a
    /// rule"*. The two disagreed, and the spot side was the courtesy. An
    /// attribute is absent from a `curl`, from a replayed POST, and from any
    /// client that is not the browser the page was rendered for.
    ///
    /// `api::unit::a_window_ending_after_today_is_refused_by_the_parser` is
    /// what holds it up, and it posts a body rather than inspecting the HTML —
    /// the previous test asserted the attribute *string was present*, which is
    /// a different claim and was the reason this went unnoticed.
    WindowInFuture {
        /// The end that has not happened.
        to: Day,
        /// Today in IST, as the clock reported it.
        today: Day,
    },
    /// The spot form's target is not one of [`SpotTarget::ALL`].
    UnknownTarget {
        /// What arrived.
        got: String,
    },
    /// The F&O form's series is not one of [`Series::ALL`].
    UnknownSeries {
        /// What arrived.
        got: String,
    },
    /// A vendor was named, and it is not one this build has a feed for.
    ///
    /// Distinct from naming none at all, which still defaults — see
    /// [`parse_vendor`] for why that half of the old behaviour is kept.
    UnknownVendor {
        /// What arrived.
        got: String,
    },
    /// The underlying is not a symbol this engine can even name.
    BadUnderlying {
        /// What arrived.
        got: String,
    },
    /// The underlying is a legal symbol with no F&O series on it.
    NotAnFnoUnderlying {
        /// The symbol, canonicalised.
        symbol: String,
    },
    /// **The expiry has not passed.** A live contract is never stored.
    LiveContract {
        /// The expiry that was asked for.
        expiry: Day,
        /// The day it was compared against.
        today: Day,
    },
    /// The window runs past the day the contract stopped trading.
    WindowOutlivesTheContract {
        /// The last day asked for.
        to: Day,
        /// The expiry.
        expiry: Day,
    },
    /// The machine's clock names no date this build can render.
    ///
    /// Not a fall-back to a guessed date: an expiry gate that compared against
    /// an invented "today" would be a gate that passes for the wrong reason.
    ClockUnusable {
        /// The calendar's own refusal.
        why: SessionError,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::FieldMissing { field } => {
                write!(f, "REFUSED · {field} was not filled in")
            }
            Self::DateNotIso { field, ref got } => write!(
                f,
                "REFUSED · {field} {got:?} is not a date — it must be ten \
                 characters of YYYY-MM-DD, which is what the vendors take"
            ),
            Self::DateImpossible {
                field,
                ref got,
                why,
            } => write!(f, "REFUSED · {field} {got:?} is not a day: {why}"),
            Self::WindowBackwards { why } => write!(f, "REFUSED · {why}"),
            Self::WindowTooLong { days, cap } => write!(
                f,
                "REFUSED · the window is {days} days; this build fetches at most {cap}"
            ),
            Self::WindowInFuture { to, today } => write!(
                f,
                "REFUSED · the window ends {to}, which is after {today}. No \
                 vendor holds a bar that has not traded yet. An attribute is a \
                 courtesy, a parser is a rule."
            ),
            Self::UnknownTarget { ref got } => {
                write!(f, "REFUSED · {got:?} is not one of the three spot targets")
            }
            Self::UnknownSeries { ref got } => {
                write!(f, "REFUSED · {got:?} is neither a future nor an option")
            }
            Self::UnknownVendor { ref got } => write!(
                f,
                "REFUSED · {got:?} is not a vendor this build has a feed for. \
                 Nothing was pulled rather than another vendor's bars being \
                 filed under a prefix you did not ask for"
            ),
            Self::BadUnderlying { ref got } => {
                write!(f, "REFUSED · {got:?} is not a symbol this engine can name")
            }
            Self::NotAnFnoUnderlying { ref symbol } => write!(
                f,
                "REFUSED · {symbol} carries no F&O series, so there is no \
                 expired contract on it to store"
            ),
            Self::LiveContract { expiry, today } => write!(
                f,
                "REFUSED · {expiry} has not expired as of {today}. A LIVE \
                 CONTRACT IS NEVER STORED — expired series only"
            ),
            Self::WindowOutlivesTheContract { to, expiry } => write!(
                f,
                "REFUSED · the window ends {to}, after the contract expired on \
                 {expiry}; there are no bars there to fetch"
            ),
            Self::ClockUnusable { why } => write!(
                f,
                "REFUSED · this machine's clock names no usable date ({why}), \
                 so no expiry can be checked against it"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// One spot pull, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotRequest {
    /// Which set of instruments.
    pub target: SpotTarget,
    /// The operator's inclusive range.
    pub window: Window,
    /// Which broker to ask.
    ///
    /// Parsed rather than hardcoded, because `CLAUDE.md` makes adding a vendor
    /// a row in `pull::vendor` — and a route that names one defeats that. An
    /// unstated or unknown vendor is **Dhan**, which is the one whose
    /// descriptor has been verified against a live body; Groww's has not, and
    /// defaulting to the unverified one would make a first-time operator debug
    /// a shape nobody has confirmed.
    pub vendor: brutex_core::vendor::Vendor,
}

/// One expired-derivative pull, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnoRequest {
    /// The underlying, canonicalised.
    pub underlying: Symbol,
    /// Future or option chain.
    pub series: Series,
    /// The expiry, which [`parse_fno`] has already proved is behind `today`.
    pub expiry: Day,
    /// The operator's inclusive range.
    pub window: Window,
}

/// One `YYYY-MM-DD` form field, as a validated day.
///
/// Strict on shape before it is strict on value: `2024-2-9` is refused as
/// malformed rather than accepted as February the 9th, because a parser that
/// tolerates a missing digit is a parser that will one day tolerate a missing
/// field.
///
/// # Errors
///
/// [`Refusal::FieldMissing`], [`Refusal::DateNotIso`], or
/// [`Refusal::DateImpossible`] carrying whatever [`Day::new`] refused.
pub fn parse_day(field: &'static str, text: &str) -> Result<Day, Refusal> {
    if text.is_empty() {
        return Err(Refusal::FieldMissing { field });
    }
    let bad = || Refusal::DateNotIso {
        field,
        got: text.to_owned(),
    };
    // Split on the separators the shape declares, then require each piece to be
    // exactly as wide as the shape says. `splitn` alone would accept
    // `2024-2-9`; the width checks are what make the format the format.
    let mut parts = text.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad());
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(bad());
    }
    let (Ok(year), Ok(month), Ok(day)) = (y.parse::<u16>(), m.parse::<u8>(), d.parse::<u8>())
    else {
        return Err(bad());
    };
    Day::new(year, month, day).map_err(|why| Refusal::DateImpossible {
        field,
        got: text.to_owned(),
        why,
    })
}

/// One date field, however the client chose to send it.
///
/// **Two spellings, one parser.** The calendar in [`crate::calendar`] posts three
/// integers — `{field}_y`, `{field}_m`, `{field}_d` — because that is what keeps
/// a no-script picker down to 57 controls instead of 4,464. A hand-built POST,
/// `curl`, and every test written before the picker existed send the whole
/// `{field}=YYYY-MM-DD`. Both arrive here.
///
/// The triple is **composed into the ISO string and handed to [`parse_day`]**
/// rather than validated separately. That is the point: one definition of what a
/// date is, so the picker cannot be accepted by rules the wire form is not, and
/// `Day::new` still owns the leap year. Zero-padding on the way in also means a
/// client that posts `m=8` and one that posts `m=08` get the same answer.
///
/// ISO wins when both are present — it is the explicit form, and the picker
/// never sends it.
///
/// # Errors
///
/// [`Refusal::FieldMissing`] when neither spelling is present, and when the
/// triple is there but incomplete — a piece absent and a piece present-but-empty
/// are the same unfinished date. Otherwise whatever [`parse_day`] refuses.
pub fn parse_day_field(body: &str, field: &'static str) -> Result<Day, Refusal> {
    let iso = param(body, field);
    if !iso.is_empty() {
        return parse_day(field, &iso);
    }
    let (y, m, d) = (
        param(body, &format!("{field}_y")),
        param(body, &format!("{field}_m")),
        param(body, &format!("{field}_d")),
    );
    // ANY PIECE MISSING IS A DATE THAT WAS NOT FINISHED, NOT A MALFORMED ONE.
    //
    // This used to require all three to be empty before saying so, and compose
    // whatever was left otherwise — so a triple with the month cleared became
    // the string `-08-06`, which came back as `DateNotIso` naming a format the
    // operator never typed.
    //
    // The picker made that reachable by design. Its drill-down header steps
    // back a pane by checking a radio whose value is the empty string (see
    // `crate::calendar`), because checking a sibling is the only way a
    // `<label>` can uncheck a radio — so a half-picked date now arrives with a
    // piece present and EMPTY rather than absent. Both spellings mean the same
    // thing and both belong in the same refusal.
    if y.is_empty() || m.is_empty() || d.is_empty() {
        return Err(Refusal::FieldMissing { field });
    }
    // Padded so `8` and `08` mean the same August, then parsed by the one
    // parser. A piece that is not a number stays un-padded and fails the width
    // check inside `parse_day`, which is where a malformed date belongs.
    let pad = |text: &str, width: usize| -> String {
        text.parse::<u16>()
            .map_or_else(|_| text.to_owned(), |n| format!("{n:0width$}"))
    };
    parse_day(
        field,
        &format!("{}-{}-{}", pad(&y, 4), pad(&m, 2), pad(&d, 2)),
    )
}

/// The operator's inclusive window, from the two date fields both forms carry.
///
/// # Errors
///
/// Whatever [`parse_day_field`] refuses, [`Refusal::WindowBackwards`], or
/// [`Refusal::WindowTooLong`].
pub fn parse_window(body: &str, today: Day) -> Result<Window, Refusal> {
    let from = parse_day_field(body, "from")?;
    let to = parse_day_field(body, "to")?;
    let window = Window::new(from, to).map_err(|why| Refusal::WindowBackwards { why })?;
    let days = window.days();
    if days > MAX_WINDOW_DAYS {
        return Err(Refusal::WindowTooLong {
            days,
            cap: MAX_WINDOW_DAYS,
        });
    }
    // THE GATE THIS FUNCTION WAS MISSING. `to` is inclusive, so a window
    // ending today is legal — today has traded, or is trading. A window ending
    // tomorrow is not, and no vendor holds a bar that has not happened.
    //
    // `today` is an argument rather than a clock call inside, for the same
    // reason `parse_fno` takes it: a gate that reads the clock cannot be tested
    // at its own boundary without waiting for midnight, and an invented "today"
    // would be a gate that passes for the wrong reason.
    if to > today {
        return Err(Refusal::WindowInFuture { to, today });
    }
    Ok(window)
}

/// One spot request, out of a form body.
///
/// # Errors
///
/// [`Refusal::UnknownTarget`], or whatever [`parse_window`] refuses.
pub fn parse_spot(body: &str, today: Day) -> Result<SpotRequest, Refusal> {
    let raw = param(body, "target");
    if raw.is_empty() {
        return Err(Refusal::FieldMissing { field: "target" });
    }
    let target = SpotTarget::from_slug(&raw).ok_or(Refusal::UnknownTarget { got: raw })?;
    Ok(SpotRequest {
        target,
        window: parse_window(body, today)?,
        vendor: {
            let raw = param(body, "vendor");
            parse_vendor(&raw).ok_or(Refusal::UnknownVendor { got: raw })?
        },
    })
}

/// Which broker a request names: `None` when it named one this build cannot serve.
///
/// # An UNNAMED vendor still defaults; a WRONGLY named one no longer does
///
/// The default when the field is absent is **Dhan**, because its descriptor is
/// the one verified against a live body — defaulting to the unverified one
/// would make a first-time operator debug a response shape nobody has
/// confirmed. That half of the original reasoning is sound and is kept.
///
/// The other half is not, and it was not wrong when it was written. It said:
///
/// > The vendor is a *route*, not a claim about the data: naming one that does
/// > not exist cannot corrupt anything.
///
/// That was true while the vendor only chose which host to call. It stopped
/// being true when `Plan.vendor` became the STORE PREFIX: bars land under
/// `bars/<vendor>/…`, so coercing an unrecognised name to Dhan files one
/// broker's prices under another's path — which `pull::vendor::Feed::store_vendor`
/// documents as destroying D-0019's per-vendor independence irreversibly. The
/// justification quietly outlived the fact it rested on.
///
/// The form offering a fixed list was the other prop under that argument, and
/// `/bars` knocks it out: it is a GET whose vendor comes from a hand-typed
/// query string with no list at all.
///
/// So: absent means Dhan and says so; present-but-unknown refuses by name.
#[must_use]
pub fn parse_vendor(raw: &str) -> Option<brutex_core::vendor::Vendor> {
    if raw.is_empty() {
        return Some(brutex_core::vendor::Vendor::Dhan);
    }
    brutex_core::vendor::Vendor::ALL
        .into_iter()
        .find(|v| v.as_str().eq_ignore_ascii_case(raw))
}

/// One expired-derivative request, out of a form body.
///
/// `today` is an argument rather than a call to the clock inside, for the
/// reason `CLAUDE.md` §3 rule 5 gives about reruns: an expiry gate whose answer
/// depends on when it ran is a gate no test can pin. The route supplies the
/// real day; a test supplies the day it means.
///
/// # Errors
///
/// [`Refusal::BadUnderlying`], [`Refusal::NotAnFnoUnderlying`],
/// [`Refusal::UnknownSeries`], [`Refusal::LiveContract`],
/// [`Refusal::WindowOutlivesTheContract`], or whatever [`parse_window`]
/// refuses.
pub fn parse_fno(body: &str, today: Day) -> Result<FnoRequest, Refusal> {
    let typed = param(body, "underlying");
    if typed.is_empty() {
        return Err(Refusal::FieldMissing {
            field: "underlying",
        });
    }
    // Upper-cased before it is validated: an operator types `nifty` and means
    // NIFTY, and `Symbol` is canonical upper case. Refusing the typing rather
    // than the symbol would be a refusal about nothing.
    let upper = typed.to_uppercase();
    let underlying = Symbol::new(&upper).map_err(|_| Refusal::BadUnderlying { got: typed })?;
    // One probe of a compile-time membership table. An underlying with no F&O
    // series has no expired contract to store, so this is a refusal rather than
    // an empty pull that looks like a vendor outage.
    if !universe::of_equity(underlying.as_str()).contains(Universe::FNO) {
        return Err(Refusal::NotAnFnoUnderlying {
            symbol: underlying.as_str().to_owned(),
        });
    }

    let raw = param(body, "series");
    if raw.is_empty() {
        return Err(Refusal::FieldMissing { field: "series" });
    }
    let series = Series::from_slug(&raw).ok_or(Refusal::UnknownSeries { got: raw })?;

    let expiry = parse_day_field(body, "expiry")?;
    // THE GATE. Strictly behind today: a contract expiring today is still
    // trading today, and `CLAUDE.md` §8's argument about a fall-back applies
    // here too — `<=` would admit exactly the case the rule exists to exclude.
    if expiry >= today {
        return Err(Refusal::LiveContract { expiry, today });
    }

    let window = parse_window(body, today)?;
    if window.to() > expiry {
        return Err(Refusal::WindowOutlivesTheContract {
            to: window.to(),
            expiry,
        });
    }
    Ok(FnoRequest {
        underlying,
        series,
        expiry,
        window,
    })
}

/// Epoch seconds of a moment, in either direction.
///
/// Split from [`today_ist`] so both directions are testable without waiting for
/// 1969: a `SystemTime` before the epoch is a value a test constructs, not a
/// machine state it has to arrange.
///
/// Public because [`crate::audit`] stamps the same number into every record it
/// writes, and two spellings of "which second is it" is exactly the second
/// definition `CLAUDE.md` §3 rule 1 is about.
#[must_use]
pub fn epoch_secs(at: SystemTime) -> i64 {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        // The saturating arm lives inside `Result::unwrap_or`, which is not
        // this repository's code to measure; it is reachable only from a clock
        // 292 billion years out, and saturating there keeps the refusal below
        // honest rather than wrapping into a plausible date.
        Ok(ahead) => i64::try_from(ahead.as_secs()).unwrap_or(i64::MAX),
        Err(behind) => {
            i64::try_from(behind.duration().as_secs()).map_or(i64::MIN, i64::saturating_neg)
        }
    }
}

/// The IST date of a moment.
///
/// # Errors
///
/// [`Refusal::ClockUnusable`] for a clock before 1970 or past 9999. There is no
/// default: an expiry compared against a guessed day is a gate that passes for
/// the wrong reason.
pub fn ist_day(at: SystemTime) -> Result<Day, Refusal> {
    IstMoment::from_epoch_secs(epoch_secs(at))
        .map(IstMoment::day)
        .map_err(|why| Refusal::ClockUnusable { why })
}

/// Today, in IST, from this machine's clock.
///
/// # Errors
///
/// Whatever [`ist_day`] refuses.
pub fn today_ist() -> Result<Day, Refusal> {
    ist_day(SystemTime::now())
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
    use std::time::Duration;

    fn day(y: u16, m: u8, d: u8) -> Day {
        Day::new(y, m, d).expect("a real date")
    }

    /// A "today" far enough ahead that the future-window gate is inert.
    ///
    /// Every window test below predates this by decades, so they exercise the
    /// rule they were written for and not the new one. The gate gets its own
    /// test at its own boundary rather than being smuggled into theirs — a
    /// shared constant that silently decides two rules is how a test starts
    /// passing for the wrong reason.
    const TEST_TODAY: Day = match Day::new(2099, 12, 31) {
        Ok(d) => d,
        Err(_) => panic!("2099-12-31 is a real date"),
    };

    /// The gate `parse_window` was missing, at both sides of its boundary.
    ///
    /// `to` is **inclusive**, so a window ending today is legal — today has
    /// traded, or is trading. Tomorrow has not.
    ///
    /// This posts a body. The test it replaces asserted that the string `max=`
    /// appeared in the rendered HTML, which proves the browser was asked
    /// nicely and proves nothing at all about `curl`, a replayed POST, or any
    /// client that is not the page. That gap is why this shipped.
    #[test]
    fn a_window_ending_after_today_is_refused_by_the_parser() {
        let today = day(2026, 8, 7);

        let ends_today = parse_window("from=2026-08-01&to=2026-08-07", today)
            .expect("a window ending today is legal — today has traded");
        assert_eq!(ends_today.days(), 7, "1st through 7th inclusive");

        let tomorrow = parse_window("from=2026-08-01&to=2026-08-08", today)
            .expect_err("a window ending tomorrow has no bars to fetch");
        assert_eq!(
            tomorrow,
            Refusal::WindowInFuture {
                to: day(2026, 8, 8),
                today,
            },
            "refused by name, carrying both dates"
        );

        let text = tomorrow.to_string();
        assert!(
            text.contains("2026-08-08") && text.contains("2026-08-07"),
            "the refusal names the end and today, so an operator can see the \
             difference without opening a calendar — got {text:?}"
        );

        let far = parse_window("from=2026-08-01&to=2099-01-01", today)
            .expect_err("a window years ahead is refused too");
        // It breaks both rules, and it is refused for the one that is checked
        // first. Asserted exactly rather than as "one of two": a test that
        // accepts either answer cannot tell the two gates apart, and would
        // still pass if the order silently changed.
        let span = Window::new(day(2026, 8, 1), day(2099, 1, 1))
            .expect("a forward window")
            .days();
        assert!(
            span > MAX_WINDOW_DAYS,
            "the fixture must break the length bound as well as the future one"
        );
        assert_eq!(
            far,
            Refusal::WindowTooLong {
                days: span,
                cap: MAX_WINDOW_DAYS,
            },
            "the length bound is checked before the future gate, so that is \
             the refusal an operator sees — got {far:?}"
        );
    }

    /// The day every F&O test compares against.
    fn today() -> Day {
        day(2026, 8, 7)
    }

    #[test]
    fn a_date_field_is_refused_by_name_whichever_way_it_is_wrong() {
        // The wire shape, accepted.
        assert_eq!(parse_day("from", "2022-01-08"), Ok(day(2022, 1, 8)));
        // A leap day that exists, and one that does not — the calendar rule is
        // `Day::new`'s and this proves it is actually consulted.
        assert_eq!(parse_day("to", "2024-02-29"), Ok(day(2024, 2, 29)));
        assert_eq!(
            parse_day("to", "2023-02-29"),
            Err(Refusal::DateImpossible {
                field: "to",
                got: "2023-02-29".to_owned(),
                why: SessionError::DayOutOfRange {
                    day: 29,
                    month_len: 28
                },
            })
        );

        // Empty is its own refusal: "you left it blank" and "you typed
        // nonsense" send an operator to different places.
        assert_eq!(
            parse_day("from", ""),
            Err(Refusal::FieldMissing { field: "from" })
        );

        // Every malformed shape is DateNotIso, and each names the field.
        for bad in [
            "2024-2-9",     // unpadded — the ambiguity this format exists to kill
            "08/01/2022",   // the format nobody can read the same way twice
            "2022-01-08x",  // trailing rubbish
            "2022-01",      // truncated
            "2022-01-08-1", // one part too many
            "yyyy-mm-dd",   // not digits
            "-001-01-01",   // a sign is not a digit
            "20220-1-08",   // right length, wrong widths
        ] {
            assert_eq!(
                parse_day("expiry", bad),
                Err(Refusal::DateNotIso {
                    field: "expiry",
                    got: bad.to_owned()
                }),
                "{bad:?} must be refused as malformed"
            );
        }

        // Out-of-calendar values reach `Day::new` and are refused there, so the
        // month and year arms are not this parser's second opinion — and the
        // calendar's OWN refusal is what comes back, not a paraphrase.
        assert_eq!(
            parse_day("from", "2022-13-01"),
            Err(Refusal::DateImpossible {
                field: "from",
                got: "2022-13-01".to_owned(),
                why: SessionError::MonthOutOfRange { month: 13 },
            })
        );
        assert_eq!(
            parse_day("from", "1969-12-31"),
            Err(Refusal::DateImpossible {
                field: "from",
                got: "1969-12-31".to_owned(),
                why: SessionError::YearOutOfRange { year: 1969 },
            })
        );

        // Every refusal SAYS which field and what it saw.
        let text = parse_day("expiry", "2024-2-9")
            .expect_err("malformed")
            .to_string();
        assert!(
            text.contains("expiry") && text.contains("2024-2-9"),
            "{text}"
        );
        assert!(text.contains("YYYY-MM-DD"), "and what it wanted: {text}");
    }

    #[test]
    fn a_window_is_inclusive_both_ends_and_a_backwards_one_is_refused_not_swapped() {
        // A single day is legal and is the commonest resume shape.
        let one = parse_window("from=2022-01-08&to=2022-01-08", TEST_TODAY).expect("one day");
        assert_eq!(one.days(), 1);
        assert_eq!(one.from(), one.to());

        // A range across a year boundary is arithmetic, not a special case.
        let over =
            parse_window("from=2021-12-30&to=2022-01-02", TEST_TODAY).expect("across the year");
        assert_eq!(over.days(), 4, "30, 31, 1, 2 — both ends included");
        assert_eq!(over.from().year(), 2021);
        assert_eq!(over.to().year(), 2022);
        // AND THE NON-INCLUSIVE VENDOR `toDate` IS THE DAY AFTER. This is the
        // correction the page tells the operator about rather than performing
        // in silence.
        assert_eq!(over.wire_to().expect("succ").to_string(), "2022-01-03");

        // Backwards is refused, and the refusal names both ends.
        let back =
            parse_window("from=2022-02-08&to=2022-01-08", TEST_TODAY).expect_err("backwards");
        assert_eq!(
            back,
            Refusal::WindowBackwards {
                why: SessionError::WindowRunsBackwards {
                    from: day(2022, 2, 8),
                    to: day(2022, 1, 8),
                },
            }
        );
        let text = back.to_string();
        assert!(
            text.contains("2022-02-08") && text.contains("2022-01-08"),
            "{text}"
        );
        assert!(text.contains("backwards"), "{text}");

        // A missing end is named, not defaulted to today.
        assert_eq!(
            parse_window("to=2022-01-08", TEST_TODAY),
            Err(Refusal::FieldMissing { field: "from" })
        );
        assert_eq!(
            parse_window("from=2022-01-08", TEST_TODAY),
            Err(Refusal::FieldMissing { field: "to" })
        );
    }

    #[test]
    fn the_window_is_bounded_at_the_boundary_and_the_bound_is_tight() {
        // Exactly the cap is accepted; one day more is refused naming both
        // numbers. A bound tested only well past itself is a bound whose
        // comparison can be flipped with the suite green.
        let last = day(2020, 1, 1)
            .days_from_epoch()
            .checked_add(MAX_WINDOW_DAYS - 1)
            .expect("in range");
        let at_cap = Day::from_days(last).expect("in range");
        let body = format!("from=2020-01-01&to={at_cap}");
        assert_eq!(
            parse_window(&body, TEST_TODAY)
                .expect("exactly the cap")
                .days(),
            MAX_WINDOW_DAYS
        );

        let over = Day::from_days(last + 1).expect("in range");
        let body = format!("from=2020-01-01&to={over}");
        assert_eq!(
            parse_window(&body, TEST_TODAY),
            Err(Refusal::WindowTooLong {
                days: MAX_WINDOW_DAYS + 1,
                cap: MAX_WINDOW_DAYS,
            })
        );
        let text = parse_window(&body, TEST_TODAY)
            .expect_err("too long")
            .to_string();
        assert!(text.contains("3654") && text.contains("3653"), "{text}");
    }

    #[test]
    fn a_spot_request_names_its_target_or_is_refused() {
        for target in SpotTarget::ALL {
            let body = format!("target={}&from=2022-01-08&to=2022-02-08", target.slug());
            let asked = parse_spot(&body, TEST_TODAY).expect("a real target");
            assert_eq!(asked.target, target);
            assert_eq!(asked.window.days(), 32);
            // Round-trip: the slug the form emits is the slug the parser reads.
            assert_eq!(SpotTarget::from_slug(target.slug()), Some(target));
        }
        // EVERY LABEL AND NOTE IS PINNED TO ITS TEXT, not merely to being
        // non-empty. These strings are the only thing on the form that says
        // which sets the engine sweeps and which it merely stores, and a
        // "non-empty" assertion is satisfied by any string at all.
        for (target, label, note) in [
            (
                SpotTarget::Swept,
                "Swept indices",
                "NSE-NIFTY and NSE-BANKNIFTY — the only two swept",
            ),
            (
                SpotTarget::Indices,
                "Reference indices",
                "stored and stamped onto trades; never swept",
            ),
            (
                SpotTarget::Equities,
                "NIFTY Total Market equities",
                "stored, never swept",
            ),
        ] {
            assert_eq!(target.label(), label);
            assert_eq!(target.note(), note);
        }
        // Only the two swept indices are swept; the other two sets are stored.
        assert_eq!(SpotTarget::Swept.universe(), Universe::INDEX);
        assert_eq!(SpotTarget::Indices.universe(), Universe::INDEX);
        assert_eq!(SpotTarget::Equities.universe(), Universe::TOTAL_MARKET);

        assert_eq!(
            parse_spot("from=2022-01-08&to=2022-02-08", TEST_TODAY),
            Err(Refusal::FieldMissing { field: "target" })
        );
        assert_eq!(
            parse_spot("target=mcx&from=2022-01-08&to=2022-02-08", TEST_TODAY),
            Err(Refusal::UnknownTarget {
                got: "mcx".to_owned()
            }),
            "MCX is not on the engine surface and is not a target"
        );
        assert_eq!(SpotTarget::from_slug("bse"), None, "BSE is not a target");
        // A bad window still reaches the operator through the spot parser.
        assert_eq!(
            parse_spot("target=swept&from=2022-02-08&to=2022-01-08", TEST_TODAY),
            Err(Refusal::WindowBackwards {
                why: SessionError::WindowRunsBackwards {
                    from: day(2022, 2, 8),
                    to: day(2022, 1, 8),
                },
            })
        );
    }

    #[test]
    fn an_expired_contract_is_accepted_and_a_live_one_can_never_be() {
        let ok = parse_fno(
            "underlying=nifty&series=fut&expiry=2026-07-30&from=2026-07-01&to=2026-07-30",
            today(),
        )
        .expect("an expired series");
        assert_eq!(
            ok.underlying.as_str(),
            "NIFTY",
            "the typing is canonicalised"
        );
        assert_eq!(ok.series, Series::Futures);
        assert_eq!(ok.expiry, day(2026, 7, 30));
        assert_eq!(ok.window.days(), 30);

        // THE GATE. An expiry after today, and an expiry ON today, are both
        // live. `>=` rather than `>` is the whole rule: a contract expiring
        // today is still trading today.
        for live in ["2026-08-08", "2026-08-07", "9998-12-31"] {
            let body =
                format!("underlying=NIFTY&series=opt&expiry={live}&from=2026-07-01&to=2026-07-30");
            let refused = parse_fno(&body, today()).expect_err("a live contract");
            assert_eq!(
                refused,
                Refusal::LiveContract {
                    expiry: parse_day("expiry", live).expect("a real date"),
                    today: today(),
                },
                "{live} must be refused as live"
            );
            let text = refused.to_string();
            assert!(text.contains("LIVE CONTRACT IS NEVER STORED"), "{text}");
            assert!(text.contains(live) && text.contains("2026-08-07"), "{text}");
        }
        // One day before today is the first legal expiry — the bound is tight.
        assert!(
            parse_fno(
                "underlying=NIFTY&series=opt&expiry=2026-08-06&from=2026-08-06&to=2026-08-06",
                today()
            )
            .is_ok(),
            "yesterday's expiry is expired"
        );
    }

    #[test]
    fn an_fno_request_is_refused_by_name_for_every_other_reason_too() {
        let base = "underlying=NIFTY&series=fut&expiry=2026-07-30&from=2026-07-01&to=2026-07-30";

        // A window that runs past the expiry asks for bars that cannot exist.
        let past = parse_fno(
            "underlying=NIFTY&series=fut&expiry=2026-07-30&from=2026-07-01&to=2026-07-31",
            today(),
        )
        .expect_err("past the expiry");
        assert_eq!(
            past,
            Refusal::WindowOutlivesTheContract {
                to: day(2026, 7, 31),
                expiry: day(2026, 7, 30),
            }
        );
        assert!(past.to_string().contains("no bars there"), "{past}");

        // An underlying with no F&O series on it. RAJESHEXPO is a real NSE
        // share and is in neither F&O list.
        let spot_only =
            parse_fno(&base.replace("NIFTY", "RAJESHEXPO"), today()).expect_err("no derivative");
        assert_eq!(
            spot_only,
            Refusal::NotAnFnoUnderlying {
                symbol: "RAJESHEXPO".to_owned()
            }
        );
        assert!(
            spot_only.to_string().contains("no F&O series"),
            "{spot_only}"
        );

        // A symbol this engine cannot even name — a space is not a legal byte.
        assert_eq!(
            parse_fno(&base.replace("NIFTY", "NIFTY+100"), today()),
            Err(Refusal::BadUnderlying {
                got: "NIFTY 100".to_owned()
            }),
            "the form encodes a space as '+', and it is still not a symbol"
        );

        // Every missing field is named as itself.
        assert_eq!(
            parse_fno(
                "series=fut&expiry=2026-07-30&from=2026-07-01&to=2026-07-30",
                today()
            ),
            Err(Refusal::FieldMissing {
                field: "underlying"
            })
        );
        assert_eq!(
            parse_fno(
                "underlying=NIFTY&expiry=2026-07-30&from=2026-07-01&to=2026-07-30",
                today()
            ),
            Err(Refusal::FieldMissing { field: "series" })
        );
        assert_eq!(
            parse_fno(
                "underlying=NIFTY&series=fut&from=2026-07-01&to=2026-07-30",
                today()
            ),
            Err(Refusal::FieldMissing { field: "expiry" })
        );
        assert_eq!(
            parse_fno(&base.replace("series=fut", "series=swap"), today()),
            Err(Refusal::UnknownSeries {
                got: "swap".to_owned()
            })
        );
        // Both series round-trip through the wire value the form emits, and
        // each label is pinned to its text: "Future" and "Option chain" are
        // what an operator picks between, and a non-empty assertion would be
        // satisfied by either of them saying the other.
        for (series, slug, label) in [
            (Series::Futures, "fut", "Future"),
            (Series::Options, "opt", "Option chain"),
        ] {
            assert_eq!(series.slug(), slug);
            assert_eq!(series.label(), label);
            assert_eq!(Series::from_slug(slug), Some(series));
        }
        assert_eq!(
            Series::ALL.len(),
            2,
            "a future and an option chain, no more"
        );
        assert_eq!(Series::from_slug("forward"), None);

        // And a malformed expiry is a date refusal, not a live-contract one:
        // the gate must never be reached with a value nobody validated.
        assert_eq!(
            parse_fno(
                &base.replace("expiry=2026-07-30", "expiry=2026-7-30"),
                today()
            ),
            Err(Refusal::DateNotIso {
                field: "expiry",
                got: "2026-7-30".to_owned(),
            })
        );

        // A window refused AFTER the expiry gate has passed. Without this the
        // early return from `parse_window` inside `parse_fno` is a path no
        // input takes — the expiry is fine and the dates are not.
        assert_eq!(
            parse_fno(
                "underlying=NIFTY&series=fut&expiry=2020-01-30&to=2020-01-30",
                today()
            ),
            Err(Refusal::FieldMissing { field: "from" })
        );
    }

    #[test]
    fn every_refusal_names_itself_and_none_of_them_says_only_invalid_input() {
        // One arm per variant, and each one rendered. A `Display` arm no test
        // enters is a message an operator will one day read for the first time
        // in production — and `Refusal` exists precisely so that "invalid
        // input" is never the answer.
        let day = day(2026, 8, 7);
        let cases: [(Refusal, &str); 11] = [
            (
                Refusal::FieldMissing { field: "from" },
                "from was not filled in",
            ),
            (
                Refusal::DateNotIso {
                    field: "to",
                    got: "08/01/2022".to_owned(),
                },
                "YYYY-MM-DD",
            ),
            (
                Refusal::DateImpossible {
                    field: "expiry",
                    got: "2023-02-29".to_owned(),
                    why: SessionError::DayOutOfRange {
                        day: 29,
                        month_len: 28,
                    },
                },
                "is not a day",
            ),
            (
                Refusal::WindowBackwards {
                    why: SessionError::WindowRunsBackwards { from: day, to: day },
                },
                "backwards",
            ),
            (
                Refusal::WindowTooLong {
                    days: 5_000,
                    cap: MAX_WINDOW_DAYS,
                },
                "5000 days",
            ),
            (
                Refusal::UnknownTarget {
                    got: "mcx".to_owned(),
                },
                "not one of the three spot targets",
            ),
            (
                Refusal::UnknownSeries {
                    got: "swap".to_owned(),
                },
                "neither a future nor an option",
            ),
            (
                Refusal::BadUnderlying {
                    got: "NIFTY 100".to_owned(),
                },
                "not a symbol this engine can name",
            ),
            (
                Refusal::NotAnFnoUnderlying {
                    symbol: "RAJESHEXPO".to_owned(),
                },
                "no F&O series",
            ),
            (
                Refusal::LiveContract {
                    expiry: day,
                    today: day,
                },
                "LIVE CONTRACT IS NEVER STORED",
            ),
            (
                Refusal::WindowOutlivesTheContract {
                    to: day,
                    expiry: day,
                },
                "no bars there",
            ),
        ];
        for (refusal, expected) in cases {
            let text = refusal.to_string();
            assert!(
                text.starts_with("REFUSED · "),
                "every refusal is loud: {text}"
            );
            assert!(text.contains(expected), "{refusal:?} rendered as {text}");
        }
        // And the twelfth, which carries a calendar refusal of its own.
        let clock = Refusal::ClockUnusable {
            why: SessionError::BeforeEpoch { secs: -1 },
        };
        assert!(clock.to_string().contains("clock names no usable date"));
        // It is an error, so a caller may propagate it rather than reformat it.
        let as_error: &dyn std::error::Error = &clock;
        assert!(as_error.to_string().contains("REFUSED"));
    }

    #[test]
    fn the_clock_is_read_in_ist_and_a_clock_that_names_no_day_is_refused() {
        // The epoch itself is 05:30 IST on 1 January 1970.
        assert_eq!(
            ist_day(SystemTime::UNIX_EPOCH).expect("the epoch"),
            day(1970, 1, 1)
        );
        // 18:30 UTC is already the next day in IST, which is the whole reason
        // the conversion is not "seconds / 86,400".
        let evening = SystemTime::UNIX_EPOCH + Duration::from_mins(18 * 60 + 30);
        assert_eq!(ist_day(evening).expect("evening"), day(1970, 1, 2));
        assert_eq!(epoch_secs(SystemTime::UNIX_EPOCH), 0);
        assert_eq!(epoch_secs(evening), 66_600);

        // A clock before the epoch is REFUSED, never defaulted. IST is +05:30,
        // so it has to be more than 19,800 seconds behind to name no day.
        let broken = SystemTime::UNIX_EPOCH - Duration::from_hours(24);
        assert_eq!(epoch_secs(broken), -86_400);
        let refused = ist_day(broken).expect_err("no such day");
        assert_eq!(
            refused,
            Refusal::ClockUnusable {
                why: SessionError::BeforeEpoch { secs: -86_400 },
            }
        );
        assert!(refused.to_string().contains("clock"), "{refused}");

        // The real clock is on this machine and this build can name its day.
        assert!(
            today_ist().is_ok(),
            "the host clock names a day this build renders"
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]
mod vendor_parse_tests {
    use super::*;

    /// **A NAMED VENDOR IS NEVER SILENTLY REPLACED BY ANOTHER.**
    ///
    /// Two copies of this parse existed. `/bars` compared with `==`,
    /// `parse_vendor` with `eq_ignore_ascii_case`, and both ended
    /// `.unwrap_or(Vendor::Dhan)` — so `?vendor=Groww` resolved differently on
    /// the two routes and *both* answers were Dhan. Asking for one broker's
    /// copy of a month and being served another's, under HTTP 200, is a wrong
    /// answer wearing the shape of a right one.
    ///
    /// The old behaviour was argued for in this file: "the vendor is a route,
    /// not a claim about the data: naming one that does not exist cannot
    /// corrupt anything". True when written. False once `Plan.vendor` became
    /// the store prefix — coercing to Dhan files one broker's prices under
    /// another's path.
    #[test]
    fn a_vendor_this_build_cannot_serve_is_refused_rather_than_replaced() {
        // Absent still defaults, and that half was always sound: Dhan is the
        // descriptor verified against a live body.
        assert_eq!(parse_vendor(""), Some(brutex_core::vendor::Vendor::Dhan));

        // Every real vendor round-trips through its own wire spelling, in any
        // case, because an operator types a query string by hand.
        for vendor in brutex_core::vendor::Vendor::ALL {
            for spelling in [
                vendor.as_str().to_owned(),
                vendor.as_str().to_uppercase(),
                {
                    let mut s = vendor.as_str().to_owned();
                    s[..1].make_ascii_uppercase();
                    s
                },
            ] {
                assert_eq!(
                    parse_vendor(&spelling),
                    Some(vendor),
                    "{spelling:?} must resolve to {vendor:?}, not fall back"
                );
            }
        }

        // AND THE HALF THAT WAS WRONG: a name this build has no feed for is
        // `None`, so a caller must refuse rather than pick something.
        for wrong in ["zerodha", "upstox", "DHANN", "gro ww", "0", "'"] {
            assert_eq!(
                parse_vendor(wrong),
                None,
                "{wrong:?} named a vendor this build cannot serve; returning \
                 Some(_) here is how a month gets served from the wrong prefix"
            );
        }
    }

    /// The refusal names what it refused, so an operator can see the typo.
    #[test]
    fn the_refusal_carries_the_word_that_was_rejected() {
        let refusal = Refusal::UnknownVendor {
            got: "zerodha".to_owned(),
        };
        let said = refusal.to_string();
        assert!(said.contains("zerodha"), "{said}");
        assert!(said.starts_with("REFUSED"), "{said}");
    }
}
