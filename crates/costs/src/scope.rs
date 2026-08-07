//! Which round trips bear the charge stack, and which are priced signal-only.
//!
//! The predecessor's `brutex/costs/scope.py` states one operator-locked rule
//! (`DEC-COST-SCOPE-INDEX-SIGNAL-ONLY-001`), and it is exactly this:
//!
//! ```text
//! cost_free  ==  the underlying is an INDEX  AND  the segment is NOT an option
//! ```
//!
//! An index has no order book of its own — a spot index cannot be bought — so a
//! directional research signal on it carries no brokerage, no statutory levy
//! and no tick slippage. An index **option** is a different animal: it is a
//! tradable premium instrument with a real bid and ask, so it bears the whole
//! stack. Index futures are admitted to the signal-only side by the same
//! operator decision.
//!
//! # Cost-free is about the CHARGES, never about the fill
//!
//! A signal-only round trip is still filled off the worst-case bar extremes
//! ([`crate::fill`]). "Cost-free" removes the charge stack and nothing else —
//! there is no second fill law, and a cost-free trip's gross profit and loss is
//! computed from exactly the same two fills a cost-bearing one would use. That
//! is why [`crate::trip::price`] short-circuits **after** the fills and the
//! notionals, not before them.
//!
//! # What is deliberately absent
//!
//! The source's rule ranges over single stocks too: every stock, in every
//! segment, is cost-bearing. That half of the rule is **not** expressed here as
//! a variant, because this crate cannot price a single stock — it holds no
//! single-name lot table and no single-name rates, and
//! [`crate::venue::swept_slot`] refuses anything outside `CLAUDE.md` §1's two
//! NSE indices before a segment is ever consulted. Adding a `SingleName`
//! variant would be a scope claim the crate cannot honour. `docs/06-limits.md`
//! §27.
//!
//! # Constant per-operation cost
//!
//! One `match` over a three-variant enum. No table, no set, no allocation.

/// What is being traded on one of the two swept indices.
///
/// The underlying is fixed by [`crate::venue::SweptSlot`], so this is the only
/// remaining axis of the source's rule: whether the traded thing is an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    /// The index level itself. Signal-only.
    IndexSpot,
    /// A futures contract on the index. Signal-only, by operator decision.
    IndexFuture,
    /// An option on the index. **Cost-bearing** — this is the whole subject of
    /// this crate.
    IndexOption,
}

impl Segment {
    /// The segment's name, for a refusal or a breakdown line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IndexSpot => "index spot",
            Self::IndexFuture => "index future",
            Self::IndexOption => "index option",
        }
    }

    /// Whether the traded thing is an option.
    ///
    /// The source's rule turns on exactly this, so it is written down once and
    /// [`is_cost_free`] reads it rather than repeating the list.
    #[must_use]
    pub const fn is_option(self) -> bool {
        matches!(self, Self::IndexOption)
    }
}

impl core::fmt::Display for Segment {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a round trip in this segment is priced signal-only: zero brokerage,
/// zero statutory levies, zero everything.
///
/// # Examples
///
/// ```
/// use costs::scope::{Segment, is_cost_free};
///
/// assert!(is_cost_free(Segment::IndexSpot));
/// assert!(is_cost_free(Segment::IndexFuture));
/// // An index OPTION is a tradable premium instrument, and bears the stack.
/// assert!(!is_cost_free(Segment::IndexOption));
/// ```
#[must_use]
pub const fn is_cost_free(segment: Segment) -> bool {
    // The underlying is an index in every variant this enum has, so the rule
    // reduces to its second clause. Written as the negation of `is_option`
    // rather than as its own list, so the two can never disagree.
    !segment.is_option()
}

/// The exact complement of [`is_cost_free`].
///
/// # Examples
///
/// ```
/// use costs::scope::{Segment, is_cost_bearing};
///
/// assert!(is_cost_bearing(Segment::IndexOption));
/// assert!(!is_cost_bearing(Segment::IndexSpot));
/// ```
#[must_use]
pub const fn is_cost_bearing(segment: Segment) -> bool {
    !is_cost_free(segment)
}

/// Every segment this crate knows, in a fixed array.
///
/// Test-only. The shipped code never enumerates segments — it matches on the
/// one it was handed.
#[cfg(test)]
pub(crate) const ALL_SEGMENTS: [Segment; 3] = [
    Segment::IndexSpot,
    Segment::IndexFuture,
    Segment::IndexOption,
];

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn only_the_option_segment_bears_the_charge_stack() {
        assert!(is_cost_free(Segment::IndexSpot));
        assert!(is_cost_free(Segment::IndexFuture));
        assert!(!is_cost_free(Segment::IndexOption));
        assert!(!is_cost_bearing(Segment::IndexSpot));
        assert!(!is_cost_bearing(Segment::IndexFuture));
        assert!(is_cost_bearing(Segment::IndexOption));
    }

    #[test]
    fn the_two_predicates_are_exact_complements_on_every_segment() {
        let mut checked = 0_u32;
        for segment in ALL_SEGMENTS {
            assert_ne!(
                is_cost_free(segment),
                is_cost_bearing(segment),
                "{segment} is both or neither"
            );
            // And the rule really is "not an option" — not a list that could
            // drift from `is_option`.
            assert_eq!(is_cost_free(segment), !segment.is_option());
            assert_eq!(is_cost_bearing(segment), segment.is_option());
            checked += 1;
        }
        assert_eq!(checked, 3);
    }

    #[test]
    fn a_segment_names_itself_and_the_three_are_distinct() {
        assert_eq!(Segment::IndexSpot.as_str(), "index spot");
        assert_eq!(Segment::IndexFuture.as_str(), "index future");
        assert_eq!(Segment::IndexOption.as_str(), "index option");
        assert_eq!(Segment::IndexOption.to_string(), "index option");
        assert_eq!(format!("{:?}", Segment::IndexFuture), "IndexFuture");
        assert_ne!(Segment::IndexSpot, Segment::IndexFuture);
        assert!(Segment::IndexSpot < Segment::IndexOption);
        let names: [&str; 3] = [
            ALL_SEGMENTS[0].as_str(),
            ALL_SEGMENTS[1].as_str(),
            ALL_SEGMENTS[2].as_str(),
        ];
        assert_eq!(names, ["index spot", "index future", "index option"]);
    }
}
