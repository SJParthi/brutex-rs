//! A clickable calendar with no script, and the arithmetic that makes one
//! possible.
//!
//! # What was wrong with the text box
//!
//! The field before this one was `<input type="text" placeholder="YYYY-MM-DD">`.
//! It was chosen over `<input type="date">` for a real reason — a native date
//! input renders in the *browser's* locale, which this server cannot influence,
//! and on this operator's machine that meant `dd/mm/yyyy`. `01/07/2025` is 1 July
//! here and 7 January in half the world, and this codebase has already been bitten
//! by exactly that ambiguity: GDFL writes `DD/MM/YYYY`, and reading it the other
//! way shifts every bar by months into a file that is internally consistent and
//! completely wrong.
//!
//! But fixing the ambiguity by making the operator **type** was fixing one defect
//! with another. Dhan puts a month grid on screen and you click a number. So does
//! `TradingView`. This module does the same, and controls the rendering itself
//! rather than asking the browser to.
//!
//! # One month at a time, and why the first build was not
//!
//! The first build of this picker put the twelve years, the twelve months and the
//! thirty-one days **on screen together**. Every control the widget owned was
//! visible at once, the panel stood about 600 px tall, and with two pickers open
//! on the ingest page the two panels overlapped each other. It was a control
//! panel, not a date picker: nothing about it said which of the three strips you
//! were meant to touch first, and the day grid it drew was the grid for whichever
//! (year, month) happened to be selected — including none.
//!
//! Dhan draws **one month**, with `‹ August 2026 ›` above it, and the title is a
//! button that drills down to the months, and from there to the years. That is
//! the shape this module now emits, and it is smaller in every dimension: about
//! 380 px tall instead of 600, and one decision on screen at a time.
//!
//! # Only one panel can be open, and that is why they cannot overlap
//!
//! Cutting the panel down did not on its own fix the overlap, and measuring said
//! so: at 1440×900 with every picker forced open, the expiry panel still hung
//! 303 px down over the panel belonging to the field on the row beneath it. Any
//! panel taller than its own row reaches the next one, so no width and no height
//! settles this.
//!
//! What settles it is that **there is never a second panel.** The popover latch
//! is a `<input type="radio" name="cal">` rather than a checkbox, so the five
//! latches on a form are five members of one group and at most one is checked.
//! Opening a picker closes whatever was open, and two panels cannot coexist to
//! be laid out badly.
//!
//! A radio cannot untick itself, which a checkbox could, so each panel carries an
//! explicit `Close` pointing at the group's resting member — [`shut`], emitted
//! once per form and `checked`, which is also what makes every panel start shut.
//! Radio groups are scoped to their **form owner**, so the ingest page's two
//! forms get one group each; they are two cards side by side and neither one's
//! panels can reach the other's.
//!
//! # Three panes, and not one extra control to switch them
//!
//! The obvious way to build a drill-down is a latch per pane. This has none.
//! **Which pane is showing is a function of which radios are already checked**,
//! read by `:has()`:
//!
//! | Checked | Pane | Header |
//! |---|---|---|
//! | nothing | the twelve years | `Choose a year` |
//! | a year | the twelve months | `‹ 2026` |
//! | a year and a month | that month's grid | `‹ August 2026 ›` |
//!
//! So the picker advances on its own: click a year and the months appear, click a
//! month and its days appear. There is no state to keep in sync with the form,
//! because the form's own controls *are* the state.
//!
//! Going back needs the one thing a `<label>` cannot do — **uncheck** a radio. So
//! each of the two groups carries one more radio whose value is the empty string:
//! `{name}_y=""` and `{name}_m=""`. Checking it is what unchecks the real one, and
//! because radios in a group are mutually exclusive that is a single click with no
//! script anywhere near it. The header title is a `<label>` for that radio: in the
//! day pane it clears the month and lands you in the months, in the month pane it
//! clears the year and lands you in the years.
//!
//! An empty group posts an empty value, which [`crate::ingest::parse_day_field`]
//! reads as *this date was not finished* and refuses by name. Half a date has
//! never been accepted here and still is not.
//!
//! # Why the arrows do not cross a year boundary
//!
//! `‹` and `›` are `<label>`s for the neighbouring month's radio, and a label
//! drives exactly one control. Stepping from January back to December would have
//! to check `m12` **and** the previous year's radio — two controls, one click, and
//! no way to spell it without a script. So January's `‹` and December's `›` are
//! inert and say so by being greyed: the title above them is the way to change
//! year, and it is one click away. `docs/06-limits.md` §31 records it rather than
//! leaving the operator to discover it.
//!
//! # Why there is no JavaScript, and why that is not a compromise
//!
//! `CLAUDE.md` §2 allows exactly seven tracked extensions and none of them is a
//! script. CI gate 1 walks tracked files — so a `<script>` block living inside a
//! Rust string literal is another language smuggled past the gate, which is why
//! four separate tests in [`crate::render`] assert the substring `<script>` never
//! appears in any page this server emits.
//!
//! The constraint turned out to be the better build. A picker with no script
//! cannot race, cannot fail to initialise, cannot break when a bundle 404s, and
//! works with the network off. What replaces the script is:
//!
//! | Job | Mechanism |
//! |---|---|
//! | open one panel and shut the rest | one radio group per form, `:checked ~ .cal` |
//! | remember the click | a radio group, which is what a form control is for |
//! | choose which pane shows | `:has()` on the radios already checked |
//! | step back a pane | one radio per group whose value is empty |
//! | step a month sideways | a `<label>` for the neighbouring month |
//! | align the 1st under its weekday | `:has()` setting `--s` on a CSS grid |
//! | grey out 31 February | `:has()` on the year and month together |
//! | show `6 Aug '26` as you click | `::after{content}` keyed by `:has()` |
//!
//! # The form submits three numbers, not one string
//!
//! Each picker posts `<name>_y`, `<name>_m` and `<name>_d`, and the server
//! composes them through [`Day::new`], which owns every calendar rule including
//! the leap year. This is not a detail of convenience — it is what keeps the
//! page small. Had a day radio carried the whole `2026-08-06`, every one of the
//! 144 (year, month) pairs would need its own 31 labels, times five date fields
//! on the ingest page. Three independent groups is 13 + 13 + 31 = 57 controls per
//! field — the two extra being the empty-valued radios above — and the alignment
//! rules are shared by every picker on the page rather than emitted once each.
//!
//! `<name>=YYYY-MM-DD` is still accepted by the server, unchanged, so a hand-built
//! POST and every existing test keep working. See [`crate::ingest::parse_day`].

use core::fmt::Write as _;
use pull::session::{Day, MIN_YEAR};

/// [`MIN_YEAR`] in the width a year is carried in.
///
/// Written as a literal in both widths rather than converted, for the reason
/// `crate::census` gives about its own pair: there is no cast to deny and no
/// fallible conversion whose failure arm no input could ever reach. The
/// assertion below keeps the two honest.
const MIN_YEAR_U16: u16 = 1_970;
const _: () = assert!(MIN_YEAR == 1_970 && MIN_YEAR_U16 == 1_970);

/// How many years the picker offers, ending at the field's own ceiling.
///
/// A **picker range, not a data claim.** Nothing here asserts that a vendor sells
/// twelve years of one-minute bars; it is the span an operator can reach without
/// paging, and the store is the only thing that knows what is actually held.
pub const YEARS_OFFERED: u16 = 12;

/// Month names as the month pane and the readout write them.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Month names as the drill-down header writes them.
///
/// Dhan's header reads `August 2026`, not `Aug 2026`, and the header is the one
/// place on the page with room for the whole word. The abbreviations above stay
/// where space is tight: the twelve month buttons, and the readout on the closed
/// field.
const FULL_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Weekday headers, Monday first — the order Dhan's grid uses.
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// Which column the 1st of this month falls in, 1 through 7, Monday first.
///
/// 1970-01-01 was a **Thursday**, which is the one fact this arithmetic rests on
/// and the reason for the `+ 3`: with Monday at index 0, Thursday is index 3, so
/// day zero of the epoch must land there. Every other date follows by counting.
///
/// [`Day::days_from_epoch`] is already the count, so this is two operations and
/// no table.
#[must_use]
pub const fn first_column(year: u16, month: u8) -> u32 {
    match Day::new(year, month, 1) {
        Ok(first) => (first.days_from_epoch() + 3) % 7 + 1,
        // A month whose 1st does not exist cannot occur: `month` is always
        // 1..=12 at every call site below. Column 1 keeps the grid rectangular
        // rather than panicking over an unreachable branch.
        Err(_) => 1,
    }
}

/// How many days this month holds, leap year included.
///
/// Derived by asking [`Day`] rather than by carrying a table: the 29th of
/// February exists exactly when `Day::new` says it does, so there is one
/// definition of a leap year in this workspace and it is not here.
/// Written as four questions rather than a countdown loop, and that is a
/// mutation-testing result rather than a style preference. `while probe > 28`
/// carries a boundary, and `>=` there is **indistinguishable**: the loop's last
/// iteration asks `Day::new(year, month, 28)`, which is true for every month, so
/// both spellings return 28. A surviving mutant on an unreachable boundary is a
/// test that cannot be written, so the boundary is gone instead — the same move
/// `crates/greeks`' `normal.rs` made for its two.
#[must_use]
pub const fn month_length(year: u16, month: u8) -> u8 {
    if Day::new(year, month, 31).is_ok() {
        return 31;
    }
    if Day::new(year, month, 30).is_ok() {
        return 30;
    }
    if Day::new(year, month, 29).is_ok() {
        return 29;
    }
    28
}

/// The earliest year the picker offers for a field capped at `max`.
///
/// **Floored at [`MIN_YEAR`], and that is not decoration.** The subtraction alone
/// answered 1959 for a cap in 1970 — twelve clickable years, the first eleven of
/// which no [`Day`] can hold, so picking one produced a date `Day::new` refuses.
/// Unreachable from this page, whose ceilings are today and yesterday, and wrong
/// anyway: the floor belongs to `Day` and is imported rather than restated.
/// The floor is arithmetic, not a comparison, for the reason [`month_length`]
/// gives: `offered < MIN_YEAR` and `offered <= MIN_YEAR` both answer 1970 at the
/// boundary, so any comparison written here is a mutant no test could kill.
///
/// `MIN_YEAR + offered.saturating_sub(MIN_YEAR)` is `max(offered, MIN_YEAR)`
/// with no branch: at or above the floor the subtraction and the addition
/// cancel, and below it the subtraction saturates to zero and the floor is what
/// is left. Every operator in it is one a test can falsify.
#[must_use]
pub const fn first_year(max: Day) -> u16 {
    let offered = max.year().saturating_sub(YEARS_OFFERED - 1);
    MIN_YEAR_U16 + offered.saturating_sub(MIN_YEAR_U16)
}

/// The one control that closes whichever picker is open, for one form.
///
/// **Emit this exactly once inside every `<form>` that holds a picker.** It is
/// the resting member of the `cal` radio group described in this module's
/// header: it is `checked` on load, which is what makes every panel start
/// closed, and the `Close` label inside each panel points back at it.
///
/// One per form and not one per page, because a radio group is scoped to its
/// **form owner** — two inputs with the same name in two different `<form>`s are
/// two independent groups, not one. That scoping is exactly the scope wanted
/// here: the ingest page's two forms are two cards side by side, and a panel in
/// one cannot reach a panel in the other.
#[must_use]
pub fn shut(form: &str) -> String {
    format!(
        "<input type=\"radio\" name=\"cal\" id=\"{form}-calshut\" value=\"\" \
         class=\"popshut\" checked>"
    )
}

/// One date picker: the readout, the popover, and every control inside it.
///
/// `name` becomes three radio groups — `{name}_y`, `{name}_m`, `{name}_d` — and
/// `id` scopes the popover's own latch so two pickers on one page cannot toggle
/// each other. `form` names the form this picker sits in, which is what its
/// `Close` label points at — see [`shut`]. Nothing later than `max` can be
/// clicked.
#[must_use]
pub fn picker(name: &str, id: &str, form: &str, max: Day) -> String {
    let mut out = String::with_capacity(8192);
    let cap = cap_class(max);

    let _ = write!(out, "<div class=\"pick {cap}\">");

    // ── THE RADIOS BELOW ARE NOT `required`, AND THAT IS A BUG FIX ──────────
    //
    // They were, and it made the ingest form IMPOSSIBLE TO SUBMIT while
    // looking like nothing was wrong. Every one of them is
    // `position:absolute; width:0; height:0; opacity:0` (see `CAL_STYLE`) and
    // sits inside a `.cal` that is `display:none` until the popover is opened.
    // When a browser finds an unsatisfied `required` control it tries to focus
    // it to anchor the validation bubble; a zero-sized control inside a hidden
    // subtree cannot be focused, so Chrome logs
    //
    //     An invalid form control with name='to_d' is not focusable.
    //
    // to a console the operator is not looking at, **refuses to submit, and
    // shows nothing.** The button appears dead. Reproduced in a live browser:
    // `form.reportValidity()` returned `false` with the message above and no
    // visible UI. This is precisely `CLAUDE.md` §4's banned shape — a failure
    // that is neither loud nor named.
    //
    // Presence is enforced where it was always actually enforced: the server.
    // `api::ingest::parse_day_field` returns `Refusal::FieldMissing` when the
    // three parts do not compose a date, and the refusal is rendered on the
    // result page with the field named. An attribute is a courtesy; a parser is
    // the rule — and here the courtesy was silently overruling the rule.
    //
    // THE POPOVER'S LATCH IS A RADIO, NOT A CHECKBOX, AND THAT IS THE FIX FOR
    // THE OVERLAP.
    //
    // A checkbox latches one panel and knows nothing about the others, so the
    // page could hold five open panels at once. Two of them did overlap:
    // measured live at 1440×900, the expiry panel hung 303 px down over the
    // panel belonging to the field on the row below it, and before the panel
    // was cut from ~600 px to one month the side-by-side pair overlapped too.
    // Tuning widths only ever moved that around — a panel taller than the row
    // it sits in will always reach the row below.
    //
    // A radio group cannot hold two members. Every latch on this form is named
    // `cal`, so opening one panel closes whatever was open, and **two panels
    // can no longer exist at the same time.** The overlap stops being a layout
    // problem and stops being possible.
    //
    // What a radio cannot do is untick itself, so the panel carries an explicit
    // `Close` in its footer pointing at the group's resting member — see
    // [`shut`]. The group posts `cal=` and no reader looks for that key.
    let _ = write!(
        out,
        "<input type=\"radio\" name=\"cal\" id=\"o-{id}\" value=\"\" class=\"popopen\">\
         <label class=\"readout\" for=\"o-{id}\">\
         <span class=\"v-ph\">Pick a date</span>\
         <span class=\"v-d\"></span><span class=\"v-m\"></span><span class=\"v-y\"></span>\
         <span class=\"v-ico\">▦</span></label>"
    );

    // THE TWO RADIOS THAT STEP BACK A PANE. A `<label>` can check a control and
    // cannot uncheck one, so "return to the months" is spelled as checking a
    // *different member of the same group* — and the only value that is not a
    // month is the empty one. The group then posts nothing, which is what
    // `parse_day_field` already reads as an unfinished date.
    //
    // They live outside `.cal` on purpose: a label drives a control wherever it
    // sits, and keeping them out of the panel means no pane rule can hide the
    // thing another pane's header points at.
    let _ = write!(
        out,
        "<input type=\"radio\" name=\"{name}_y\" id=\"{id}y0\" value=\"\" class=\"yr-x\">\
         <input type=\"radio\" name=\"{name}_m\" id=\"{id}m0\" value=\"\" class=\"mo-x\">"
    );

    out.push_str("<div class=\"cal\">");

    // ---- PANE 1 · YEARS --------------------------------------------------
    // Shown while no year is checked, which on a fresh page is immediately.
    out.push_str(
        "<div class=\"pane yrs\">\
         <div class=\"hd\"><span class=\"hdt lead\">Choose a year</span></div>\
         <div class=\"strip\">",
    );
    let first = first_year(max);
    for year in first..=max.year() {
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"{name}_y\" id=\"{id}y{year}\" value=\"{year}\" \
             class=\"yr-in y{year}\">\
             <label for=\"{id}y{year}\">{year}</label>"
        );
    }
    out.push_str("</div></div>");

    // ---- PANE 2 · MONTHS -------------------------------------------------
    // Shown once a year is checked and until a month is. Its header steps back
    // to the years by clearing the year.
    let _ = write!(
        out,
        "<div class=\"pane mos\">\
         <div class=\"hd\"><label class=\"hdt\" for=\"{id}y0\">\
         <span class=\"ar\">‹</span><b class=\"v-y4\"></b></label></div>\
         <div class=\"strip\">"
    );
    for (index, label) in MONTHS.iter().enumerate() {
        // `index` is 0..12, so this cast is exact and the month is 1..=12.
        #[allow(clippy::cast_possible_truncation)]
        let month = index as u8 + 1;
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"{name}_m\" id=\"{id}m{month}\" value=\"{month}\" \
             class=\"mo-in m{month}\">\
             <label for=\"{id}m{month}\" class=\"k{month}\">{label}</label>"
        );
    }
    out.push_str("</div></div>");

    // ---- PANE 3 · ONE MONTH ----------------------------------------------
    out.push_str("<div class=\"pane days\">");
    out.push_str(&month_header(id));

    out.push_str("<div class=\"dow\">");
    for day in WEEKDAYS {
        let _ = write!(out, "<span>{day}</span>");
    }
    out.push_str("</div><div class=\"pad\">");
    for day in 1u8..=31 {
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"{name}_d\" id=\"{id}d{day}\" value=\"{day}\" \
             class=\"dy-in d{day}\">\
             <label for=\"{id}d{day}\" class=\"c{day}\">{day}</label>"
        );
    }
    out.push_str("</div></div>");

    let _ = write!(
        out,
        "<p class=\"calnote\">Latest that can be asked for: <b>{max}</b>. \
         Days that do not exist in the month you picked grey out — there is no \
         31 February here and no 29 February outside a leap year. \
         <label class=\"shut\" for=\"{form}-calshut\">Close</label></p>"
    );
    out.push_str("</div></div>");
    out
}

/// The day pane's header: `‹ August 2026 ›`.
///
/// **Twelve `‹ ›` pairs are emitted and CSS shows one.** A `<label>`'s target is
/// written in the markup, so "the previous month" cannot be worked out at click
/// time — it has to be spelled out once per month it could be clicked from, and
/// `.pick:has(.m8:checked) .n8` is what picks the pair that belongs to August.
///
/// January's `‹` and December's `›` are inert `<span>`s rather than labels:
/// stepping across a year boundary would mean checking the month radio *and* the
/// year radio on one click, and a label drives exactly one control. The title
/// between them is a label for the empty-valued month radio, which is how a
/// click there clears the month and drops back to the month pane.
fn month_header(id: &str) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<div class=\"hd\">");
    for month in 1u8..=12 {
        let _ = write!(out, "<span class=\"nav n{month}\">");
        if month == 1 {
            out.push_str("<span class=\"st pv off\">‹</span>");
        } else {
            let _ = write!(
                out,
                "<label class=\"st pv\" for=\"{id}m{}\">‹</label>",
                month - 1
            );
        }
        if month == 12 {
            out.push_str("<span class=\"st nx off\">›</span>");
        } else {
            let _ = write!(
                out,
                "<label class=\"st nx\" for=\"{id}m{}\">›</label>",
                month + 1
            );
        }
        out.push_str("</span>");
    }
    let _ = write!(
        out,
        "<label class=\"hdt\" for=\"{id}m0\">\
         <b class=\"v-mn\"></b> <b class=\"v-y4\"></b></label></div>"
    );
    out
}

/// The class that carries a field's ceiling into the shared stylesheet.
///
/// Two pickers with the same ceiling share one rule. The ingest page has five
/// date fields and exactly **two** distinct ceilings — today for spot, yesterday
/// for anything with an expiry — so this collapses ten rules into two.
#[must_use]
pub fn cap_class(max: Day) -> String {
    format!("cap{:04}{:02}{:02}", max.year(), max.month(), max.day())
}

/// Every rule whose value had to be computed rather than written.
///
/// Four families, all shared by every picker on the page:
///
/// 1. **Alignment.** For each (year, month) offered, which column the 1st sits
///    in. This is the rule that makes the thing a calendar rather than a pad of
///    numbers.
/// 2. **Impossible days.** For each (year, month), the day cells past the end of
///    that month, greyed and unclickable.
/// 3. **The ceiling.** Months after the cap's own month in the cap's own year,
///    and days after the cap's own day in the cap's own month.
/// 4. **The readouts.** `6`, `Aug` and `'26` on the closed field, and `August`
///    and `2026` in the drill-down header, as `::after` content keyed by which
///    radio is checked — so the field reads back in Dhan's own format while the
///    wire still carries three integers.
///
/// `caps` is the distinct set of ceilings on the page. Passing them in rather
/// than assuming keeps this honest when a page grows a third one.
#[must_use]
pub fn dynamic_css(caps: &[Day]) -> String {
    let mut out = String::with_capacity(40_960);

    // The union of every year any picker offers, so one alignment rule serves
    // all of them.
    let (Some(low), Some(high)) = (
        caps.iter().map(|c| first_year(*c)).min(),
        caps.iter().map(|c| c.year()).max(),
    ) else {
        return out;
    };

    for year in low..=high {
        for month in 1u8..=12 {
            let _ = write!(
                out,
                ".pick:has(.y{year}:checked):has(.m{month}:checked) .pad{{--s:{}}}",
                first_column(year, month)
            );
            let length = month_length(year, month);
            if length < 31 {
                let _ = write!(out, ".pick:has(.y{year}:checked):has(.m{month}:checked) ");
                for (n, day) in (length + 1..=31).enumerate() {
                    if n > 0 {
                        let _ = write!(out, ",.pick:has(.y{year}:checked):has(.m{month}:checked) ");
                    }
                    let _ = write!(out, ".c{day}");
                }
                out.push_str("{opacity:.16;pointer-events:none;text-decoration:line-through}");
            }
        }
    }

    // ---- THE CEILING -----------------------------------------------------
    //
    // Two rules per cap, and the FIRST ONE IS NEW. `picker` stops emitting
    // years at the cap's own year, so no year past the ceiling exists to be
    // clicked — but all twelve months are always emitted, and in the cap's year
    // the months after it are months no vendor holds a bar in. They used to be
    // clickable, and clicking one took you to a grid of days that all led to a
    // server refusal. Greyed here instead, where the operator can see it.
    for cap in caps {
        let class = cap_class(*cap);
        let (year, month) = (cap.year(), cap.month());

        if month < 12 {
            let _ = write!(out, ".pick.{class}:has(.y{year}:checked) ");
            for (n, later) in (month + 1..=12).enumerate() {
                if n > 0 {
                    let _ = write!(out, ",.pick.{class}:has(.y{year}:checked) ");
                }
                let _ = write!(out, ".k{later}");
            }
            out.push_str("{opacity:.16;pointer-events:none;text-decoration:line-through}");
        }

        // The ceiling's own month, where some days are reachable and some are
        // not. A cap landing on the 31st has nothing after it to grey.
        if cap.day() >= 31 {
            continue;
        }
        let _ = write!(
            out,
            ".pick.{class}:has(.y{year}:checked):has(.m{month}:checked) "
        );
        for (n, day) in (cap.day() + 1..=31).enumerate() {
            if n > 0 {
                let _ = write!(
                    out,
                    ",.pick.{class}:has(.y{year}:checked):has(.m{month}:checked) "
                );
            }
            let _ = write!(out, ".c{day}");
        }
        out.push_str("{opacity:.16;pointer-events:none}");
    }

    // ---- THE READOUTS ----------------------------------------------------
    for day in 1u8..=31 {
        let _ = write!(
            out,
            ".pick:has(.d{day}:checked) .v-d::after{{content:'{day}'}}"
        );
    }
    for (index, label) in MONTHS.iter().enumerate() {
        let month = index + 1;
        let _ = write!(
            out,
            ".pick:has(.m{month}:checked) .v-m::after{{content:'{label}'}}"
        );
    }
    // The header's own spelling. Keyed by the same radio as the abbreviation
    // above, so the closed field and the open header can never name different
    // months.
    for (index, label) in FULL_MONTHS.iter().enumerate() {
        let month = index + 1;
        let _ = write!(
            out,
            ".pick:has(.m{month}:checked) .v-mn::after{{content:'{label}'}}"
        );
    }
    for year in low..=high {
        let _ = write!(
            out,
            ".pick:has(.y{year}:checked) .v-y::after{{content:\"'{:02}\"}}\
             .pick:has(.y{year}:checked) .v-y4::after{{content:'{year}'}}",
            year % 100
        );
    }
    out
}

/// The part of the picker's stylesheet that no arithmetic could change.
pub const CAL_STYLE: &str = "\
.pick{position:relative;display:block}\
.pick input,.popshut{position:absolute;opacity:0;width:0;height:0;pointer-events:none}\
.readout{display:flex;align-items:center;gap:6px;cursor:pointer;user-select:none;\
font:inherit;font-variant-numeric:tabular-nums;padding:10px 13px;border:1px solid var(--line);\
border-radius:11px;background:var(--panel);color:var(--ink);box-shadow:var(--sh);\
transition:border-color .2s,box-shadow .2s,transform .2s}\
.readout:hover{border-color:var(--acc);transform:translateY(-1px)}\
.pick .v-ph{color:var(--dim)}\
.pick:has(.yr-in:checked):has(.mo-in:checked):has(.dy-in:checked) .v-ph{display:none}\
.pick .v-ico{margin-left:auto;color:var(--acc);font-size:15px}\
.pick .v-d,.pick .v-m,.pick .v-y{font-weight:700}\
.popopen:checked~.readout{border-color:var(--acc);\
box-shadow:0 0 0 4px color-mix(in srgb,var(--acc) 18%,transparent)}\
.popopen:checked~.readout .v-ico{transform:rotate(90deg)}\
.pick:has(.popopen:checked){z-index:41}\
.cal{display:none;position:absolute;z-index:40;top:calc(100% + 8px);left:0;\
width:100%;min-width:min(254px,100%);max-width:330px;\
background:var(--panel);border:1px solid var(--line);border-radius:15px;padding:12px;\
box-shadow:0 18px 48px rgba(16,24,40,.18),var(--sh)}\
.popopen:checked~.cal{display:block;animation:calin .22s cubic-bezier(.2,.8,.2,1) both}\
@keyframes calin{from{opacity:0;transform:translateY(-6px) scale(.98)}}\
.cal .pane{display:none}\
.pick:not(:has(.yr-in:checked)) .pane.yrs{display:block}\
.pick:has(.yr-in:checked):not(:has(.mo-in:checked)) .pane.mos{display:block}\
.pick:has(.yr-in:checked):has(.mo-in:checked) .pane.days{display:block}\
.cal .hd{display:grid;grid-template-columns:28px 1fr 28px;align-items:center;\
margin-bottom:9px;padding-bottom:8px;border-bottom:1px solid var(--line)}\
.cal .hd .hdt{grid-row:1;grid-column:2;display:flex;align-items:center;justify-content:center;\
gap:6px;font-size:13px;font-weight:750;color:var(--ink);cursor:pointer;user-select:none;\
padding:6px 8px;border-radius:9px;font-variant-numeric:tabular-nums;transition:background .16s}\
.cal .hd .hdt:hover{background:color-mix(in srgb,var(--acc) 12%,transparent);color:var(--acc)}\
.cal .hd .hdt.lead{cursor:default;color:var(--dim);font-weight:650}\
.cal .hd .hdt.lead:hover{background:none;color:var(--dim)}\
.cal .hd .ar{color:var(--acc);font-size:15px;line-height:1}\
.cal .nav{display:none}\
.pick:has(.m1:checked) .n1,.pick:has(.m2:checked) .n2,.pick:has(.m3:checked) .n3,\
.pick:has(.m4:checked) .n4,.pick:has(.m5:checked) .n5,.pick:has(.m6:checked) .n6,\
.pick:has(.m7:checked) .n7,.pick:has(.m8:checked) .n8,.pick:has(.m9:checked) .n9,\
.pick:has(.m10:checked) .n10,.pick:has(.m11:checked) .n11,.pick:has(.m12:checked) .n12\
{display:contents}\
.cal .hd .st{grid-row:1;display:flex;align-items:center;justify-content:center;\
width:28px;height:28px;border-radius:9px;cursor:pointer;user-select:none;font-size:16px;\
line-height:1;color:var(--acc);transition:background .16s}\
.cal .hd .st.pv{grid-column:1}\
.cal .hd .st.nx{grid-column:3}\
.cal .hd .st:hover{background:color-mix(in srgb,var(--acc) 13%,transparent)}\
.cal .hd .st.off{color:var(--dim);opacity:.28;cursor:default;pointer-events:none;\
text-decoration:none}\
.cal .strip{display:grid;grid-template-columns:repeat(3,1fr);gap:6px}\
.cal .strip label{cursor:pointer;text-align:center;font-size:12.5px;font-weight:650;\
padding:12px 0;border-radius:10px;border:1px solid transparent;color:var(--ink);\
transition:all .16s;font-variant-numeric:tabular-nums}\
.cal .strip label:hover{background:color-mix(in srgb,var(--acc) 10%,transparent);\
border-color:var(--acc)}\
.cal .strip input:checked+label{background:linear-gradient(135deg,var(--acc),var(--acc2));\
color:#fff;border-color:transparent;box-shadow:0 3px 10px color-mix(in srgb,var(--acc) 34%,transparent)}\
.cal .dow{display:grid;grid-template-columns:repeat(7,1fr);gap:3px;margin-bottom:4px}\
.cal .dow span{text-align:center;font-size:10px;font-weight:800;letter-spacing:.6px;\
text-transform:uppercase;color:var(--dim);padding:3px 0}\
.cal .pad{display:grid;grid-template-columns:repeat(7,1fr);gap:3px}\
.cal .pad label:first-of-type{grid-column-start:var(--s,1)}\
.cal .pad label{cursor:pointer;text-align:center;padding:7px 0;border-radius:9px;font-size:13px;\
font-weight:600;font-variant-numeric:tabular-nums;color:var(--ink);\
transition:background .15s,transform .15s,color .15s}\
.cal .pad label:hover{background:color-mix(in srgb,var(--acc) 13%,transparent);transform:scale(1.08)}\
.cal .pad input:checked+label{background:linear-gradient(135deg,var(--acc),var(--acc2));color:#fff;\
box-shadow:0 4px 12px color-mix(in srgb,var(--acc) 40%,transparent)}\
.cal .calnote{font-size:11px;line-height:1.45;color:var(--dim);margin-top:10px;\
border-top:1px solid var(--line);padding-top:8px}\
.cal .calnote b{color:var(--ink);font-variant-numeric:tabular-nums}\
.cal .calnote .shut{display:inline-block;margin-top:7px;cursor:pointer;user-select:none;\
font-weight:700;color:var(--acc);padding:4px 10px;border-radius:8px;\
border:1px solid var(--line);transition:background .16s,border-color .16s}\
.cal .calnote .shut:hover{background:color-mix(in srgb,var(--acc) 12%,transparent);\
border-color:var(--acc)}\
@media(prefers-reduced-motion:reduce){.popopen:checked~.cal{animation:none}\
.readout:hover,.cal .pad label:hover{transform:none}}\
";

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot panic cannot fail, and these lints exist to \
              keep panics out of the crate rather than out of its tests"
)]
mod tests {
    use super::*;

    fn day(y: u16, m: u8, d: u8) -> Day {
        Day::new(y, m, d).expect("a real date")
    }

    /// The whole picker, for a field capped on 2026-08-07.
    fn built() -> String {
        picker("from", "sfrom", "s", day(2026, 8, 7))
    }

    /// Monday-first columns, checked against dates whose weekday is known.
    ///
    /// 1970-01-01 was a Thursday, which is the fact the `+ 3` encodes; the rest
    /// are counted forward from it. 2026-08-01 is a Saturday, so it sits in
    /// column 6 — and the live browser agreed, which is what caught the sign of
    /// the offset being wrong in an earlier draft.
    #[test]
    fn the_first_of_a_month_lands_under_its_own_weekday() {
        assert_eq!(first_column(1970, 1), 4, "1970-01-01 was a Thursday");
        assert_eq!(first_column(2026, 8), 6, "2026-08-01 is a Saturday");
        assert_eq!(first_column(2026, 6), 1, "2026-06-01 is a Monday");
        assert_eq!(first_column(2026, 2), 7, "2026-02-01 is a Sunday");
    }

    /// A month whose 1st does not exist cannot be reached from `picker`, but the
    /// arm exists and returning column 1 is what keeps the grid rectangular
    /// instead of panicking.
    #[test]
    fn an_impossible_month_still_yields_a_column() {
        assert_eq!(first_column(2026, 0), 1);
        assert_eq!(first_column(2026, 13), 1);
    }

    /// The leap year is `Day`'s to know, not this module's.
    #[test]
    fn month_length_asks_day_rather_than_carrying_a_table() {
        assert_eq!(month_length(2026, 2), 28, "2026 is not a leap year");
        assert_eq!(month_length(2024, 2), 29, "2024 is");
        assert_eq!(month_length(1900, 2), 28, "1900 is not — the century rule");
        assert_eq!(month_length(2000, 2), 29, "2000 is — the 400 rule");
        assert_eq!(month_length(2026, 4), 30);
        assert_eq!(month_length(2026, 1), 31);
    }

    #[test]
    fn the_offered_span_ends_at_the_cap_and_is_twelve_years_long() {
        let max = day(2026, 8, 7);
        assert_eq!(first_year(max), 2015);
        assert_eq!(YEARS_OFFERED, max.year() - first_year(max) + 1);
        // AND IT IS FLOORED AT `Day`'S OWN FLOOR. The subtraction alone answers
        // 1959 here, which is eleven clickable years no date can be built in.
        assert_eq!(first_year(day(1970, 1, 1)), 1970);
        assert_eq!(first_year(day(1975, 6, 1)), 1970);
        assert_eq!(
            first_year(day(1981, 6, 1)),
            1970,
            "the first cap that reaches"
        );
        assert_eq!(
            first_year(day(1982, 6, 1)),
            1971,
            "and the first that clears it"
        );
    }

    #[test]
    fn the_cap_class_is_the_date_zero_padded() {
        assert_eq!(cap_class(day(2026, 8, 7)), "cap20260807");
        assert_eq!(cap_class(day(2026, 12, 31)), "cap20261231");
    }

    /// **NO SCRIPT, IN THE ONE MODULE MOST TEMPTED TO EMIT ONE.**
    ///
    /// `crate::render` asserts this over whole pages; this asserts it over the
    /// widget those pages embed, so a regression is named here rather than three
    /// call sites away.
    #[test]
    fn nothing_the_picker_emits_is_a_script() {
        let html = format!(
            "{}{}{}",
            built(),
            shut("s"),
            dynamic_css(&[day(2026, 8, 7)])
        );
        for forbidden in ["<script", "javascript:", "onclick", "onload", "onerror"] {
            assert!(!html.contains(forbidden), "{forbidden} must never appear");
        }
    }

    /// THE DEFECT THIS REBUILD EXISTS FOR: years, months and days were all on
    /// screen at once. Three panes are emitted and CSS shows exactly one.
    #[test]
    fn three_panes_are_emitted_and_the_year_pane_is_the_one_with_no_prerequisite() {
        let html = built();
        assert_eq!(
            html.matches("class=\"pane ").count(),
            3,
            "years, months, days"
        );
        for pane in ["pane yrs", "pane mos", "pane days"] {
            assert!(html.contains(pane), "{pane} is missing");
        }
        // Every pane starts hidden, and each is revealed by what is already
        // checked rather than by a latch of its own.
        assert!(CAL_STYLE.contains(".cal .pane{display:none}"));
        assert!(CAL_STYLE.contains(".pick:not(:has(.yr-in:checked)) .pane.yrs{display:block}"));
        assert!(CAL_STYLE.contains(
            ".pick:has(.yr-in:checked):not(:has(.mo-in:checked)) .pane.mos{display:block}"
        ));
        assert!(
            CAL_STYLE.contains(
                ".pick:has(.yr-in:checked):has(.mo-in:checked) .pane.days{display:block}"
            )
        );
    }

    /// The two radios that step back a pane, and the fact that they post nothing.
    ///
    /// A `<label>` cannot uncheck a radio, so "return to the months" is spelled
    /// as checking the group's empty-valued member. If either one ever carried a
    /// real value it would submit that value as the date.
    #[test]
    fn each_group_carries_one_empty_valued_radio_and_the_header_points_at_it() {
        let html = built();
        assert!(html.contains("name=\"from_y\" id=\"sfromy0\" value=\"\" class=\"yr-x\""));
        assert!(html.contains("name=\"from_m\" id=\"sfromm0\" value=\"\" class=\"mo-x\""));
        assert!(
            html.contains("<label class=\"hdt\" for=\"sfromy0\">"),
            "the month pane's title steps back to the years"
        );
        assert!(
            html.contains("<label class=\"hdt\" for=\"sfromm0\">"),
            "the day pane's title steps back to the months"
        );
    }

    /// `‹` and `›` target the neighbouring month, and stop at the year boundary.
    ///
    /// A label drives exactly one control, so January's `‹` would have to check
    /// both `m12` and the previous year — it is an inert span instead, and the
    /// title above it is the way across.
    #[test]
    fn the_arrows_step_one_month_and_do_not_cross_a_year_boundary() {
        let html = built();
        for month in 1u8..=12 {
            assert!(
                html.contains(&format!("<span class=\"nav n{month}\">")),
                "month {month} has no arrow pair"
            );
        }
        assert!(html.contains("<label class=\"st pv\" for=\"sfromm7\">‹</label>"));
        assert!(html.contains("<label class=\"st nx\" for=\"sfromm9\">›</label>"));
        assert_eq!(
            html.matches("<span class=\"st pv off\">‹</span>").count(),
            1,
            "exactly one inert `‹`, and it belongs to January"
        );
        assert_eq!(
            html.matches("<span class=\"st nx off\">›</span>").count(),
            1,
            "exactly one inert `›`, and it belongs to December"
        );
        // The inert ones are spans, so there is no `for` to follow at all.
        assert!(
            !html.contains("for=\"sfromm0\">‹"),
            "January must not step to m0"
        );
        assert!(
            !html.contains("for=\"sfromm13\""),
            "December must not step past December"
        );
    }

    /// **THE OVERLAP.** Five panels could be open at once because each had its
    /// own checkbox. One radio group per form means at most one is, so two
    /// panels cannot exist to be laid out on top of each other.
    #[test]
    fn the_latch_is_a_radio_so_two_panels_cannot_be_open_at_once() {
        let html = built();
        assert!(
            html.contains(
                "<input type=\"radio\" name=\"cal\" id=\"o-sfrom\" value=\"\" class=\"popopen\">"
            ),
            "a checkbox here is five independently open panels: {html}"
        );
        assert!(
            !html.contains("type=\"checkbox\""),
            "no latch may be a checkbox"
        );
        // And the way back out, since a radio cannot untick itself.
        assert!(html.contains("<label class=\"shut\" for=\"s-calshut\">Close</label>"));
        let sentinel = shut("s");
        assert!(sentinel.contains("id=\"s-calshut\""));
        assert!(sentinel.contains("checked"), "every panel starts shut");
        assert!(
            sentinel.contains("name=\"cal\""),
            "same group as the latches"
        );
        assert_ne!(shut("s"), shut("f"), "one per form, and distinguishable");
    }

    /// Presence is the parser's job. A `required` on a zero-sized radio inside a
    /// hidden subtree blocks submission and shows nothing — see the comment in
    /// `picker`, and the regression test in `crate::server`.
    #[test]
    fn no_control_in_the_picker_is_required() {
        assert!(!built().contains("required"));
    }

    /// The three radio groups the form actually posts, at their full counts.
    #[test]
    fn the_picker_posts_three_groups_and_fifty_seven_controls() {
        let html = built();
        // 2015..=2026 is twelve years, plus the empty one.
        assert_eq!(html.matches("name=\"from_y\"").count(), 13);
        assert_eq!(html.matches("name=\"from_m\"").count(), 13);
        assert_eq!(html.matches("name=\"from_d\"").count(), 31);
    }

    /// Alignment, impossible days, and both halves of the ceiling.
    #[test]
    fn the_computed_rules_cover_alignment_the_month_end_and_the_ceiling() {
        let css = dynamic_css(&[day(2026, 8, 7)]);
        // 2026-08-01 is a Saturday: column 6, Monday first.
        assert!(css.contains(".pick:has(.y2026:checked):has(.m8:checked) .pad{--s:6}"));

        // ── EVERY RULE BELOW IS ASSERTED WHOLE, AND THAT IS THE POINT ───────
        //
        // These were `contains(".c29")` and friends, which pass whether the
        // rule greys 29..31, 27..31, or 29 alone. **Every boundary in
        // `dynamic_css` survived mutation testing** against that: `length < 31`
        // as `<=`, `length + 1` as `length - 1`, `n > 0` as `n >= 0`. A
        // substring cannot falsify a range. The expected text is built here
        // from what the rule must MEAN, so it does not move when the emitter
        // does.
        let struck = "{opacity:.16;pointer-events:none;text-decoration:line-through}";
        let month_rule = |year: u16, month: u8, days: std::ops::RangeInclusive<u8>| {
            days.map(|d| format!(".pick:has(.y{year}:checked):has(.m{month}:checked) .c{d}"))
                .collect::<Vec<_>>()
                .join(",")
                + struck
        };
        // February 2026 has 28 days: exactly 29, 30 and 31 are struck out.
        assert!(css.contains(&month_rule(2026, 2, 29..=31)), "Feb: {css}");
        // April has 30: exactly the 31st.
        assert!(css.contains(&month_rule(2026, 4, 31..=31)), "Apr: {css}");
        // A 31-DAY MONTH GETS NO RULE AT ALL. Without this, `length < 31` as
        // `<=` emits January a rule whose selector list is empty.
        assert!(
            !css.contains(".pick:has(.y2026:checked):has(.m1:checked) .c"),
            "January has 31 days and nothing to grey: {css}"
        );

        // THE CEILING ON MONTHS — the half that did not exist before. All twelve
        // months are always emitted, so in the cap's own year the ones after it
        // were clickable and led to a grid whose every day the server refused.
        // Exactly September through December, and August itself untouched.
        let months = (9u8..=12)
            .map(|m| format!(".pick.cap20260807:has(.y2026:checked) .k{m}"))
            .collect::<Vec<_>>()
            .join(",")
            + struck;
        assert!(
            css.contains(&months),
            "exactly Sep..Dec are past the cap: {css}"
        );
        assert!(
            !css.contains(".pick.cap20260807:has(.y2026:checked) .k8"),
            "August is the cap's own month and stays reachable: {css}"
        );

        // THE CEILING WITHIN THAT MONTH — exactly the 8th through the 31st, so
        // the 7th itself stays reachable.
        let days = (8u8..=31)
            .map(|d| format!(".pick.cap20260807:has(.y2026:checked):has(.m8:checked) .c{d}"))
            .collect::<Vec<_>>()
            .join(",")
            + "{opacity:.16;pointer-events:none}";
        assert!(css.contains(&days), "exactly 8..31 are past the cap: {css}");
    }

    /// **Every rule this emits is well-formed, and the cap's own day survives.**
    ///
    /// Three mutation-testing results in one test, all of the same kind: an
    /// off-by-one in `dynamic_css` produces CSS that is still a *superset* of
    /// the correct text, so no `contains` of the right rule can falsify it. What
    /// falsifies it is asserting the shapes that must never appear.
    ///
    /// * `n > 0` as `n >= 0` writes the joining comma before the **first**
    ///   selector too, giving `…:checked) ,…`. A selector list may not start
    ///   with a comma, and nothing correct emits one.
    /// * `length < 31` as `<=`, and `month < 12` as `<=`, emit a rule whose
    ///   selector list is **empty** — `…:checked) {opacity…` — for a month with
    ///   nothing to grey. Every real rule has a class between the `)` and the
    ///   `{`.
    /// * `cap.day() + 1` as `- 1` or `* 1` starts the greyed range **at or
    ///   below the ceiling itself**, so the last day an operator is allowed to
    ///   ask for stops being clickable.
    #[test]
    fn no_generated_rule_is_malformed_and_the_ceiling_day_stays_clickable() {
        // Several caps and a leap year, so every branch of the emitter runs —
        // including a **December** cap and a **31st** cap, which are the two
        // that skip a rule entirely and so are the two that can emit an empty
        // one. Without them `month < 12` as `<=` is a mutant nothing here sees:
        // for any earlier month both spellings are true.
        let css = dynamic_css(&[
            day(2026, 8, 7),
            day(2026, 8, 6),
            day(2024, 2, 29),
            day(2026, 12, 31),
            day(2025, 12, 1),
        ]);

        assert!(
            !css.contains(":checked) ,"),
            "a selector list starts with a comma — the join wrote one before \
             the first selector: {css}"
        );
        assert!(
            !css.contains(":checked) {"),
            "a rule has an empty selector list — a month with nothing to grey \
             still emitted one: {css}"
        );
        assert!(
            !css.contains(",{"),
            "a selector list ends with a trailing comma: {css}"
        );

        // THE CEILING'S OWN DAY IS REACHABLE. It is the latest date the field
        // exists to let an operator pick.
        assert!(
            !css.contains(".pick.cap20260807:has(.y2026:checked):has(.m8:checked) .c7"),
            "the 7th IS the cap of 2026-08-07 and must stay clickable: {css}"
        );
        assert!(
            !css.contains(".pick.cap20260806:has(.y2026:checked):has(.m8:checked) .c6"),
            "and the 6th is the cap of 2026-08-06: {css}"
        );
        // The day after each cap is greyed, so the boundary is tight on both
        // sides rather than merely absent.
        assert!(css.contains(".pick.cap20260807:has(.y2026:checked):has(.m8:checked) .c8"));
        assert!(css.contains(".pick.cap20260806:has(.y2026:checked):has(.m8:checked) .c7"));

        // And the cap's own MONTH is reachable while the next one is not.
        assert!(!css.contains(".pick.cap20240229:has(.y2024:checked) .k2"));
        assert!(css.contains(".pick.cap20240229:has(.y2024:checked) .k3"));
    }

    /// A leap February greys 30 and 31 and leaves the 29th alone.
    ///
    /// The only case where the impossible-day rule depends on the *year*, so
    /// it is the one that proves the rule is computed per (year, month) rather
    /// than per month.
    #[test]
    fn a_leap_february_keeps_its_twenty_ninth() {
        let css = dynamic_css(&[day(2024, 8, 7)]);
        assert!(
            css.contains(
                ".pick:has(.y2024:checked):has(.m2:checked) .c30,\
                 .pick:has(.y2024:checked):has(.m2:checked) .c31\
                 {opacity:.16;pointer-events:none;text-decoration:line-through}"
            ),
            "2024 is a leap year: only 30 and 31 are impossible: {css}"
        );
        assert!(
            !css.contains(".pick:has(.y2024:checked):has(.m2:checked) .c29"),
            "the 29th of February 2024 exists and must stay clickable: {css}"
        );
        // And the neighbouring non-leap year in the same stylesheet does strike it.
        assert!(css.contains(".pick:has(.y2023:checked):has(.m2:checked) .c29"));
    }

    /// The month radios carry 1 through 12, and the day radios 1 through 31.
    ///
    /// `index as u8 + 1` reads as obviously right, and `index as u8 * 1`
    /// compiles to a picker whose months are 0 through 11 — every count in the
    /// other tests stays identical, and the form posts a month `Day::new`
    /// refuses. It survived mutation testing until this was written.
    #[test]
    fn the_month_and_day_radios_carry_the_values_a_date_is_built_from() {
        let html = built();
        for month in 1u8..=12 {
            assert!(
                html.contains(&format!(
                    "name=\"from_m\" id=\"sfromm{month}\" value=\"{month}\""
                )),
                "month {month} is missing or misnumbered"
            );
        }
        assert!(
            !html.contains("name=\"from_m\" id=\"sfromm0\" value=\"0\""),
            "there is no month zero; m0 is the empty-valued back-step"
        );
        // Scoped to the month group: `value="13"` on its own is day 13.
        assert!(
            !html.contains("name=\"from_m\" id=\"sfromm13\""),
            "and no thirteenth month"
        );
        for d in 1u8..=31 {
            assert!(
                html.contains(&format!("name=\"from_d\" id=\"sfromd{d}\" value=\"{d}\"")),
                "day {d} is missing or misnumbered"
            );
        }
        for year in 2015u16..=2026 {
            assert!(
                html.contains(&format!(
                    "name=\"from_y\" id=\"sfromy{year}\" value=\"{year}\""
                )),
                "year {year} is missing or misnumbered"
            );
        }
        assert!(!html.contains("value=\"2027\""), "nothing past the ceiling");
        assert!(!html.contains("value=\"2014\""), "nothing before the span");
    }

    /// Both branches the ceiling can skip: a December cap has no later month,
    /// and a cap on the 31st has no later day.
    #[test]
    fn a_cap_at_the_end_of_a_month_or_a_year_emits_no_rule_it_does_not_need() {
        let css = dynamic_css(&[day(2026, 12, 31)]);
        assert!(
            !css.contains(".pick.cap20261231:has(.y2026:checked) .k"),
            "December is the last month: there is nothing after it to grey"
        );
        assert!(
            !css.contains(".pick.cap20261231:has(.y2026:checked):has(.m12:checked) .c"),
            "the 31st is the last day: there is nothing after it to grey"
        );
    }

    /// The closed field reads `6 Aug '26`; the open header reads `August 2026`.
    /// Both are keyed by the same radios, so they cannot name different months.
    #[test]
    fn the_readout_and_the_header_are_driven_by_the_same_radios() {
        let css = dynamic_css(&[day(2026, 8, 7)]);
        assert!(css.contains(".pick:has(.d6:checked) .v-d::after{content:'6'}"));
        assert!(css.contains(".pick:has(.m8:checked) .v-m::after{content:'Aug'}"));
        assert!(css.contains(".pick:has(.m8:checked) .v-mn::after{content:'August'}"));
        assert!(css.contains(".pick:has(.y2026:checked) .v-y::after{content:\"'26\"}"));
        assert!(css.contains(".pick:has(.y2026:checked) .v-y4::after{content:'2026'}"));
    }

    /// Two ceilings on one page share every rule that does not mention a cap.
    #[test]
    fn two_caps_yield_one_alignment_family_and_two_ceilings() {
        let css = dynamic_css(&[day(2026, 8, 7), day(2026, 8, 6)]);
        assert_eq!(
            css.matches(".pick:has(.y2026:checked):has(.m8:checked) .pad{--s:6}")
                .count(),
            1,
            "alignment does not depend on a cap and is emitted once"
        );
        assert!(css.contains("cap20260807"));
        assert!(css.contains("cap20260806"));
    }

    /// No caps is no picker, and no rules — not a panic and not a default year.
    #[test]
    fn a_page_with_no_date_field_gets_no_rules_at_all() {
        assert!(dynamic_css(&[]).is_empty());
    }
}
