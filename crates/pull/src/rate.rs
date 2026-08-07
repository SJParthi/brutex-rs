//! The adaptive rate governor: the rate a vendor **actually** honours,
//! converged on from both directions.
//!
//! # The question this exists to answer
//!
//! *May I issue a request right now, and if not, when?* A vendor publishes a
//! number and that number is a claim, not a promise. `docs/00-charter.md` §4
//! records one vendor's per-second cap as **UNVERIFIED** with the note that
//! *"the published 10/s applies to a different endpoint group"* and that the
//! production ceiling of 8/s is *"chosen not measured"* — which is this whole
//! module's premise written down before the module existed. A governor pinned
//! to a published figure is a governor that discovers the truth by being
//! refused, and pays for every discovery with a `429`.
//!
//! So the published figure is used for exactly one thing: an **upper bound the
//! allowance may never pass**. Everything below it is earned. One success buys
//! one permit back; one refusal halves the allowance. The allowance therefore
//! walks *toward* whatever the vendor is honouring today, from above after a
//! refusal and from below after a recovery, and it never walks past what the
//! vendor published.
//!
//! # Why AIMD, stated as an argument rather than asserted
//!
//! Additive increase, multiplicative decrease — the rule TCP congestion control
//! uses, taken for the reason TCP takes it (Chiu & Jain, 1989). Consider *n*
//! clients sharing one vendor budget. The state is the vector of allowances
//! `(x₁ … xₙ)`; the **efficiency line** is `Σxᵢ = X*`, the rate the vendor
//! actually honours; the **fairness line** is `x₁ = … = xₙ`. A control is an
//! increase rule applied when no refusal is seen and a decrease rule applied
//! when one is. There are four combinations and only one of them converges:
//!
//! | Control | Increase moves | Decrease moves | What is invariant | Converges |
//! |---|---|---|---|---|
//! | AIAD | translation along the diagonal | translation along the diagonal | the *difference* `xᵢ − xⱼ` | no |
//! | MIMD | scaling from the origin | scaling from the origin | the *ratio* `xᵢ / xⱼ` | no |
//! | MIAD | scaling from the origin | translation along the diagonal | — (spread grows) | diverges |
//! | **AIMD** | translation along the diagonal | scaling from the origin | — | **yes** |
//!
//! An additive step adds the same constant to every allowance, so it preserves
//! the difference and shrinks the ratio toward 1 — it moves the state *parallel*
//! to the fairness line. A multiplicative step multiplies every allowance by the
//! same factor, so it preserves the ratio and shrinks the difference by that
//! factor — it moves the state *along the ray through the origin*, which crosses
//! the fairness line. AIAD and MIMD each hold one of the two quantities fixed
//! forever, so whatever unfairness the system starts with it keeps; the state
//! oscillates on a line that never touches the fairness line. AIMD is the only
//! combination whose composite cycle is a strict contraction: each refusal
//! multiplies `|xᵢ − xⱼ|` by the decrease factor while the additive phase
//! restores the sum, so the distance to the fairness line falls geometrically
//! and the fixed point is the intersection of the two lines. That is the
//! convergence, and it is a property of the *pair* of rules; neither half has it
//! alone.
//!
//! The second reason is the one an operator feels, and it is about cost rather
//! than fairness. From an allowance of `c`, one refusal costs `c/2` permits in a
//! single step and regaining them costs `c/2` successes. A client that is far
//! over the honoured rate therefore loses more from a refusal than a client that
//! is barely over — the penalty scales with the offence. Under a symmetric rule
//! (AIAD) a refusal costs one permit no matter how far over the client was, so a
//! client at ten times the honoured rate must be refused `9c/10` times to come
//! down, and it emits every one of those refusals *at the vendor*. The
//! asymmetry is not a tuning preference: it is what drives the refusal rate
//! itself to zero. See `docs/05-decisions.md` D-0037.
//!
//! # Permits per window, not microseconds between requests
//!
//! Two units were available and only one of them can express what these vendors
//! publish.
//!
//! An **inter-request interval** — "wait 200,000 µs between calls" — is a
//! pacing rule. It cannot express a daily quota: `docs/00-charter.md` §4 records
//! 100,000 requests per day for the secondary vendor, and a pull that runs for
//! ten minutes and stops is entitled to spend that budget in ten minutes. Spread
//! as an interval it becomes 864,000 µs between calls and the pull takes a day.
//! A quota is a *budget over a span*, not a spacing.
//!
//! Worse, several spans are enforced at once, and a single interval cannot
//! represent several spans without dividing one by another — which is a
//! rounding, and a rounding in this direction issues above the vendor's number.
//! `clippy::float_arithmetic` is denied workspace-wide and this is one of the
//! places where the denial is doing real work rather than being a house style:
//! there is no representable "5 requests per second" in binary floating point
//! whose reciprocal is exactly 200,000 µs.
//!
//! So the unit is **integer permits per window**, and the accounting is done in
//! *micro-permits* — an integer credit where one permit costs `len_micros` and
//! the window earns `permitted` micro-permits per elapsed microsecond. Both
//! sides of that trade are exact integers, no division happens on the earning
//! path at all, and the only division anywhere is the `div_ceil` that computes
//! how long a denied caller must wait — which rounds **up**, i.e. toward
//! issuing less.
//!
//! # A token bucket, because a sliding window is not O(1) space
//!
//! A sliding window that answers *"how many requests in the last second"*
//! exactly must remember when each of those requests happened. That is O(c)
//! timestamps for a ceiling of `c`, which for the 100,000-per-day window is a
//! 100,000-element ring — and its space is set by the vendor's number rather
//! than by anything this code chooses. A fixed-slot ring is O(slots) and buys
//! only an approximation of the sliding count.
//!
//! [`Window`] is a token bucket instead: one `u64` of credit, one `u32` of
//! allowance, one `u32` of ceiling, one span tag. **The space bound is proved
//! rather than argued** — [`Governor`] is `Copy`, and a `Copy` type cannot own
//! an allocation, so its whole state is its `size_of`, which the compile-time
//! assertion below pins under 128 bytes. Nothing in this file allocates, grows,
//! or retains anything per request; a governor that has admitted ten million
//! requests is byte-identical in size to one that has admitted none.
//!
//! What a bucket gives up against an exact sliding window is written down in
//! `docs/06-limits.md` §21: over an interval of length `T` the bucket admits at
//! most `capacity + rate·T`, so a caller that has been idle can spend a full
//! bucket at once and then earn at the sustained rate. The long-run rate never
//! exceeds `permitted` per window; the instantaneous burst is bounded by the
//! bucket, and the bucket is the allowance. That bound is stated, not hidden.
//!
//! # Several windows at once, in a fixed array
//!
//! `docs/00-charter.md` §4 gives one vendor a per-second cap and a daily quota
//! and **no per-minute governor**, and the other a per-minute cap and **no daily
//! quota**. So the spans that exist are per-second, per-minute and per-day, all
//! three may be live at once, and an absent one is a recorded fact rather than a
//! default. [`Governor`] therefore holds `[Option<Window>; WINDOW_COUNT]` — a
//! fixed array of exactly three, never a `Vec` — and every window must admit
//! before any window is charged. A request that the per-second window would
//! allow and the per-minute window would not is denied, and the refusal names
//! **which** span bound it.
//!
//! # No clock, and that is the point
//!
//! Nothing here reads a clock. Time arrives as a monotonic microsecond value on
//! [`Governor::admit`], which is what makes every test in
//! `crates/pull/tests/unit.rs` deterministic and makes none of them sleep. A
//! test that sleeps to observe a rate limiter is a test that is slow when it
//! passes and flaky when the host is loaded, and it proves the least interesting
//! case — the one where time really did pass.
//!
//! A caller's clock can still go backwards (an NTP step, a suspended laptop, a
//! virtualised TSC). [`Governor::admit`] clamps its cursor forward-only: a
//! `now` below the cursor yields zero elapsed and leaves the cursor where it
//! was, so the blip grants no capacity and the recovery afterwards is measured
//! from the highest instant ever seen rather than from the dip. It cannot panic,
//! because every arithmetic step on that path is saturating.
//! `pull::unit::a_clock_that_goes_backwards_grants_nothing` holds that up.
//!
//! # No network
//!
//! This file is arithmetic. It issues nothing, it knows no URL, and it takes no
//! HTTP dependency — the caller tells it that a request succeeded or was
//! throttled, exactly as `crates/store` is told what to write rather than
//! writing it.

use brutex_core::vendor::Vendor;
use std::fmt;

/// Spans a vendor may bound at the same time — three, and exactly three.
///
/// Not a growable list: `docs/00-charter.md` §4 records per-second, per-minute
/// and per-day figures and nothing else, and a fourth span would be a new
/// external fact that `CLAUDE.md` §3 rule 1 requires be recorded there first.
pub const WINDOW_COUNT: usize = 3;

/// The largest per-window ceiling this build accepts.
///
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary. The
/// bound is not decorative: a window's full bucket is `ceiling · len_micros`
/// micro-permits, and the longest span is a day at 86,400,000,000 µs, so a
/// `u32` ceiling taken at face value would overflow a `u64` credit. At this
/// limit the widest product is 8.64 × 10¹⁶, which is 0.47 % of `u64::MAX`.
///
/// The largest figure the charter records is 100,000 per day, so this is 10×
/// the measured need. The refusal names the number, so an operator whose vendor
/// genuinely publishes more is told what to raise.
pub const MAX_CEILING: u32 = 1_000_000;

/// Microseconds in the shortest span this governor bounds.
pub const MICROS_PER_SECOND: u64 = 1_000_000;

/// Microseconds in a minute.
pub const MICROS_PER_MINUTE: u64 = 60 * MICROS_PER_SECOND;

/// Microseconds in a day.
pub const MICROS_PER_DAY: u64 = 86_400 * MICROS_PER_SECOND;

const _: () = assert!(MICROS_PER_MINUTE == 60_000_000);
const _: () = assert!(MICROS_PER_DAY == 86_400_000_000);
const _: () = assert!(MAX_CEILING as u64 * MICROS_PER_DAY < u64::MAX / 100);

// ---------------------------------------------------------------------------
// The published figures, each carrying the evidence lane it has in the charter
// ---------------------------------------------------------------------------

/// The secondary vendor's per-second cap: **5**.
///
/// `docs/00-charter.md` §4, evidence lane **documented**. This is the one
/// vendor figure in this module that is neither operator-assertion nor
/// unverified.
pub const DHAN_PER_SECOND: u32 = 5;

/// The secondary vendor's daily quota: **100,000**.
///
/// `docs/00-charter.md` §4, evidence lane **documented**. The same row records
/// *"no per-minute governor"*, which is why a governor built from these two
/// figures passes `None` for the minute span rather than inventing one.
pub const DHAN_PER_DAY: u32 = 100_000;

/// The primary vendor's per-minute cap: **500**.
///
/// `docs/00-charter.md` §4, evidence lane **operator-confirmed, not published**.
/// The same row records *"No daily quota"*.
///
/// **This is not the 300 per minute an operator may quote from a vendor
/// changelog.** The charter is this repository's authority on every external
/// fact (`CLAUDE.md` §3 rule 1), a figure that disagrees with it is a charter
/// amendment rather than a constant, and the two differ by 40 % — which is the
/// difference between a pull that finishes and a pull that is throttled all
/// day. `docs/06-limits.md` §21 records the disagreement instead of resolving
/// it here.
pub const GROWW_PER_MINUTE: u32 = 500;

/// The primary vendor's per-second cap: **8, and UNVERIFIED**.
///
/// `docs/00-charter.md` §4 states it in full: *"UNVERIFIED. The published 10/s
/// applies to a different endpoint group. Production ceiling is 8/s, chosen not
/// measured."* The name carries the lane so that no caller can take it for a
/// documented figure by reading the call site alone.
///
/// This constant is the single strongest argument for the whole module. A
/// number that was chosen rather than measured is exactly a number the governor
/// must be free to walk away from in both directions.
pub const GROWW_PER_SECOND_UNVERIFIED: u32 = 8;

/// Which span a window bounds.
///
/// A tag rather than a stored duration, so that two windows of the same span
/// cannot be constructed with different lengths — the length is a function of
/// the tag and there is one definition of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WindowSpan {
    /// One second.
    Second,
    /// One minute.
    Minute,
    /// One day.
    Day,
}

impl WindowSpan {
    /// Every span, shortest first. The order a refusal is tie-broken in.
    pub const ALL: [Self; WINDOW_COUNT] = [Self::Second, Self::Minute, Self::Day];

    /// This span's length in microseconds, and the micro-permit cost of one
    /// permit inside it.
    ///
    /// Those are the same number on purpose: a window earns `permitted`
    /// micro-permits per elapsed microsecond, so `len_micros` of elapsed time
    /// earns exactly `permitted` permits. The identity is what removes division
    /// from the earning path entirely.
    #[must_use]
    pub const fn len_micros(self) -> u64 {
        match self {
            Self::Second => MICROS_PER_SECOND,
            Self::Minute => MICROS_PER_MINUTE,
            Self::Day => MICROS_PER_DAY,
        }
    }
}

impl fmt::Display for WindowSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Second => f.write_str("second"),
            Self::Minute => f.write_str("minute"),
            Self::Day => f.write_str("day"),
        }
    }
}

/// Why a governor was refused at construction.
///
/// Both arms are about the *ceiling*, because the ceiling is the only untrusted
/// input this type takes: everything else is a span tag or a clock reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GovernorError {
    /// A span was given a ceiling of zero.
    ///
    /// Refused rather than treated as "no window": a vendor that permits zero
    /// requests is a vendor that cannot be pulled from at all, and silently
    /// reading that as *absent* would turn a configuration mistake into an
    /// unbounded pull. Absence is spelled `None`, which is a different thing
    /// and says so.
    CeilingIsZero {
        /// The span whose ceiling was zero.
        span: WindowSpan,
    },
    /// A span's ceiling is past [`MAX_CEILING`].
    CeilingOutOfRange {
        /// The span whose ceiling was refused.
        span: WindowSpan,
        /// The ceiling that was refused.
        ceiling: u32,
        /// The bound.
        limit: u32,
    },
}

impl fmt::Display for GovernorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::CeilingIsZero { span } => {
                write!(f, "the per-{span} ceiling is zero; absence is not zero")
            }
            Self::CeilingOutOfRange {
                span,
                ceiling,
                limit,
            } => write!(
                f,
                "the per-{span} ceiling {ceiling} is past the {limit} this build holds"
            ),
        }
    }
}

impl std::error::Error for GovernorError {}

/// What [`Governor::admit`] decided.
///
/// A refusal carries **which** span bound it and **how long** the caller must
/// wait, because a governor that only says "no" leaves the caller to guess, and
/// every guess is either a busy loop or a sleep longer than it needed to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// Issue the request. A permit has been charged against every live window.
    Admit,
    /// Do not issue the request.
    Deny {
        /// The span that bound this decision — the one with the longest wait.
        span: WindowSpan,
        /// The smallest number of microseconds after which the same call would
        /// be admitted, assuming nothing else is charged in between.
        ///
        /// Minimal, not merely sufficient: waiting one microsecond less earns
        /// strictly fewer micro-permits than the shortfall.
        /// `pull::unit::a_denial_names_the_exact_wait_that_clears_it` asserts
        /// both halves of that.
        wait_micros: u64,
    },
}

/// One span's token bucket and its AIMD allowance.
///
/// Four numbers and a tag. There is no history, no ring and no timestamp list:
/// see the module header on why an exact sliding window is not O(1) space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    span: WindowSpan,
    ceiling: u32,
    permitted: u32,
    credit: u64,
}

impl Window {
    /// One window for `span`, or `None` when the vendor publishes no bound on
    /// that span.
    ///
    /// The bucket starts **full**. A governor built for a pull that has not
    /// started yet has by definition not spent anything, and starting it empty
    /// would make the first request of every pull wait for a budget nobody had
    /// consumed.
    const fn new(span: WindowSpan, ceiling: Option<u32>) -> Result<Option<Self>, GovernorError> {
        let Some(ceiling) = ceiling else {
            return Ok(None);
        };
        if ceiling == 0 {
            return Err(GovernorError::CeilingIsZero { span });
        }
        if ceiling > MAX_CEILING {
            return Err(GovernorError::CeilingOutOfRange {
                span,
                ceiling,
                limit: MAX_CEILING,
            });
        }
        Ok(Some(Self {
            span,
            ceiling,
            permitted: ceiling,
            // `as u64` rather than `u64::from`, and for the reason
            // `session::Day::new` gives four hundred lines away: `From` is not
            // const-stable on this toolchain, and this constructor is `const`
            // so a vendor's governor can be built in a `const` item. The widen
            // cannot truncate — `MAX_CEILING` is checked one line above — so
            // `cast_possible_truncation` is not in play.
            credit: ceiling as u64 * span.len_micros(),
        }))
    }

    /// Micro-permits a full bucket holds at the current allowance.
    ///
    /// Cannot overflow: `permitted <= ceiling <= MAX_CEILING`, and the
    /// compile-time assertion beside [`MAX_CEILING`] proves the widest product
    /// is under a hundredth of `u64::MAX`.
    const fn capacity(self) -> u64 {
        self.permitted as u64 * self.span.len_micros()
    }

    /// Earns `elapsed` microseconds' worth of micro-permits, clamped to a full
    /// bucket.
    ///
    /// **Saturating, and still exact.** `elapsed · permitted` can overflow a
    /// `u64` when a caller hands over a clock reading near `u64::MAX` on the
    /// first call. Saturation only happens when the true product is at or above
    /// `u64::MAX`, which is far above `capacity()`, so the `min` that follows
    /// returns `capacity()` — which is exactly what the un-saturated
    /// computation would have been clamped to. The clamp makes the saturation
    /// unobservable rather than merely survivable, and
    /// `pull::unit::a_clock_at_the_far_end_of_u64_does_not_wrap` is where that
    /// is checked rather than believed.
    fn earn(&mut self, elapsed: u64) {
        let earned = elapsed.saturating_mul(u64::from(self.permitted));
        self.credit = self.credit.saturating_add(earned).min(self.capacity());
    }

    /// `None` if a permit is affordable now; otherwise the minimal wait.
    ///
    /// `deficit.div_ceil(permitted)` is the smallest `w` with
    /// `credit + w·permitted >= cost`. It rounds **up**, i.e. toward issuing
    /// less, which is the only safe direction to round in a rate limiter.
    /// `permitted` is never zero — [`Window::relax`] floors it at one — so
    /// there is no division-by-zero arm to leave untested.
    fn shortfall(self) -> Option<u64> {
        let cost = self.span.len_micros();
        if self.credit >= cost {
            return None;
        }
        Some((cost - self.credit).div_ceil(u64::from(self.permitted)))
    }

    /// Charges one permit.
    ///
    /// Plain subtraction rather than `checked_sub`, and deliberately: every
    /// caller has already had [`Window::shortfall`] return `None` for this
    /// window in the same call, with no time advanced in between, so
    /// `credit >= cost` holds. A `checked_sub` here would add a failure arm no
    /// input could reach — a branch nobody has checked, sitting behind a
    /// coverage gate that would still report 100 %. It is the same argument
    /// `Manifest::record` makes about its row subtraction.
    fn charge(&mut self) {
        self.credit -= self.span.len_micros();
    }

    /// Additive increase: one permit back, never past the published ceiling.
    ///
    /// `+1` cannot overflow — `permitted <= ceiling <= MAX_CEILING`, which is
    /// far below `u32::MAX` — and the `min` is what makes the published figure
    /// an upper bound rather than a target.
    fn tighten_toward_ceiling(&mut self) {
        self.permitted = (self.permitted + 1).min(self.ceiling);
    }

    /// Multiplicative decrease: halve the allowance, floor at one, drain the
    /// bucket.
    ///
    /// **The floor is one, never zero.** An allowance of zero is an absorbing
    /// state — no request is ever admitted, so no success is ever observed, so
    /// nothing ever raises it again. A governor that can reach it is a governor
    /// that can permanently stop a pull on one bad minute, and the recovery
    /// would look exactly like a hung process.
    ///
    /// **The bucket is drained as well as the allowance halved.** A refusal
    /// says the budget the governor believed in was wrong; leaving the credit
    /// in place would let the caller spend the disproven budget immediately
    /// after being told it does not exist.
    fn relax(&mut self) {
        self.permitted = (self.permitted / 2).max(1);
        self.credit = 0;
    }
}

/// One vendor's governor for one request kind.
///
/// `Copy`, and that is the space proof: a `Copy` type owns no allocation, so
/// the whole of this governor's state is its `size_of` — a constant, asserted
/// below, independent of how many requests it has admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Governor {
    windows: [Option<Window>; WINDOW_COUNT],
    cursor_micros: u64,
}

const _: () = assert!(size_of::<Governor>() <= 128);

impl Governor {
    /// A governor bounding any combination of the three spans.
    ///
    /// `None` means *this vendor publishes no bound on that span*, which
    /// `docs/00-charter.md` §4 records explicitly for both vendors — one has no
    /// per-minute governor, the other no daily quota. It is a recorded fact,
    /// not a default, and [`GovernorError::CeilingIsZero`] exists so that a
    /// zero cannot be mistaken for it.
    ///
    /// # Errors
    ///
    /// [`GovernorError::CeilingIsZero`] or [`GovernorError::CeilingOutOfRange`],
    /// naming the span. Past this gate every arithmetic step in the type is
    /// provably total, which is why nothing below carries a failure arm.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pull::rate::{DHAN_PER_DAY, DHAN_PER_SECOND, Governor, GovernorError, WindowSpan};
    /// // docs/00-charter.md §4: 5/s, 100,000/day, no per-minute governor.
    /// let dhan = Governor::new(Some(DHAN_PER_SECOND), None, Some(DHAN_PER_DAY))?;
    /// assert_eq!(dhan.ceiling(WindowSpan::Second), Some(5));
    /// assert_eq!(dhan.ceiling(WindowSpan::Minute), None);
    /// assert_eq!(dhan.ceiling(WindowSpan::Day), Some(100_000));
    /// # Ok::<(), GovernorError>(())
    /// ```
    pub const fn new(
        per_second: Option<u32>,
        per_minute: Option<u32>,
        per_day: Option<u32>,
    ) -> Result<Self, GovernorError> {
        // Written as three matches rather than a loop because this is a `const
        // fn` and `?` on a `Result` is not const-stable on this toolchain. The
        // shape is mechanical and the compile-time assertion on `WindowSpan::ALL`
        // below is what keeps the three in the same order as the tags.
        let second = match Window::new(WindowSpan::Second, per_second) {
            Ok(window) => window,
            Err(refusal) => return Err(refusal),
        };
        let minute = match Window::new(WindowSpan::Minute, per_minute) {
            Ok(window) => window,
            Err(refusal) => return Err(refusal),
        };
        let day = match Window::new(WindowSpan::Day, per_day) {
            Ok(window) => window,
            Err(refusal) => return Err(refusal),
        };
        Ok(Self {
            windows: [second, minute, day],
            cursor_micros: 0,
        })
    }

    /// Whether a request may be issued at `now_micros`, and if not, when.
    ///
    /// `now_micros` is a **monotonic** microsecond reading supplied by the
    /// caller. Nothing here reads a clock: see the module header on why that is
    /// the difference between a deterministic test and a sleeping one.
    ///
    /// Every live window must afford a permit before **any** window is charged.
    /// Charging as the windows are walked would let a request that the day
    /// window refuses still consume a second-window permit, so a heavily
    /// throttled pull would drain the short span it was never allowed to use.
    ///
    /// The refusal names the span with the **longest** wait — the binding
    /// constraint — and ties go to the shorter span, which is the order
    /// [`WindowSpan::ALL`] is written in.
    ///
    /// A governor with no live window admits everything. That is not a silent
    /// fallback: it is only reachable by passing three `None`s, which says in
    /// the constructor call that the vendor publishes no bound at all.
    #[must_use]
    pub fn admit(&mut self, now_micros: u64) -> Verdict {
        self.advance_to(now_micros);

        let mut binding: Option<(WindowSpan, u64)> = None;
        for window in self.windows.iter().flatten() {
            let Some(wait) = window.shortfall() else {
                continue;
            };
            let takes_over = match binding {
                None => true,
                Some((_, worst)) => wait > worst,
            };
            if takes_over {
                binding = Some((window.span, wait));
            }
        }

        if let Some((span, wait_micros)) = binding {
            return Verdict::Deny { span, wait_micros };
        }
        for window in self.windows.iter_mut().flatten() {
            window.charge();
        }
        Verdict::Admit
    }

    /// Moves the cursor forward and earns the elapsed micro-permits.
    ///
    /// **Forward only.** A `now` below the cursor is an NTP step, a resumed
    /// laptop or a virtualised counter, and it yields zero elapsed and leaves
    /// the cursor untouched — so the blip grants nothing, and the time that
    /// passes after it is measured from the highest instant ever seen rather
    /// than from the bottom of the dip. A cursor that followed the clock down
    /// would hand out the whole regression as free capacity the moment the
    /// clock came back.
    fn advance_to(&mut self, now_micros: u64) {
        let elapsed = now_micros.saturating_sub(self.cursor_micros);
        if now_micros > self.cursor_micros {
            self.cursor_micros = now_micros;
        }
        for window in self.windows.iter_mut().flatten() {
            window.earn(elapsed);
        }
    }

    /// The vendor honoured a request: additive increase on every span.
    ///
    /// One permit, once, per observed success — the `a` of AIMD. Recovery from
    /// a halving therefore costs as many successes as the halving destroyed
    /// permits, which is the asymmetry the module header argues for.
    pub fn record_success(&mut self) {
        for window in self.windows.iter_mut().flatten() {
            window.tighten_toward_ceiling();
        }
    }

    /// The vendor refused a request for rate: multiplicative decrease on every
    /// span.
    ///
    /// **Every span, because a `429` names none of them.** A vendor's throttle
    /// response says the caller was over budget; it does not say over *which*
    /// budget, and this repository does not invent a vendor fact to guess with
    /// (`CLAUDE.md` §3 rule 1). Backing every span off is the conservative
    /// reading, and its cost is bounded: the short spans earn their allowance
    /// back fastest because successes arrive fastest there.
    ///
    /// Reserved for a **rate** refusal. An authentication failure is
    /// [`crate::secret::CredentialHalt`]'s business and must halt loudly rather
    /// than be absorbed as congestion — degrading a dead credential into a
    /// slower pull is precisely the fallback `CLAUDE.md` §4 bans.
    pub fn record_throttled(&mut self) {
        for window in self.windows.iter_mut().flatten() {
            window.relax();
        }
    }

    /// The published ceiling for a span, or `None` if that span is unbounded.
    ///
    /// The allowance never passes this. One walk of a three-element array.
    #[must_use]
    pub fn ceiling(&self, span: WindowSpan) -> Option<u32> {
        self.window(span).map(|window| window.ceiling)
    }

    /// The current AIMD allowance for a span, or `None` if that span is
    /// unbounded.
    ///
    /// This is the number the operator's requirement is about: it rises by one
    /// per success and halves per refusal, and it lives in `1..=ceiling`.
    #[must_use]
    pub fn permitted(&self, span: WindowSpan) -> Option<u32> {
        self.window(span).map(|window| window.permitted)
    }

    /// Micro-permits currently banked for a span, or `None` if that span is
    /// unbounded.
    ///
    /// Exposed because a bound nothing can observe is a bound no test can
    /// prove. One permit costs [`WindowSpan::len_micros`].
    #[must_use]
    pub fn credit_micro_permits(&self, span: WindowSpan) -> Option<u64> {
        self.window(span).map(|window| window.credit)
    }

    /// The highest instant this governor has been shown.
    ///
    /// Forward-only, so it is also the instant every window's earnings are
    /// measured from.
    #[must_use]
    pub const fn cursor_micros(&self) -> u64 {
        self.cursor_micros
    }

    /// The window bounding `span`, if any. A walk of three `Option`s.
    fn window(&self, span: WindowSpan) -> Option<Window> {
        self.windows
            .iter()
            .flatten()
            .find(|window| window.span == span)
            .copied()
    }
}

/// Which of a vendor's request budgets a call is drawn against.
///
/// **Pooling is per request kind, and that is a charter fact rather than a
/// guess.** `docs/00-charter.md` §4 records the primary vendor's per-second cap
/// as unverified *because* "the published 10/s applies to a **different
/// endpoint group**" — which is a statement that endpoint groups carry separate
/// budgets. The same table gives that vendor one history method for spot and
/// derivatives, so the split that the charter evidences is history against
/// everything live.
///
/// Two consequences, both load-bearing. A historical backfill cannot starve a
/// live quote of its budget, because they draw on different governors. And a
/// `429` on one kind halves that kind's allowance alone, so a throttled
/// backfill does not slow a live feed that the vendor never complained about.
///
/// Exhaustive on purpose, unlike [`Vendor`]: a `#[non_exhaustive]` enum forces
/// every match outside this crate through a wildcard arm no test can reach,
/// which is exactly the false green `core::vendor::VendorSet` was written to
/// avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequestKind {
    /// Quotes, depth, and anything else priced off the live endpoint group.
    Live,
    /// The history endpoint — the backfill this repository exists to run.
    Historical,
}

impl RequestKind {
    /// Every kind, in the order [`Pools`] stores them.
    pub const ALL: [Self; 2] = [Self::Live, Self::Historical];
}

impl fmt::Display for RequestKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Live => f.write_str("live"),
            Self::Historical => f.write_str("historical"),
        }
    }
}

/// What a governor is kept under: a vendor and a request kind.
///
/// A value rather than a comment, so that a log line, a refusal or an operator
/// page can name the pool that denied a request without reconstructing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoolKey {
    /// Whose budget.
    pub vendor: Vendor,
    /// Which of that vendor's budgets.
    pub kind: RequestKind,
}

impl fmt::Display for PoolKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.vendor.as_str(), self.kind)
    }
}

/// One vendor's governors, one per request kind.
///
/// A fixed pair, never a map: [`RequestKind`] has two variants and the
/// selection below is an exhaustive match on them, so there is no hash, no
/// lookup and no absent-key arm. `docs/07-o1-architecture.md` law 4 —
/// arithmetic beats lookup, and a match beats both.
///
/// Kept per vendor rather than as one global table because [`Vendor`] is
/// `#[non_exhaustive]`: indexing a table by it outside `crates/core` needs
/// either a match with an unreachable wildcard or a search whose miss arm no
/// test can enter. Holding one [`Pools`] per vendor and carrying the vendor on
/// the value removes the question instead of answering it badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pools {
    vendor: Vendor,
    governors: [Governor; 2],
}

const _: () = assert!(RequestKind::ALL.len() == 2);
const _: () = assert!(WindowSpan::ALL.len() == WINDOW_COUNT);

impl Pools {
    /// One vendor's two pools.
    ///
    /// The two governors are given separately rather than derived from the
    /// vendor, because the ceilings are external facts whose evidence lanes
    /// differ per span (see [`GROWW_PER_SECOND_UNVERIFIED`]) and a
    /// `match vendor` here would need a wildcard arm no test could reach.
    #[must_use]
    pub const fn new(vendor: Vendor, live: Governor, historical: Governor) -> Self {
        Self {
            vendor,
            governors: [live, historical],
        }
    }

    /// Whose budgets these are.
    #[must_use]
    pub const fn vendor(&self) -> Vendor {
        self.vendor
    }

    /// The key one of these pools is held under.
    #[must_use]
    pub const fn key(&self, kind: RequestKind) -> PoolKey {
        PoolKey {
            vendor: self.vendor,
            kind,
        }
    }

    /// One kind's governor.
    ///
    /// Destructured rather than indexed: this workspace denies
    /// `clippy::indexing_slicing`, and an array pattern is a total selection
    /// with no bound to check.
    #[must_use]
    pub const fn governor(&self, kind: RequestKind) -> &Governor {
        let [live, historical] = &self.governors;
        match kind {
            RequestKind::Live => live,
            RequestKind::Historical => historical,
        }
    }

    /// One kind's governor, to drive.
    pub const fn governor_mut(&mut self, kind: RequestKind) -> &mut Governor {
        let [live, historical] = &mut self.governors;
        match kind {
            RequestKind::Live => live,
            RequestKind::Historical => historical,
        }
    }
}
