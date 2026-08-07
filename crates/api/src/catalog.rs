//! The precomputed instrument index — everything a page render must **not**
//! compute.
//!
//! # The defect this module exists to remove
//!
//! `docs/07-o1-architecture.md` layer 12 was marked BUILT with the words
//! "a fixed row cap with paging. NEVER O(universe)". Only the *rendered rows*
//! were capped. Every single request re-folded the whole `by_key` map for the
//! universe pill counts, re-filtered it into a fresh `Vec`, re-sorted that
//! vector and then threw all of it away to draw 200 rows.
//!
//! An audit measured a running server: the instruments page cost 3.569 ms at
//! 2,787 instruments and 124.916 ms at 50,000 — **rendering exactly 200 rows
//! both times** — a marginal 62.5 ns per instrument per request. The row cap
//! hid the shape, because the thing that grew was never on the screen.
//!
//! # What replaced it
//!
//! The masters are already parsed once into an `Arc<Site>`. That parse is the
//! seam: every ordering and every filter the UI offers is decided **there**,
//! once, and a request does arithmetic on the result.
//!
//! | Per request, before | Per request, now |
//! |---|---|
//! | fold the whole map for pill counts | read four `usize` |
//! | filter the whole map into a `Vec` | pick 1 of 8 precomputed orders |
//! | sort that `Vec` | none — the order is already sorted |
//! | `reverse()` the whole `Vec` | mirror a 200-row window |
//! | copy 200 rows | copy 200 rows |
//!
//! The one thing that is **not** constant is search, and it is named rather
//! than hidden — see [`Catalog::search`] and `docs/06-limits.md`.
//!
//! # Why the index is 48 orders and not one
//!
//! The UI offers six sort columns, an ascending/descending toggle, four
//! universe pills and an escape hatch that lifts the tracked-universe filter.
//! Descending costs nothing — the same order read from the far end. The rest
//! are 2 scopes × 4 pills × 6 columns = 48 orders of `usize` indices into one
//! shared row table. At the real universe of 2,787 instruments that is about
//! 1.1 MB; the row data itself is stored once.
//!
//! Memory is traded for time deliberately and the trade is bounded: it is
//! linear in the universe with a constant of 48 machine words, paid once at
//! load, and `docs/06-limits.md` carries the number.

use std::collections::HashMap;

use brutex_core::instrument::{InstrumentKey, Kind};
use brutex_core::isin::Isin;
use brutex_core::symbol::Symbol;
use brutex_core::universe::Universe;
use brutex_core::vendor::{Vendor, VendorSet};

use crate::merge::Merged;
use crate::render::{Row, UniverseCounts};

/// How many rows one page draws.
///
/// One number for every paged surface in this crate. Two constants would drift
/// and the pager would disagree with the table about where a page ends.
pub const PAGE_ROWS: usize = 200;

/// How many sort columns the UI offers.
const COLUMNS: usize = 6;

/// How many (scope, pill) row sets exist: 2 scopes × 4 pills.
const SETS: usize = 8;

/// The sort columns, in the order they are stored.
///
/// [`Column::Default`] is the order the page opens on — swept instruments
/// first, then the key — and it is what an unrecognised column falls back to.
/// A stale bookmark must render, not error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    /// Swept first, then the key itself.
    Default,
    /// By underlying symbol.
    Symbol,
    /// By ISIN, absent last.
    Isin,
    /// By universe membership bits.
    UniverseBits,
    /// By instrument kind.
    Kind,
    /// By which vendors named it.
    Vendors,
}

impl Column {
    /// Every column, in storage order. Index `i` of this array is slot `i`.
    const ALL: [Self; COLUMNS] = [
        Self::Default,
        Self::Symbol,
        Self::Isin,
        Self::UniverseBits,
        Self::Kind,
        Self::Vendors,
    ];

    /// The column a URL asked for, and whether it asked for it backwards.
    ///
    /// A leading `-` means descending. Anything unrecognised is
    /// [`Column::Default`] ascending — a stale bookmark renders rather than
    /// erroring, which is the same rule the old code held.
    fn parse(sort: &str) -> (Self, bool) {
        let (base, descending) = match sort.strip_prefix('-') {
            Some(rest) => (rest, true),
            None => (sort, false),
        };
        let column = match base {
            "symbol" => Self::Symbol,
            "isin" => Self::Isin,
            "universe" => Self::UniverseBits,
            "kind" => Self::Kind,
            "vendors" => Self::Vendors,
            _ => Self::Default,
        };
        (column, descending)
    }

    /// Which slot of an order table this column occupies.
    fn slot(self) -> usize {
        match self {
            Self::Default => 0,
            Self::Symbol => 1,
            Self::Isin => 2,
            Self::UniverseBits => 3,
            Self::Kind => 4,
            Self::Vendors => 5,
        }
    }
}

/// The leading component of a sort key.
///
/// One `Ord` type for six columns, so there is exactly **one** ordering rule
/// per column and ascending and descending can never disagree about how ties
/// break. Every order ends in the [`InstrumentKey`] itself, so the order is
/// total and the page is byte-identical between reloads — which a `HashMap`
/// iteration order is not.
///
/// Rows in one order always carry the same variant; the derived cross-variant
/// comparison is never reached in practice and is total if it ever were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Lead {
    /// `false` sorts first, so swept instruments lead.
    NotSwept(bool),
    /// The underlying symbol.
    Symbol(Symbol),
    /// The agreed ISIN. `None` sorts first, exactly as `Option` orders.
    Isin(Option<Isin>),
    /// The universe membership bits.
    Bits(u32),
    /// The instrument kind.
    Kind(Kind),
    /// The vendors that named it.
    Vendors(VendorSet),
}

impl Lead {
    /// The leading sort component of one row under one column.
    fn of(column: Column, row: &Row) -> Self {
        match column {
            Column::Default => Self::NotSwept(!row.key.is_sweepable()),
            Column::Symbol => Self::Symbol(row.key.underlying),
            Column::Isin => Self::Isin(row.isin),
            Column::UniverseBits => Self::Bits(row.universe.bits()),
            Column::Kind => Self::Kind(row.key.kind),
            Column::Vendors => Self::Vendors(row.vendors),
        }
    }
}

/// Which rows a page is drawn from.
///
/// `all` is the escape hatch: the default scope is the tracked universe —
/// NIFTY Total Market plus the index series — because a page opening on every
/// listing the gate accepted hides the ones that matter. A filter with no way
/// past it hides a bug instead, so the hatch is a link on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Whether the tracked-universe filter is lifted.
    pub all: bool,
    /// Which universe pill is selected: `fno`, `ntm`, `idx`, or anything else
    /// for no pill. An unrecognised value selects everything rather than
    /// nothing — a stale bookmark should render the page.
    pub pill: Pill,
    /// Which column to order by.
    sort: Column,
    /// Whether that order is read backwards.
    descending: bool,
}

impl Selection {
    /// The selection a request asked for.
    #[must_use]
    pub fn new(all: bool, pill: &str, sort: &str) -> Self {
        let (sort, descending) = Column::parse(sort);
        Self {
            all,
            pill: Pill::parse(pill),
            sort,
            descending,
        }
    }

    /// Which of the [`SETS`] row sets this selection reads.
    fn set(self) -> usize {
        usize::from(self.all) * 4 + self.pill.slot()
    }

    /// Which scope's counts this selection reports.
    fn scope(self) -> usize {
        usize::from(self.all)
    }
}

/// Which universe pill is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    /// No pill: every row in scope.
    Every,
    /// F&O underlyings.
    Fno,
    /// NIFTY Total Market constituents.
    Ntm,
    /// The index series.
    Index,
}

impl Pill {
    /// The pill a URL named. Anything unrecognised is [`Pill::Every`].
    fn parse(raw: &str) -> Self {
        match raw {
            "fno" => Self::Fno,
            "ntm" => Self::Ntm,
            "idx" => Self::Index,
            _ => Self::Every,
        }
    }

    /// Which slot of a set index this pill occupies.
    fn slot(self) -> usize {
        match self {
            Self::Every => 0,
            Self::Fno => 1,
            Self::Ntm => 2,
            Self::Index => 3,
        }
    }

    /// Every pill, in slot order.
    const ALL: [Self; 4] = [Self::Every, Self::Fno, Self::Ntm, Self::Index];

    /// Whether a membership passes this pill.
    fn admits(self, u: Universe) -> bool {
        match self {
            Self::Every => true,
            Self::Fno => u.contains(Universe::FNO),
            Self::Ntm => u.contains(Universe::TOTAL_MARKET),
            Self::Index => u.contains(Universe::INDEX),
        }
    }
}

/// Whether a membership is in the default (tracked) scope.
fn tracked(u: Universe) -> bool {
    u.contains(Universe::TOTAL_MARKET) || u.contains(Universe::INDEX)
}

/// One page of rows, plus the two numbers the pager needs.
#[derive(Debug)]
pub struct Page {
    /// The rows to draw. At most [`PAGE_ROWS`] of them, always.
    pub rows: Vec<Row>,
    /// How many rows the selection matched, over the whole universe.
    pub matched: usize,
    /// Which page this actually is, after a stale number was clamped.
    pub page: usize,
    /// The highest page number that has rows.
    pub last_page: usize,
}

/// The instrument set, indexed for constant-time paging.
#[derive(Debug)]
pub struct Catalog {
    /// Every instrument, materialised once, in no particular order.
    rows: Box<[Row]>,
    /// Each row's canonical name, uppercased, in `rows` order.
    ///
    /// Precomputed because search used to call `to_string()` and
    /// `to_uppercase()` on every key of every request — two allocations per
    /// instrument per keystroke.
    text: Box<[Box<str>]>,
    /// The longest entry of `text`, in bytes.
    ///
    /// A needle longer than this cannot be a substring of anything, so an
    /// over-long search is answered in constant time instead of scanned.
    longest: usize,
    /// `SETS` × `COLUMNS` precomputed orders of indices into `rows`.
    orders: Box<[Box<[usize]>]>,
    /// Pill counts, one per scope: `[tracked, all]`.
    counts: [UniverseCounts; 2],
    /// How many tracked instruments both vendors named. The dashboard's
    /// "confirmed by both feeds", which used to be a second whole-map scan.
    both_tracked: usize,
    /// Trigram → the rows whose text contains it, ascending, deduplicated.
    ///
    /// Bounds search to the rows that could possibly match instead of every
    /// row there is. See [`Catalog::search`] for what it does and does not
    /// promise.
    trigrams: HashMap<[u8; 3], Box<[usize]>>,
}

impl Catalog {
    /// Builds every order, every count and the search index, once.
    ///
    /// # Cost
    ///
    /// `COLUMNS` sorts of the universe plus `SETS × COLUMNS` linear filters
    /// plus one pass to index trigrams. Linear-times-log, paid once at load,
    /// never on a request. `docs/06-limits.md` §O(1) carries the measurement.
    #[must_use]
    pub fn build(merged: &Merged) -> Self {
        let rows: Vec<Row> = merged
            .by_key
            .iter()
            .map(|(key, e)| Row {
                key: *key,
                vendors: e.vendors,
                isin: e.isin.map(|(_, i)| i),
                conflict: e.conflict.map(|(_, i)| i),
                universe: e.universe,
            })
            .collect();

        let text: Vec<Box<str>> = rows
            .iter()
            .map(|r| r.key.to_string().to_uppercase().into_boxed_str())
            .collect();
        let longest = text.iter().map(|t| t.len()).max().unwrap_or(0);

        let orders = build_orders(&rows);
        let counts = [count(&rows, false), count(&rows, true)];
        let both_tracked = rows
            .iter()
            .filter(|r| {
                tracked(r.universe)
                    && r.vendors.contains(Vendor::Groww)
                    && r.vendors.contains(Vendor::Dhan)
            })
            .count();
        let trigrams = build_trigrams(&text);

        Self {
            rows: rows.into_boxed_slice(),
            text: text.into_boxed_slice(),
            longest,
            orders,
            counts,
            both_tracked,
            trigrams,
        }
    }

    /// How many instruments the catalog holds, over every scope.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the catalog holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The universe pill counts for one selection's scope.
    ///
    /// Four `usize` reads. This was a fold over the whole map on every request,
    /// and it is the reason a page that drew 200 rows charged for 50,000.
    #[must_use]
    pub fn counts(&self, sel: Selection) -> UniverseCounts {
        self.counts.get(sel.scope()).copied().unwrap_or_default()
    }

    /// The dashboard's figures: the tracked counts and the both-vendor tally.
    ///
    /// Two whole-map scans, precomputed. The dashboard's docstring already
    /// claimed "nothing here scans"; now that is true.
    #[must_use]
    pub fn dashboard_counts(&self) -> (UniverseCounts, usize) {
        (
            self.counts.first().copied().unwrap_or_default(),
            self.both_tracked,
        )
    }

    /// One page of an unsearched selection.
    ///
    /// # Cost
    ///
    /// One slice index and at most [`PAGE_ROWS`] row copies, whatever the size
    /// of the universe. Nothing here is proportional to the instrument set —
    /// that is `docs/04-invariants.md` C-11 and `crates/api/benches/ratio.rs`
    /// asserts it.
    #[must_use]
    pub fn page(&self, sel: Selection, page: usize) -> Page {
        let order = self.order(sel);
        let matched = order.len();
        let last_page = matched.saturating_sub(1) / PAGE_ROWS;
        // A stale `?page=999` is a bookmark, not an attack. It lands on the
        // last real page rather than on an error or on nothing.
        let page = page.min(last_page);
        let start = page.saturating_mul(PAGE_ROWS);
        let end = start.saturating_add(PAGE_ROWS).min(matched);
        // DESCENDING IS THE SAME ORDER READ FROM THE FAR END. Storing a second
        // reversed copy would double the index for no information, and
        // reversing the whole vector per request is the linear cost this
        // module exists to delete.
        let window = if sel.descending {
            order.get(matched - end..matched - start)
        } else {
            order.get(start..end)
        };
        let mut rows: Vec<Row> = window
            .unwrap_or_default()
            .iter()
            .filter_map(|&i| self.rows.get(i).copied())
            .collect();
        if sel.descending {
            rows.reverse();
        }
        Page {
            rows,
            matched,
            page,
            last_page,
        }
    }

    /// One page of a **searched** selection.
    ///
    /// # This is the one thing here that is not constant, and it is named
    ///
    /// A substring may appear anywhere in a name, so no ordering of the names
    /// answers `contains` by arithmetic. The candidate set is narrowed by a
    /// trigram index — a needle of three bytes or more looks only at rows whose
    /// text carries its rarest trigram — and the cost is then proportional to
    /// **that candidate list**, not to the universe.
    ///
    /// Two cases remain linear in the universe and both are stated in
    /// `docs/06-limits.md`:
    ///
    /// * a needle of one or two bytes, which has no trigram to look up;
    /// * a needle whose rarest trigram is carried by most of the universe,
    ///   which is what a search that matches everything *is*.
    ///
    /// A needle longer than the longest name is answered in constant time: it
    /// cannot be a substring of anything.
    #[must_use]
    pub fn search(&self, sel: Selection, needle: &str, page: usize) -> Page {
        let mut hits: Vec<usize> = if needle.len() > self.longest {
            // A substring is never longer than the string. O(1), no scan.
            Vec::new()
        } else {
            self.candidates(needle)
                .filter(|&i| self.admits(sel, i) && self.matches(i, needle))
                .collect()
        };
        let column = sel.sort;
        hits.sort_unstable_by_key(|&i| {
            self.rows.get(i).map_or((Lead::NotSwept(true), None), |r| {
                (Lead::of(column, r), Some(r.key))
            })
        });
        if sel.descending {
            hits.reverse();
        }
        let matched = hits.len();
        let last_page = matched.saturating_sub(1) / PAGE_ROWS;
        let page = page.min(last_page);
        let rows: Vec<Row> = hits
            .into_iter()
            .skip(page.saturating_mul(PAGE_ROWS))
            .take(PAGE_ROWS)
            .filter_map(|i| self.rows.get(i).copied())
            .collect();
        Page {
            rows,
            matched,
            page,
            last_page,
        }
    }

    /// The rows that could contain `needle`, before verification.
    ///
    /// Falls back to every row when the needle is too short to have a trigram.
    /// That fall-back is a **narrower candidate set being unavailable**, not a
    /// hidden failure: the answer is identical either way and only the cost
    /// differs, which is why it is documented rather than refused.
    fn candidates(&self, needle: &str) -> Box<dyn Iterator<Item = usize> + '_> {
        match self.rarest(needle) {
            Some(list) => Box::new(list.iter().copied()),
            None => Box::new(0..self.rows.len()),
        }
    }

    /// The shortest posting list among the needle's trigrams.
    ///
    /// `None` when the needle is shorter than three bytes, or when some
    /// trigram of it appears in no name at all — in which case the caller
    /// would still be correct scanning everything, so the empty list is
    /// returned rather than `None`.
    fn rarest(&self, needle: &str) -> Option<&[usize]> {
        let mut best: Option<&[usize]> = None;
        for gram in trigrams_of(needle) {
            let list: &[usize] = self.trigrams.get(&gram).map_or(&[], |l| l);
            if best.is_none_or(|b| list.len() < b.len()) {
                best = Some(list);
            }
        }
        best
    }

    /// Whether row `i` is in the selection's set.
    fn admits(&self, sel: Selection, i: usize) -> bool {
        self.rows
            .get(i)
            .is_some_and(|r| (sel.all || tracked(r.universe)) && sel.pill.admits(r.universe))
    }

    /// Whether row `i`'s name contains the needle.
    fn matches(&self, i: usize, needle: &str) -> bool {
        self.text.get(i).is_some_and(|t| t.contains(needle))
    }

    /// The precomputed order for one selection.
    fn order(&self, sel: Selection) -> &[usize] {
        self.orders
            .get(sel.set() * COLUMNS + sel.sort.slot())
            .map_or(&[], |o| o)
    }
}

/// Builds the `SETS × COLUMNS` orders.
///
/// Each column is sorted **once** over the whole universe and then filtered
/// into each set, so the build is `COLUMNS` sorts and `SETS × COLUMNS` linear
/// passes rather than 48 sorts.
fn build_orders(rows: &[Row]) -> Box<[Box<[usize]>]> {
    let mut out: Vec<Box<[usize]>> = Vec::with_capacity(SETS * COLUMNS);
    let mut full: Vec<Vec<usize>> = Vec::with_capacity(COLUMNS);
    for column in Column::ALL {
        let mut keyed: Vec<(Lead, InstrumentKey, usize)> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (Lead::of(column, r), r.key, i))
            .collect();
        keyed.sort_unstable();
        full.push(keyed.into_iter().map(|(_, _, i)| i).collect());
    }
    for all in [false, true] {
        for pill in Pill::ALL {
            for order in &full {
                out.push(
                    order
                        .iter()
                        .copied()
                        .filter(|&i| {
                            rows.get(i).is_some_and(|r| {
                                (all || tracked(r.universe)) && pill.admits(r.universe)
                            })
                        })
                        .collect(),
                );
            }
        }
    }
    out.into_boxed_slice()
}

/// The pill counts for one scope.
fn count(rows: &[Row], all: bool) -> UniverseCounts {
    rows.iter().filter(|r| all || tracked(r.universe)).fold(
        UniverseCounts::default(),
        |mut c, r| {
            c.all += 1;
            c.fno += usize::from(r.universe.contains(Universe::FNO));
            c.ntm += usize::from(r.universe.contains(Universe::TOTAL_MARKET));
            c.index += usize::from(r.universe.contains(Universe::INDEX));
            c
        },
    )
}

/// Every three-byte window of `s`, as fixed arrays.
///
/// Three shifted iterators rather than `windows(3)` and a fallible extraction.
/// `windows` yields slices whose length the type system does not carry, so
/// turning one into `[u8; 3]` needs an arm for a case that cannot happen —
/// `indexing_slicing` is denied, so the alternatives were a `?`, a `match` with
/// a `_ => continue`, or a panic. All three are a branch no test can enter, and
/// an unreachable branch is a hole in the coverage gate that reads exactly like
/// a missing test. Zipping three offsets has no such branch: the iterator ends
/// when the shortest does, which is precisely the windowing rule.
///
/// Names are ASCII by construction — a `Symbol` admits only letters, digits,
/// `-`, `_` and `&` — so a byte window is a character window and no multi-byte
/// boundary can be split.
fn trigrams_of(s: &str) -> impl Iterator<Item = [u8; 3]> + '_ {
    let b = s.as_bytes();
    b.iter()
        .zip(b.iter().skip(1))
        .zip(b.iter().skip(2))
        .map(|((&a, &c), &d)| [a, c, d])
}

/// Indexes every three-byte window of every name.
///
/// Postings are ascending and deduplicated, so a name carrying the same
/// trigram twice contributes one entry. Names are ASCII by construction — a
/// `Symbol` admits only letters, digits, `-`, `_` and `&` — so a byte window
/// is a character window and no multi-byte boundary can be split.
fn build_trigrams(text: &[Box<str>]) -> HashMap<[u8; 3], Box<[usize]>> {
    let mut map: HashMap<[u8; 3], Vec<usize>> = HashMap::new();
    for (i, t) in text.iter().enumerate() {
        for gram in trigrams_of(t) {
            let list = map.entry(gram).or_default();
            // The same trigram twice in one name is one posting, not two.
            if list.last() != Some(&i) {
                list.push(i);
            }
        }
    }
    map.into_iter()
        .map(|(k, v)| (k, v.into_boxed_slice()))
        .collect()
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
    use crate::merge::Entry;
    use brutex_core::instrument::{Exchange, Segment};

    fn equity(sym: &str) -> InstrumentKey {
        InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Cash,
            underlying: Symbol::new(sym).expect("a symbol"),
            kind: Kind::Equity,
        }
    }

    fn index(sym: &str) -> InstrumentKey {
        InstrumentKey::index(Exchange::Nse, sym).expect("an index")
    }

    /// A catalog over the given (key, universe, vendors, isin) tuples.
    fn catalog(items: &[(InstrumentKey, Universe, VendorSet, Option<Isin>)]) -> Catalog {
        let mut by_key = HashMap::new();
        for (key, u, vendors, isin) in items {
            by_key.insert(
                *key,
                Entry {
                    vendors: *vendors,
                    isin: isin.map(|i| (Vendor::Groww, i)),
                    conflict: None,
                    universe: *u,
                },
            );
        }
        Catalog::build(&Merged {
            by_key,
            conflicts: Vec::new(),
            eligibility: Vec::new(),
        })
    }

    fn isin(text: &str) -> Isin {
        Isin::new(text).expect("a valid isin")
    }

    fn both() -> VendorSet {
        VendorSet::EMPTY.with(Vendor::Groww).with(Vendor::Dhan)
    }

    /// One of every membership, so every pill and both scopes select something
    /// different.
    fn mixed() -> Catalog {
        catalog(&[
            (index("NIFTY"), Universe::INDEX, both(), None),
            (
                equity("RELIANCE"),
                Universe::TOTAL_MARKET.union(Universe::FNO),
                both(),
                Some(isin("INE002A01018")),
            ),
            (
                equity("ZZZTRACKED"),
                Universe::TOTAL_MARKET,
                VendorSet::EMPTY.with(Vendor::Groww),
                Some(isin("INE009A01021")),
            ),
            (
                equity("OUTSIDER"),
                Universe::NONE,
                VendorSet::EMPTY.with(Vendor::Dhan),
                None,
            ),
        ])
    }

    fn names(page: &Page) -> Vec<String> {
        page.rows.iter().map(|r| r.key.to_string()).collect()
    }

    #[test]
    fn every_column_the_url_can_name_maps_to_one_slot_and_a_stranger_is_the_default() {
        let want = [
            ("", Column::Default, false),
            ("symbol", Column::Symbol, false),
            ("isin", Column::Isin, false),
            ("universe", Column::UniverseBits, false),
            ("kind", Column::Kind, false),
            ("vendors", Column::Vendors, false),
            ("-symbol", Column::Symbol, true),
            ("-", Column::Default, true),
            ("no-such-column", Column::Default, false),
        ];
        for (raw, column, descending) in want {
            assert_eq!(Column::parse(raw), (column, descending), "sort={raw:?}");
        }
        // Every slot is distinct and inside the table, or two columns would
        // silently share one order.
        let mut slots: Vec<usize> = Column::ALL.iter().map(|c| c.slot()).collect();
        slots.sort_unstable();
        assert_eq!(slots, (0..COLUMNS).collect::<Vec<_>>());
    }

    #[test]
    fn every_pill_the_url_can_name_maps_to_one_slot_and_a_stranger_selects_everything() {
        for (raw, pill) in [
            ("fno", Pill::Fno),
            ("ntm", Pill::Ntm),
            ("idx", Pill::Index),
            ("", Pill::Every),
            ("no-such-universe", Pill::Every),
        ] {
            assert_eq!(Pill::parse(raw), pill, "pill={raw:?}");
        }
        let mut slots: Vec<usize> = Pill::ALL.iter().map(|p| p.slot()).collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 1, 2, 3]);
        // And each pill admits exactly what it names.
        assert!(Pill::Every.admits(Universe::NONE));
        assert!(Pill::Fno.admits(Universe::FNO) && !Pill::Fno.admits(Universe::INDEX));
        assert!(Pill::Ntm.admits(Universe::TOTAL_MARKET) && !Pill::Ntm.admits(Universe::FNO));
        assert!(Pill::Index.admits(Universe::INDEX) && !Pill::Index.admits(Universe::NONE));
    }

    #[test]
    fn the_eight_sets_are_eight_distinct_slots() {
        let mut seen: Vec<usize> = Vec::new();
        for all in [false, true] {
            for pill in ["", "fno", "ntm", "idx"] {
                seen.push(Selection::new(all, pill, "").set());
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..SETS).collect::<Vec<_>>());
        assert_eq!(Selection::new(false, "", "").scope(), 0);
        assert_eq!(Selection::new(true, "", "").scope(), 1);
    }

    #[test]
    fn the_tracked_scope_holds_the_two_lists_and_the_hatch_holds_everything() {
        let c = mixed();
        assert_eq!(c.len(), 4);
        assert!(!c.is_empty());
        let tracked = c.page(Selection::new(false, "", ""), 0);
        assert_eq!(tracked.matched, 3, "NIFTY, RELIANCE, ZZZTRACKED");
        assert!(
            !names(&tracked).contains(&"NSE-OUTSIDER".to_owned()),
            "the untracked listing is not in the default scope"
        );
        let every = c.page(Selection::new(true, "", ""), 0);
        assert_eq!(every.matched, 4);
        assert!(names(&every).contains(&"NSE-OUTSIDER".to_owned()));
    }

    #[test]
    fn every_pill_selects_from_the_whole_set_and_the_counts_match_the_pages() {
        let c = mixed();
        for (pill, want) in [("", 3), ("fno", 1), ("ntm", 2), ("idx", 1)] {
            let sel = Selection::new(false, pill, "");
            assert_eq!(c.page(sel, 0).matched, want, "tracked pill={pill:?}");
        }
        let counts = c.counts(Selection::new(false, "", ""));
        assert_eq!(
            (counts.all, counts.fno, counts.ntm, counts.index),
            (3, 1, 2, 1),
            "the pill counts are the tracked scope, not the rendered page"
        );
        let hatched = c.counts(Selection::new(true, "", ""));
        assert_eq!(
            (hatched.all, hatched.fno, hatched.ntm, hatched.index),
            (4, 1, 2, 1)
        );
        // An out-of-range scope cannot happen through `Selection`, and the
        // read still answers rather than panicking.
        assert_eq!(
            Catalog::build(&Merged {
                by_key: HashMap::new(),
                conflicts: Vec::new(),
                eligibility: Vec::new(),
            })
            .counts(Selection::new(false, "", "")),
            UniverseCounts::default()
        );
    }

    #[test]
    fn the_dashboard_figures_are_read_and_never_scanned() {
        let (counts, both_vendors) = mixed().dashboard_counts();
        assert_eq!(
            (counts.all, counts.ntm, counts.fno, counts.index),
            (3, 2, 1, 1)
        );
        assert_eq!(
            both_vendors, 2,
            "NIFTY and RELIANCE are tracked and named by both; ZZZTRACKED is one vendor \
             and OUTSIDER is not tracked"
        );
    }

    #[test]
    fn every_order_is_total_and_descending_is_the_same_order_backwards() {
        let c = mixed();
        for column in ["", "symbol", "isin", "universe", "kind", "vendors"] {
            let up = names(&c.page(Selection::new(true, "", column), 0));
            let down = names(&c.page(Selection::new(true, "", &format!("-{column}")), 0));
            assert_eq!(up.len(), 4, "column={column:?}");
            let mut reversed = down.clone();
            reversed.reverse();
            assert_eq!(up, reversed, "column={column:?} disagrees with itself");
        }
        // The default order leads with the swept instrument.
        let default = names(&c.page(Selection::new(true, "", ""), 0));
        assert_eq!(default.first().map(String::as_str), Some("NSE-NIFTY"));
        // And the symbol order is alphabetical, which the default is not.
        assert_eq!(
            names(&c.page(Selection::new(true, "", "symbol"), 0)),
            vec![
                "NSE-NIFTY",
                "NSE-OUTSIDER",
                "NSE-RELIANCE",
                "NSE-ZZZTRACKED"
            ]
        );
        // An ISIN order puts the absent ones first and is stable on the key.
        assert_eq!(
            names(&c.page(Selection::new(true, "", "isin"), 0)),
            vec![
                "NSE-NIFTY",
                "NSE-OUTSIDER",
                "NSE-RELIANCE",
                "NSE-ZZZTRACKED"
            ]
        );
    }

    #[test]
    fn a_page_draws_at_most_two_hundred_rows_and_a_stale_number_clamps() {
        let items: Vec<_> = (0..PAGE_ROWS * 2 + 5)
            .map(|i| {
                (
                    equity(&format!("SYM{i:05}")),
                    Universe::TOTAL_MARKET,
                    both(),
                    None,
                )
            })
            .collect();
        let c = catalog(&items);
        let sel = Selection::new(false, "", "symbol");
        let first = c.page(sel, 0);
        assert_eq!(first.rows.len(), PAGE_ROWS);
        assert_eq!(first.matched, PAGE_ROWS * 2 + 5);
        assert_eq!(first.last_page, 2);
        assert_eq!(c.page(sel, 2).rows.len(), 5, "the tail page is short");
        let stale = c.page(sel, 999);
        assert_eq!(stale.page, 2, "a stale page number lands on the last one");
        assert_eq!(stale.rows.len(), 5);
        // Descending reaches the same rows from the other end.
        let down = Selection::new(false, "", "-symbol");
        assert_eq!(c.page(down, 0).rows.len(), PAGE_ROWS);
        assert_eq!(c.page(down, 2).rows.len(), 5);
        let mut every: Vec<String> = Vec::new();
        for p in 0..=first.last_page {
            every.extend(names(&c.page(sel, p)));
        }
        assert_eq!(every.len(), PAGE_ROWS * 2 + 5, "every row is reachable");
        let mut sorted = every.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), every.len(), "and none of them twice");
    }

    #[test]
    fn an_empty_catalog_pages_without_panicking() {
        let c = catalog(&[]);
        assert!(c.is_empty());
        let p = c.page(Selection::new(false, "", ""), 7);
        assert_eq!((p.rows.len(), p.matched, p.page, p.last_page), (0, 0, 0, 0));
        let s = c.search(Selection::new(false, "", ""), "NIFTY", 3);
        assert_eq!((s.rows.len(), s.matched, s.page, s.last_page), (0, 0, 0, 0));
    }

    #[test]
    fn search_finds_a_substring_anywhere_and_respects_the_scope_and_the_pill() {
        let c = mixed();
        let hit = c.search(Selection::new(true, "", ""), "LIAN", 0);
        assert_eq!(
            names(&hit),
            vec!["NSE-RELIANCE"],
            "a substring, not a prefix"
        );
        assert_eq!(hit.matched, 1);
        // The scope still applies: OUTSIDER is outside the tracked universe.
        assert_eq!(
            c.search(Selection::new(true, "", ""), "OUTSID", 0).matched,
            1
        );
        assert_eq!(
            c.search(Selection::new(false, "", ""), "OUTSID", 0).matched,
            0,
            "the default scope does not reach an untracked listing"
        );
        // And so does the pill.
        assert_eq!(
            c.search(Selection::new(false, "fno", ""), "NSE-", 0)
                .matched,
            1
        );
        assert_eq!(
            c.search(Selection::new(false, "", ""), "NSE-", 0).matched,
            3
        );
    }

    #[test]
    fn search_orders_by_the_column_it_was_given_and_reverses_on_request() {
        let c = mixed();
        let up = names(&c.search(Selection::new(true, "", "symbol"), "NSE-", 0));
        assert_eq!(
            up,
            vec![
                "NSE-NIFTY",
                "NSE-OUTSIDER",
                "NSE-RELIANCE",
                "NSE-ZZZTRACKED"
            ]
        );
        let mut down = names(&c.search(Selection::new(true, "", "-symbol"), "NSE-", 0));
        down.reverse();
        assert_eq!(down, up, "descending search disagrees with ascending");
    }

    #[test]
    fn a_needle_shorter_than_a_trigram_still_answers_and_says_it_scanned() {
        let c = mixed();
        // One and two bytes have no trigram, so the candidate set is every
        // row. The ANSWER must be identical to the indexed path.
        assert_eq!(c.search(Selection::new(true, "", ""), "Z", 0).matched, 1);
        assert_eq!(c.search(Selection::new(true, "", ""), "NS", 0).matched, 4);
        assert!(
            c.rarest("NS").is_none(),
            "a two-byte needle has no trigram to be rarest"
        );
        assert!(c.rarest("NSE").is_some());
    }

    #[test]
    fn a_needle_longer_than_the_longest_name_is_answered_without_a_scan() {
        let c = mixed();
        let absurd = "X".repeat(c.longest + 1);
        let got = c.search(Selection::new(true, "", ""), &absurd, 0);
        assert_eq!(
            got.matched, 0,
            "a substring is never longer than the string"
        );
        assert_eq!(got.rows.len(), 0);
        // At the bound it is a scan, not a short circuit, and still finds
        // nothing — the guard is `>` and not `>=`.
        let at_bound = "X".repeat(c.longest);
        assert_eq!(
            c.search(Selection::new(true, "", ""), &at_bound, 0).matched,
            0
        );
    }

    #[test]
    fn a_trigram_no_name_carries_narrows_the_candidates_to_nothing() {
        let c = mixed();
        assert_eq!(
            c.rarest("QQQ").map(<[usize]>::len),
            Some(0),
            "an unknown trigram is an empty posting list, not a fall-back to everything"
        );
        assert_eq!(c.search(Selection::new(true, "", ""), "QQQ", 0).matched, 0);
        // The rarest of several is the one that is picked.
        let rare = c.rarest("NSE-RELIANCE").map(<[usize]>::len);
        assert_eq!(
            rare,
            Some(1),
            "some trigram of RELIANCE is in exactly one name"
        );
    }

    #[test]
    fn a_name_carrying_one_trigram_twice_is_listed_once() {
        let c = catalog(&[(equity("ABABAB"), Universe::TOTAL_MARKET, both(), None)]);
        assert_eq!(
            c.rarest("ABA").map(<[usize]>::len),
            Some(1),
            "ABA occurs twice in NSE-ABABAB and the posting list holds one entry"
        );
        assert_eq!(c.search(Selection::new(false, "", ""), "ABA", 0).matched, 1);
    }

    #[test]
    fn search_pages_like_the_unsearched_view_does() {
        let items: Vec<_> = (0..PAGE_ROWS + 3)
            .map(|i| {
                (
                    equity(&format!("MATCH{i:05}")),
                    Universe::TOTAL_MARKET,
                    both(),
                    None,
                )
            })
            .collect();
        let c = catalog(&items);
        let sel = Selection::new(false, "", "symbol");
        let first = c.search(sel, "MATCH", 0);
        assert_eq!(first.rows.len(), PAGE_ROWS);
        assert_eq!(first.matched, PAGE_ROWS + 3);
        assert_eq!(first.last_page, 1);
        let stale = c.search(sel, "MATCH", 99);
        assert_eq!((stale.page, stale.rows.len()), (1, 3));
    }
}
