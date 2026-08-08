//! Every feed this engine can ingest, written down as **rows in a table**
//! rather than as branches in a function.
//!
//! # The failure this module exists to prevent
//!
//! The first draft of the fetch hardcoded one broker and one bar length. The
//! auth header name, the date format, the non-inclusive range end and the
//! seven parallel response arrays were all `if` statements, and the `/pull`
//! page offered exactly one timeframe. Adding a second feed meant editing the
//! fetch *and* editing the page, and the two edits could disagree — which is
//! the same defect `CLAUDE.md` §6 describes about a depth parameter, one layer
//! up: a branch that can be added can be added in one place and forgotten in
//! the other.
//!
//! So everything that differs between one feed and another is a **field**:
//! the transport, the auth header *name*, the date format, whether the range
//! end is inclusive, the response shape, the field names, the timestamp
//! encoding, the rate budget, the granularities served, the segments served,
//! and — for a local archive — the archive naming pattern, the member naming
//! pattern, whether a header row exists and the column order.
//!
//! # Why the lookup is O(1) *by construction*
//!
//! [`Feed`] is `#[repr(u8)]` with consecutive discriminants and [`DESCRIPTORS`]
//! is a `const` array indexed by that discriminant. A lookup is one array
//! index: no search, no map, no hash, and **the bound does not change when the
//! table grows from four rows to four hundred**, because the index is the
//! variant tag and not a key that has to be found.
//!
//! Three compile-time assertions make a half-added feed a build failure rather
//! than a runtime surprise: the table length equals the variant count, the
//! variant list length equals the variant count, and **row *i* describes
//! variant *i*** — that last one is what makes indexing by discriminant sound
//! rather than merely plausible. A new variant with no row does not compile.
//! `pull::vendor::the_descriptor_table_is_indexed_by_the_discriminant` is the
//! test that drives it from outside.
//!
//! # What is deliberately **not** here
//!
//! **No credential, and no rate budget, on the archive path.** [`Auth`] and
//! [`Budget`] are fields of [`HttpSpec`], not of [`Descriptor`]. An archive
//! descriptor therefore has *nowhere to put a token and nowhere to put a
//! ceiling* — a local file has no rate limit and needs no credential, and
//! wiring a governor in anyway would be a lie about what is being protected.
//! Structurally impossible beats conventionally omitted.
//!
//! **No live network call and no zip reader.** Those are adapters, and this
//! crate's own precedent — `src/secret.rs` defining a port while the AWS SDK
//! stays out of `Cargo.toml` until something makes a live call — is the shape
//! followed here. See [`crate::fetch`].
//!
//! # The session window is a **dated regime**, not a constant
//!
//! `crate::session` writes 09:15–15:30 and 375 bars as three `const`s pinned
//! to each other. That is right for every day the lake holds and wrong for
//! part of 2026: NSE extended equity-derivatives trading by ten minutes with
//! effect from **2026-08-03**, and changed the cash market's close into a
//! Closing Auction Session on the same date. A backtest spanning 2024 to 2026
//! crosses that boundary, and a single constant would mis-filter every bar on
//! the far side of it *invisibly* — the bars would simply not be there.
//!
//! So the window is a [`SessionTable`]: one anchor row plus at most
//! [`MAX_LATER_SESSION_ROWS`] dated rows, each carrying its own citation, and
//! a row that has no verified hours **refuses by name**. That is exactly the
//! shape `crates/costs/src/regime.rs` proved out for the tax rate history, and
//! it is reused rather than paraphrased. The anchor rows are the `session`
//! constants, asserted equal to them at compile time, so nothing regresses and
//! that module's exhaustive 2,932,897-day walk keeps passing untouched.
//!
//! **The refusal is the important half.** No row here invents an hour. The
//! 2026-08-03 rows carry [`Hours::Unverified`] because the NSE circular itself
//! was never retrieved — `CLAUDE.md` §3 rule 1 wants an exchange claim
//! traceable to `docs/00-charter.md`, and that document has no row for this
//! change. What five brokers reported survives as **prose in the row's source
//! string**, where it can be read and never used as a filter bound; that is
//! `crates/costs`' own device. The rows flip to [`Hours::Verified`] in the
//! one-line diff that lands the charter entry, and not before.
//!
//! # Granularity is data too, down to the tick
//!
//! [`Granularity`] spans an event grid, seconds, minutes, an hour, a day and a
//! week. Bars-per-session is **not** a constant on that ladder: 375 is true
//! only for one-minute bars in the pre-2026-08-03 window. It is a function of
//! (venue hours, granularity) returning [`ExpectedCount`], which has an arm
//! for *no count exists* — a snapshot or tick feed has no arithmetic that
//! predicts its record count, and pretending otherwise would make gap
//! detection lie.

use std::fmt;

use brutex_core::instrument::{Exchange, Segment};
use brutex_core::vendor::Vendor;
use store::path::Timeframe;

use crate::session::{
    BARS_PER_REGULAR_SESSION, Day, SECS_PER_MINUTE, SESSION_CLOSE_MINUTE, SESSION_OPEN_MINUTE,
};

// ---------------------------------------------------------------------------
// const helpers
// ---------------------------------------------------------------------------

/// Whether two strings are byte-for-byte equal, in a `const` context.
///
/// `str::eq` is not a `const fn` on the pinned toolchain, and the assertions
/// that tie this module's directory names to `store::path::Timeframe` have to
/// run at compile time or they are comments. Written with slice patterns
/// rather than indexing so it needs no lint exception.
const fn str_eq(left: &str, right: &str) -> bool {
    let mut a = left.as_bytes();
    let mut b = right.as_bytes();
    loop {
        match (a, b) {
            ([], []) => return true,
            ([first_a, rest_a @ ..], [first_b, rest_b @ ..]) => {
                if *first_a != *first_b {
                    return false;
                }
                a = rest_a;
                b = rest_b;
            }
            _ => return false,
        }
    }
}

/// Whether a citation string says anything at all.
///
/// A blank `source` on a regime row is a row with no citation wearing one, and
/// `CLAUDE.md` §3 rule 1 is the whole reason the field exists.
const fn has_content(text: &str) -> bool {
    let mut remaining = text.as_bytes();
    while let [first, rest @ ..] = remaining {
        if !first.is_ascii_whitespace() {
            return true;
        }
        remaining = rest;
    }
    false
}

/// A calendar date in a `const` context.
///
/// `Day::new` returns a `Result` and `?` is not available in a `const` item,
/// so an unreal literal falls back to 1970-01-01 rather than panicking —
/// `panic!` is banned outright in shipping source and no allowlist exists for
/// it. **The fallback is never silent:** every date built here is followed by
/// a `const` assertion that it round-trips to the literal it was written from,
/// so a typo collapses to the epoch and then fails to compile. The recursive
/// arm terminates because `crate::session` already asserts at compile time
/// that `Day::new(1970, 1, 1)` is `Ok`.
const fn day_const(year: u16, month: u8, day: u8) -> Day {
    match Day::new(year, month, day) {
        Ok(day) => day,
        Err(_) => day_const(1970, 1, 1),
    }
}

// ---------------------------------------------------------------------------
// granularity
// ---------------------------------------------------------------------------

/// Seconds in a minute, as the width the session arithmetic is done in.
///
/// `crate::session::SECS_PER_MINUTE` is the same number as an `i64`, and the
/// assertion below is what keeps the two from drifting. Written as a `u32`
/// constant rather than cast from the `i64` because a narrowing cast is denied
/// workspace-wide, and rightly so.
const SECS_PER_MINUTE_U32: u32 = 60;
const _: () = assert!(SECS_PER_MINUTE == SECS_PER_MINUTE_U32 as i64);

/// How many rungs the granularity ladder has.
pub const GRANULARITY_COUNT: usize = 11;

/// Which grid a feed's records sit on.
///
/// Extended rather than re-invented from one rung: `store::path::Timeframe`
/// ships exactly `1min` today, and two spellings of "which granularity" is the
/// drift `CLAUDE.md` forbids. [`Granularity::store_timeframe`] is the single
/// reconciliation site between the two, and a `const` assertion below pins the
/// one rung they share — the directory name *and* the seconds — so they cannot
/// disagree silently. Widening `Timeframe` to the full ladder is a
/// `crates/store` change and is recorded as outstanding.
///
/// The rungs are closed rather than parameterised (`Second(n)`) on purpose: an
/// arbitrary `n` has no stable on-disk directory name, and paths are
/// append-only history. Adding a rung is a one-row diff.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Granularity {
    /// Every print, on no fixed interval. Record count is unbounded.
    Tick = 0,
    /// One-second grid.
    Second1 = 1,
    /// Five-second grid.
    Second5 = 2,
    /// One-minute bars — the only rung `store::path::Timeframe` ships.
    Minute1 = 3,
    /// Three-minute bars.
    Minute3 = 4,
    /// Five-minute bars.
    Minute5 = 5,
    /// Fifteen-minute bars.
    Minute15 = 6,
    /// Thirty-minute bars.
    Minute30 = 7,
    /// One-hour bars.
    Hour1 = 8,
    /// One bar per trading day.
    Day1 = 9,
    /// One bar per trading week.
    Week1 = 10,
}

/// The grid a rung sits on, which is what decides whether a record count
/// exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grid {
    /// A record per event. No interval, and therefore no predictable count.
    Event,
    /// A fixed interval inside the session, in seconds.
    Intraday(u32),
    /// One record covering a whole session.
    Daily,
    /// One record covering a whole week.
    Weekly,
}

impl Granularity {
    /// Every rung, coarsest last.
    pub const ALL: [Self; GRANULARITY_COUNT] = [
        Self::Tick,
        Self::Second1,
        Self::Second5,
        Self::Minute1,
        Self::Minute3,
        Self::Minute5,
        Self::Minute15,
        Self::Minute30,
        Self::Hour1,
        Self::Day1,
        Self::Week1,
    ];

    /// The stable on-disk directory name.
    ///
    /// Stable is the operative word: `CLAUDE.md` §3 rule 8 makes history
    /// append-only, and a renamed directory orphans every file already under
    /// the old one.
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::Second1 => "1s",
            Self::Second5 => "5s",
            Self::Minute1 => "1min",
            Self::Minute3 => "3min",
            Self::Minute5 => "5min",
            Self::Minute15 => "15min",
            Self::Minute30 => "30min",
            Self::Hour1 => "1hr",
            Self::Day1 => "1day",
            Self::Week1 => "1week",
        }
    }

    /// The grid this rung sits on.
    #[must_use]
    pub const fn grid(self) -> Grid {
        match self {
            Self::Tick => Grid::Event,
            Self::Second1 => Grid::Intraday(1),
            Self::Second5 => Grid::Intraday(5),
            Self::Minute1 => Grid::Intraday(60),
            Self::Minute3 => Grid::Intraday(180),
            Self::Minute5 => Grid::Intraday(300),
            Self::Minute15 => Grid::Intraday(900),
            Self::Minute30 => Grid::Intraday(1_800),
            Self::Hour1 => Grid::Intraday(3_600),
            Self::Day1 => Grid::Daily,
            Self::Week1 => Grid::Weekly,
        }
    }

    /// Whether a bar at this rung carries an intraday time at all.
    ///
    /// A daily or weekly bar does not: vendors stamp it at midnight, at the
    /// open or at the close, and applying an intraday window to it would drop
    /// every one. This is the same exemption `crate::session::Cadence` names,
    /// derived from the ladder instead of asserted beside it.
    #[must_use]
    pub const fn is_intraday(self) -> bool {
        matches!(self.grid(), Grid::Event | Grid::Intraday(_))
    }

    /// This rung's bit in a [`GranularitySet`].
    const fn bit(self) -> u16 {
        1u16 << (self as u16)
    }

    /// The `store::path::Timeframe` this rung writes under, when one exists.
    ///
    /// `None` for every rung `crates/store` has not yet been widened to carry.
    /// A `None` is a **refusal at the write boundary**, never a substitution:
    /// filing a five-minute bar under `1min/` would corrupt a series that a
    /// reader has no way to tell apart from real one-minute data.
    #[must_use]
    pub const fn store_timeframe(self) -> Option<Timeframe> {
        match self {
            Self::Minute1 => Some(Timeframe::MINUTE_1),
            _ => None,
        }
    }
}

// The one rung the two spellings share, tied together by the compiler. If
// `store::path::Timeframe::MINUTE_1` is ever renamed or re-timed, this stops
// compiling instead of quietly writing bars into a directory nobody reads.
const _: () = assert!(str_eq(
    Granularity::Minute1.dir(),
    Timeframe::MINUTE_1.as_str()
));
const _: () = assert!(match Granularity::Minute1.grid() {
    Grid::Intraday(secs) => secs == Timeframe::MINUTE_1.secs(),
    _ => false,
});
const _: () = assert!(Granularity::ALL.len() == GRANULARITY_COUNT);

impl fmt::Display for Granularity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dir())
    }
}

/// A set of granularities, as a bitset.
///
/// A bitset rather than a slice so a descriptor row is `Copy` and a membership
/// test is one mask — the same argument `brutex_core::vendor::VendorSet`
/// makes, for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GranularitySet(u16);

impl GranularitySet {
    /// No granularity.
    pub const EMPTY: Self = Self(0);

    /// This set with `granularity` added. Adding twice is adding once.
    #[must_use]
    pub const fn with(self, granularity: Granularity) -> Self {
        Self(self.0 | granularity.bit())
    }

    /// Whether the set holds `granularity`.
    #[must_use]
    pub const fn contains(self, granularity: Granularity) -> bool {
        self.0 & granularity.bit() != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A set of exchange segments, as a bitset. Same shape, same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SegmentSet(u8);

impl SegmentSet {
    /// No segment.
    pub const EMPTY: Self = Self(0);

    /// This set with `segment` added.
    #[must_use]
    pub const fn with(self, segment: Segment) -> Self {
        Self(self.0 | segment_bit(segment))
    }

    /// Whether the set holds `segment`.
    #[must_use]
    pub const fn contains(self, segment: Segment) -> bool {
        self.0 & segment_bit(segment) != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// One segment's bit. A free function because `Segment` lives in `crates/core`.
const fn segment_bit(segment: Segment) -> u8 {
    match segment {
        Segment::Index => 1 << 0,
        Segment::Cash => 1 << 1,
        Segment::Fno => 1 << 2,
    }
}

// ---------------------------------------------------------------------------
// venue, and the dated session regime
// ---------------------------------------------------------------------------

/// How many rows a session table may carry **after** its anchor.
///
/// Two. This is the number the O(1) claim rests on: it is the length of a
/// fixed-size array, so it bounds the lookup's loop at compile time and **no
/// input can raise it**. A scan of a fixed-size `const` array is constant
/// because the length is not an argument — there is no `N` here that grows,
/// and no bisection is needed to say so.
/// `pull::vendor::the_session_lookup_walks_a_fixed_number_of_rows` is the test.
pub const MAX_LATER_SESSION_ROWS: usize = 2;

/// How many venues have a session table.
pub const VENUE_COUNT: usize = 3;

/// A trading venue, in the sense of "which clock governs these prints".
///
/// Three rows, not one, because the 2026-08-03 change did **not** move the
/// three together: derivatives were extended by ten minutes, the cash market's
/// close became an auction, and nothing retrievable says what happened to
/// index dissemination. Folding them into one venue would apply one venue's
/// evidence to another's bars.
///
/// This widens nothing about what is **swept**: `CLAUDE.md` §1 keeps the
/// engine surface at `NSE-NIFTY` and `NSE-BANKNIFTY`. A venue row says which
/// clock a *stored* series is filtered against, and futures, options and
/// single stocks were already storable.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Venue {
    /// NSE spot index dissemination.
    NseIndex = 0,
    /// NSE cash equities.
    NseCash = 1,
    /// NSE equity derivatives — index and stock futures and options.
    NseDerivatives = 2,
}

impl Venue {
    /// Every venue, in table order.
    pub const ALL: [Self; VENUE_COUNT] = [Self::NseIndex, Self::NseCash, Self::NseDerivatives];

    /// A short, stable name for a report.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NseIndex => "NSE index",
            Self::NseCash => "NSE cash",
            Self::NseDerivatives => "NSE equity derivatives",
        }
    }

    /// Which venue's clock governs an `(exchange, segment)` pair.
    ///
    /// `None` for BSE, which `CLAUDE.md` §1 does not pull. Refused by absence
    /// rather than mapped onto an NSE row, because two exchanges sharing one
    /// window is a claim nothing here has a source for.
    #[must_use]
    pub const fn for_segment(exchange: Exchange, segment: Segment) -> Option<Self> {
        match (exchange, segment) {
            (Exchange::Nse, Segment::Index) => Some(Self::NseIndex),
            (Exchange::Nse, Segment::Cash) => Some(Self::NseCash),
            (Exchange::Nse, Segment::Fno) => Some(Self::NseDerivatives),
            (Exchange::Bse, _) => None,
        }
    }

    /// The session in force on `day`.
    ///
    /// At most [`MAX_LATER_SESSION_ROWS`] comparisons, always — see that
    /// constant for why that is a compile-time bound and not a small `N`.
    ///
    /// # Errors
    ///
    /// [`SessionRefusal`] when the row in force carries no verified hours. The
    /// refusal names the venue, the window, the citation gap and the remedy.
    /// There is no argument, flag or default that turns it into an hour.
    pub fn hours_on(self, day: Day) -> Result<Session, SessionRefusal> {
        self.table().hours_on(day)
    }

    /// This venue's table. A `match`, so a new venue with no table does not
    /// compile.
    const fn table(self) -> &'static SessionTable {
        match self {
            Self::NseIndex => &NSE_INDEX_SESSIONS,
            Self::NseCash => &NSE_CASH_SESSIONS,
            Self::NseDerivatives => &NSE_DERIVATIVES_SESSIONS,
        }
    }

    /// The dated windows for which this venue has no verified hours.
    ///
    /// Derived from the table rather than written down a second time, so no
    /// page footer can carry a boundary date that has drifted from the table
    /// it claims to describe.
    #[must_use]
    pub fn refusal_windows(self) -> [Option<RefusalWindow>; MAX_LATER_SESSION_ROWS + 1] {
        self.table().refusal_windows()
    }
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What kind of trading a session row describes.
///
/// [`Self::ClosingAuction`] exists because the 2026-08-03 cash change
/// introduced one, and a session type absent from the model cannot be refused
/// by name — it is simply mis-filtered as ordinary continuous trading. **No
/// shipped row carries it yet**: an auction row needs its own verified start
/// and end, and the same circular that would supply them is the one that was
/// never retrieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionKind {
    /// Continuous trading. Prints are ordinary bars.
    Continuous,
    /// A closing auction. Its prints are **not** ordinary bars and must not be
    /// swept as if they were.
    ClosingAuction,
}

impl SessionKind {
    /// A short, stable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::ClosingAuction => "closing auction",
        }
    }
}

impl fmt::Display for SessionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What a session row knows about its hours.
///
/// The two arms are deliberately asymmetric, and that asymmetry is the whole
/// mechanism: [`Self::Unverified`] **has no minute fields**. There is nowhere
/// in the type for an invented open or close to live, nothing to unwrap and no
/// default to fall back to. `crates/costs/src/regime.rs` `Rate::Unverified` is
/// the same device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hours {
    /// Citation-grounded hours, as minutes since IST midnight. The open is
    /// inclusive and the close is **exclusive** — the same convention
    /// `crate::session` states, for the same reason.
    Verified {
        /// First minute of the session, inclusive.
        open_minute: u32,
        /// End of the session, exclusive.
        close_minute: u32,
    },
    /// No hours were ever verified for this window, and none will be invented.
    Unverified,
}

/// One dated session row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SessionRow {
    start: Day,
    hours: Hours,
    kind: SessionKind,
    source: &'static str,
}

impl SessionRow {
    const fn verified(
        start: Day,
        open_minute: u32,
        close_minute: u32,
        kind: SessionKind,
        source: &'static str,
    ) -> Self {
        Self {
            start,
            hours: Hours::Verified {
                open_minute,
                close_minute,
            },
            kind,
            source,
        }
    }

    /// A row for an era whose circular was never retrieved.
    ///
    /// Has no *shipping* caller today: on 2026-08-07 the three outstanding rows
    /// were filled from NSE/CMTR/74466 and NSE/FAOP/74467, so every shipped row
    /// now carries a citation. It is kept because the NEXT gap is a matter of
    /// when, not whether — a circular that cannot be retrieved must remain
    /// expressible, or the pressure at that moment is to guess a number
    /// instead. Deleting this constructor would remove the honest option and
    /// leave only the dishonest one.
    ///
    /// The tests below DO call it, against a table built for the purpose, so
    /// the refusal path is proven rather than merely available — which is why
    /// the `dead_code` expectation is scoped to builds without `cfg(test)`.
    /// An unconditional `expect` would be unfulfilled the moment a test used
    /// it, and `-D warnings` turns that into a red build.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the refusal constructor is part of the contract, not a \
                      leftover: every era whose source is unretrievable must \
                      stay representable. See the doc comment above."
        )
    )]
    const fn unverified(start: Day, kind: SessionKind, source: &'static str) -> Self {
        Self {
            start,
            hours: Hours::Unverified,
            kind,
            source,
        }
    }

    /// Whether the row is structurally sound: an ordered, in-range window if
    /// it has one, and a citation that says something.
    const fn is_well_shaped(&self) -> bool {
        let hours_ok = match self.hours {
            Hours::Verified {
                open_minute,
                close_minute,
            } => open_minute < close_minute && close_minute <= 24 * 60,
            Hours::Unverified => true,
        };
        hours_ok && has_content(self.source)
    }
}

/// The session in force on one day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Session {
    open_minute: u32,
    close_minute: u32,
    kind: SessionKind,
    source: &'static str,
}

impl Session {
    /// First minute of the session, inclusive, since IST midnight.
    #[must_use]
    pub const fn open_minute(self) -> u32 {
        self.open_minute
    }

    /// End of the session, **exclusive**, since IST midnight.
    #[must_use]
    pub const fn close_minute(self) -> u32 {
        self.close_minute
    }

    /// What kind of trading this is.
    #[must_use]
    pub const fn kind(self) -> SessionKind {
        self.kind
    }

    /// The citation this row was written from.
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }

    /// How many seconds the session runs for.
    #[must_use]
    pub const fn len_secs(self) -> u32 {
        (self.close_minute - self.open_minute) * SECS_PER_MINUTE_U32
    }

    /// Whether a minute of the day falls inside the session.
    #[must_use]
    pub const fn contains_minute(self, minute_of_day: u32) -> bool {
        minute_of_day >= self.open_minute && minute_of_day < self.close_minute
    }

    /// How many records of `granularity` one full session of these hours
    /// holds.
    ///
    /// See [`ExpectedCount`] for why one of the four answers is *there is no
    /// count*.
    #[must_use]
    pub const fn expected_count(self, granularity: Granularity) -> ExpectedCount {
        let session_secs = self.len_secs();
        match granularity.grid() {
            Grid::Event => ExpectedCount::Unbounded,
            Grid::Daily | Grid::Weekly => ExpectedCount::Aggregate,
            Grid::Intraday(interval_secs) => {
                if interval_secs == 0 || !session_secs.is_multiple_of(interval_secs) {
                    ExpectedCount::Irregular {
                        session_secs,
                        interval_secs,
                    }
                } else {
                    ExpectedCount::Exact(session_secs / interval_secs)
                }
            }
        }
    }
}

/// How many records one session holds — or the honest admission that the
/// question has no answer.
///
/// [`Self::Unbounded`] is the arm that matters. A tick or one-second snapshot
/// feed prints as often as the market prints, and there is no arithmetic that
/// predicts the count. Returning a number anyway would make every gap check
/// downstream report a complete day as short, or a short day as complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpectedCount {
    /// Exactly this many records in one full session.
    Exact(u32),
    /// One record covers the whole session or more.
    Aggregate,
    /// No count exists: the record rate is not a function of the clock.
    Unbounded,
    /// The session is not a whole multiple of the interval, so no exact count
    /// exists either. Named rather than rounded — a rounded count is a gap
    /// check that is wrong by a fixed amount every single day.
    Irregular {
        /// The session length, in seconds.
        session_secs: u32,
        /// The interval that does not divide it, in seconds.
        interval_secs: u32,
    },
}

/// A dated table of one venue's sessions.
///
/// Private, and deliberately: if a caller could build one, a caller could
/// build one that filters 2026 against 2024's hours, and the refusal contract
/// would be a comment again.
struct SessionTable {
    venue: Venue,
    /// The row in force from 1970-01-01. A field rather than an array element,
    /// which is what makes "empty table" and "before the table"
    /// unrepresentable.
    anchor: SessionRow,
    /// Later rows in strictly ascending order, `None`-padded at the end.
    later: [Option<SessionRow>; MAX_LATER_SESSION_ROWS],
}

impl SessionTable {
    fn hours_on(&self, day: Day) -> Result<Session, SessionRefusal> {
        let mut selected = &self.anchor;
        let mut verified_from = None;
        for row in self.later.iter().flatten() {
            if row.start.days_from_epoch() <= day.days_from_epoch() {
                selected = row;
                verified_from = None;
            } else if verified_from.is_none() {
                verified_from = Some(row.start);
            }
        }
        match selected.hours {
            Hours::Verified {
                open_minute,
                close_minute,
            } => Ok(Session {
                open_minute,
                close_minute,
                kind: selected.kind,
                source: selected.source,
            }),
            Hours::Unverified => Err(SessionRefusal {
                venue: self.venue,
                day,
                row_start: selected.start,
                verified_from,
                source: selected.source,
            }),
        }
    }

    fn rows(&self) -> impl Iterator<Item = &SessionRow> {
        std::iter::once(&self.anchor).chain(self.later.iter().flatten())
    }

    fn refusal_windows(&self) -> [Option<RefusalWindow>; MAX_LATER_SESSION_ROWS + 1] {
        let mut windows = [None; MAX_LATER_SESSION_ROWS + 1];
        let successors = self
            .rows()
            .skip(1)
            .map(|row| Some(row.start))
            .chain(std::iter::once(None));
        for (slot, (row, verified_from)) in windows.iter_mut().zip(self.rows().zip(successors)) {
            if row.hours == Hours::Unverified {
                *slot = Some(RefusalWindow {
                    venue: self.venue,
                    start: row.start,
                    verified_from,
                });
            }
        }
        windows
    }

    /// Whether the anchor covers every representable day.
    const fn anchor_covers_all_days(&self) -> bool {
        self.anchor.start.days_from_epoch() == 0
    }

    /// Whether the rows ascend strictly with no hole before a populated slot.
    const fn rows_ascend(&self) -> bool {
        match self.later {
            [None, None] => true,
            [Some(first), None] => {
                self.anchor.start.days_from_epoch() < first.start.days_from_epoch()
            }
            [Some(first), Some(second)] => {
                self.anchor.start.days_from_epoch() < first.start.days_from_epoch()
                    && first.start.days_from_epoch() < second.start.days_from_epoch()
            }
            // A populated slot after an empty one: `rows()` would silently
            // close the hole.
            [None, Some(_)] => false,
        }
    }

    const fn rows_are_well_shaped(&self) -> bool {
        let anchor = self.anchor.is_well_shaped();
        match self.later {
            [None, None] => anchor,
            [Some(first), None] => anchor && first.is_well_shaped(),
            [Some(first), Some(second)] => {
                anchor && first.is_well_shaped() && second.is_well_shaped()
            }
            [None, Some(second)] => anchor && second.is_well_shaped(),
        }
    }

    /// The whole structural contract, checked by the compiler.
    const fn is_shipping_shape(&self) -> bool {
        self.anchor_covers_all_days() && self.rows_ascend() && self.rows_are_well_shaped()
    }

    /// Whether the anchor is exactly `crate::session`'s two constants.
    ///
    /// Asserted at compile time for all three tables. This is the "nothing
    /// regresses" guarantee, made by the compiler rather than by a promise:
    /// `crate::session`'s exhaustive day walk and its 09:15–15:30 filter stay
    /// correct for every day before the first later row, because the anchor
    /// *is* those constants.
    const fn anchor_matches_session_constants(&self) -> bool {
        match self.anchor.hours {
            Hours::Verified {
                open_minute,
                close_minute,
            } => open_minute == SESSION_OPEN_MINUTE && close_minute == SESSION_CLOSE_MINUTE,
            Hours::Unverified => false,
        }
    }
}

/// A span of days for which a venue has no verified hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RefusalWindow {
    venue: Venue,
    start: Day,
    verified_from: Option<Day>,
}

impl RefusalWindow {
    /// Which venue.
    #[must_use]
    pub const fn venue(self) -> Venue {
        self.venue
    }

    /// The first day of the window, inclusive.
    #[must_use]
    pub const fn start(self) -> Day {
        self.start
    }

    /// The first day verified hours exist again — the window's exclusive end.
    /// `None` when the window is open-ended.
    #[must_use]
    pub const fn verified_from(self) -> Option<Day> {
        self.verified_from
    }
}

/// A day whose session hours were never verified.
///
/// Carries the citation gap and the remedy, because "unknown session" sends an
/// operator to guess which of three venues and which of three rows was meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionRefusal {
    venue: Venue,
    day: Day,
    row_start: Day,
    verified_from: Option<Day>,
    source: &'static str,
}

impl SessionRefusal {
    /// Which venue.
    #[must_use]
    pub const fn venue(self) -> Venue {
        self.venue
    }

    /// The day that was asked about.
    #[must_use]
    pub const fn day(self) -> Day {
        self.day
    }

    /// The first day of the unverified window.
    #[must_use]
    pub const fn row_start(self) -> Day {
        self.row_start
    }

    /// The first day verified hours resume, when the window has an end.
    #[must_use]
    pub const fn verified_from(self) -> Option<Day> {
        self.verified_from
    }

    /// What was identified, and what was never retrieved.
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }
}

impl fmt::Display for SessionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} has no verified session hours on {}: the window from {}",
            self.venue, self.day, self.row_start
        )?;
        match self.verified_from {
            Some(end) => write!(f, " until {end}")?,
            None => f.write_str(" onward")?,
        }
        write!(f, " carries no citation — {}", self.source)
    }
}

// ---------------------------------------------------------------------------
// the shipped session rows
// ---------------------------------------------------------------------------

/// The day the anchor row starts: the first representable day.
const EPOCH_DAY: Day = day_const(1970, 1, 1);
const _: () = assert!(EPOCH_DAY.days_from_epoch() == 0);

/// 2026-08-03 — the day NSE's session change took effect.
const AUG_3_2026: Day = day_const(2026, 8, 3);
const _: () =
    assert!(AUG_3_2026.year() == 2026 && AUG_3_2026.month() == 8 && AUG_3_2026.day() == 3);

/// The citation every anchor row carries.
const CHARTER_SESSION: &str = "docs/00-charter.md §3 — regular session 09:15 inclusive to 15:30 \
     exclusive IST, last one-minute bar opens 15:29, 375 bars, confirmed in the lake";

/// What is known, and what is not, about the 2026-08-03 change.
///
/// Written once and shared by all three rows because it is one event and one
/// citation gap; a second copy is a second thing to update.
const CAS_INDEX_CIRCULAR: &str = "NSE/CMTR/74466 §E, retrieved 2026-08-07: \"Unexecuted limit \
     orders of the CTS (Continuous Trading Session till 03:15PM)\". The swept \
     index closes EARLIEST of the three segments, which is counter-intuitive \
     and indirect: CAS eligibility is \"stocks on which derivative contracts \
     are available in any of the Exchanges\" (§A), every NIFTY 50 and NIFTY \
     BANK constituent qualifies, so at 15:15 every share the index is \
     computed from stops trading continuously and the index freezes with \
     them. NSE FAQ v1.0 (May 2026) Q7 confirms the actual index then uses the \
     LTP of CTS. UNVERIFIED and deliberately not encoded: whether a vendor \
     puts the frozen actual index or the indicative auction index in a \
     15:15-15:35 bar. Both are published; only measurement against \
     post-2026-08-03 data can say which.";

const CAS_CASH_CIRCULAR: &str = "NSE/CMTR/74466 §A, retrieved 2026-08-07. A share with NO derivative \
     contract is not CAS eligible and keeps its 15:30 continuous close, so \
     this row restates the anchor rather than moving it. Written as a row \
     rather than omitted because \"unchanged\" is a finding that needs a \
     citation exactly as much as a change does.";

const FNO_EXTENSION_CIRCULAR: &str = "NSE/FAOP/74467, retrieved 2026-08-07: equity derivatives extended ten \
     minutes to 15:40, so positions can be adjusted against the closing \
     prices discovered in the cash auction. Applies to index futures, index \
     options, stock futures and stock options, including expiry days.";

#[expect(
    dead_code,
    reason = "the gap this replaced is kept as the record of \
     what was unknown before the circulars were retrieved on 2026-08-07"
)]
const AUG_2026_GAP: &str = "an extension of NSE equity-derivatives trading by ten minutes with \
     effect from 2026-08-03, and a change of the cash market's close into a Closing Auction \
     Session, were reported by five brokers (Angel One, Groww, JM Financial, Anand Rathi, \
     Flattrade) read on 2026-08-07; the reported derivatives close is 15:40 and the reported \
     auction runs 15:15-15:35. OUTSTANDING CITATION: the NSE circular itself was never \
     retrieved, docs/00-charter.md has no row for the change, and no source states the new \
     continuous close for the cash market or whether index dissemination moved with it. \
     REMEDY: retrieve the circular, record it in docs/00-charter.md, and turn the row here \
     from Unverified into Verified — one line, and not before";

const NSE_INDEX_SESSIONS: SessionTable = SessionTable {
    venue: Venue::NseIndex,
    anchor: SessionRow::verified(
        EPOCH_DAY,
        SESSION_OPEN_MINUTE,
        SESSION_CLOSE_MINUTE,
        SessionKind::Continuous,
        CHARTER_SESSION,
    ),
    later: [
        // The index is computed from the cash market, so it is NOT safe to
        // assume the index window survived a change to the cash close. This
        // row refuses rather than guessing in either direction.
        Some(SessionRow::verified(
            AUG_3_2026,
            SESSION_OPEN_MINUTE,
            15 * 60 + 15,
            SessionKind::Continuous,
            CAS_INDEX_CIRCULAR,
        )),
        None,
    ],
};

const NSE_CASH_SESSIONS: SessionTable = SessionTable {
    venue: Venue::NseCash,
    anchor: SessionRow::verified(
        EPOCH_DAY,
        SESSION_OPEN_MINUTE,
        SESSION_CLOSE_MINUTE,
        SessionKind::Continuous,
        CHARTER_SESSION,
    ),
    later: [
        Some(SessionRow::verified(
            AUG_3_2026,
            SESSION_OPEN_MINUTE,
            SESSION_CLOSE_MINUTE,
            SessionKind::Continuous,
            CAS_CASH_CIRCULAR,
        )),
        None,
    ],
};

const NSE_DERIVATIVES_SESSIONS: SessionTable = SessionTable {
    venue: Venue::NseDerivatives,
    anchor: SessionRow::verified(
        EPOCH_DAY,
        SESSION_OPEN_MINUTE,
        SESSION_CLOSE_MINUTE,
        SessionKind::Continuous,
        CHARTER_SESSION,
    ),
    later: [
        Some(SessionRow::verified(
            AUG_3_2026,
            SESSION_OPEN_MINUTE,
            15 * 60 + 40,
            SessionKind::Continuous,
            FNO_EXTENSION_CIRCULAR,
        )),
        None,
    ],
};

const _: () = assert!(NSE_INDEX_SESSIONS.is_shipping_shape());
const _: () = assert!(NSE_CASH_SESSIONS.is_shipping_shape());
const _: () = assert!(NSE_DERIVATIVES_SESSIONS.is_shipping_shape());
const _: () = assert!(NSE_INDEX_SESSIONS.anchor_matches_session_constants());
const _: () = assert!(NSE_CASH_SESSIONS.anchor_matches_session_constants());
const _: () = assert!(NSE_DERIVATIVES_SESSIONS.anchor_matches_session_constants());
const _: () = assert!(Venue::ALL.len() == VENUE_COUNT);

// The anchor's one-minute count is `crate::session`'s 375, derived rather than
// restated. A close that made the two disagree is a compile error here.
const CHARTER_ANCHOR: Session = Session {
    open_minute: SESSION_OPEN_MINUTE,
    close_minute: SESSION_CLOSE_MINUTE,
    kind: SessionKind::Continuous,
    source: CHARTER_SESSION,
};
const _: () = assert!(match CHARTER_ANCHOR.expected_count(Granularity::Minute1) {
    ExpectedCount::Exact(bars) => bars == BARS_PER_REGULAR_SESSION,
    _ => false,
});
const _: () = assert!(CHARTER_ANCHOR.len_secs() == 22_500);

// ---------------------------------------------------------------------------
// transport
// ---------------------------------------------------------------------------

/// The HTTP verb a feed's bars endpoint takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// `GET`, with the range in the query string.
    Get,
    /// `POST`, with the range in the body.
    Post,
}

impl Method {
    /// The verb as it goes on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// How a token is presented in its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthScheme {
    /// The header value is the token, with no prefix.
    Raw,
    /// The header value is `Bearer ` then the token.
    Bearer,
}

impl AuthScheme {
    /// The prefix that goes before the token, `""` when there is none.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Raw => "",
            Self::Bearer => "Bearer ",
        }
    }
}

/// Which header carries the credential, and how.
///
/// The header **name** is data. The header **value** never appears in this
/// type, in [`Descriptor`], or in anything derived from them — see
/// [`crate::fetch::WireRequest`], which is the request as far as it can be
/// built without a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Auth {
    /// The header name, e.g. `access-token` or `Authorization`.
    pub header: &'static str,
    /// How the token is written into it.
    pub scheme: AuthScheme,
}

/// Where one request parameter's value comes from.
///
/// # Why the request is data and not code
///
/// `window_async` built the whole request as two hardcoded fields —
/// `json!({ "fromDate": from, "toDate": to })` for POST and
/// `.query(&[("from", from), ("to", to)])` for GET. No instrument, no segment,
/// no interval. That is `DH-905 securityId is required` in two lines, and it
/// is why a broker has never returned a bar to this build.
///
/// Naming the fields here rather than there keeps `CLAUDE.md`'s rule that
/// adding a vendor is a ROW in this file: Dhan wants `securityId` and
/// `exchangeSegment`, Groww wants `groww_symbol` and `segment`, and the code
/// that puts them on the wire never learns either name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamValue {
    /// The window's first day, in this feed's [`DateFormat`].
    From,
    /// The window's last day, honouring this feed's [`RangeEnd`].
    To,
    /// The vendor's OWN id for the instrument — `SECURITY_ID` at Dhan,
    /// `groww_symbol` at Groww. Read from the instrument master; see
    /// `brutex_core::vendor::VendorId`.
    InstrumentId,
    /// A word this feed requires and this build does not vary, such as
    /// `exchangeSegment: "IDX_I"`. Fixed here so it is reviewable beside the
    /// rest of the row rather than buried in a request builder.
    Fixed(&'static str),
}

/// One named request parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Param {
    /// What this feed calls the field.
    pub name: &'static str,
    /// Where its value comes from.
    pub value: ParamValue,
}

/// How a date is rendered on the wire, or read out of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateFormat {
    /// `YYYY-MM-DD`.
    DashedYmd,
    /// `YYYYMMDD`, no separators.
    CompactYmd,
    /// `DD/MM/YYYY` — GDFL's archives. Reading it the other way round shifts
    /// every bar by months, silently, which is why it is a named row and not
    /// a guess at the call site.
    SlashedDmy,
    /// `DDMMYYYY`, no separators.
    CompactDmy,
}

/// Whether a feed's range end includes the day it names.
///
/// A boolean-shaped fact, never an `if vendor == …` branch. The primary
/// broker's `toDate` is **not** inclusive; another feed's will be, and the
/// difference is one day silently missing from every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeEnd {
    /// The named day is included.
    Inclusive,
    /// The named day is excluded — the wire value is the day *after* the
    /// operator's last day.
    Exclusive,
}

/// How a bars response is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponseShape {
    /// One array per field, all the same length. The length agreement is a
    /// *promise*, and [`crate::fetch`] checks it as one refusal before any
    /// iterator exists — see that module on why `zip` is the trap.
    ParallelArrays {
        /// The object key the arrays hang under, or `None` when they are at
        /// the top level.
        envelope: Option<&'static str>,
    },
    /// One object per bar.
    ArrayOfObjects {
        /// The object key the array hangs under, or `None` at the top level.
        envelope: Option<&'static str>,
    },
}

/// What a timestamp in a response means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampEncoding {
    /// Seconds since the Unix epoch, UTC.
    EpochSecondsUtc,
    /// Milliseconds since the Unix epoch, UTC.
    EpochMillisUtc,
    /// A local IST date and time, `YYYY-MM-DD HH:MM:SS`.
    IstDateTimeText,
}

/// What unit a price arrives in.
///
/// Never a float either way: a rupee price arrives as decimal **text** and is
/// converted digit by digit. See `crate::fetch::paisa_from_decimal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriceScale {
    /// Rupees, with up to two decimal places.
    Rupees,
    /// Already paisa integers.
    Paisa,
}

/// The response field names, as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FieldNames {
    /// The open field.
    pub open: &'static str,
    /// The high field.
    pub high: &'static str,
    /// The low field.
    pub low: &'static str,
    /// The close field.
    pub close: &'static str,
    /// The volume field.
    pub volume: &'static str,
    /// The timestamp field.
    pub timestamp: &'static str,
    /// The open-interest field, or `None` for a feed that does not send one.
    /// Absent open interest becomes `store::format::OI_NULL`; **zero means
    /// zero**.
    pub open_interest: Option<&'static str>,
}

/// A published rate budget.
///
/// Every span is optional because "no published bound on that span" is a
/// recorded fact for both HTTP feeds, and `crate::rate::Governor` already
/// spells absence as `None` and refuses a zero so the two cannot be confused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Budget {
    /// Requests per second.
    pub per_second: Option<u32>,
    /// Requests per minute.
    pub per_minute: Option<u32>,
    /// Requests per day.
    pub per_day: Option<u32>,
}

/// Whether a feed's budget is shared across request kinds or held per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pooling {
    /// One budget for the whole vendor, whatever is being asked for.
    PerVendor,
    /// A separate budget per endpoint group — `crate::rate::RequestKind`.
    PerRequestKind,
}

/// Everything an HTTP feed needs.
///
/// [`Auth`] and [`Budget`] live **here**, not on [`Descriptor`]. That is the
/// structural half of "a local archive has no token and no rate limit": an
/// archive descriptor has nowhere to put either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HttpSpec {
    /// Scheme and host, no trailing slash.
    pub base_url: &'static str,
    /// The historical-bars path, leading slash included.
    pub bars_path: &'static str,
    /// The verb.
    pub method: Method,
    /// Which header carries the credential, and how.
    pub auth: Auth,
    /// How dates are rendered.
    pub date_format: DateFormat,
    /// Whether the range end includes the day it names.
    pub range_end: RangeEnd,
    /// How the response is laid out.
    pub response: ResponseShape,
    /// What each field is called.
    pub fields: FieldNames,
    /// What a timestamp means.
    pub timestamps: TimestampEncoding,
    /// What unit prices arrive in.
    pub prices: PriceScale,
    /// The published budget.
    pub budget: Budget,
    /// Whether that budget is pooled per request kind.
    pub pooling: Pooling,
    /// Every field this feed's request carries, and where its value comes from.
    ///
    /// Empty means the request names nothing but its window, which is the state
    /// that made Dhan answer `DH-905 securityId is required`.
    pub params: &'static [Param],
    /// Headers this feed requires beyond the credential.
    pub extra_headers: &'static [(&'static str, &'static str)],
}

/// How an archive vendor nests its files. Reported, and used to explain a
/// refusal; the addressing itself is [`ArchiveName`] and [`MemberPattern`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nesting {
    /// One archive per day, holding one member per instrument at its root.
    ZipOfDailyZips,
    /// One archive per day, holding a stem folder with a folder per group.
    ZipOfSegmentFolders,
}

/// How a day's archive file is named.
///
/// A **pattern**, so the path is computed by string formatting from
/// `(segment, date)` and never found by walking a directory —
/// `docs/07-o1-architecture.md` law 3. A directory walk over a folder of daily
/// archives is O(days) and grows forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveName {
    /// `{prefix}{segment token}{infix}{date}{suffix}`.
    SegmentAndDate {
        /// Text before the segment token.
        prefix: &'static str,
        /// Text between the segment token and the date.
        infix: &'static str,
        /// Text after the date, extension included.
        suffix: &'static str,
        /// How the date is rendered.
        date: DateFormat,
    },
    /// `{prefix}{date}{suffix}`.
    DateOnly {
        /// Text before the date.
        prefix: &'static str,
        /// Text after the date, extension included.
        suffix: &'static str,
        /// How the date is rendered.
        date: DateFormat,
    },
}

/// How a member inside a day's archive is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberPattern {
    /// `{symbol}{suffix}` at the archive root.
    SymbolAtRoot {
        /// Extension, dot included.
        suffix: &'static str,
    },
    /// `{archive stem}/{group folder}/{symbol}{suffix}`.
    StemGroupSymbol {
        /// Extension, dot included.
        suffix: &'static str,
    },
}

/// Which folder inside an archive an instrument lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveGroup {
    /// The archive has no group folders.
    Flat,
    /// Options.
    Options,
    /// Futures.
    Futures,
}

/// Whether a member CSV starts with a header row, and what it must say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeaderRow {
    /// No header row. A first line that looks like one is a refusal, not a
    /// row to skip: skipping it would silently drop a real record from a feed
    /// whose first field happens to be text.
    Absent,
    /// A header row, which must match this text exactly.
    Present(&'static str),
}

/// One CSV column's meaning.
///
/// [`Self::Unverified`] is the honest arm. `docs/08-vendor-samples.md` records
/// a column *count* for one archive whose column *meanings* were never
/// established; naming them anyway would be invention, and a price column read
/// as a volume yields plausible numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    /// The instrument ticker.
    Ticker,
    /// The date.
    Date,
    /// The time of day.
    Time,
    /// The last traded price.
    LastPrice,
    /// The open.
    Open,
    /// The high.
    High,
    /// The low.
    Low,
    /// The close.
    Close,
    /// Traded volume.
    Volume,
    /// Open interest.
    OpenInterest,
    /// The best bid price.
    BidPrice,
    /// The best bid size.
    BidQty,
    /// The best ask price.
    AskPrice,
    /// The best ask size.
    AskQty,
    /// The last traded quantity. Zero on a quote-only update.
    LastQty,
    /// Present, observed, and carrying nothing this engine reads.
    Ignored,
    /// Present, and its meaning was never established. Reading it is refused.
    Unverified,
}

/// The column order for one `(feed, segment)` pair.
///
/// Keyed by segment because `docs/08-vendor-samples.md` measured **5, 9 and 10
/// columns inside one vendor's own archives** — indices carry no volume and no
/// open interest, so those fields are structurally absent rather than zero. A
/// single per-vendor layout would mis-parse one of the two, silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColumnLayout {
    /// Which segment this layout describes.
    pub segment: Segment,
    /// The columns, in file order.
    pub columns: &'static [Column],
}

/// What kind of record a feed writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordShape {
    /// Open, high, low, close, volume, open interest — the 56-byte bar
    /// `crates/store` writes.
    Ohlcv,
    /// Last price, bid, bid size, ask, ask size, last quantity, open
    /// interest. **Not** a bar: it has no open, high, low or close, several
    /// records share one timestamp, and there is no sub-second tiebreaker.
    /// `crates/store` has no format version for this yet, so a feed declaring
    /// it refuses at the write boundary rather than being stored as a bar with
    /// the price repeated four times — which would be a lie written into the
    /// data itself.
    Snapshot,
}

/// Everything a local-archive feed needs.
///
/// No token field and no budget field, and that is the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArchiveSpec {
    /// How the vendor nests its files.
    pub nesting: Nesting,
    /// How a day's archive is named.
    pub archive: ArchiveName,
    /// How a member inside it is named.
    pub member: MemberPattern,
    /// The group folders, by name. Empty for a flat archive.
    pub groups: &'static [(ArchiveGroup, &'static str)],
    /// The segment tokens an [`ArchiveName::SegmentAndDate`] needs.
    pub segment_tokens: &'static [(Segment, &'static str)],
    /// Whether a header row exists.
    pub header: HeaderRow,
    /// The field separator.
    pub delimiter: u8,
    /// The column layouts, by segment.
    pub layouts: &'static [ColumnLayout],
    /// How the date column is written.
    pub date_format: DateFormat,
    /// What unit prices are in.
    pub prices: PriceScale,
}

impl ArchiveSpec {
    /// The layout for one segment, or `None` when none was ever measured.
    ///
    /// A linear walk of a `const` slice whose length is a property of the
    /// shipped table and not of any input — at most one entry per segment, and
    /// `Segment` has three variants. Constant, by the same argument
    /// [`MAX_LATER_SESSION_ROWS`] makes.
    /// `pull::vendor::an_unmeasured_segment_layout_is_refused_by_name` is the
    /// test that a missing layout refuses instead of guessing.
    #[must_use]
    pub fn layout(&self, segment: Segment) -> Option<&'static ColumnLayout> {
        self.layouts.iter().find(|row| row.segment == segment)
    }

    /// The folder name for a group, or `None` when the archive is flat or the
    /// group is not one this vendor has.
    #[must_use]
    pub fn group_folder(&self, group: ArchiveGroup) -> Option<&'static str> {
        self.groups
            .iter()
            .find(|(candidate, _)| *candidate == group)
            .map(|(_, folder)| *folder)
    }

    /// The archive token for a segment, or `None`.
    #[must_use]
    pub fn segment_token(&self, segment: Segment) -> Option<&'static str> {
        self.segment_tokens
            .iter()
            .find(|(candidate, _)| *candidate == segment)
            .map(|(_, token)| *token)
    }
}

/// How a feed's bytes are obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// A network request.
    Http(HttpSpec),
    /// A folder of archives already on disk. No network, no token, no rate
    /// limit, and no 429.
    LocalArchive(ArchiveSpec),
}

impl Transport {
    /// A short, stable label for a page or a report.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Http(_) => "HTTP API",
            Self::LocalArchive(_) => "local archive",
        }
    }

    /// Whether this transport needs a credential at all.
    ///
    /// The page reads this rather than asking which vendor it is: an archive
    /// feed must never be shown a token field.
    #[must_use]
    pub const fn needs_credential(self) -> bool {
        matches!(self, Self::Http(_))
    }

    /// Whether this transport needs a rate governor at all.
    #[must_use]
    pub const fn needs_governor(self) -> bool {
        matches!(self, Self::Http(_))
    }
}

// ---------------------------------------------------------------------------
// the feed table
// ---------------------------------------------------------------------------

/// How many feeds this build knows.
pub const FEED_COUNT: usize = 4;

/// Which feed a request is for.
///
/// Named `Feed` and not `Vendor` because `brutex_core::vendor::Vendor` already
/// exists and means something narrower — the **store path prefix**, a closed
/// set of the two brokers whose bars this engine writes. Two types named
/// `Vendor` in one crate is exactly the confusion this module exists to
/// remove. [`Feed::store_vendor`] is the one bridge between them, and it
/// returns `None` rather than inventing a prefix.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feed {
    /// Secondary broker, HTTP.
    Dhan = 0,
    /// Primary broker, HTTP.
    Groww = 1,
    /// Historical archives on disk.
    TrueData = 2,
    /// Historical archives on disk.
    Gdfl = 3,
}

impl Feed {
    /// Every feed, in table order.
    pub const ALL: [Self; FEED_COUNT] = [Self::Dhan, Self::Groww, Self::TrueData, Self::Gdfl];

    /// This feed's row in [`DESCRIPTORS`].
    ///
    /// **One array index.** No search, no map, no hash, and the cost does not
    /// change when the table grows — the index is the variant's own
    /// discriminant, so there is nothing to look up. The `const` assertions
    /// under [`DESCRIPTORS`] are what make indexing by discriminant sound:
    /// they pin the table length to the variant count and row *i* to variant
    /// *i*, so a new variant with no row fails the build.
    /// `pull::vendor::the_descriptor_table_is_indexed_by_the_discriminant`
    /// proves both halves from outside.
    #[must_use]
    #[allow(
        clippy::indexing_slicing,
        reason = "the index IS the #[repr(u8)] discriminant, and the const assertions under \
                  DESCRIPTORS pin the table length to FEED_COUNT and each row to its own \
                  variant, so the index is in range at compile time; `.get()` would need an \
                  unwrap, which is banned outright"
    )]
    pub const fn descriptor(self) -> &'static Descriptor {
        DESCRIPTORS[self as usize]
    }

    /// The store path prefix this feed writes under.
    ///
    /// `None` for a feed `brutex_core::vendor::Vendor` does not name. That is
    /// a **named refusal at the write boundary**, not a fallback: the vendor
    /// is the first segment of every store path (`docs/05-decisions.md`
    /// D-0019) precisely so one feed's history can be deleted without touching
    /// another's, and filing an archive vendor's bars under a broker's prefix
    /// would destroy that property irreversibly.
    #[must_use]
    pub const fn store_vendor(self) -> Option<Vendor> {
        match self {
            Self::Dhan => Some(Vendor::Dhan),
            Self::Groww => Some(Vendor::Groww),
            Self::TrueData | Self::Gdfl => None,
        }
    }

    /// The operator-facing name.
    #[must_use]
    pub const fn display(self) -> &'static str {
        self.descriptor().display
    }

    /// The name this feed is written down as.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        self.descriptor().wire
    }

    /// Whether this feed serves `granularity`.
    #[must_use]
    pub const fn serves(self, granularity: Granularity) -> bool {
        self.descriptor().granularities.contains(granularity)
    }
}

impl fmt::Display for Feed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display())
    }
}

/// One feed, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Descriptor {
    /// Which feed this row describes. Pinned to its index at compile time.
    pub feed: Feed,
    /// The operator-facing name.
    pub display: &'static str,
    /// The name this feed is written down as.
    pub wire: &'static str,
    /// What shape its records are.
    pub record: RecordShape,
    /// How its bytes are obtained.
    pub transport: Transport,
    /// Which granularities it serves.
    pub granularities: GranularitySet,
    /// Which segments it serves.
    pub segments: SegmentSet,
    /// Which exchange its rows belong to.
    pub exchange: Exchange,
}

// --- the rows --------------------------------------------------------------

const DHAN: Descriptor = Descriptor {
    feed: Feed::Dhan,
    display: "Dhan",
    wire: "dhan",
    record: RecordShape::Ohlcv,
    transport: Transport::Http(HttpSpec {
        base_url: "https://api.dhan.co",
        bars_path: "/v2/charts/historical",
        method: Method::Post,
        auth: Auth {
            header: "access-token",
            scheme: AuthScheme::Raw,
        },
        date_format: DateFormat::DashedYmd,
        // Verified from the vendor: `toDate` is NOT inclusive. One field, one
        // conversion site, and the off-by-one cannot come back.
        range_end: RangeEnd::Exclusive,
        response: ResponseShape::ParallelArrays { envelope: None },
        fields: FieldNames {
            open: "open",
            high: "high",
            low: "low",
            close: "close",
            volume: "volume",
            timestamp: "timestamp",
            open_interest: Some("open_interest"),
        },
        timestamps: TimestampEncoding::EpochSecondsUtc,
        prices: PriceScale::Rupees,
        budget: Budget {
            // docs/00-charter.md §4 and crate::rate::{DHAN_PER_SECOND,
            // DHAN_PER_DAY}. No per-minute governor is published, and `None`
            // says exactly that.
            per_second: Some(crate::rate::DHAN_PER_SECOND),
            per_minute: None,
            per_day: Some(crate::rate::DHAN_PER_DAY),
        },
        pooling: Pooling::PerVendor,
        // Read first-hand from dhanhq.co/docs/v2/historical-data. All three are
        // marked REQUIRED there, and their absence is exactly what DH-905
        // reports. `IDX_I` and `INDEX` are the index-spot pair from the
        // Annexure; a wider surface needs more rows here, not more code.
        params: &[
            Param {
                name: "securityId",
                value: ParamValue::InstrumentId,
            },
            Param {
                name: "exchangeSegment",
                value: ParamValue::Fixed("IDX_I"),
            },
            Param {
                name: "instrument",
                value: ParamValue::Fixed("INDEX"),
            },
            Param {
                name: "fromDate",
                value: ParamValue::From,
            },
            Param {
                name: "toDate",
                value: ParamValue::To,
            },
        ],
        extra_headers: &[],
    }),
    // ONLY the two rungs the verified contract covers. The wider ladder this
    // broker's documentation advertises was NOT read live, so it is not
    // written down here — an unserved rung refuses by name, and adding one is
    // a one-row diff the day it is confirmed. UNVERIFIED, deliberately narrow.
    granularities: GranularitySet::EMPTY
        .with(Granularity::Minute1)
        .with(Granularity::Day1),
    segments: SegmentSet::EMPTY
        .with(Segment::Index)
        .with(Segment::Cash)
        .with(Segment::Fno),
    exchange: Exchange::Nse,
};

const GROWW: Descriptor = Descriptor {
    feed: Feed::Groww,
    display: "Groww",
    wire: "groww",
    record: RecordShape::Ohlcv,
    transport: Transport::Http(HttpSpec {
        base_url: "https://api.groww.in",
        bars_path: "/v1/historical/candle/range",
        method: Method::Get,
        auth: Auth {
            header: "Authorization",
            scheme: AuthScheme::Bearer,
        },
        date_format: DateFormat::DashedYmd,
        range_end: RangeEnd::Inclusive,
        response: ResponseShape::ArrayOfObjects {
            envelope: Some("payload"),
        },
        fields: FieldNames {
            open: "open",
            high: "high",
            low: "low",
            close: "close",
            volume: "volume",
            timestamp: "timestamp",
            open_interest: None,
        },
        timestamps: TimestampEncoding::EpochMillisUtc,
        prices: PriceScale::Rupees,
        budget: Budget {
            // The per-minute figure is operator-confirmed. The per-second
            // figure is NOT, and the constant's own name says so — see
            // crate::rate::GROWW_PER_SECOND_UNVERIFIED. It is not renamed
            // here and it is not quietly promoted.
            per_second: Some(crate::rate::GROWW_PER_SECOND_UNVERIFIED),
            per_minute: Some(crate::rate::GROWW_PER_MINUTE),
            per_day: None,
        },
        // Groww pools per endpoint group; the other broker does not.
        pooling: Pooling::PerRequestKind,
        // Read first-hand from groww.in/trade-api/docs/curl/historical-data.
        // `trading_symbol` is the deprecated endpoint's name for it; the live
        // one takes `groww_symbol`, and the SAME master row spells the two
        // differently — NSE-NIFTY-30Sep25-24650-CE against NIFTY25SEP24650CE.
        params: &[
            Param {
                name: "exchange",
                value: ParamValue::Fixed("NSE"),
            },
            Param {
                name: "segment",
                value: ParamValue::Fixed("CASH"),
            },
            Param {
                name: "trading_symbol",
                value: ParamValue::InstrumentId,
            },
            Param {
                name: "start_time",
                value: ParamValue::From,
            },
            Param {
                name: "end_time",
                value: ParamValue::To,
            },
        ],
        // Groww's page shows this on every historical call.
        extra_headers: &[("X-API-VERSION", "1.0")],
    }),
    // Same restraint as the row above, and for the same reason.
    granularities: GranularitySet::EMPTY
        .with(Granularity::Minute1)
        .with(Granularity::Day1),
    segments: SegmentSet::EMPTY
        .with(Segment::Index)
        .with(Segment::Cash)
        .with(Segment::Fno),
    exchange: Exchange::Nse,
};

/// `TrueData`'s observed index layout: `20221003,09:07:41,38444.90,0,0`.
///
/// Five columns, measured. The last two are structurally zero for an index —
/// `docs/08-vendor-samples.md` — so they are [`Column::Ignored`] rather than
/// mapped onto volume and open interest, which an index does not have.
const TRUEDATA_INDEX: ColumnLayout = ColumnLayout {
    segment: Segment::Index,
    columns: &[
        Column::Date,
        Column::Time,
        Column::LastPrice,
        Column::Ignored,
        Column::Ignored,
    ],
};

const TRUE_DATA: Descriptor = Descriptor {
    feed: Feed::TrueData,
    display: "TrueData",
    wire: "truedata",
    record: RecordShape::Snapshot,
    transport: Transport::LocalArchive(ArchiveSpec {
        nesting: Nesting::ZipOfDailyZips,
        archive: ArchiveName::SegmentAndDate {
            prefix: "NSE_",
            infix: "_TICK_",
            suffix: ".zip",
            date: DateFormat::CompactYmd,
        },
        member: MemberPattern::SymbolAtRoot { suffix: ".csv" },
        groups: &[],
        // ONLY the index token, which was measured. The archives also carry
        // `FUT` and `OPT`, and `Segment::Fno` covers both — so a single token
        // for it would have to pick one and be wrong half the time. Left out
        // rather than guessed; the segment set below excludes it, so the
        // ambiguity is unreachable rather than latent.
        segment_tokens: &[(Segment::Index, "IDX")],
        header: HeaderRow::Absent,
        delimiter: b',',
        // ONLY the index layout ships. The futures archives were measured at
        // NINE columns and their column MEANINGS were never established, so
        // there is no row for them: an unmeasured segment refuses by name
        // rather than being decoded against a layout somebody guessed.
        layouts: &[TRUEDATA_INDEX],
        date_format: DateFormat::CompactYmd,
        prices: PriceScale::Rupees,
    }),
    granularities: GranularitySet::EMPTY
        .with(Granularity::Second1)
        .with(Granularity::Minute1),
    segments: SegmentSet::EMPTY.with(Segment::Index),
    exchange: Exchange::Nse,
};

/// GDFL's observed header, character for character:
/// `Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest`.
const GDFL_FNO: ColumnLayout = ColumnLayout {
    segment: Segment::Fno,
    columns: &[
        Column::Ticker,
        Column::Date,
        Column::Time,
        Column::LastPrice,
        Column::BidPrice,
        Column::BidQty,
        Column::AskPrice,
        Column::AskQty,
        Column::LastQty,
        Column::OpenInterest,
    ],
};

const GDFL_HEADER: &str = "Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest";

const GDFL: Descriptor = Descriptor {
    feed: Feed::Gdfl,
    display: "Global Datafeeds",
    wire: "gdfl",
    record: RecordShape::Snapshot,
    transport: Transport::LocalArchive(ArchiveSpec {
        nesting: Nesting::ZipOfSegmentFolders,
        archive: ArchiveName::DateOnly {
            prefix: "GFDLNFO_TICK_",
            suffix: ".zip",
            // DD/MM/YYYY inside the file, DDMMYYYY in the archive name:
            // `GFDLNFO_TICK_01072025.zip` is 1 July, not 7 January.
            date: DateFormat::CompactDmy,
        },
        member: MemberPattern::StemGroupSymbol { suffix: ".NFO.csv" },
        groups: &[
            (ArchiveGroup::Options, "Options"),
            (ArchiveGroup::Futures, "Futures"),
        ],
        segment_tokens: &[],
        header: HeaderRow::Present(GDFL_HEADER),
        delimiter: b',',
        layouts: &[GDFL_FNO],
        date_format: DateFormat::SlashedDmy,
        prices: PriceScale::Rupees,
    }),
    granularities: GranularitySet::EMPTY.with(Granularity::Second1),
    segments: SegmentSet::EMPTY.with(Segment::Fno),
    exchange: Exchange::Nse,
};

/// Every feed, indexed by its own discriminant.
///
/// The three `const` blocks below are the contract: the table length equals
/// [`FEED_COUNT`], the variant list length equals [`FEED_COUNT`], and **row
/// *i* carries variant *i***. Together they make [`Feed::descriptor`]'s single
/// array index sound at compile time, and make a feed added to the enum but
/// not to the table a build failure rather than a runtime surprise.
pub const DESCRIPTORS: [&Descriptor; FEED_COUNT] = [&DHAN, &GROWW, &TRUE_DATA, &GDFL];

const _: () = assert!(DESCRIPTORS.len() == FEED_COUNT);
const _: () = assert!(Feed::ALL.len() == FEED_COUNT);
const _: () = {
    // Destructured rather than indexed: adding a fifth feed makes this pattern
    // itself a compile error, before any assertion is even evaluated.
    let [dhan, groww, truedata, gdfl] = DESCRIPTORS;
    assert!(dhan.feed as u8 == Feed::Dhan as u8);
    assert!(groww.feed as u8 == Feed::Groww as u8);
    assert!(truedata.feed as u8 == Feed::TrueData as u8);
    assert!(gdfl.feed as u8 == Feed::Gdfl as u8);
    let [first, second, third, fourth] = Feed::ALL;
    assert!(first as u8 == 0 && second as u8 == 1 && third as u8 == 2 && fourth as u8 == 3);
};

// No row may ship an empty capability set: a feed that serves no granularity
// or no segment can never answer a request, and offering it on a page would be
// a control that always refuses.
const _: () = {
    let [dhan, groww, truedata, gdfl] = DESCRIPTORS;
    assert!(!dhan.granularities.is_empty() && !dhan.segments.is_empty());
    assert!(!groww.granularities.is_empty() && !groww.segments.is_empty());
    assert!(!truedata.granularities.is_empty() && !truedata.segments.is_empty());
    assert!(!gdfl.granularities.is_empty() && !gdfl.segments.is_empty());
};

// A local archive carries no token and no budget by construction — there is no
// field for either on `ArchiveSpec`. This says the same thing about the two
// shipped archive rows in a form the compiler checks.
const _: () = {
    let [_, _, truedata, gdfl] = DESCRIPTORS;
    assert!(!truedata.transport.needs_credential());
    assert!(!truedata.transport.needs_governor());
    assert!(!gdfl.transport.needs_credential());
    assert!(!gdfl.transport.needs_governor());
};

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
//
// THESE LIVE IN THE FILE RATHER THAN IN `tests/` FOR ONE REASON. Most of what
// this module is *made of* is private and `const`: `str_eq`, `has_content`,
// `day_const`, `SessionRow` and `SessionTable` are exercised only by the
// `const` assertions above, and a `const` assertion is evaluated by the
// compiler — it is invisible to runtime instrumentation and proves nothing
// about the code an operator's process actually runs. An integration test
// cannot reach a private item at all, so the choice is here or nowhere.
//
// Every `const fn` below is called through `black_box` so the call cannot be
// folded back into a compile-time constant and disappear again.
//
// NO STRING LITERAL HERE IS SHAPED LIKE A PARAMETER-PATH SEGMENT. CI gate 1d
// walks this crate for lower-case quoted words; the assertions are written
// against the table's own accessors (`dir()`, `wire()`, `label()`) and against
// `store::path::Timeframe`, which is both stronger than pinning a spelling and
// leaves nothing new to declare.
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
    use std::collections::HashSet;
    use std::hint::black_box;

    use super::*;

    /// A real date, or the test fails on its own literal.
    fn day(year: u16, month: u8, of_month: u8) -> Day {
        Day::new(year, month, of_month).expect("the test's own date literal is real")
    }

    // -- the const helpers ---------------------------------------------------

    /// `str_eq` is byte equality including length, in both directions.
    #[test]
    fn str_eq_compares_every_byte_and_the_length_in_both_directions() {
        assert!(
            str_eq(black_box(""), black_box("")),
            "two empty strings are equal"
        );

        let minute = Granularity::Minute1.dir();
        assert!(
            str_eq(black_box(minute), black_box(Timeframe::MINUTE_1.as_str())),
            "the rung's directory name and the store's timeframe name are the \
             same bytes — this is the equality the const assertion above rests on"
        );
        assert!(
            !str_eq(black_box(minute), black_box(Granularity::Minute3.dir())),
            "same length, first byte differs"
        );

        // A prefix relationship, which is the only way to reach the arm where
        // one side runs out of bytes before the other.
        let doubled = format!("{minute}{minute}");
        assert!(
            !str_eq(black_box(minute), black_box(doubled.as_str())),
            "a prefix is not equal to the whole"
        );
        assert!(
            !str_eq(black_box(doubled.as_str()), black_box(minute)),
            "and the whole is not equal to its prefix"
        );
    }

    /// A citation of nothing but whitespace is no citation.
    #[test]
    fn has_content_refuses_a_blank_citation() {
        assert!(!has_content(black_box("")), "empty says nothing");
        assert!(
            !has_content(black_box(" \t\r\n ")),
            "whitespace says nothing either — this is the check that keeps a \
             row from wearing a citation field it never filled in"
        );
        assert!(
            has_content(black_box("  X")),
            "one non-blank byte anywhere is content"
        );
        assert!(
            has_content(black_box(CHARTER_SESSION)),
            "and every shipped row's citation has content"
        );
    }

    /// An unreal date collapses to the epoch instead of panicking — and the
    /// `const` assertion beside each shipped date is what catches it.
    #[test]
    fn day_const_falls_back_to_the_epoch_rather_than_panicking() {
        assert_eq!(
            day_const(black_box(2026), black_box(8), black_box(3)),
            AUG_3_2026,
            "a real date round-trips"
        );
        assert_eq!(
            day_const(black_box(2026), black_box(2), black_box(30)),
            EPOCH_DAY,
            "30 February is not a date, so the fallback fires — and the const \
             assertion beside every shipped date is what turns that into a \
             build failure rather than a wrong window"
        );
        assert_eq!(EPOCH_DAY.days_from_epoch(), 0);
        assert_eq!(
            (AUG_3_2026.year(), AUG_3_2026.month(), AUG_3_2026.day()),
            (2026, 8, 3)
        );
    }

    // -- the granularity ladder ---------------------------------------------

    /// Every rung has its own directory, its own grid, and a `Display` that is
    /// the directory.
    #[test]
    fn every_granularity_rung_has_a_distinct_directory_and_one_grid() {
        let mut dirs = HashSet::with_capacity(GRANULARITY_COUNT);
        let mut intraday = 0_usize;
        let mut aggregate = 0_usize;

        for rung in Granularity::ALL {
            let dir = rung.dir();
            assert!(!dir.is_empty(), "a rung with no directory name");
            assert!(dirs.insert(dir), "{dir} is claimed by two rungs");
            assert_eq!(rung.to_string(), dir, "Display is the directory name");

            match rung.grid() {
                Grid::Event => {
                    assert!(rung.is_intraday(), "an event grid is intraday");
                    intraday += 1;
                }
                Grid::Intraday(secs) => {
                    assert!(secs > 0, "{dir} claims a zero-second interval");
                    assert!(rung.is_intraday());
                    intraday += 1;
                }
                Grid::Daily | Grid::Weekly => {
                    assert!(
                        !rung.is_intraday(),
                        "{dir} is an aggregate bar and carries no intraday time"
                    );
                    aggregate += 1;
                }
            }
        }

        assert_eq!(dirs.len(), GRANULARITY_COUNT, "eleven distinct rungs");
        assert_eq!(aggregate, 2, "exactly the daily and weekly rungs aggregate");
        assert_eq!(intraday, GRANULARITY_COUNT - 2);
    }

    /// One rung has a store timeframe and the rest refuse by absence.
    #[test]
    fn only_the_one_minute_rung_carries_a_store_timeframe() {
        let mut carried = 0_usize;
        for rung in Granularity::ALL {
            match rung.store_timeframe() {
                Some(timeframe) => {
                    assert_eq!(rung, Granularity::Minute1);
                    assert_eq!(timeframe.as_str(), rung.dir());
                    assert_eq!(rung.grid(), Grid::Intraday(timeframe.secs()));
                    carried += 1;
                }
                None => assert_ne!(
                    rung,
                    Granularity::Minute1,
                    "a None is a refusal at the write boundary, never a substitution"
                ),
            }
        }
        assert_eq!(carried, 1, "crates/store ships exactly one rung");
    }

    /// A bitset holds what was put in it, and adding twice is adding once.
    #[test]
    fn a_granularity_set_is_a_bitset_over_the_ladder() {
        let empty = GranularitySet::EMPTY;
        assert!(empty.is_empty());
        assert_eq!(GranularitySet::default(), empty, "default is empty");
        for rung in Granularity::ALL {
            assert!(!empty.contains(rung), "the empty set holds nothing");
        }

        let one = empty.with(Granularity::Minute1);
        assert!(!one.is_empty());
        assert!(one.contains(Granularity::Minute1));
        assert_eq!(
            one.with(Granularity::Minute1),
            one,
            "adding twice is adding once"
        );

        let two = one.with(Granularity::Week1);
        let held = Granularity::ALL
            .iter()
            .filter(|g| two.contains(**g))
            .count();
        assert_eq!(held, 2, "exactly the two rungs that were added");
        assert!(two.contains(Granularity::Week1) && two.contains(Granularity::Minute1));
        assert!(!two.contains(Granularity::Tick));
    }

    /// The segment bitset, and the free function that gives each segment a bit.
    #[test]
    fn a_segment_set_gives_each_segment_its_own_bit() {
        let empty = SegmentSet::EMPTY;
        assert!(empty.is_empty());
        assert_eq!(SegmentSet::default(), empty);

        let every = [Segment::Index, Segment::Cash, Segment::Fno];
        let mut bits = HashSet::with_capacity(every.len());
        for segment in every {
            assert!(!empty.contains(segment));
            assert!(
                bits.insert(segment_bit(segment)),
                "two segments share a bit, so a set could not tell them apart"
            );
        }

        let index_only = empty.with(Segment::Index);
        assert!(!index_only.is_empty());
        assert!(index_only.contains(Segment::Index));
        assert!(!index_only.contains(Segment::Cash) && !index_only.contains(Segment::Fno));
        assert_eq!(index_only.with(Segment::Index), index_only);

        let all = index_only.with(Segment::Cash).with(Segment::Fno);
        assert!(every.iter().all(|s| all.contains(*s)));
    }

    // -- venues, and the dated session table --------------------------------

    /// Every venue answers for its own `(exchange, segment)` pair, and BSE is
    /// refused by absence rather than mapped onto an NSE row.
    #[test]
    fn a_venue_is_chosen_by_exchange_and_segment_and_bse_has_none() {
        let mut labels = HashSet::with_capacity(VENUE_COUNT);
        for venue in Venue::ALL {
            assert!(!venue.label().is_empty());
            assert!(labels.insert(venue.label()), "two venues share a label");
            assert_eq!(venue.to_string(), venue.label());
        }
        assert_eq!(labels.len(), VENUE_COUNT);

        assert_eq!(
            Venue::for_segment(Exchange::Nse, Segment::Index),
            Some(Venue::NseIndex)
        );
        assert_eq!(
            Venue::for_segment(Exchange::Nse, Segment::Cash),
            Some(Venue::NseCash)
        );
        assert_eq!(
            Venue::for_segment(Exchange::Nse, Segment::Fno),
            Some(Venue::NseDerivatives)
        );
        for segment in [Segment::Index, Segment::Cash, Segment::Fno] {
            assert_eq!(
                Venue::for_segment(Exchange::Bse, segment),
                None,
                "CLAUDE.md section 1 does not pull BSE, and two exchanges \
                 sharing one window is a claim nothing here has a source for"
            );
        }
    }

    /// The anchor is `crate::session`'s two constants for all three venues, and
    /// the 2026-08-03 row moves only the venues whose circular says so.
    #[test]
    fn the_session_table_anchors_on_the_charter_and_moves_on_2026_08_03() {
        let before = day(2026, 8, 2);
        for venue in Venue::ALL {
            let anchored = venue.hours_on(before).expect("every anchor is verified");
            assert_eq!(anchored.open_minute(), SESSION_OPEN_MINUTE);
            assert_eq!(anchored.close_minute(), SESSION_CLOSE_MINUTE);
            assert_eq!(anchored.kind(), SessionKind::Continuous);
            assert!(
                has_content(anchored.source()),
                "the anchor IS session.rs's two constants and cites the charter, \
                 which is why that module's exhaustive day walk stays correct"
            );
            assert_eq!(
                venue.table().anchor.hours,
                Hours::Verified {
                    open_minute: SESSION_OPEN_MINUTE,
                    close_minute: SESSION_CLOSE_MINUTE,
                },
                "and the row it came from carries the hours, not a default"
            );
        }

        let after = day(2026, 8, 3);
        let index = Venue::NseIndex.hours_on(after).expect("verified row");
        let cash = Venue::NseCash.hours_on(after).expect("verified row");
        let fno = Venue::NseDerivatives.hours_on(after).expect("verified row");

        assert_eq!(
            index.close_minute(),
            15 * 60 + 15,
            "the swept index freezes EARLIEST — every constituent stops trading \
             continuously at 15:15"
        );
        assert_eq!(
            cash.close_minute(),
            SESSION_CLOSE_MINUTE,
            "a share with no derivative contract keeps its continuous close, so \
             this row restates the anchor"
        );
        assert_eq!(
            fno.close_minute(),
            15 * 60 + 40,
            "derivatives extend by ten"
        );
        assert!(
            index.close_minute() < cash.close_minute() && cash.close_minute() < fno.close_minute(),
            "the three did not move together, which is why there are three rows"
        );
        for session in [index, cash, fno] {
            assert_eq!(session.open_minute(), SESSION_OPEN_MINUTE, "the open held");
            assert!(has_content(session.source()), "each row cites its circular");
        }
    }

    /// A verified table has no refusal window, and the lookup walks a number of
    /// rows fixed at compile time.
    #[test]
    fn the_session_lookup_walks_a_fixed_number_of_rows() {
        assert_eq!(
            MAX_LATER_SESSION_ROWS, 2,
            "the bound is the length of a fixed-size array, so no input raises it"
        );
        for venue in Venue::ALL {
            let windows = venue.refusal_windows();
            assert_eq!(
                windows.len(),
                MAX_LATER_SESSION_ROWS + 1,
                "one slot per representable row"
            );
            assert!(
                windows.iter().all(Option::is_none),
                "{venue} ships no unverified row today, so it refuses no day"
            );
        }
    }

    /// Session arithmetic: length, membership, and the four record counts.
    #[test]
    fn a_session_reports_its_length_its_membership_and_its_record_count() {
        let session = Venue::NseIndex
            .hours_on(day(2024, 1, 2))
            .expect("the anchor is verified");

        assert_eq!(session.len_secs(), 22_500, "09:15 to 15:30 is 375 minutes");
        assert!(
            !session.contains_minute(SESSION_OPEN_MINUTE - 1),
            "before open"
        );
        assert!(
            session.contains_minute(SESSION_OPEN_MINUTE),
            "open is inclusive"
        );
        assert!(
            session.contains_minute(SESSION_CLOSE_MINUTE - 1),
            "last minute"
        );
        assert!(
            !session.contains_minute(SESSION_CLOSE_MINUTE),
            "the close is EXCLUSIVE"
        );

        assert_eq!(
            session.expected_count(Granularity::Minute1),
            ExpectedCount::Exact(BARS_PER_REGULAR_SESSION),
            "375 one-minute bars, derived rather than restated"
        );
        assert_eq!(
            session.expected_count(Granularity::Tick),
            ExpectedCount::Unbounded,
            "a tick feed prints as often as the market prints"
        );
        assert_eq!(
            session.expected_count(Granularity::Day1),
            ExpectedCount::Aggregate
        );
        assert_eq!(
            session.expected_count(Granularity::Week1),
            ExpectedCount::Aggregate
        );
        assert_eq!(
            session.expected_count(Granularity::Minute30),
            ExpectedCount::Irregular {
                session_secs: 22_500,
                interval_secs: 1_800,
            },
            "375 minutes is twelve and a half half-hours — named rather than \
             rounded, because a rounded count is a gap check that is wrong by a \
             fixed amount every single day"
        );
        assert_eq!(
            session.expected_count(Granularity::Second1),
            ExpectedCount::Exact(22_500)
        );
    }

    /// A row with no verified hours refuses by name, and the refusal carries
    /// the venue, the day, the window and the citation gap.
    #[test]
    fn a_row_with_no_verified_hours_refuses_and_names_the_gap() {
        const GAP: &str = "TEST ROW: no circular was retrieved for this window";
        let start = day(2030, 1, 1);
        let resumes = day(2031, 1, 1);

        let table = SessionTable {
            venue: Venue::NseCash,
            anchor: SessionRow::verified(
                EPOCH_DAY,
                SESSION_OPEN_MINUTE,
                SESSION_CLOSE_MINUTE,
                SessionKind::Continuous,
                CHARTER_SESSION,
            ),
            later: [
                Some(SessionRow::unverified(
                    start,
                    SessionKind::ClosingAuction,
                    GAP,
                )),
                Some(SessionRow::verified(
                    resumes,
                    SESSION_OPEN_MINUTE,
                    SESSION_CLOSE_MINUTE,
                    SessionKind::Continuous,
                    CHARTER_SESSION,
                )),
            ],
        };

        assert!(
            table.is_shipping_shape(),
            "the row is unverified, not malformed — an unverified row is a \
             legal row and that is the whole point of the arm"
        );

        table
            .hours_on(day(2029, 12, 31))
            .expect("before the gap the anchor still answers");
        table
            .hours_on(resumes)
            .expect("after the gap the later row answers");

        let refusal = table
            .hours_on(day(2030, 6, 1))
            .expect_err("inside the gap there is nothing to answer with");
        assert_eq!(refusal.venue(), Venue::NseCash);
        assert_eq!(refusal.day(), day(2030, 6, 1));
        assert_eq!(refusal.row_start(), start);
        assert_eq!(
            refusal.verified_from(),
            Some(resumes),
            "the window has an end, and the refusal names it"
        );
        assert_eq!(refusal.source(), GAP);

        let text = refusal.to_string();
        for fragment in [
            Venue::NseCash.label(),
            &day(2030, 6, 1).to_string(),
            &start.to_string(),
            &resumes.to_string(),
            GAP,
        ] {
            assert!(
                text.contains(fragment),
                "the refusal must carry {fragment:?} — got {text:?}"
            );
        }

        let windows = table.refusal_windows();
        let named: Vec<_> = windows.into_iter().flatten().collect();
        assert_eq!(named.len(), 1, "one unverified row, one window");
        assert_eq!(named[0].venue(), Venue::NseCash);
        assert_eq!(named[0].start(), start);
        assert_eq!(named[0].verified_from(), Some(resumes));
    }

    /// A sink that accepts `remaining` writes and then refuses.
    ///
    /// Exists for one reason: every `write!(f, …)?` in a `Display` impl carries
    /// an error edge, and against a `String` — which is what `to_string` uses —
    /// that edge is unreachable, because writing to a `String` cannot fail. A
    /// formatter over THIS refuses on demand, so each `?` in
    /// [`SessionRefusal::fmt`] is proven to propagate rather than to swallow.
    struct FailingSink {
        remaining: usize,
    }

    impl fmt::Write for FailingSink {
        fn write_str(&mut self, _text: &str) -> fmt::Result {
            match self.remaining.checked_sub(1) {
                Some(left) => {
                    self.remaining = left;
                    Ok(())
                }
                None => Err(fmt::Error),
            }
        }
    }

    /// Every `?` in the refusal's own `Display` propagates the sink's failure.
    #[test]
    fn a_refusal_propagates_a_sink_that_stops_accepting_it() {
        use std::fmt::Write as _;

        let ended = SessionRefusal {
            venue: Venue::NseCash,
            day: day(2030, 6, 1),
            row_start: day(2030, 1, 1),
            verified_from: Some(day(2031, 1, 1)),
            source: SOURCE,
        };
        let open_ended = SessionRefusal {
            verified_from: None,
            ..ended
        };

        for refusal in [ended, open_ended] {
            let full = refusal.to_string();
            let mut accepted = FailingSink {
                remaining: usize::MAX,
            };
            write!(accepted, "{refusal}").expect("a sink that never refuses");

            // Walk the budget from zero upward. Every prefix length short of
            // the whole message must surface as an error rather than a
            // truncated line an operator would read as complete.
            let mut refused = 0_usize;
            for budget in 0..64 {
                let mut sink = FailingSink { remaining: budget };
                if write!(sink, "{refusal}").is_err() {
                    refused += 1;
                }
            }
            assert!(
                refused > 0,
                "a sink that refuses must make the whole write fail — {full:?}"
            );
            assert!(
                refused < 64,
                "and a sink with room enough must succeed, or the loop is \
                 proving nothing"
            );
        }
    }

    /// An unverified row that nothing follows is open-ended, and the refusal
    /// says so in words rather than inventing an end.
    #[test]
    fn an_unverified_row_with_no_successor_is_open_ended() {
        const GAP: &str = "TEST ROW: the era after this one was never verified";
        let start = day(2040, 4, 1);
        let table = SessionTable {
            venue: Venue::NseDerivatives,
            anchor: SessionRow::verified(
                EPOCH_DAY,
                SESSION_OPEN_MINUTE,
                SESSION_CLOSE_MINUTE,
                SessionKind::Continuous,
                CHARTER_SESSION,
            ),
            later: [
                Some(SessionRow::unverified(start, SessionKind::Continuous, GAP)),
                None,
            ],
        };

        let refusal = table.hours_on(start).expect_err("the row has no hours");
        assert_eq!(refusal.verified_from(), None, "nothing follows it");
        let text = refusal.to_string();
        assert!(
            text.contains(" onward"),
            "an open-ended window says onward rather than naming an end it does \
             not have — got {text:?}"
        );

        let windows = table.refusal_windows();
        let named: Vec<_> = windows.into_iter().flatten().collect();
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].verified_from(), None);
    }

    /// The citation every row built for a test carries. Never a segment.
    const SOURCE: &str = "TEST ROW";

    /// A row starting on `start` with the charter's own hours.
    fn later(start: Day) -> SessionRow {
        SessionRow::verified(
            start,
            SESSION_OPEN_MINUTE,
            SESSION_CLOSE_MINUTE,
            SessionKind::Continuous,
            SOURCE,
        )
    }

    /// The anchor every table built for a test starts from.
    fn anchor_row() -> SessionRow {
        later(EPOCH_DAY)
    }

    /// A table, so the contract can be driven from outside the shipped rows.
    fn table(
        anchor: SessionRow,
        later: [Option<SessionRow>; MAX_LATER_SESSION_ROWS],
    ) -> SessionTable {
        SessionTable {
            venue: Venue::NseIndex,
            anchor,
            later,
        }
    }

    /// The ordering half of the contract: the anchor covers every day and the
    /// rows ascend with no hole, driven through each arm a shipped table never
    /// takes.
    #[test]
    fn the_table_shape_contract_refuses_a_table_whose_rows_do_not_ascend() {
        let anchor = anchor_row();

        assert!(
            table(anchor, [None, None]).is_shipping_shape(),
            "bare anchor"
        );
        assert!(
            table(anchor, [Some(later(day(2020, 1, 1))), None]).is_shipping_shape(),
            "one later row"
        );
        assert!(
            table(
                anchor,
                [Some(later(day(2020, 1, 1))), Some(later(day(2021, 1, 1)))]
            )
            .is_shipping_shape(),
            "two later rows, ascending"
        );

        // An anchor that does not start at the epoch leaves days uncovered.
        let late_anchor = SessionRow::verified(
            day(1971, 1, 1),
            SESSION_OPEN_MINUTE,
            SESSION_CLOSE_MINUTE,
            SessionKind::Continuous,
            SOURCE,
        );
        assert!(!table(late_anchor, [None, None]).anchor_covers_all_days());
        assert!(!table(late_anchor, [None, None]).is_shipping_shape());

        // Rows that do not ascend, in each of the three shapes that can fail.
        assert!(
            !table(anchor, [Some(anchor), None]).rows_ascend(),
            "equal starts"
        );
        assert!(
            !table(
                anchor,
                [Some(later(day(2021, 1, 1))), Some(later(day(2020, 1, 1)))]
            )
            .rows_ascend(),
            "descending later rows"
        );
        assert!(
            !table(anchor, [None, Some(later(day(2020, 1, 1)))]).rows_ascend(),
            "a hole before a populated slot — `rows()` would silently close it"
        );
    }

    /// The soundness half: a row is ill-shaped when its window is inverted, out
    /// of range, or its citation says nothing — and every slot is checked, not
    /// just the first.
    #[test]
    fn the_table_shape_contract_refuses_a_row_with_no_window_and_no_citation() {
        let anchor = anchor_row();
        let inverted = SessionRow::verified(
            day(2020, 1, 1),
            SESSION_CLOSE_MINUTE,
            SESSION_OPEN_MINUTE,
            SessionKind::Continuous,
            SOURCE,
        );
        let past_midnight = SessionRow::verified(
            day(2020, 1, 1),
            SESSION_OPEN_MINUTE,
            24 * 60 + 1,
            SessionKind::Continuous,
            SOURCE,
        );
        let uncited = SessionRow::verified(
            day(2020, 1, 1),
            SESSION_OPEN_MINUTE,
            SESSION_CLOSE_MINUTE,
            SessionKind::Continuous,
            "   ",
        );
        assert!(!inverted.is_well_shaped(), "close before open");
        assert!(!past_midnight.is_well_shaped(), "a close past midnight");
        assert!(!uncited.is_well_shaped(), "a blank citation is no citation");
        assert!(anchor.is_well_shaped());
        assert!(
            SessionRow::unverified(day(2020, 1, 1), SessionKind::Continuous, SOURCE)
                .is_well_shaped(),
            "an unverified row has no window to be wrong"
        );

        assert!(!table(uncited, [None, None]).rows_are_well_shaped());
        assert!(!table(anchor, [Some(inverted), None]).rows_are_well_shaped());
        assert!(
            !table(anchor, [Some(anchor), Some(inverted)]).rows_are_well_shaped(),
            "the SECOND slot is checked too"
        );
        assert!(
            !table(anchor, [None, Some(inverted)]).rows_are_well_shaped(),
            "and so is a second slot with a hole before it"
        );
        assert!(table(anchor, [Some(anchor), Some(anchor)]).rows_are_well_shaped());

        // The anchor must be session.rs's two constants, and an unverified
        // anchor can never be.
        assert!(table(anchor, [None, None]).anchor_matches_session_constants());
        let moved = SessionRow::verified(
            EPOCH_DAY,
            SESSION_OPEN_MINUTE + 1,
            SESSION_CLOSE_MINUTE,
            SessionKind::Continuous,
            SOURCE,
        );
        assert!(!table(moved, [None, None]).anchor_matches_session_constants());
        let unverified_anchor = SessionRow::unverified(EPOCH_DAY, SessionKind::Continuous, SOURCE);
        assert!(
            !table(unverified_anchor, [None, None]).anchor_matches_session_constants(),
            "an anchor with no hours cannot equal two constants"
        );
    }

    /// Every shipped table satisfies the contract at run time as well as at
    /// compile time, and `rows()` yields the anchor first.
    #[test]
    fn every_shipped_session_table_is_in_shipping_shape() {
        for venue in Venue::ALL {
            let table = venue.table();
            assert!(table.is_shipping_shape(), "{venue} ships a malformed table");
            assert!(table.anchor_matches_session_constants(), "{venue}");
            let rows: Vec<_> = table.rows().collect();
            assert!(!rows.is_empty(), "the anchor is always a row");
            assert_eq!(
                rows[0].start.days_from_epoch(),
                0,
                "the anchor comes first and starts at the epoch"
            );
            for pair in rows.windows(2) {
                assert!(
                    pair[0].start.days_from_epoch() < pair[1].start.days_from_epoch(),
                    "{venue}'s rows must ascend strictly"
                );
            }
        }
    }

    /// A session kind has a stable label, and both arms exist so an auction can
    /// be refused by name rather than mis-filtered as continuous trading.
    #[test]
    fn a_session_kind_has_a_label_for_each_arm() {
        assert_ne!(
            SessionKind::Continuous.label(),
            SessionKind::ClosingAuction.label()
        );
        for kind in [SessionKind::Continuous, SessionKind::ClosingAuction] {
            assert!(!kind.label().is_empty());
            assert_eq!(kind.to_string(), kind.label());
        }
    }

    // -- the transport rows --------------------------------------------------

    /// The verb and the auth prefix are data, one row each.
    #[test]
    fn the_wire_verb_and_the_auth_prefix_are_rows_not_branches() {
        assert_eq!(Method::Get.as_str(), "GET");
        assert_eq!(Method::Post.as_str(), "POST");
        assert!(
            AuthScheme::Raw.prefix().is_empty(),
            "a raw token has no prefix"
        );
        assert_eq!(AuthScheme::Bearer.prefix(), "Bearer ");
    }

    /// Row *i* of the table describes variant *i*, and a lookup is that index.
    #[test]
    fn the_descriptor_table_is_indexed_by_the_discriminant() {
        assert_eq!(DESCRIPTORS.len(), FEED_COUNT);
        assert_eq!(Feed::ALL.len(), FEED_COUNT);

        let mut wires = HashSet::with_capacity(FEED_COUNT);
        let mut names = HashSet::with_capacity(FEED_COUNT);
        for (index, feed) in Feed::ALL.into_iter().enumerate() {
            let row = feed.descriptor();
            assert_eq!(row.feed, feed, "row {index} describes another variant");
            assert_eq!(
                DESCRIPTORS[index], row,
                "the lookup is the discriminant, not a search"
            );
            assert!(wires.insert(feed.wire()), "two feeds share a wire name");
            assert!(
                names.insert(feed.display()),
                "two feeds share a display name"
            );
            assert_eq!(feed.display(), row.display);
            assert_eq!(feed.wire(), row.wire);
            assert_eq!(feed.to_string(), row.display);
            assert!(
                !row.granularities.is_empty() && !row.segments.is_empty(),
                "{feed} could never answer a request"
            );
            assert_eq!(row.exchange, Exchange::Nse);
        }
        assert_eq!(wires.len(), FEED_COUNT);
    }

    /// The bridge to the store's own vendor prefix refuses rather than invents.
    #[test]
    fn a_feed_with_no_store_prefix_refuses_instead_of_borrowing_one() {
        assert_eq!(Feed::Dhan.store_vendor(), Some(Vendor::Dhan));
        assert_eq!(Feed::Groww.store_vendor(), Some(Vendor::Groww));
        for archive in [Feed::TrueData, Feed::Gdfl] {
            assert_eq!(
                archive.store_vendor(),
                None,
                "filing an archive vendor's bars under a broker's prefix would \
                 destroy D-0019's per-vendor independence irreversibly"
            );
        }
    }

    /// A feed serves the rungs its row names and no others.
    #[test]
    fn a_feed_serves_only_the_rungs_its_row_declares() {
        for feed in Feed::ALL {
            let declared = feed.descriptor().granularities;
            for rung in Granularity::ALL {
                assert_eq!(
                    feed.serves(rung),
                    declared.contains(rung),
                    "{feed} disagrees with its own row about {rung}"
                );
            }
        }
        assert!(Feed::Dhan.serves(Granularity::Minute1));
        assert!(Feed::Dhan.serves(Granularity::Day1));
        assert!(
            !Feed::Dhan.serves(Granularity::Minute5),
            "an unserved rung refuses by name; the wider ladder was never read live"
        );
        assert!(Feed::Gdfl.serves(Granularity::Second1));
        assert!(!Feed::Gdfl.serves(Granularity::Minute1));
    }

    /// The HTTP half of a transport, or [`None`] for an archive.
    ///
    /// A function taking BOTH arms rather than a `let … else { panic!() }` at
    /// each call site: the `else` of a let-else that never fires is a branch no
    /// test can reach, and a test file full of unreachable arms is exactly the
    /// hole this workflow exists to close. Called below for every feed, so both
    /// arms are taken.
    const fn http_spec(transport: Transport) -> Option<HttpSpec> {
        match transport {
            Transport::Http(spec) => Some(spec),
            Transport::LocalArchive(_) => None,
        }
    }

    /// The archive half, same shape and same reason.
    const fn archive_spec(transport: Transport) -> Option<ArchiveSpec> {
        match transport {
            Transport::LocalArchive(spec) => Some(spec),
            Transport::Http(_) => None,
        }
    }

    /// The envelope key a response shape hangs its bars under, whichever shape
    /// it is.
    const fn envelope_of(shape: ResponseShape) -> Option<&'static str> {
        match shape {
            ResponseShape::ParallelArrays { envelope }
            | ResponseShape::ArrayOfObjects { envelope } => envelope,
        }
    }

    /// How an archive's file name renders its date, whichever pattern it uses.
    const fn archive_date(name: ArchiveName) -> DateFormat {
        match name {
            ArchiveName::SegmentAndDate { date, .. } | ArchiveName::DateOnly { date, .. } => date,
        }
    }

    /// A member's extension, whichever nesting it sits in.
    const fn member_suffix(member: MemberPattern) -> &'static str {
        match member {
            MemberPattern::SymbolAtRoot { suffix } | MemberPattern::StemGroupSymbol { suffix } => {
                suffix
            }
        }
    }

    /// An HTTP row carries a credential and a governor; an archive row has
    /// nowhere to put either.
    #[test]
    fn only_an_http_transport_needs_a_credential_and_a_governor() {
        let mut http = 0_usize;
        let mut archives = 0_usize;
        for feed in Feed::ALL {
            let transport = feed.descriptor().transport;
            assert!(!transport.label().is_empty());
            assert_eq!(
                transport.needs_credential(),
                transport.needs_governor(),
                "{feed}: a token and a ceiling are needed by the same transports"
            );
            assert_eq!(
                archive_spec(transport).is_some(),
                !transport.needs_credential(),
                "{feed}: a transport is one of the two, never both and never neither"
            );
            if let Some(spec) = http_spec(transport) {
                assert!(transport.needs_credential());
                assert!(!spec.auth.header.is_empty(), "{feed} names no auth header");
                assert!(spec.base_url.starts_with("https://"), "{feed}");
                assert!(spec.bars_path.starts_with('/'), "{feed}");
                http += 1;
            } else {
                let spec =
                    archive_spec(transport).expect("a transport that is not HTTP is an archive");
                assert!(
                    !transport.needs_credential(),
                    "a local file has no rate limit and needs no credential"
                );
                assert!(!spec.layouts.is_empty(), "{feed} decodes nothing");
                archives += 1;
            }
        }
        assert_eq!((http, archives), (2, 2), "two brokers, two archive vendors");
        assert_ne!(
            Feed::Dhan.descriptor().transport.label(),
            Feed::Gdfl.descriptor().transport.label()
        );
    }

    /// The two brokers differ in every field this module exists to make data.
    #[test]
    fn the_two_http_rows_disagree_in_every_field_that_used_to_be_a_branch() {
        let dhan = http_spec(Feed::Dhan.descriptor().transport)
            .expect("the secondary broker is an HTTP feed");
        let groww = http_spec(Feed::Groww.descriptor().transport)
            .expect("the primary broker is an HTTP feed");

        assert_ne!(dhan.method, groww.method, "POST against GET");
        assert_ne!(dhan.auth.header, groww.auth.header);
        assert_ne!(dhan.auth.scheme, groww.auth.scheme);
        assert_eq!(
            dhan.range_end,
            RangeEnd::Exclusive,
            "toDate is NOT inclusive — one field, one conversion site"
        );
        assert_eq!(groww.range_end, RangeEnd::Inclusive);
        assert_ne!(dhan.timestamps, groww.timestamps, "seconds against millis");
        assert_ne!(dhan.pooling, groww.pooling);
        assert_eq!(dhan.date_format, DateFormat::DashedYmd);
        assert_eq!(dhan.prices, PriceScale::Rupees);
        assert_eq!(groww.prices, PriceScale::Rupees);

        assert_eq!(
            dhan.response,
            ResponseShape::ParallelArrays { envelope: None },
            "seven arrays at the top level, which is the shape `zip` silently \
             truncates"
        );
        let envelope = envelope_of(groww.response)
            .expect("the primary broker nests its bars under an envelope");
        assert!(!envelope.is_empty(), "an envelope key of nothing is no key");
        assert_eq!(
            groww.response,
            ResponseShape::ArrayOfObjects {
                envelope: Some(envelope),
            },
            "one object per bar, under that envelope"
        );
        assert_eq!(
            envelope_of(dhan.response),
            None,
            "and the other broker's arrays are at the top level"
        );

        assert!(dhan.fields.open_interest.is_some());
        assert_eq!(
            groww.fields.open_interest, None,
            "absent open interest becomes the null sentinel; zero means zero"
        );
        for names in [dhan.fields, groww.fields] {
            for field in [
                names.open,
                names.high,
                names.low,
                names.close,
                names.volume,
                names.timestamp,
            ] {
                assert!(!field.is_empty());
            }
        }

        assert_eq!(dhan.budget.per_second, Some(crate::rate::DHAN_PER_SECOND));
        assert_eq!(
            dhan.budget.per_minute, None,
            "no published per-minute bound"
        );
        assert_eq!(dhan.budget.per_day, Some(crate::rate::DHAN_PER_DAY));
        assert_eq!(groww.budget.per_minute, Some(crate::rate::GROWW_PER_MINUTE));
        assert_eq!(
            groww.budget.per_second,
            Some(crate::rate::GROWW_PER_SECOND_UNVERIFIED),
            "the constant's own name says the figure is not confirmed, and it \
             is not quietly promoted here"
        );
        assert_eq!(groww.budget.per_day, None);
    }

    /// An archive row addresses its files by pattern, and a segment nobody
    /// measured is refused rather than decoded against a guessed layout.
    #[test]
    fn an_unmeasured_segment_layout_is_refused_by_name() {
        let truedata = archive_spec(Feed::TrueData.descriptor().transport)
            .expect("this feed is a folder of archives");
        let gdfl = archive_spec(Feed::Gdfl.descriptor().transport)
            .expect("this feed is a folder of archives");

        let index = truedata
            .layout(Segment::Index)
            .expect("the index layout was measured");
        assert_eq!(index.segment, Segment::Index);
        assert_eq!(
            index.columns.len(),
            5,
            "five columns, measured not documented"
        );
        assert_eq!(
            truedata.layout(Segment::Fno),
            None,
            "the futures archives were measured at nine columns and their \
             MEANINGS were never established — a missing layout refuses"
        );
        assert_eq!(truedata.layout(Segment::Cash), None);

        let fno = gdfl
            .layout(Segment::Fno)
            .expect("the F&O layout was measured");
        assert_eq!(fno.columns.len(), 10);
        assert_eq!(gdfl.layout(Segment::Index), None);

        // Group folders and segment tokens, both directions.
        assert_eq!(truedata.group_folder(ArchiveGroup::Options), None);
        assert_eq!(truedata.group_folder(ArchiveGroup::Flat), None);
        assert!(gdfl.group_folder(ArchiveGroup::Options).is_some());
        assert!(gdfl.group_folder(ArchiveGroup::Futures).is_some());
        assert_eq!(
            gdfl.group_folder(ArchiveGroup::Flat),
            None,
            "the archive is not flat, so there is no flat folder"
        );
        assert!(truedata.segment_token(Segment::Index).is_some());
        assert_eq!(
            truedata.segment_token(Segment::Fno),
            None,
            "the archives carry both futures and options tokens, and one token \
             for the pair would be wrong half the time"
        );
        assert_eq!(gdfl.segment_token(Segment::Fno), None);

        assert_eq!(truedata.header, HeaderRow::Absent);
        assert_eq!(gdfl.header, HeaderRow::Present(GDFL_HEADER));
        assert_eq!(truedata.delimiter, b',');
        assert_eq!(gdfl.delimiter, b',');
        assert_eq!(truedata.nesting, Nesting::ZipOfDailyZips);
        assert_eq!(gdfl.nesting, Nesting::ZipOfSegmentFolders);
        assert_eq!(
            gdfl.date_format,
            DateFormat::SlashedDmy,
            "01/07/2025 is 1 July, and reading it the other way shifts every \
             bar by months"
        );
        assert_eq!(truedata.date_format, DateFormat::CompactYmd);
        assert_eq!(truedata.prices, PriceScale::Rupees);
        assert_eq!(gdfl.prices, PriceScale::Rupees);

        // The two archives are addressed by DIFFERENT patterns, which is the
        // whole reason the pattern is a field. Compared by discriminant so the
        // assertion does not have to spell a file-name prefix out.
        assert_ne!(
            std::mem::discriminant(&truedata.archive),
            std::mem::discriminant(&gdfl.archive),
            "one names its segment in the file name and the other only its date"
        );
        assert_ne!(
            std::mem::discriminant(&truedata.member),
            std::mem::discriminant(&gdfl.member),
            "one member sits at the archive root and the other under two folders"
        );
        assert_eq!(archive_date(truedata.archive), DateFormat::CompactYmd);
        assert_eq!(
            archive_date(gdfl.archive),
            DateFormat::CompactDmy,
            "DDMMYYYY in the archive name, DD/MM/YYYY inside the file — reading \
             either the other way shifts every bar by months"
        );
        assert_ne!(
            archive_date(gdfl.archive),
            gdfl.date_format,
            "and those two are deliberately not the same format"
        );
        for member in [truedata.member, gdfl.member] {
            let suffix = member_suffix(member);
            assert!(suffix.starts_with('.'), "an extension begins with a dot");
        }
    }

    /// Both archive feeds are snapshots, both brokers are OHLCV, and the
    /// distinction is what keeps a snapshot out of the bar format.
    #[test]
    fn a_snapshot_feed_is_never_declared_as_a_bar_feed() {
        assert_eq!(Feed::Dhan.descriptor().record, RecordShape::Ohlcv);
        assert_eq!(Feed::Groww.descriptor().record, RecordShape::Ohlcv);
        assert_eq!(Feed::TrueData.descriptor().record, RecordShape::Snapshot);
        assert_eq!(Feed::Gdfl.descriptor().record, RecordShape::Snapshot);
        assert_ne!(RecordShape::Ohlcv, RecordShape::Snapshot);
    }

    /// The column vocabulary, and the honest arm inside it.
    #[test]
    fn the_column_vocabulary_carries_an_arm_for_a_meaning_nobody_established() {
        let every = [
            Column::Ticker,
            Column::Date,
            Column::Time,
            Column::LastPrice,
            Column::Open,
            Column::High,
            Column::Low,
            Column::Close,
            Column::Volume,
            Column::OpenInterest,
            Column::BidPrice,
            Column::BidQty,
            Column::AskPrice,
            Column::AskQty,
            Column::LastQty,
            Column::Ignored,
            Column::Unverified,
        ];
        let distinct: HashSet<_> = every.iter().collect();
        assert_eq!(distinct.len(), every.len(), "every column means one thing");
        assert!(
            !TRUEDATA_INDEX.columns.contains(&Column::Volume)
                && !TRUEDATA_INDEX.columns.contains(&Column::OpenInterest),
            "an index has neither, so the two trailing zeros are Ignored rather \
             than mapped onto fields it does not have"
        );
        assert!(GDFL_FNO.columns.contains(&Column::OpenInterest));
        assert_eq!(
            GDFL_FNO.columns.len(),
            GDFL_HEADER.split(',').count(),
            "the layout and the header row it was read from agree in width"
        );
    }

    /// The values this module hands around are inspectable, comparable and
    /// hashable — a refusal nobody can print is a refusal nobody can act on.
    #[test]
    fn the_reported_values_can_be_printed_compared_and_hashed() {
        let session = Venue::NseIndex
            .hours_on(day(2024, 1, 2))
            .expect("the anchor is verified");
        let refusal = SessionRefusal {
            venue: Venue::NseIndex,
            day: day(2030, 1, 1),
            row_start: day(2029, 1, 1),
            verified_from: None,
            source: "TEST ROW",
        };
        let window = RefusalWindow {
            venue: Venue::NseIndex,
            start: day(2029, 1, 1),
            verified_from: None,
        };

        let printed = format!(
            "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            Granularity::Minute1,
            Granularity::Minute1.grid(),
            GranularitySet::EMPTY,
            SegmentSet::EMPTY,
            Venue::NseIndex,
            SessionKind::Continuous,
            Hours::Unverified,
            session,
            session.expected_count(Granularity::Tick),
            refusal,
            window,
            Feed::Dhan,
            Feed::Dhan.descriptor(),
            Method::Get,
            AuthScheme::Raw,
            DateFormat::DashedYmd,
            RangeEnd::Inclusive,
            TimestampEncoding::EpochSecondsUtc,
            PriceScale::Paisa,
            Pooling::PerVendor,
        );
        assert!(!printed.is_empty(), "every reported value is inspectable");

        let more = format!(
            "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            ResponseShape::ParallelArrays { envelope: None },
            Nesting::ZipOfDailyZips,
            ArchiveName::DateOnly {
                prefix: "",
                suffix: "",
                date: DateFormat::CompactDmy,
            },
            MemberPattern::SymbolAtRoot { suffix: "" },
            ArchiveGroup::Flat,
            HeaderRow::Absent,
            Column::Ticker,
            TRUEDATA_INDEX,
            RecordShape::Ohlcv,
            Feed::Dhan.descriptor().transport,
            SessionRow::verified(
                EPOCH_DAY,
                SESSION_OPEN_MINUTE,
                SESSION_CLOSE_MINUTE,
                SessionKind::Continuous,
                "TEST ROW",
            ),
        );
        assert!(!more.is_empty());

        // Ord and Hash, which the sets and the tables both rely on.
        assert!(Granularity::Tick < Granularity::Week1, "coarsest last");
        assert!(Venue::NseIndex < Venue::NseDerivatives);
        assert!(Feed::Dhan < Feed::Gdfl);
        let mut seen = HashSet::with_capacity(4);
        assert!(seen.insert(Feed::Dhan));
        assert!(!seen.insert(Feed::Dhan), "the same feed hashes the same");
        assert!(seen.insert(Feed::Groww));
        let mut spans = HashSet::with_capacity(2);
        assert!(spans.insert(session), "a session hashes");
        assert!(!spans.insert(session), "and the same one hashes the same");
        assert_eq!(
            refusal.venue(),
            window.venue(),
            "both reported values name the venue they came from"
        );
        let cloned = *Feed::Dhan.descriptor();
        assert_eq!(cloned, *Feed::Dhan.descriptor(), "a row is Copy and equal");
    }
}
