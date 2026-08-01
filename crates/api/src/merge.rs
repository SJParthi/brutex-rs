//! Merging every vendor's master into one map, and cross-checking it.
//!
//! # What the merge is
//!
//! Two vendors naming the same contract produce the same
//! [`brutex_core::instrument::InstrumentKey`], so they collide
//! by design and that collision **is** the deduplication —
//! `docs/05-decisions.md` D-0015. This module is where the collision happens
//! and where the two vendors are made to agree about what collided.
//!
//! # The cross-check, and why the ISIN is not simply part of the key
//!
//! The ISIN is the one field a broker does not mint: it comes from a national
//! numbering agency, so both masters carry the same string for the same paper.
//! That makes it the ideal check on a merge — and a catastrophic key
//! component. As a field of the key, a vendor disagreement would produce two
//! different keys, the map would hold the instrument twice, and nobody would
//! ever learn there was a disagreement. Beside the key, the same disagreement
//! is one loud line naming the key and both ISINs. See [`brutex_core::isin`].
//!
//! # Two kinds of disagreement, and only one of them used to be looked for
//!
//! A merge that compares the ISINs of rows **both vendors kept** cannot see
//! the disagreement that actually existed in this data: one vendor calling a
//! security an equity while the other called it a bond. Those rows never
//! reached the merge at all — the reader counted them and dropped them — so
//! the report printed `0 isin conflicts` while the two masters disagreed about
//! the eligibility of 62 instruments, every one of which carried the **same
//! ISIN in both files**. The check had the key it needed and never looked.
//!
//! So there are two, and they are counted separately because they mean
//! different things:
//!
//! | | What disagreed | Held in |
//! |---|---|---|
//! | Identity | one key, two different ISINs | [`Merged::conflicts`] |
//! | Eligibility | one ISIN, kept by one vendor and declined by another | [`Merged::eligibility`] |
//!
//! Under D-0025 both are **zero** on the real masters — the gate reads one
//! exchange-issued series code from both vendors, so their verdicts agree on
//! 4,080 of 4,080 shared ISINs. That is the point: the checks are what make
//! the zero worth anything.
//!
//! # What a disagreement does
//!
//! It **refuses**, and the refusal is a returned value rather than a log line:
//! [`Merged::verdict`] is [`Verdict::Disputed`], the caller's exit code is
//! non-zero and `/health` stops returning 200. D-0026. Earlier text here
//! claimed a non-empty conflicts vector "is a refusal to believe the merge",
//! and cited D-0020 for it, while the code printed a line, kept the
//! instrument, and exited 0 — the "primary vendor wins, log the difference"
//! shape D-0020 exists to reject. Naming a thing a refusal does not make it
//! one.
//!
//! # The suffix, and why it is confirmed rather than normalised
//!
//! Groww leaks `internal_trading_symbol` into `trading_symbol` on exactly 209
//! of the 4,080 ISINs the two masters share — `BLUECHIP-BE`, `CBAZAAR-ST`,
//! `HDFCLIQUID-EQ`, `LOWVOL-EQ` — and the rule is exact with zero residual:
//! `groww.trading_symbol == dhan.UNDERLYING_SYMBOL + "-" + series`. It would
//! be very easy, and wrong, to strip a trailing `-XX` everywhere:
//! `BAJAJ-AUTO`, `NAM-INDIA` and `M&M` are real tickers, and
//! `crates/core/src/symbol.rs` argues at length that blind normalisation
//! manufactures the collision it exists to prevent.
//!
//! So the strip needs **two** independent agreements before it is applied: the
//! trailing segment must be the row's own series (decided in
//! [`brutex_core::vendor`], which is where the series is), and some vendor
//! must have asserted the stripped identity under the **same ISIN** (decided
//! here, which is where both vendors are). Neither alone is enough, and where
//! they do not both hold the symbol is left exactly as the vendor wrote it.

use brutex_core::instrument::InstrumentKey;
use brutex_core::isin::Isin;
use brutex_core::universe::{self, Universe};
use brutex_core::vendor::{Listing, Skip, Vendor, VendorSet};
use std::collections::{BTreeMap, HashMap, HashSet};

/// One instrument, after every vendor has had its say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Entry {
    /// Which vendors listed it. Seeing two here is the deduplication, on
    /// screen.
    pub vendors: VendorSet,
    /// The ISIN, and the vendor that gave it. `None` for an index, which has
    /// no ISIN at all.
    pub isin: Option<(Vendor, Isin)>,
    /// A **different** ISIN a later vendor gave for the same key.
    ///
    /// Both are kept. Picking one would be choosing a winner without evidence,
    /// and dropping the row would hide the disagreement entirely.
    pub conflict: Option<(Vendor, Isin)>,
    /// Which of the engine's universes this instrument is in.
    pub universe: Universe,
}

/// One vendor's whole contribution to the merge.
///
/// Both halves are needed: what a vendor **kept** builds the map, and what it
/// **declined** is the only way another vendor's keep can be recognised as a
/// disagreement rather than as a row nobody mentioned.
#[derive(Debug)]
pub struct Source {
    /// Whose rows these are.
    pub vendor: Vendor,
    /// The instruments this vendor listed.
    pub kept: Vec<Listing>,
    /// The ISINs this vendor declined, and why.
    pub declined: Vec<(Isin, Skip)>,
}

/// Whether the merged universe may be believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every vendor that spoke agreed. The normal state.
    Clean,
    /// At least one cross-vendor disagreement survived the merge.
    ///
    /// The instruments are still in the map and still on the page — dropping
    /// them would hide the disagreement, which is the failure `CLAUDE.md` §4
    /// forbids — but the *universe as a whole* is refused: a caller that turns
    /// this into an exit code must not return success. D-0026.
    Disputed,
}

/// The merged universe, and everything that disagreed while it was built.
#[derive(Debug, Default)]
pub struct Merged {
    /// One entry per distinct instrument.
    pub by_key: HashMap<InstrumentKey, Entry>,
    /// One line per key two vendors gave different ISINs for, naming the key
    /// and both ISINs. Empty is the normal state.
    pub conflicts: Vec<String>,
    /// One line per ISIN one vendor kept and another declined, naming the
    /// ISIN, both vendors and the decline reason.
    ///
    /// Separate from [`Merged::conflicts`] because it is a different
    /// disagreement: not "which paper is this" but "is this paper an equity at
    /// all". Folding the two together would let a count of one hide the other.
    pub eligibility: Vec<String>,
}

impl Merged {
    /// The number of distinct instruments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether nothing merged at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Whether this universe may be believed.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        if self.conflicts.is_empty() && self.eligibility.is_empty() {
            Verdict::Clean
        } else {
            Verdict::Disputed
        }
    }

    /// How many members of each universe the merge resolved, and how many of
    /// those two vendors both named.
    ///
    /// The second number is the one that means something. An instrument named
    /// by one vendor rests entirely on that vendor's gate; an instrument named
    /// by both has been cross-checked. Reporting only the first would let two
    /// vendors' worth of confidence and one vendor's worth look identical.
    #[must_use]
    pub fn universe_census(&self) -> Vec<(&'static str, usize, usize)> {
        let mut out = Vec::new();
        for (name, bit) in [
            ("F&O underlyings", Universe::FNO),
            ("NIFTY Total Market", Universe::TOTAL_MARKET),
        ] {
            let members = self.by_key.iter().filter(|(_, e)| e.universe.contains(bit));
            let mut present = 0;
            let mut both = 0;
            for (_, e) in members {
                present += 1;
                if Vendor::ALL.iter().all(|v| e.vendors.contains(*v)) {
                    both += 1;
                }
            }
            out.push((name, present, both));
        }
        out
    }

    /// Universe members only one vendor named, by universe and key.
    ///
    /// An index carries no ISIN, so nothing cross-checks its identity at all —
    /// and the two masters spell most index names differently. This is where
    /// that shows up instead of being invisible: `MIDCPNIFTY` and `NIFTYNXT50`
    /// are F&O underlyings Dhan names and Groww calls `NIFTYMIDSELECT` and
    /// `NIFTYJR`. `docs/06-limits.md` §12.
    #[must_use]
    pub fn single_vendor_members(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .by_key
            .iter()
            .filter(|(_, e)| {
                !e.universe.is_none() && !Vendor::ALL.iter().all(|v| e.vendors.contains(*v))
            })
            .map(|(k, e)| {
                let who: Vec<&str> = Vendor::ALL
                    .iter()
                    .filter(|v| e.vendors.contains(**v))
                    .map(|v| v.as_str())
                    .collect();
                format!("{k} ({})", who.join(" "))
            })
            .collect();
        out.sort_unstable();
        out
    }
}

/// Merges every vendor's kept listings into one map, and cross-checks it.
///
/// Two passes, because the confirmation a strip needs may come from a vendor
/// that has not been read yet. The first pass records every identity any
/// vendor asserted; the second resolves each listing against it. Both are
/// O(1) per listing — a hash probe on fixed-width `Copy` keys, never a scan.
/// The eligibility check is a third probe against a map built once, so it is
/// O(1) per declined row too.
#[must_use]
pub fn merge(sources: &[Source]) -> Merged {
    // Pass 1. Every (identity, ISIN) pair anybody asserted. A row can never
    // confirm ITSELF: it asserts its raw key, and the candidate it offers is
    // by construction a different symbol.
    let mut asserted: HashSet<(InstrumentKey, Isin)> = HashSet::new();
    // And every ISIN each vendor KEPT, which is what a decline is checked
    // against. `BTreeMap` rather than `HashMap` so the conflict lines come out
    // in a stable order and two runs produce byte-identical output.
    let mut kept_isins: BTreeMap<Isin, Vec<Vendor>> = BTreeMap::new();
    for s in sources {
        for l in &s.kept {
            if let Some(i) = l.isin {
                asserted.insert((l.key, i));
                kept_isins.entry(i).or_default().push(s.vendor);
            }
        }
    }

    let mut out = Merged::default();
    for s in sources {
        let vendor = s.vendor;
        for l in &s.kept {
            let key = resolve(l, &asserted);
            let e = out.by_key.entry(key).or_default();
            e.vendors = e.vendors.with(vendor);
            e.universe = universe::of_instrument(&key);
            match (e.isin, l.isin) {
                // Nothing on record yet: take what this vendor said, whether
                // that is an ISIN or the honest absence of one.
                (None, given) => e.isin = given.map(|i| (vendor, i)),
                // Two vendors, two different ISINs, one key. Name both.
                (Some((first_vendor, first)), Some(given)) if first != given => {
                    e.conflict = Some((vendor, given));
                    out.conflicts.push(format!(
                        "{key}: {} says {first}, {} says {given}",
                        first_vendor.as_str(),
                        vendor.as_str()
                    ));
                }
                // Agreement, or a vendor with nothing to add.
                _ => {}
            }
        }
    }

    // THE ELIGIBILITY CHECK. One vendor declined this ISIN; did another keep
    // it? Sorted by ISIN, so the output does not depend on map iteration
    // order. This is what used to be structurally impossible: the declined
    // rows were dropped before the merge ever saw them.
    let mut disputes: BTreeMap<Isin, Vec<String>> = BTreeMap::new();
    for s in sources {
        for &(isin, reason) in &s.declined {
            // Only a decline that judges the PAPER can contradict a keep. A
            // decline for the venue cannot: `RELIANCE` is `INE002A01018` on
            // both NSE and BSE, so one vendor declining the BSE row while the
            // other keeps the NSE row is two correct decisions, not a
            // disagreement — and counting it as one produced 3,000 false
            // conflicts on the real masters.
            if !reason.judges_the_paper() {
                continue;
            }
            let Some(keepers) = kept_isins.get(&isin) else {
                continue;
            };
            for keeper in keepers {
                // A vendor listing the same ISIN twice — once kept, once
                // declined — is a row shape neither master has, but it would
                // be a statement about ONE vendor and not a cross-vendor
                // disagreement, so it is not one.
                if *keeper != s.vendor {
                    disputes.entry(isin).or_default().push(format!(
                        "{isin}: {} kept it, {} declined it as {}",
                        keeper.as_str(),
                        s.vendor.as_str(),
                        reason.reason()
                    ));
                }
            }
        }
    }
    out.eligibility = disputes.into_values().flatten().collect();
    out
}

/// The identity to file a listing under.
///
/// The candidate key from the decoder is adopted **only** when some vendor has
/// asserted that identity under this row's own ISIN. Without that confirmation
/// the vendor's symbol stands, suffix and all: an unmerged row is a visible
/// duplicate, while a wrongly merged one is two instruments silently becoming
/// one.
fn resolve(l: &Listing, asserted: &HashSet<(InstrumentKey, Isin)>) -> InstrumentKey {
    match (l.unsuffixed, l.isin) {
        (Some(candidate), Some(isin)) if asserted.contains(&(candidate, isin)) => candidate,
        _ => l.key,
    }
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
    use brutex_core::instrument::{Exchange, Kind, Segment};
    use brutex_core::symbol::Symbol;

    fn key(sym: &str, kind: Kind) -> InstrumentKey {
        InstrumentKey {
            exchange: Exchange::Nse,
            segment: if kind == Kind::Index {
                Segment::Index
            } else {
                Segment::Cash
            },
            underlying: Symbol::new(sym).expect("valid"),
            kind,
        }
    }

    fn equity(sym: &str, isin: &str) -> Listing {
        Listing {
            key: key(sym, Kind::Equity),
            isin: Some(Isin::new(isin).expect("valid")),
            unsuffixed: None,
        }
    }

    fn index(sym: &str) -> Listing {
        Listing {
            key: key(sym, Kind::Index),
            isin: None,
            unsuffixed: None,
        }
    }

    /// One vendor that declined nothing, which is what most tests need.
    fn from(vendor: Vendor, kept: Vec<Listing>) -> Source {
        Source {
            vendor,
            kept,
            declined: Vec::new(),
        }
    }

    #[test]
    fn one_instrument_named_by_both_vendors_is_one_entry_with_two_tags() {
        let m = merge(&[
            from(Vendor::Groww, vec![equity("RELIANCE", "INE002A01018")]),
            from(Vendor::Dhan, vec![equity("RELIANCE", "INE002A01018")]),
        ]);
        assert_eq!(m.len(), 1, "the collision IS the deduplication");
        assert!(!m.is_empty());
        let e = m.by_key[&key("RELIANCE", Kind::Equity)];
        assert!(e.vendors.contains(Vendor::Groww));
        assert!(e.vendors.contains(Vendor::Dhan));
        assert_eq!(e.conflict, None);
        assert!(m.conflicts.is_empty(), "agreement is silent");
        assert!(m.eligibility.is_empty());
        assert_eq!(m.verdict(), Verdict::Clean, "agreement is believable");
        assert!(
            e.universe.contains(Universe::FNO) && e.universe.contains(Universe::TOTAL_MARKET),
            "the merge stamps the universe it resolved into"
        );
    }

    #[test]
    fn a_cross_vendor_isin_conflict_is_reported_and_neither_side_is_dropped() {
        // The whole reason the ISIN is beside the key rather than in it: as a
        // key field this would be two entries and no one would ever know.
        let m = merge(&[
            from(Vendor::Groww, vec![equity("CHOLAFIN", "INE121A01024")]),
            from(Vendor::Dhan, vec![equity("CHOLAFIN", "INE121A08PJ0")]),
        ]);
        assert_eq!(m.len(), 1, "one key, not two");
        assert_eq!(m.conflicts.len(), 1);
        let line = &m.conflicts[0];
        assert!(line.contains("NSE-CHOLAFIN"), "the key is named: {line}");
        assert!(
            line.contains("INE121A01024"),
            "both ISINs are named: {line}"
        );
        assert!(
            line.contains("INE121A08PJ0"),
            "both ISINs are named: {line}"
        );
        assert!(line.contains("groww") && line.contains("dhan"), "{line}");

        let e = m.by_key[&key("CHOLAFIN", Kind::Equity)];
        assert_eq!(
            e.isin.map(|(_, i)| i.to_string()).as_deref(),
            Some("INE121A01024"),
            "the first ISIN is kept"
        );
        assert_eq!(
            e.conflict.map(|(_, i)| i.to_string()).as_deref(),
            Some("INE121A08PJ0"),
            "and so is the second -- neither is dropped, neither wins"
        );
        assert!(e.vendors.contains(Vendor::Groww) && e.vendors.contains(Vendor::Dhan));
        assert_eq!(
            m.verdict(),
            Verdict::Disputed,
            "a named disagreement REFUSES the universe; it is not a log line"
        );
    }

    #[test]
    fn one_vendor_keeping_what_another_declined_is_a_named_disagreement() {
        // THE CHECK THAT WAS STRUCTURALLY IMPOSSIBLE. Every declined row was
        // dropped at the reader, so a disagreement about ELIGIBILITY -- the
        // disagreement that actually existed -- produced no conflict, no note
        // and no count, and the report read `0 isin conflicts` while the two
        // masters disagreed about 62 instruments carrying the SAME ISIN in
        // both files.
        //
        // INF090I01VS3 is one of them: a Franklin fund plan Dhan filed as an
        // `ETF` and Groww declined as series `MF`.
        let disputed = Isin::new("INF090I01VS3").expect("valid");
        let m = merge(&[
            from(Vendor::Dhan, vec![equity("FISTIPD3GP", "INF090I01VS3")]),
            Source {
                vendor: Vendor::Groww,
                kept: Vec::new(),
                declined: vec![(disputed, Skip::NotEquityListing)],
            },
        ]);
        assert!(
            m.conflicts.is_empty(),
            "the ISINs agree; the verdicts do not"
        );
        assert_eq!(m.eligibility.len(), 1);
        let line = &m.eligibility[0];
        assert!(line.contains("INF090I01VS3"), "{line}");
        assert!(line.contains("dhan kept it"), "{line}");
        assert!(line.contains("groww declined it"), "{line}");
        assert!(line.contains("not an equity listing"), "{line}");
        assert_eq!(m.verdict(), Verdict::Disputed);
        // The instrument stays in the map. Dropping it would hide the
        // disagreement, which is the failure the check exists to prevent.
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn a_decline_about_the_venue_is_not_a_disagreement_about_the_paper() {
        // RELIANCE is INE002A01018 on NSE and on BSE. One vendor declining the
        // BSE row for being BSE while the other keeps the NSE row is two
        // CORRECT decisions about two different rows -- and counting it as a
        // conflict produced 3,000 false lines on the real masters, which is a
        // check nobody reads twice.
        let reliance = Isin::new("INE002A01018").expect("valid");
        for venue in [
            Skip::ForeignExchange,
            Skip::ForeignSegment,
            Skip::LiveContract,
            Skip::TestInstrument,
        ] {
            let m = merge(&[
                from(Vendor::Dhan, vec![equity("RELIANCE", "INE002A01018")]),
                Source {
                    vendor: Vendor::Groww,
                    kept: vec![equity("RELIANCE", "INE002A01018")],
                    declined: vec![(reliance, venue)],
                },
            ]);
            // Bound rather than called inside the failure message: a call
            // there is a region only a FAILING assertion runs.
            let what = venue.reason();
            assert!(
                m.eligibility.is_empty(),
                "{what} judges the venue, not the paper"
            );
            assert_eq!(m.verdict(), Verdict::Clean);
        }
        // The three that DO judge the paper are the three that can conflict.
        for paper in [
            Skip::NotEquityListing,
            Skip::SmeBoard,
            Skip::UnrecognisedListingClass,
        ] {
            let m = merge(&[
                from(Vendor::Dhan, vec![equity("RELIANCE", "INE002A01018")]),
                Source {
                    vendor: Vendor::Groww,
                    kept: Vec::new(),
                    declined: vec![(reliance, paper)],
                },
            ]);
            let what = paper.reason();
            assert_eq!(m.eligibility.len(), 1, "{what} judges the paper");
        }
    }

    #[test]
    fn a_decline_nobody_else_kept_is_not_a_disagreement() {
        // A master is mostly declines. Only a decline that some OTHER vendor
        // contradicts is news, or every bond in the file would be a conflict.
        let m = merge(&[
            from(Vendor::Dhan, vec![equity("RELIANCE", "INE002A01018")]),
            Source {
                vendor: Vendor::Groww,
                kept: vec![equity("RELIANCE", "INE002A01018")],
                declined: vec![
                    (
                        Isin::new("INE121A08PJ0").expect("valid"),
                        Skip::NotEquityListing,
                    ),
                    (
                        Isin::new("IN1520250086").expect("valid"),
                        Skip::NotEquityListing,
                    ),
                ],
            },
        ]);
        assert!(
            m.eligibility.is_empty(),
            "nobody kept those, so nobody disagrees"
        );
        assert_eq!(m.verdict(), Verdict::Clean);

        // And one vendor declining what IT ALSO kept is a statement about one
        // vendor, not a cross-vendor disagreement.
        let m = merge(&[Source {
            vendor: Vendor::Groww,
            kept: vec![equity("RELIANCE", "INE002A01018")],
            declined: vec![(Isin::new("INE002A01018").expect("valid"), Skip::SmeBoard)],
        }]);
        assert!(m.eligibility.is_empty());
        assert_eq!(m.verdict(), Verdict::Clean);
    }

    #[test]
    fn the_census_separates_what_two_vendors_confirmed_from_what_one_asserted() {
        // 211 of the 213 F&O underlyings are named by both masters; MIDCPNIFTY
        // and NIFTYNXT50 are Dhan's spellings for indices Groww calls
        // NIFTYMIDSELECT and NIFTYJR, and an index carries no ISIN, so nothing
        // cross-checks them at all. Reporting only "213 present" would make
        // one vendor's word look like two vendors' agreement.
        let m = merge(&[
            from(
                Vendor::Groww,
                vec![
                    index("NIFTY"),
                    index("NIFTYJR"),
                    equity("RELIANCE", "INE002A01018"),
                ],
            ),
            from(
                Vendor::Dhan,
                vec![
                    index("NIFTY"),
                    index("NIFTYNXT50"),
                    equity("RELIANCE", "INE002A01018"),
                ],
            ),
        ]);
        let census = m.universe_census();
        assert_eq!(
            census,
            vec![
                // NIFTY, NIFTYNXT50 and RELIANCE are F&O underlyings; only
                // NIFTY and RELIANCE are named by both. NIFTYJR is in neither
                // list under that spelling, which is exactly the finding.
                ("F&O underlyings", 3, 2),
                ("NIFTY Total Market", 1, 1),
            ]
        );
        // BOTH spellings surface, because both are indices named by one vendor
        // only and an index has no ISIN to reconcile them with. Naming them is
        // the whole substitute for a cross-check that cannot exist.
        let single = m.single_vendor_members();
        assert_eq!(
            single,
            vec![
                "NSE-NIFTYJR (groww)".to_owned(),
                "NSE-NIFTYNXT50 (dhan)".to_owned()
            ]
        );
    }

    #[test]
    fn a_suffixed_symbol_merges_only_when_the_isin_confirms_it() {
        // Groww says BLUECHIP-BE, Dhan says BLUECHIP, and the ISIN says they
        // are one instrument.
        let suffixed = Listing {
            key: key("BLUECHIP-BE", Kind::Equity),
            isin: Some(Isin::new("INE657B01025").expect("valid")),
            unsuffixed: Some(key("BLUECHIP", Kind::Equity)),
        };
        let m = merge(&[
            from(Vendor::Groww, vec![suffixed]),
            from(Vendor::Dhan, vec![equity("BLUECHIP", "INE657B01025")]),
        ]);
        assert_eq!(m.len(), 1, "confirmed, so they merge");
        let e = m.by_key[&key("BLUECHIP", Kind::Equity)];
        assert!(e.vendors.contains(Vendor::Groww) && e.vendors.contains(Vendor::Dhan));
        assert!(m.conflicts.is_empty());
        assert_eq!(m.verdict(), Verdict::Clean);
    }

    #[test]
    fn an_unconfirmed_suffix_is_left_exactly_as_the_vendor_wrote_it() {
        // Same shape, but no vendor asserts BLUECHIP under that ISIN. An
        // unmerged row is a visible duplicate; a wrongly merged one is two
        // instruments silently becoming one.
        let suffixed = Listing {
            key: key("BLUECHIP-BE", Kind::Equity),
            isin: Some(Isin::new("INE657B01025").expect("valid")),
            unsuffixed: Some(key("BLUECHIP", Kind::Equity)),
        };
        let m = merge(&[from(Vendor::Groww, vec![suffixed])]);
        assert_eq!(m.len(), 1);
        assert!(m.by_key.contains_key(&key("BLUECHIP-BE", Kind::Equity)));

        // And a candidate confirmed under a DIFFERENT ISIN is not confirmed at
        // all -- this is the case that separates "same ticker" from "same
        // paper", and it must not merge.
        let m = merge(&[
            from(Vendor::Groww, vec![suffixed]),
            from(Vendor::Dhan, vec![equity("BLUECHIP", "INE002A01018")]),
        ]);
        assert_eq!(m.len(), 2, "different ISIN, so different instruments");
        assert!(m.conflicts.is_empty(), "they never met, so nothing clashed");
    }

    #[test]
    fn an_index_carries_no_isin_and_still_merges_on_identity() {
        let m = merge(&[
            from(Vendor::Groww, vec![index("NIFTY")]),
            from(Vendor::Dhan, vec![index("NIFTY")]),
        ]);
        assert_eq!(m.len(), 1);
        let e = m.by_key[&key("NIFTY", Kind::Index)];
        assert_eq!(e.isin, None, "no sentinel ISIN is invented for NIFTY");
        assert!(e.vendors.contains(Vendor::Groww) && e.vendors.contains(Vendor::Dhan));
        assert!(m.conflicts.is_empty());
        assert!(e.universe.contains(Universe::INDEX));
        assert!(
            m.single_vendor_members().is_empty(),
            "both vendors spell NIFTY the same way, so nothing rests on one"
        );
    }

    #[test]
    fn a_vendor_with_nothing_to_add_neither_overwrites_nor_conflicts() {
        // One vendor knows the ISIN, the other does not carry one. That is not
        // a disagreement, and the known value must survive it.
        let bare = Listing {
            key: key("RELIANCE", Kind::Equity),
            isin: None,
            unsuffixed: None,
        };
        let m = merge(&[
            from(Vendor::Groww, vec![equity("RELIANCE", "INE002A01018")]),
            from(Vendor::Dhan, vec![bare]),
        ]);
        let e = m.by_key[&key("RELIANCE", Kind::Equity)];
        assert_eq!(
            e.isin.map(|(v, _)| v),
            Some(Vendor::Groww),
            "the vendor that knew it is recorded"
        );
        assert_eq!(e.conflict, None);
        assert!(m.conflicts.is_empty());

        // And in the other order, the entry starts empty and is filled later.
        let m = merge(&[
            from(Vendor::Dhan, vec![bare]),
            from(Vendor::Groww, vec![equity("RELIANCE", "INE002A01018")]),
        ]);
        assert_eq!(
            m.by_key[&key("RELIANCE", Kind::Equity)]
                .isin
                .map(|(v, _)| v),
            Some(Vendor::Groww)
        );
    }

    #[test]
    fn merging_nothing_produces_nothing_rather_than_a_default_row() {
        let m = merge(&[]);
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert!(m.conflicts.is_empty());
        assert!(m.eligibility.is_empty());
        assert_eq!(
            m.verdict(),
            Verdict::Clean,
            "nothing said, nothing disputed"
        );
        assert_eq!(
            m.universe_census(),
            vec![("F&O underlyings", 0, 0), ("NIFTY Total Market", 0, 0)]
        );
        assert!(m.single_vendor_members().is_empty());
    }
}
