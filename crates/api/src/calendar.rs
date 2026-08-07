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
//! | open and close the popover | a checkbox and `:checked ~ .cal` |
//! | remember the click | a radio group, which is what a form control is for |
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
//! on the ingest page. Three independent groups is 12 + 12 + 31 = 55 controls per
//! field, and the alignment rules are shared by every picker on the page rather
//! than emitted once each.
//!
//! `<name>=YYYY-MM-DD` is still accepted by the server, unchanged, so a hand-built
//! POST and every existing test keep working. See [`crate::ingest::parse_day`].

use core::fmt::Write as _;
use pull::session::Day;

/// How many years the picker offers, ending at the field's own ceiling.
///
/// A **picker range, not a data claim.** Nothing here asserts that a vendor sells
/// twelve years of one-minute bars; it is the span an operator can reach without
/// paging, and the store is the only thing that knows what is actually held.
pub const YEARS_OFFERED: u16 = 12;

/// Month names, as Dhan and `TradingView` both write them.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
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
#[must_use]
pub const fn month_length(year: u16, month: u8) -> u8 {
    let mut probe = 31;
    while probe > 28 {
        if Day::new(year, month, probe).is_ok() {
            return probe;
        }
        probe -= 1;
    }
    28
}

/// The earliest year the picker offers for a field capped at `max`.
#[must_use]
pub const fn first_year(max: Day) -> u16 {
    max.year().saturating_sub(YEARS_OFFERED - 1)
}

/// One date picker: the readout, the popover, and every control inside it.
///
/// `name` becomes three radio groups — `{name}_y`, `{name}_m`, `{name}_d` — and
/// `id` scopes the popover's checkbox so two pickers on one page cannot toggle
/// each other. Nothing later than `max` can be clicked.
#[must_use]
pub fn picker(name: &str, id: &str, max: Day) -> String {
    let mut out = String::with_capacity(6144);
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
    // `api::ingest::parse_day_field` returns `Refusal::FieldMissing` when all
    // three parts are absent, and the refusal is rendered on the result page
    // with the field named. An attribute is a courtesy; a parser is the rule —
    // and here the courtesy was silently overruling the rule.
    //
    // The popover's latch. A checkbox is the only element in HTML that
    // remembers a two-state click without a script, and `:checked ~ .cal` is
    // what turns that memory into a visible panel.
    let _ = write!(
        out,
        "<input type=\"checkbox\" id=\"o-{id}\" class=\"popopen\">\
         <label class=\"readout\" for=\"o-{id}\">\
         <span class=\"v-ph\">Pick a date</span>\
         <span class=\"v-d\"></span><span class=\"v-m\"></span><span class=\"v-y\"></span>\
         <span class=\"v-ico\">▦</span></label>"
    );

    out.push_str("<div class=\"cal\">");

    // ---- YEARS -----------------------------------------------------------
    out.push_str("<div class=\"strip yr\">");
    let first = first_year(max);
    for year in first..=max.year() {
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"{name}_y\" id=\"{id}y{year}\" value=\"{year}\" \
             class=\"y{year}\">\
             <label for=\"{id}y{year}\">{year}</label>"
        );
    }
    out.push_str("</div>");

    // ---- MONTHS ----------------------------------------------------------
    out.push_str("<div class=\"strip mo\">");
    for (index, label) in MONTHS.iter().enumerate() {
        // `index` is 0..12, so this cast is exact and the month is 1..=12.
        #[allow(clippy::cast_possible_truncation)]
        let month = index as u8 + 1;
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"{name}_m\" id=\"{id}m{month}\" value=\"{month}\" \
             class=\"m{month}\">\
             <label for=\"{id}m{month}\">{label}</label>"
        );
    }
    out.push_str("</div>");

    // ---- THE GRID --------------------------------------------------------
    out.push_str("<div class=\"dow\">");
    for day in WEEKDAYS {
        let _ = write!(out, "<span>{day}</span>");
    }
    out.push_str("</div><div class=\"pad\">");
    for day in 1u8..=31 {
        let _ = write!(
            out,
            "<input type=\"radio\" name=\"{name}_d\" id=\"{id}d{day}\" value=\"{day}\" \
             class=\"d{day}\">\
             <label for=\"{id}d{day}\" class=\"c{day}\">{day}</label>"
        );
    }
    out.push_str("</div>");

    let _ = write!(
        out,
        "<p class=\"calnote\">Latest that can be asked for: <b>{max}</b>. \
         Days that do not exist in the month you picked grey out — there is no \
         31 February here and no 29 February outside a leap year.</p>"
    );
    out.push_str("</div></div>");
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
/// Three families, all shared by every picker on the page:
///
/// 1. **Alignment.** For each (year, month) offered, which column the 1st sits
///    in. This is the rule that makes the thing a calendar rather than a pad of
///    numbers.
/// 2. **Impossible days.** For each (year, month), the day cells past the end of
///    that month, greyed and unclickable.
/// 3. **The readout.** `6`, `Aug` and `'26` as `::after` content keyed by which
///    radio is checked, so the field reads back in Dhan's own format while the
///    wire still carries three integers.
///
/// `caps` is the distinct set of ceilings on the page. Passing them in rather
/// than assuming keeps this honest when a page grows a third one.
#[must_use]
pub fn dynamic_css(caps: &[Day]) -> String {
    let mut out = String::with_capacity(32_768);

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

    // The ceiling. Years and months past it are struck out in the markup; the
    // only case that needs a computed rule is the ceiling's own month, where
    // some days are reachable and some are not.
    for cap in caps {
        if cap.day() >= 31 {
            continue;
        }
        let class = cap_class(*cap);
        let (year, month) = (cap.year(), cap.month());
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

    // ---- THE READOUT -----------------------------------------------------
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
    for year in low..=high {
        let _ = write!(
            out,
            ".pick:has(.y{year}:checked) .v-y::after{{content:\"'{:02}\"}}",
            year % 100
        );
    }
    out
}

/// The part of the picker's stylesheet that no arithmetic could change.
pub const CAL_STYLE: &str = "\
.pick{position:relative;display:block}\
.pick input{position:absolute;opacity:0;width:0;height:0;pointer-events:none}\
.readout{display:flex;align-items:center;gap:6px;cursor:pointer;user-select:none;\
font:inherit;font-variant-numeric:tabular-nums;padding:10px 13px;border:1px solid var(--line);\
border-radius:11px;background:var(--panel);color:var(--ink);box-shadow:var(--sh);\
transition:border-color .2s,box-shadow .2s,transform .2s}\
.readout:hover{border-color:var(--acc);transform:translateY(-1px)}\
.pick .v-ph{color:var(--dim)}\
.pick:has(input:checked) .v-ph{display:none}\
.pick .v-ico{margin-left:auto;color:var(--acc);font-size:15px}\
.pick .v-d,.pick .v-m,.pick .v-y{font-weight:700}\
.popopen:checked~.readout{border-color:var(--acc);\
box-shadow:0 0 0 4px color-mix(in srgb,var(--acc) 18%,transparent)}\
.popopen:checked~.readout .v-ico{transform:rotate(90deg)}\
.cal{display:none;position:absolute;z-index:40;top:calc(100% + 8px);left:0;min-width:290px;\
background:var(--panel);border:1px solid var(--line);border-radius:15px;padding:12px;\
box-shadow:0 18px 48px rgba(16,24,40,.18),var(--sh)}\
.popopen:checked~.cal{display:block;animation:calin .22s cubic-bezier(.2,.8,.2,1) both}\
@keyframes calin{from{opacity:0;transform:translateY(-6px) scale(.98)}}\
.cal .strip{display:flex;flex-wrap:wrap;gap:4px;margin-bottom:9px}\
.cal .strip label{cursor:pointer;font-size:12px;font-weight:650;padding:5px 9px;border-radius:8px;\
border:1px solid transparent;color:var(--dim);transition:all .16s;font-variant-numeric:tabular-nums}\
.cal .strip label:hover{background:color-mix(in srgb,var(--acc) 10%,transparent);color:var(--ink)}\
.cal .strip input:checked+label{background:linear-gradient(135deg,var(--acc),var(--acc2));\
color:#fff;border-color:transparent;box-shadow:0 3px 10px color-mix(in srgb,var(--acc) 34%,transparent)}\
.cal .yr{border-bottom:1px solid var(--line);padding-bottom:9px}\
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
.cal .off{opacity:.2;pointer-events:none;text-decoration:line-through}\
.cal .calnote{font-size:11px;line-height:1.45;color:var(--dim);margin-top:10px;\
border-top:1px solid var(--line);padding-top:8px}\
.cal .calnote b{color:var(--ink);font-variant-numeric:tabular-nums}\
@media(prefers-reduced-motion:reduce){.popopen:checked~.cal{animation:none}\
.readout:hover,.cal .pad label:hover{transform:none}}\
";
