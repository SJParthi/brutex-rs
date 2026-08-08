//! Turning one vendor's instrument row into the canonical [`InstrumentKey`].
//!
//! # Why this reads columns and never parses the display symbol
//!
//! Every vendor ships a human-facing trading symbol, and it is tempting to
//! parse it. Real rows from the primary broker's own master show why that is a
//! trap:
//!
//! | Trading symbol | Real `expiry_date` column |
//! |---|---|
//! | `NIFTY2680419450CE` | `2026-08-04` — `26` year, `8` month, `04` day, weekly |
//! | `BANKNIFTY25DEC27000PE` | `2025-12-24` — `25` year, `DEC` month, **no day at all** |
//!
//! Two different encodings in one file, and the monthly form does not carry the
//! day, so the expiry is **not recoverable from the symbol**. Worse,
//! `BANKNIFTY25SEP…` happens to expire on the 25th, so a day-first reading and
//! a year-first reading agree on that row and disagree on the other — the kind
//! of coincidence that hides a bug through a whole test suite.
//!
//! The master already carries `expiry_date`, `strike_price` and
//! `underlying_symbol` as separate structured columns. Reading them is exact,
//! it is O(1) per row, and it makes the symbology question disappear rather
//! than answering it. The display symbol is never an input to identity.
//!
//! # Prices
//!
//! Strikes arrive in **rupees** and are stored in **paisa**. `27000` in the
//! master is `2_700_000` here. One missed multiplication makes every strike
//! wrong by a factor of a hundred, so the conversion goes through
//! [`Paisa::from_rupees_half_up`] like every other price.

use crate::error::InstrumentError;
use crate::instrument::{Exchange, Expiry, InstrumentKey, Kind, Segment};
use crate::isin::Isin;
use crate::price::Paisa;
use crate::symbol::Symbol;

/// Which vendor a row came from.
///
/// This is the first segment of the store path — `docs/05-decisions.md`
/// D-0019 — so each vendor owns a completely independent series and can be
/// added, re-pulled or deleted without touching any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Vendor {
    /// Primary broker.
    Groww,
    /// Secondary broker.
    Dhan,
}

impl Vendor {
    /// Every vendor this engine reads, in path order.
    pub const ALL: [Self; 2] = [Self::Groww, Self::Dhan];

    /// What this vendor's instrument master is called on disk.
    ///
    /// # Why the file name is a property of the vendor
    ///
    /// It was a hand-written list in `api::server::master_paths`:
    ///
    /// ```text
    /// vec![(Vendor::Groww, dir.join("groww_instruments.csv")),
    ///      (Vendor::Dhan,  dir.join("dhan_scrip.csv"))]
    /// ```
    ///
    /// So adding a feed meant editing that function — and forgetting to meant a
    /// vendor the engine knows about whose master is silently never read, which
    /// reports as "this vendor lists nothing" rather than as the wiring bug it
    /// is. A `match` on `Self` cannot be forgotten: a new variant is a compile
    /// error until it names its file.
    ///
    /// Everything downstream already iterates [`Self::ALL`] — the census grid,
    /// the merge, the ingest form, the coverage table. This was the one place
    /// that did not, and it was the entry point.
    #[must_use]
    pub const fn master_file(self) -> &'static str {
        match self {
            Self::Groww => "groww_instruments.csv",
            Self::Dhan => "dhan_scrip.csv",
        }
    }

    /// The path segment for this vendor.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Groww => "groww",
            Self::Dhan => "dhan",
        }
    }

    /// This vendor's bit in a [`VendorSet`].
    const fn bit(self) -> u8 {
        match self {
            Self::Groww => 1 << 0,
            Self::Dhan => 1 << 1,
        }
    }
}

/// A set of vendors, as a bitset.
///
/// A merged instrument is listed by one vendor or by several, and the set of
/// vendors that named it is a *property of the merge*, never of the identity —
/// exactly like [`crate::universe::Universe`], and for exactly the same reason:
/// a field on [`InstrumentKey`] would split one instrument into two keys.
///
/// It exists rather than a pair of booleans because a `bool` per vendor forces
/// every caller to `match` on [`Vendor`], which is `#[non_exhaustive]`. Outside
/// this crate that match needs a wildcard arm that no test can ever reach, so
/// the coverage gate can never go green on it. The bitset moves the one match
/// here, where it is exhaustive and provable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct VendorSet(u8);

impl VendorSet {
    /// No vendor has named this instrument.
    pub const EMPTY: Self = Self(0);

    /// This set with `vendor` added. Adding twice is the same as adding once.
    #[must_use]
    pub const fn with(self, vendor: Vendor) -> Self {
        Self(self.0 | vendor.bit())
    }

    /// Whether `vendor` is in this set.
    #[must_use]
    pub const fn contains(self, vendor: Vendor) -> bool {
        self.0 & vendor.bit() != 0
    }

    /// Whether no vendor is in this set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// The longest vendor instrument id this build will hold, in bytes.
///
/// **Derived from the widest grammar either vendor uses, then checked against
/// both live masters** rather than chosen:
///
/// | Part | Bytes | Measured on 2026-08-08 |
/// |---|---|---|
/// | exchange | 3 | `NSE`, `BSE` |
/// | underlying | 10 | longest present |
/// | expiry | 7 | `DDMmmYY`, e.g. `30Sep25` |
/// | strike | 6 | longest present, `100000` |
/// | side | 3 | `FUT`; `CE`/`PE` are shorter |
/// | separators | 4 | |
/// | **total** | **33** | longest id actually present: **32** |
///
/// 48 leaves room for a seven-digit strike and a longer underlying without
/// being a number nobody can justify. Dhan's `SECURITY_ID` is at most 7 bytes
/// and entirely numeric across all 204,819 rows, so it is far inside this.
///
/// [`crate::symbol::SYMBOL_CAPACITY`] is 24 and therefore **cannot** hold a
/// Groww symbol — `NSE-NIFTYNXT50-25Aug26-100000-PE` is 32. That is why this is
/// a separate type rather than a reuse.
pub const VENDOR_ID_CAPACITY: usize = 48;

const _: () = assert!(VENDOR_ID_CAPACITY >= 33, "the derived worst case");

/// A vendor's own identifier for an instrument, inline and never heap-allocated.
///
/// Dhan calls it `SECURITY_ID` and it is a number; Groww calls it
/// `groww_symbol` and it is `NSE-NIFTY-30Sep25-24650-CE`. Both are opaque here:
/// this type carries bytes the vendor chose and hands them back unchanged,
/// because the moment it parsed them it would own a grammar the vendor can
/// change without telling anyone.
///
/// # Why this exists at all
///
/// Neither column was read. `Vendor::master_columns` declared ten names and
/// neither `SECURITY_ID` nor `groww_symbol` was among them, so the decoder
/// stepped over the id — column 3 of Dhan's file, on the way to column 5 — and
/// dropped it. `groww_symbol` appeared nowhere in the workspace at all. Without
/// it nothing can name an instrument to either vendor, which is why Dhan
/// answers `DH-905 securityId is required`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct VendorId {
    bytes: [u8; VENDOR_ID_CAPACITY],
    len: u8,
}

impl VendorId {
    /// The id a vendor wrote down, or `None` if it is empty or too long.
    ///
    /// # Errors
    ///
    /// `None` for an empty id — a row with no id cannot be requested — and for
    /// one past [`VENDOR_ID_CAPACITY`], which is a grammar this build has not
    /// seen and must not silently truncate into a different instrument.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > VENDOR_ID_CAPACITY {
            return None;
        }
        let mut bytes = [0_u8; VENDOR_ID_CAPACITY];
        // `get_mut` rather than an index: the guard above already bounds
        // `raw.len()`, but a slice expression carries a panic this workspace
        // denies, and a `?` that no input can take is cheaper than the lint
        // exception it would otherwise need.
        bytes.get_mut(..raw.len())?.copy_from_slice(raw.as_bytes());
        // `len` fits while VENDOR_ID_CAPACITY <= 255, pinned below.
        let Ok(len) = u8::try_from(raw.len()) else {
            return None;
        };
        Some(Self { bytes, len })
    }

    /// The id, as the vendor wrote it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Every byte came from a `&str` in `new`, so the prefix is valid UTF-8;
        // and `len` is at most VENDOR_ID_CAPACITY, so the slice is in range.
        // Both are expressed as fallible lookups rather than asserted, because
        // an index expression carries a panic this workspace denies.
        self.bytes
            .get(..usize::from(self.len))
            .and_then(|held| core::str::from_utf8(held).ok())
            .unwrap_or("")
    }
}

const _: () = assert!(VENDOR_ID_CAPACITY <= 255, "len is a u8");

impl core::fmt::Debug for VendorId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VendorId({:?})", self.as_str())
    }
}

impl core::fmt::Display for VendorId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of a vendor instrument master, already split into fields.
///
/// Borrowed rather than owned: a master has hundreds of thousands of rows and
/// the vast majority are rejected, so allocating for each one would be work
/// done to throw away.
#[derive(Debug, Clone, Copy)]
pub struct MasterRow<'a> {
    /// The vendor's own id for this instrument — `SECURITY_ID` at Dhan,
    /// `groww_symbol` at Groww. Opaque; see [`VendorId`].
    pub vendor_id: &'a str,
    /// Exchange code, e.g. `NSE`.
    pub exchange: &'a str,
    /// Segment code, e.g. `CASH` or `FNO`.
    pub segment: &'a str,
    /// The underlying symbol. **Empty on every Groww CASH row**, including
    /// NIFTY and BANKNIFTY -- which is why [`MasterRow::trading_symbol`]
    /// exists and why identity is chosen per instrument type.
    pub underlying: &'a str,
    /// The vendor's own tradable symbol. For a cash or index row this is the
    /// only place the instrument is named at all.
    pub trading_symbol: &'a str,
    /// Instrument type: `IDX`, `EQ`, `FUT`, `CE`, `PE`.
    pub instrument_type: &'a str,
    /// The **NSE board series** this cash-segment row trades under: `EQ`,
    /// `BE`, `BZ`, `SM`, `N0`, `SG`, …
    ///
    /// This is one fact issued by one exchange, and both vendors carry it —
    /// Groww in `series`, Dhan in `SERIES`. It is named for what it *means*
    /// here rather than for either vendor's column heading, and it is read
    /// through [`Vendor::master_columns`]. Empty on index and derivative rows,
    /// which is why `board_of` is consulted only for cash equity.
    ///
    /// D-0025 replaced Dhan's `INSTRUMENT_TYPE` with `SERIES` here. That
    /// column is a vendor-minted paper class, and it is **measurably wrong**:
    /// it files 54 Franklin/PGIM/Bandhan mutual-fund plans as `ETF` and three
    /// real listings (`IVZINNIFTY`, `INFRABEES`, `NARMADA`) as `Other`/`MF`.
    /// The series column disagrees with it on exactly those 57 rows and is
    /// right on all 57.
    pub listing_class: &'a str,
    /// The vendor's ISIN column. Empty, or junk, on anything that is not a
    /// cash listing: measured, Groww writes the index ticker here on its
    /// index rows (`NIFTY`) and Dhan writes `NA`. Read only for equity.
    pub isin: &'a str,
    /// Expiry as `YYYY-MM-DD`, empty for cash and index rows.
    pub expiry: &'a str,
    /// Strike in **rupees**, empty for anything that is not an option.
    pub strike_rupees: &'a str,
    /// The vendor's separate option-side column (`CE`/`PE`), empty when the
    /// vendor encodes the side in `instrument_type` instead.
    pub option_side: &'a str,
}

impl MasterRow<'_> {
    /// The first field wider than [`MAX_FIELD_BYTES`], with its width.
    ///
    /// Every check is `str::len`, which is a field read on a fat pointer — the
    /// cost is the same ten reads whether a field holds two bytes or two
    /// gigabytes, which is the entire point. Returned rather than raised so the
    /// caller decides what an over-wide field means; [`decode_master_row`]
    /// treats it as a refusal.
    #[must_use]
    pub const fn over_wide(&self) -> Option<(&'static str, usize)> {
        // Written out rather than iterated because a `[( &str, &str ); 10]`
        // array would be built on every row -- ten pointer pairs written to the
        // stack to answer a question that is ten comparisons. Order follows the
        // struct.
        let checks: [(&'static str, usize); 11] = [
            ("vendor_id", self.vendor_id.len()),
            ("exchange", self.exchange.len()),
            ("segment", self.segment.len()),
            ("underlying", self.underlying.len()),
            ("trading_symbol", self.trading_symbol.len()),
            ("instrument_type", self.instrument_type.len()),
            ("listing_class", self.listing_class.len()),
            ("isin", self.isin.len()),
            ("expiry", self.expiry.len()),
            ("strike_rupees", self.strike_rupees.len()),
            ("option_side", self.option_side.len()),
        ];
        let mut i = 0;
        while i < checks.len() {
            // `const fn` cannot call `<[T]>::get`, and the index is bounded by
            // the array's own length one line above; const evaluation of the
            // bound is not possible here, but the loop condition is the bound.
            #[allow(clippy::indexing_slicing)]
            let (name, len) = checks[i];
            if len > MAX_FIELD_BYTES {
                return Some((name, len));
            }
            i += 1;
        }
        None
    }
}

/// The widest a single vendor master field may be before the row is refused.
///
/// # Why a bound exists at all — D-0033
///
/// [`crate::symbol::Symbol`] opens by arguing that an unbounded identifier must
/// never reach a hot path: *"A vendor that one day emits a 4 KiB identifier
/// would silently make every dedup probe 200× more expensive, and no test would
/// notice because nothing would be wrong, only slow."* The `TEST_MARKERS`
/// substring scan in [`decode_master_row`] reintroduced exactly that, one layer
/// **above** the 24-byte guard written to prevent it: it searched two raw
/// vendor `&str` before anything had bounded them.
///
/// Measured on this machine before the bound, with `underlying` pinned to
/// `RELIANCE` so only the scanned-but-unused `trading_symbol` grew: 8 B →
/// 56.4 ns, 1 KiB → 102.0 ns, 16 KiB → 997.8 ns, 4 MiB → 245,839.2 ns. Linear
/// over four orders of magnitude. And worse than the cost: the 4 MiB row was
/// **accepted and stored**, because `trading_symbol` only becomes the identity
/// when `underlying` is empty, so the width guard never saw it.
///
/// # Why 64
///
/// Measured across both real masters on 2026-08-01 — 33,990,514 B of
/// `dhan_scrip.csv` and 19,224,497 B of `groww_instruments.csv` — the widest
/// value in any column this decoder reads is **28 bytes**
/// (`MCX_MCXBULLDEX28AUG2632100CE` in Groww's `isin`,
/// `NIFTYNXT50-Aug2026-101500-CE` in Dhan's `SYMBOL_NAME`). 64 is 2.28× that,
/// so ordinary vendor drift does not trip it. The widest value in *any* column
/// of either file, including ones this decoder never reads, is 80 bytes — a
/// fund name — so a vendor moving a prose column into a column we read is
/// refused, loudly, which is the case worth catching.
///
/// This is not [`crate::symbol::SYMBOL_CAPACITY`] and must not be collapsed
/// into it. That bound is what an *identity* may be; this is what this engine
/// is willing to *look at*. A 40-byte expiry string is not a symbol and never
/// will be, but refusing to read it would refuse rows that decode correctly
/// today.
pub const MAX_FIELD_BYTES: usize = 64;

/// Which of a vendor's master columns fill a [`MasterRow`], by header name.
///
/// This lives beside the rest of the per-vendor knowledge rather than in the
/// reader, for two reasons. A reader in another crate cannot match on
/// [`Vendor`] without a wildcard arm — the enum is `#[non_exhaustive]` — and
/// that arm is unreachable, untestable, and permanently uncovered. And a
/// second reader would otherwise have to guess the same names again; the
/// column map is vendor knowledge, and vendor knowledge belongs here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterColumns {
    /// Which column carries the vendor's own instrument id.
    pub vendor_id: &'static str,
    /// Header of the exchange column.
    pub exchange: &'static str,
    /// Header of the segment column.
    pub segment: &'static str,
    /// Header of the underlying-symbol column.
    pub underlying: &'static str,
    /// Header of the vendor's own tradable-symbol column.
    pub trading_symbol: &'static str,
    /// Header of the instrument-type column.
    pub instrument_type: &'static str,
    /// Header of the column carrying the NSE board series of a cash row.
    pub listing_class: &'static str,
    /// Header of the ISIN column.
    pub isin: &'static str,
    /// Header of the expiry column.
    pub expiry: &'static str,
    /// Header of the strike column.
    pub strike: &'static str,
    /// Header of the separate option-side column, when the vendor has one.
    pub option_side: Option<&'static str>,
}

impl Vendor {
    /// The header names this vendor's master uses for each field of a
    /// [`MasterRow`].
    #[must_use]
    pub const fn master_columns(self) -> MasterColumns {
        match self {
            Self::Groww => MasterColumns {
                // The symbol Groww's own historical endpoints take:
                // `NSE-NIFTY-30Sep25-24650-CE`. NOT `trading_symbol`, which is
                // the SAME instrument spelled `NIFTY25SEP24650CE` on the same
                // row — two encodings per row, and only this one is accepted
                // by /v1/historical/candles.
                vendor_id: "groww_symbol",
                exchange: "exchange",
                segment: "segment",
                underlying: "underlying_symbol",
                trading_symbol: "trading_symbol",
                instrument_type: "instrument_type",
                listing_class: "series",
                isin: "isin",
                expiry: "expiry_date",
                strike: "strike_price",
                option_side: None,
            },
            Self::Dhan => MasterColumns {
                // What `securityId` on every Dhan request body must be. Column
                // 3 of the file, which the decoder used to step over on its way
                // to INSTRUMENT at column 5.
                vendor_id: "SECURITY_ID",
                exchange: "EXCH_ID",
                segment: "SEGMENT",
                underlying: "UNDERLYING_SYMBOL",
                trading_symbol: "SYMBOL_NAME",
                // The real type is INSTRUMENT. `INSTRUMENT_TYPE` is a
                // different column holding a vendor-minted paper class (ES,
                // DEB, ETF); it is deliberately NOT read — see the
                // `listing_class` doc and D-0025.
                instrument_type: "INSTRUMENT",
                listing_class: "SERIES",
                isin: "ISIN",
                expiry: "SM_EXPIRY_DATE",
                strike: "STRIKE_PRICE",
                option_side: Some("OPTION_TYPE"),
            },
        }
    }
}

/// A row was skipped, and why.
///
/// Skipping is not failing. A master holds every instrument the vendor knows
/// about, and most of them are legitimately not ours. The reason is carried so
/// an ingest can report *what* it declined rather than a bare count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Skip {
    /// The vendor left its own id blank, or wrote one longer than
    /// [`VENDOR_ID_CAPACITY`]. Either way the instrument cannot be named back
    /// to them, so it is declined rather than carried: a listing nothing can
    /// request is one every later lookup finds and no pull can use.
    NoVendorId,
    /// Not an exchange this engine stores. `docs/05-decisions.md` D-0017.
    ForeignExchange,
    /// An exchange test instrument, not a real listing.
    TestInstrument,
    /// A segment this engine does not store, such as commodity.
    ForeignSegment,
    /// A **currently listed** future or option.
    ///
    /// A live contract's bars are today's. Backtesting runs on history, and
    /// history is EXPIRED contracts — which come from the vendors' historical
    /// endpoints and from the existing lake, never from the live instrument
    /// master. Storing the live chain would add ~148,000 contracts holding a
    /// few weeks of data each, none of which is ever swept.
    LiveContract,
    /// A row that trades on the equity segment but is **not a share**.
    ///
    /// A debenture, a government or corporate bond, a treasury bill, a mutual
    /// fund unit, a REIT or an infrastructure trust unit, a warrant. Every one
    /// of them is a real listing; none of them is an equity, and none belongs
    /// in an equity universe.
    ///
    /// Raised only for a series code in [`NON_EQUITY_SERIES`] — a code this
    /// engine has **measured and recognises**. A code it does not recognise
    /// gets [`Skip::UnrecognisedListingClass`] instead, because "I know this
    /// is a bond" and "I have never seen this code" are different facts and
    /// filing them under one reason is how a rename becomes invisible.
    ///
    /// # Why this is a gate and not a nicety
    ///
    /// `INSTRUMENT = EQUITY` does not mean "a share". It means "trades on the
    /// equity segment". Measured across both masters: of the 12,617 rows that
    /// reach this gate, **7,137 are not equity at all** — 4,336 `SG` state
    /// development loans, 1,106 `N0` debentures, 174 `MF` fund units, 164 `GS`
    /// government securities, 85 `TB` treasury bills, and 116 further debt
    /// codes.
    ///
    /// Without the gate those rows do not merely inflate a count — they
    /// **capture the ticker**. Dhan line 167146 is
    /// `NSE,E,19257,INE121A08PJ0,EQUITY,,CHOLAFIN,…,DEB,D1,…,5.0` — a 7.5% NCD
    /// on series `D1` — and it appears **before** line 171414,
    /// `NSE,E,685,INE121A01024,EQUITY,,CHOLAFIN,…,ES,EQ,…,10.0`, the share on
    /// series `EQ`. An insert-if-absent merge therefore resolved `CHOLAFIN` to
    /// the bond and took its tick size, silently and order-dependently.
    /// `MOTHERSON` (`INE775A08105`, an NCD, series `D1`) and `ELECTCAST`
    /// (`INE086A13016`, a warrant, series `W1`) went the same way. All three
    /// are NIFTY Total Market members and two are F&O underlyings. After the
    /// gate, duplicate tickers in Dhan's equity segment fall from **4 to 0**.
    NotEquityListing,
    /// A listing on the SME board rather than the main board.
    ///
    /// A separate reason from [`Skip::NotEquityListing`] on purpose: an SME
    /// listing IS a share, so lumping it in with debentures would hide a real
    /// choice behind a wrong label. It is declined because the engine's equity
    /// universe is F&O underlyings plus NIFTY Total Market
    /// ([`crate::universe`]), and neither contains an SME listing — measured,
    /// and proven by
    /// `crate::universe::no_measured_sme_ticker_belongs_to_either_universe`.
    /// An SME row can only ever be stored, never swept and never ranked.
    ///
    /// Counted separately so the decision stays visible and reversible: the
    /// day the universe widens, this reason names exactly what to re-admit.
    /// Both vendors carry the board in the NSE series (`SM`, `ST`), so unlike
    /// before D-0025 this reason is raised **symmetrically**: 558 rows at
    /// Groww and 559 at Dhan, and the ISINs are the same paper.
    SmeBoard,
    /// A cash-equity row whose NSE series code this engine has never seen.
    ///
    /// # Why this is not filed with the bonds
    ///
    /// This is the variant that exists because of what happened when it did
    /// not. Before it, both arms of the gate ended in `_ => NotEquity`, so an
    /// unrecognised code was reported with the identical wording a routine
    /// debenture gets. Demonstrated on the real Dhan master by rewriting the
    /// equity series `EQ` to `EQX`: **2,438 shares vanished**, every F&O
    /// underlying among them, the report still printed `ok`, and the only
    /// trace was one counter moving. That is precisely the failure
    /// `Vendor::segment_of` documents as unrepeatable.
    ///
    /// It is a decline rather than an error because the alphabet is genuinely
    /// open-ended — NSE mints a debt series whenever it needs one and 120
    /// already exist, so a new bond series must not fail an ingest. But it is
    /// its **own** decline: its own reason string, its own counter, and the
    /// offending code itself is carried to the operator by the reader (see
    /// `api::master::Loaded::unrecognised`). A universe holding one is
    /// *disputed*, so the process reporting it exits non-zero rather than
    /// printing a routine skip and `ok`.
    UnrecognisedListingClass,
}

impl Skip {
    /// A short human-readable reason, stable enough to be a report key.
    ///
    /// It lives here rather than in a reporter because [`Skip`] is
    /// `#[non_exhaustive]`: outside this crate every `match` on it needs a
    /// wildcard arm, which silently swallows a variant added later under
    /// whatever label the wildcard chose. Inside the crate the match is
    /// exhaustive, so a new variant is a compile error until it is named.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoVendorId => "no vendor id on the row",
            Self::ForeignExchange => "foreign exchange",
            Self::TestInstrument => "exchange test instrument",
            Self::ForeignSegment => "segment not stored",
            Self::LiveContract => "live derivative contract",
            Self::NotEquityListing => "not an equity listing",
            Self::SmeBoard => "SME board",
            Self::UnrecognisedListingClass => "unrecognised listing class",
        }
    }

    /// Whether this decline is a routine business outcome.
    ///
    /// Every reason here is a *deliberate* decline, but they are not equally
    /// expected. A bond, a foreign exchange and a live contract are what a
    /// master is full of. An unrecognised series code is the vendor, or the
    /// exchange, having changed something under us — and the difference has to
    /// reach an exit code, or the two are the same fact to a monitor.
    #[must_use]
    pub const fn is_routine(self) -> bool {
        !matches!(self, Self::UnrecognisedListingClass)
    }

    /// Whether this decline judges the **paper** rather than the venue.
    ///
    /// Only a judgement about the paper can contradict another vendor keeping
    /// the same ISIN. A decline for the exchange, the segment or the contract
    /// being live says where the row was found, and the same security
    /// legitimately appears at another venue — `RELIANCE` is `INE002A01018` on
    /// both NSE and BSE, and one vendor declining the BSE row while the other
    /// keeps the NSE row is two correct decisions about two different rows.
    /// Treating that as a disagreement produced 3,000 false conflicts on the
    /// real masters, which is a check nobody would read twice.
    #[must_use]
    pub const fn judges_the_paper(self) -> bool {
        matches!(
            self,
            Self::NotEquityListing | Self::SmeBoard | Self::UnrecognisedListingClass
        )
    }
}

/// A row this engine keeps: the canonical key, and the vendor's ISIN **beside
/// it** rather than in it.
///
/// See [`crate::isin`] for why the ISIN is not a field of [`InstrumentKey`].
/// Every field is fixed-width and [`Copy`], so a listing is as cheap to hash
/// and move as the key alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Listing {
    /// The vendor's own id for this instrument, carried through so a request
    /// can name it. Without this nothing reaches either broker — see
    /// [`VendorId`].
    pub vendor_id: VendorId,
    /// The canonical identity, from the columns the vendor actually fills.
    pub key: InstrumentKey,
    /// The vendor's ISIN for this row, when it has one. `None` for an index,
    /// which has no ISIN at all — no sentinel is invented for NIFTY.
    pub isin: Option<Isin>,
    /// The same identity with the vendor's own series suffix removed, when
    /// the symbol carries one — `BLUECHIP-BE` under series `BE` gives
    /// `BLUECHIP`.
    ///
    /// A **candidate**, never a substitution. Groww leaks
    /// `internal_trading_symbol` into `trading_symbol` on exactly 209 of the
    /// 4,080 ISINs the two masters share, and the leak reaches tradeable
    /// equity and ETFs (`BLUECHIP-BE`, `CBAZAAR-ST`, `HDFCLIQUID-EQ`,
    /// `LOWVOL-EQ`), so "only debt is suffixed" is false. But `BAJAJ-AUTO` is
    /// a real ticker that ends in a dash, and stripping blind would
    /// manufacture the very collision [`crate::symbol`] refuses to
    /// manufacture. So this is computed only when the trailing segment IS the
    /// row's own series, and the caller adopts it only when a second vendor
    /// confirms the identity by ISIN.
    pub unsuffixed: Option<InstrumentKey>,
}

/// A row this engine declined, and the evidence of what it declined.
///
/// # Why the ISIN travels with a decline
///
/// A merge that only compares the rows both vendors KEPT cannot see the
/// disagreement that actually matters: one vendor calling a security an equity
/// while the other calls it a bond. Before this struct existed, a declined row
/// was dropped at the reader and the merge reported `0 conflicts` while the
/// two masters disagreed about the eligibility of 62 instruments — every one
/// of which carried the **same ISIN in both files**, so the check had the key
/// it needed and never looked. See `api::merge::Merged::eligibility`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declined {
    /// Why the row was declined.
    pub reason: Skip,
    /// The row's ISIN, when the vendor gave one that parses.
    ///
    /// `None` is not a failure being hidden: the row is declined and counted
    /// under [`Declined::reason`] either way, and this field is *evidence for
    /// a cross-check*, never identity. It is deliberately parsed leniently
    /// here — unlike on a kept equity, where a bad ISIN is an error — because
    /// the one real row in either master whose check digit fails
    /// (`IN1520250085`, a state development loan) is a declined row, and
    /// erroring on it would turn a correct decline into a false failure.
    pub isin: Option<Isin>,
}

/// The outcome of reading one master row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// A real instrument this engine stores.
    Keep(Listing),
    /// A row deliberately declined, with the reason and the evidence.
    Skipped(Declined),
}

impl Decoded {
    /// The reason this row was declined, or `None` if it was kept.
    ///
    /// Exists so a caller that cares only about the reason does not have to
    /// spell out the evidence beside it.
    #[must_use]
    pub const fn skip(self) -> Option<Skip> {
        match self {
            Self::Keep(_) => None,
            Self::Skipped(d) => Some(d.reason),
        }
    }
}

/// Exchange test listings carry these markers in the underlying symbol.
///
/// Real rows observed in the primary broker's master include
/// `031NSETEST36DECFUT` and `061NSETEST36DECFUT`, whose underlyings are
/// `031NSETEST` and `061NSETEST`. Storing them would put fabricated
/// instruments beside real ones, and they would be indistinguishable later.
const TEST_MARKERS: [&str; 2] = ["NSETEST", "BSETEST"];

/// What a vendor's segment code means to us.
///
/// It carries no `Segment`: the vendor's column is a GATE only. Our segment is
/// derived from the instrument type, because the primary broker files spot
/// indices under `CASH` and adopting its value would put NIFTY in the
/// equities directory.
#[derive(PartialEq, Eq)]
enum SegmentVerdict {
    /// Rows in this segment are stored.
    Store,
    /// A segment this engine legitimately does not store.
    Decline,
}

/// What an NSE board series means to us.
///
/// Consulted **only** for a row that already decoded as cash equity. An index
/// row has no series at all — Groww leaves `series` empty on all 24 of its NSE
/// index rows, Dhan writes `NA` — so gating on it before the instrument type
/// is known would decline `NIFTY` and `BANKNIFTY`, which is every instrument
/// the engine exists to sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EquityVerdict {
    /// A genuine main-board share or ETF.
    MainBoard,
    /// A share, but on the SME board.
    Sme,
    /// Not a share at all: debt, a fund unit, a warrant.
    NotEquity,
    /// A code this engine has never seen. Never merely "not equity".
    Unrecognised,
}

/// The NSE series codes that ARE the equity board.
///
/// Measured across both real masters, 2026-08-01, over every cash-equity row:
///
/// | Series | Rows | What it is |
/// |---|---:|---|
/// | `EQ` | 4,845 | the rolling-settlement equity board |
/// | `BE` | 580 | trade-for-trade equity |
/// | `BZ` | 63 | trade-for-trade under surveillance |
/// | `E1` | 5 | partly-paid equity |
/// | `IT` | 4 | trade-for-trade, illiquid |
/// | `SZ` | 3 | trade-for-trade, surveillance (second list) |
///
/// `BZ`, `IT`, `SZ` and `E1` were declined before D-0025, and the decline was
/// counted under "not an equity listing", which was false about 30 real
/// shares — `HDIL`, `HMT`, `RAJESHEXPO`, `IL&FSENGG`, `ANSALAPI`, `FEL`,
/// `ARSHIYA`, `CEREBRAINT` among them. Three independent confirmations that
/// they are equity: the ISIN's NSDL security-type digits are `01` (ordinary
/// equity) on 27 of the 30 and `IN9…` (partly paid) on the other 3, against
/// `08` for the `CHOLAFIN` NCD and `13` for the `ELECTCAST` warrant this gate
/// still declines; every one of them is `ES` in Dhan's paper-class column; and
/// they are ordinary listed companies.
///
/// Sorted, because `board_of` binary-searches it and an unsorted array makes
/// `binary_search` return garbage in silence.
pub const EQUITY_BOARD_SERIES: [&str; 6] = ["BE", "BZ", "E1", "EQ", "IT", "SZ"];

/// The NSE series codes that are the SME board.
///
/// Measured: `SM` 815 rows and `ST` 302 across both masters. A share, on a
/// board this engine's universes do not reach. See [`Skip::SmeBoard`].
pub const SME_BOARD_SERIES: [&str; 2] = ["SM", "ST"];

/// Every NSE series this engine has measured that is **not** an equity.
///
/// 120 codes, transcribed from the union of both real masters on 2026-08-01 —
/// every distinct `series` on a Groww `NSE`/`CASH`/`EQ` row and every distinct
/// `SERIES` on a Dhan `NSE`/`E`/`EQUITY` row, minus the eight board codes
/// above. Debentures (`N0`…`NZ`, `Y*`, `Z*`, `AK`…`BX`, `D1`, `W1`), state
/// development loans (`SG`), government securities (`GS`, `GB`), treasury
/// bills (`TB`), fund units (`MF`, `SF`), REITs (`RR`), infrastructure trusts
/// (`IV`),
/// pass-through certificates and preference shares (`P1`).
///
/// # Why an explicit list rather than "everything else"
///
/// "Everything else" is what made a renamed equity code indistinguishable from
/// a bond. Enumerating what is known turns the unknown into its own visible
/// outcome — [`Skip::UnrecognisedListingClass`]. The list is data, so a new
/// NSE debt series is a one-line append and nothing else moves.
///
/// Sorted, for `board_of`'s binary search.
pub const NON_EQUITY_SERIES: [&str; 120] = [
    "AK", "AL", "AM", "AN", "AZ", "BA", "BC", "BR", "BS", "BU", "BV", "BW", "BX", "D1", "GB", "GS",
    "IV", "MF", "N0", "N1", "N2", "N3", "N4", "N5", "N6", "N7", "N8", "N9", "NA", "NB", "NC", "ND",
    "NE", "NF", "NG", "NH", "NI", "NJ", "NK", "NL", "NM", "NN", "NO", "NP", "NQ", "NR", "NS", "NT",
    "NU", "NV", "NW", "NX", "NY", "NZ", "P1", "RR", "SF", "SG", "TB", "W1", "Y0", "Y1", "Y2", "Y3",
    "Y4", "Y5", "Y6", "Y7", "Y8", "Y9", "YA", "YB", "YC", "YD", "YG", "YH", "YI", "YJ", "YK", "YL",
    "YM", "YP", "YQ", "YR", "YS", "YT", "YU", "YV", "YW", "YX", "YY", "YZ", "Z0", "Z1", "Z2", "Z3",
    "Z4", "Z5", "Z6", "Z7", "Z8", "Z9", "ZC", "ZF", "ZG", "ZH", "ZI", "ZJ", "ZK", "ZL", "ZM", "ZN",
    "ZO", "ZP", "ZQ", "ZR", "ZS", "ZT", "ZY", "ZZ",
];

/// What an NSE board series means, for either vendor.
///
/// # Why this is not per-vendor any more
///
/// It was, and the doc argued that it had to be: "the two alphabets are
/// disjoint, and one flat table invites one vendor's code to be silently
/// accepted for the other". That argument was true of the columns the gate
/// used to read — Groww's NSE `series` against Dhan's own `INSTRUMENT_TYPE`
/// paper class — and it stopped being true when D-0025 pointed both vendors at
/// the series. A series code is minted by the exchange, not by a broker, so it
/// is **one alphabet**, and duplicating it per vendor would be two copies of
/// one fact free to drift apart.
///
/// It is not an assumption. Measured on the 4,080 ISINs the two masters share:
/// the two vendors' series columns disagree on 22 rows — all of them
/// snapshot skew inside one board, `EQ`↔`BE` or `SM`↔`ST`, e.g. `SICALLOG` —
/// and the verdict this function returns differs on **0**.
///
/// An unrecognised code is a decline, not an error, for the reason
/// [`Skip::UnrecognisedListingClass`] gives — but it is its own decline, and
/// never confused with a bond.
fn board_of(series: &str) -> EquityVerdict {
    // Dhan pads this column, e.g. `"   ES   "`. Trimming Groww's already-tight
    // values costs nothing and cannot change a verdict.
    let series = series.trim();
    if EQUITY_BOARD_SERIES.binary_search(&series).is_ok() {
        EquityVerdict::MainBoard
    } else if SME_BOARD_SERIES.binary_search(&series).is_ok() {
        EquityVerdict::Sme
    } else if NON_EQUITY_SERIES.binary_search(&series).is_ok() {
        EquityVerdict::NotEquity
    } else {
        EquityVerdict::Unrecognised
    }
}

impl Vendor {
    /// Maps this vendor's segment code.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] for a code this vendor is not known to
    /// emit. That is deliberately an ERROR and not a decline: the secondary
    /// broker writes its segments as single letters, and treating an
    /// unrecognised code as "not ours" made the decoder discard **all 200,460
    /// of its rows while reporting a routine skip**. A mapping bug must never
    /// be indistinguishable from a legitimate refusal.
    fn segment_of(self, code: &str) -> Result<SegmentVerdict, InstrumentError> {
        // Nested per vendor rather than matched on the pair: the two vendors
        // use disjoint alphabets, and a flat match invites one vendor's code
        // to be silently accepted for the other.
        let (store, decline): (&[&str], &[&str]) = match self {
            Self::Groww => (&["INDEX", "CASH", "FNO"], &["COMMODITY"]),
            // I index, E equity cash, D equity+index derivatives,
            // C currency, M commodity.
            Self::Dhan => (&["I", "E", "D"], &["C", "M"]),
        };
        if store.contains(&code) {
            Ok(SegmentVerdict::Store)
        } else if decline.contains(&code) {
            Ok(SegmentVerdict::Decline)
        } else {
            Err(InstrumentError::Malformed)
        }
    }

    /// Normalises this vendor's instrument-type code to `IDX`/`EQ`/`FUT`/`CE`/`PE`.
    ///
    /// `side` is the vendor's separate option-side column, needed because the
    /// secondary broker types every option as `OPTSTK`/`OPTIDX` and carries
    /// call-versus-put in its own field.
    ///
    /// # Errors
    ///
    /// [`InstrumentError::Malformed`] for an unrecognised code, for the same
    /// reason as [`Vendor::segment_of`].
    fn type_of(self, code: &str, side: &str) -> Result<Option<&'static str>, InstrumentError> {
        match self {
            Self::Groww => match code {
                "IDX" | "EQ" | "FUT" | "CE" | "PE" => Ok(Some(match code {
                    "IDX" => "IDX",
                    "EQ" => "EQ",
                    "FUT" => "FUT",
                    "CE" => "CE",
                    _ => "PE",
                })),
                _ => Err(InstrumentError::Malformed),
            },
            Self::Dhan => match code {
                "INDEX" => Ok(Some("IDX")),
                "EQUITY" => Ok(Some("EQ")),
                "FUTSTK" | "FUTIDX" => Ok(Some("FUT")),
                "OPTSTK" | "OPTIDX" => match side {
                    "CE" => Ok(Some("CE")),
                    "PE" => Ok(Some("PE")),
                    _ => Err(InstrumentError::Malformed),
                },
                // Currency derivatives are declined, not stored.
                "FUTCUR" | "OPTCUR" => Ok(None),
                _ => Err(InstrumentError::Malformed),
            },
        }
    }
}

/// Reads one master row into a canonical key.
///
/// # Errors
///
/// [`InstrumentError`] when a field is present but malformed, or when a
/// segment or instrument-type code is one this vendor is not known to emit. A
/// malformed row is an error rather than a skip: skipping is for rows that are
/// *validly* not ours, and quietly dropping a row we failed to understand is
/// how an instrument silently vanishes from a universe.
pub fn decode_master_row(vendor: Vendor, row: MasterRow<'_>) -> Result<Decoded, InstrumentError> {
    // THE WIDTH GATE IS FIRST, AND THAT POSITION IS THE WHOLE FIX.
    //
    // Everything below this line reads a vendor field: the ISIN parse in
    // `declined`, the exchange parse, and — the one that made this a defect
    // rather than a worry — the `TEST_MARKERS` substring scan, which searches
    // TWO raw fields with a seven-byte needle. Before D-0033 the only width
    // bound in the pipeline was `Symbol::new`, about a HUNDRED lines further
    // down, and it saw only whichever field became the identity. A 4 MiB
    // `trading_symbol` on a row with a populated `underlying` was scanned in
    // full, cost 246 us, and was then ACCEPTED. Even the rows it did refuse, it
    // refused only after paying the scan.
    //
    // `str::len` is a field read. Ten of them is a constant, whatever the
    // vendor sent. CLAUDE.md §3 rule 4.
    if let Some((field, len)) = row.over_wide() {
        return Err(InstrumentError::FieldTooWide { field, len });
    }

    // A declined row's ISIN is EVIDENCE, never identity, so it is parsed
    // leniently and only ever used to compare two vendors' verdicts. Computed
    // once here rather than at each of the seven decline sites below.
    let declined = |reason: Skip| {
        Ok(Decoded::Skipped(Declined {
            reason,
            isin: Isin::new(row.isin).ok(),
        }))
    };

    // Only NSE is stored. D-0017. An unparseable exchange is a decline: a
    // master legitimately lists venues we do not store.
    if !matches!(Exchange::parse(row.exchange), Ok(Exchange::Nse)) {
        return declined(Skip::ForeignExchange);
    }
    let exchange = Exchange::Nse;

    if TEST_MARKERS
        .iter()
        .any(|m| row.underlying.contains(m) || row.trading_symbol.contains(m))
    {
        return declined(Skip::TestInstrument);
    }

    // The vendor's segment column is a GATE, never our segment. The primary
    // broker files spot indices under `CASH`, so adopting its value would put
    // NIFTY in the equities directory and make `is_sweepable` false for the
    // two instruments the engine exists to sweep. Our segment comes from the
    // instrument type below, which is the only field whose meaning is stable
    // across vendors.
    if matches!(vendor.segment_of(row.segment)?, SegmentVerdict::Decline) {
        return declined(Skip::ForeignSegment);
    }

    let Some(ty) = vendor.type_of(row.instrument_type, row.option_side)? else {
        return declined(Skip::ForeignSegment);
    };

    // WHERE THE INSTRUMENT IS NAMED DEPENDS ON WHAT IT IS.
    //
    // The primary broker leaves `underlying_symbol` EMPTY on every cash and
    // index row -- all 4,104 of them, NIFTY and BANKNIFTY included. The name
    // lives in `trading_symbol` there. On derivative rows the opposite holds:
    // `trading_symbol` is the contract (`ASHOKLEY26SEP117.5CE`, which contains
    // a `.` and is not a legal Symbol), while `underlying_symbol` is the
    // underlying we actually want.
    //
    // Reading one column for everything fails either way round. This was found
    // by decoding the real master, not by reading it.
    // WHERE THE TICKER LIVES IS PER-VENDOR AND OPPOSITE BETWEEN THEM.
    //
    // Groww leaves `underlying_symbol` EMPTY on all 4,104 cash and index rows
    // and puts the ticker in `trading_symbol`.
    //
    // Dhan does the reverse: `UNDERLYING_SYMBOL` is the ticker (`GOLDSTAR`,
    // `ARE&M`) while `SYMBOL_NAME` is the COMPANY NAME -- "GOLDSTAR POWER
    // LIMITED", "AMARA RAJA ENERGY MOB LTD". 9,623 of its 9,674 NSE equity
    // rows carry a space there, so reading it refused almost the entire
    // vendor. Measured, not guessed.
    //
    // Preferring the underlying and falling back to the trading symbol
    // satisfies both without a vendor branch: Groww's underlying is empty so
    // the fallback fires; Dhan's is populated so it wins.
    let name = if row.underlying.is_empty() {
        row.trading_symbol
    } else {
        row.underlying
    };
    let underlying = Symbol::new(name)?;

    // A live instrument master lists only CURRENTLY LISTED contracts -- both
    // vendors purge on expiry, and the earliest expiry in either master is
    // three days from now. So every derivative row here is live by definition,
    // and none of it is backtest data. The expiry is still PARSED first, so a
    // malformed date is an error rather than being hidden behind the skip.
    let (segment, kind) = match ty {
        "IDX" => (Segment::Index, Kind::Index),
        // THE EQUITY GATE APPLIES HERE AND ONLY HERE. An index row carries no
        // series -- Groww's `series` is empty on all 24 of its NSE index rows
        // and Dhan writes `NA` -- so gating any earlier deletes NIFTY and
        // BANKNIFTY.
        "EQ" => match board_of(row.listing_class) {
            EquityVerdict::MainBoard => (Segment::Cash, Kind::Equity),
            EquityVerdict::Sme => return declined(Skip::SmeBoard),
            EquityVerdict::NotEquity => return declined(Skip::NotEquityListing),
            EquityVerdict::Unrecognised => return declined(Skip::UnrecognisedListingClass),
        },
        "FUT" => {
            parse_expiry(row.expiry)?;
            return declined(Skip::LiveContract);
        }
        _ => {
            parse_expiry(row.expiry)?;
            parse_strike(row.strike_rupees)?;
            return declined(Skip::LiveContract);
        }
    };

    let key = InstrumentKey {
        exchange,
        segment,
        underlying,
        kind,
    };

    // THE ISIN IS READ FOR EQUITY AND NOTHING ELSE, and it is read only after
    // the gate above. Both halves of that sentence are load-bearing:
    //
    //   * An index row's ISIN column is not empty, it is JUNK. Measured: Groww
    //     writes the index ticker there (`NIFTY`) and Dhan writes `NA`.
    //     Parsing it would refuse every index in both masters.
    //   * The one row in either master whose check digit does not verify,
    //     `IN1520250085`, is a state development loan on series `SG`. It never
    //     reaches this line because the gate declined it first. Order is what
    //     keeps that true.
    //
    // On an equity row the ISIN is REQUIRED, not optional: every one of the
    // 2,726 Groww and 2,774 Dhan main-board rows carries one, so a missing or
    // malformed value means the row is not what we think it is, and that is an
    // error rather than a quiet `None`.
    let isin = match kind {
        Kind::Equity => Some(Isin::new(row.isin)?),
        _ => None,
    };

    let Some(vendor_id) = VendorId::new(row.vendor_id) else {
        // A row with no usable id cannot be requested from the vendor, so it is
        // DECLINED rather than kept: keeping it would put an instrument in the
        // index that every later lookup would find and no pull could name.
        return declined(Skip::NoVendorId);
    };
    Ok(Decoded::Keep(Listing {
        vendor_id,
        key,
        isin,
        unsuffixed: unsuffixed_key(key, name, row.listing_class)?,
    }))
}

/// The same key with the row's own series suffix removed, if it has one.
///
/// `BLUECHIP-BE` under series `BE` gives `BLUECHIP`; `RELIANCE` under `EQ`
/// gives nothing; and `BAJAJ-AUTO` under `EQ` gives nothing either, which is
/// the whole point — the trailing segment must BE the row's own series, not
/// merely look like a suffix.
///
/// # Errors
///
/// [`InstrumentError::Malformed`] if stripping would leave nothing to name the
/// instrument with. A row called `-EQ` is a vendor bug, and a bug is loud here
/// rather than silently unstripped.
fn unsuffixed_key(
    key: InstrumentKey,
    name: &str,
    class: &str,
) -> Result<Option<InstrumentKey>, InstrumentError> {
    // Only a cash listing has a series. An empty class would make
    // `strip_suffix` succeed on every symbol, so it is excluded explicitly.
    let class = class.trim();
    if key.kind != Kind::Equity || class.is_empty() {
        return Ok(None);
    }
    let Some(stripped) = name
        .strip_suffix(class)
        .and_then(|rest| rest.strip_suffix('-'))
    else {
        return Ok(None);
    };
    Ok(Some(InstrumentKey {
        underlying: Symbol::new(stripped)?,
        ..key
    }))
}

/// Parses an `YYYY-MM-DD` expiry from the master's own column.
///
/// # Errors
///
/// [`InstrumentError::Malformed`] on any shape other than exactly
/// `YYYY-MM-DD` with numeric parts, or on a date that does not exist.
fn parse_expiry(text: &str) -> Result<Expiry, InstrumentError> {
    let mut parts = text.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(InstrumentError::Malformed);
    };
    // Fixed widths, so a value like `2026-8-4` is refused rather than guessed
    // at — a vendor that changes its date format should be a loud failure.
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(InstrumentError::Malformed);
    }
    let year: u16 = y.parse().map_err(|_| InstrumentError::Malformed)?;
    let month: u8 = m.parse().map_err(|_| InstrumentError::Malformed)?;
    let day: u8 = d.parse().map_err(|_| InstrumentError::Malformed)?;
    Expiry::new(year, month, day)
}

/// Converts a rupee strike to paisa.
///
/// # Errors
///
/// [`InstrumentError::Malformed`] if the value is not a number or does not fit
/// in `i64` paisa.
fn parse_strike(text: &str) -> Result<Paisa, InstrumentError> {
    let rupees: f64 = text.parse().map_err(|_| InstrumentError::Malformed)?;
    Paisa::from_rupees_half_up(rupees).map_err(|_| InstrumentError::Malformed)
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

    /// A real main-board ISIN, so that a row which reaches the ISIN parse
    /// carries one. `RELIANCE`, verbatim from both masters.
    const REAL_ISIN: &str = "INE002A01018";

    /// The whole kept listing, or `None` if the row was skipped.
    ///
    /// Returning an Option rather than destructuring with `else { panic!() }`
    /// keeps every branch reachable, so the coverage gate stays honest: an
    /// unreachable panic arm is an uncovered region that no test can ever
    /// exercise.
    fn listing(d: Decoded) -> Option<Listing> {
        match d {
            Decoded::Keep(l) => Some(l),
            Decoded::Skipped(_) => None,
        }
    }

    /// The kept key alone, for the tests that care only about identity.
    fn kept(d: Decoded) -> Option<InstrumentKey> {
        listing(d).map(|l| l.key)
    }

    /// Builds a Groww-shaped row. `trading_symbol` mirrors `underlying` for
    /// derivative rows, which is what the real master does.
    ///
    /// The listing class and ISIN are those of an ordinary main-board share,
    /// because that is what most rows are; every test of the equity gate sets
    /// them explicitly rather than relying on this.
    fn row<'a>(
        exchange: &'a str,
        segment: &'a str,
        underlying: &'a str,
        ty: &'a str,
        expiry: &'a str,
        strike: &'a str,
    ) -> MasterRow<'a> {
        MasterRow {
            vendor_id: "1333",
            exchange,
            segment,
            underlying,
            trading_symbol: underlying,
            instrument_type: ty,
            listing_class: "EQ",
            isin: REAL_ISIN,
            expiry,
            strike_rupees: strike,
            option_side: "",
        }
    }

    /// Decodes a Groww row.
    fn groww(r: MasterRow<'_>) -> Result<Decoded, InstrumentError> {
        decode_master_row(Vendor::Groww, r)
    }

    /// A decline with no ISIN beside it, for the helpers' own tests.
    fn bare(reason: Skip) -> Decoded {
        Decoded::Skipped(Declined { reason, isin: None })
    }

    #[test]
    fn the_kept_helper_covers_its_negative_arm() {
        assert!(kept(bare(Skip::TestInstrument)).is_none());
        assert!(kept(bare(Skip::LiveContract)).is_none());
        // `skip` is the mirror image, and both arms are exercised here so no
        // caller has to prove them again.
        assert_eq!(bare(Skip::LiveContract).skip(), Some(Skip::LiveContract));
        let keep = groww(row("NSE", "CASH", "RELIANCE", "EQ", "", "")).expect("ok");
        assert_eq!(keep.skip(), None, "a kept row was not skipped");
    }

    #[test]
    fn vendor_path_segments_are_stable() {
        assert_eq!(Vendor::Groww.as_str(), "groww");
        assert_eq!(Vendor::Dhan.as_str(), "dhan");
        assert_ne!(Vendor::Groww, Vendor::Dhan);
    }

    #[test]
    fn the_two_engine_indices_decode() {
        // Exactly as they appear in the real master:
        //   NSE,NIFTY,NIFTY,NSE-NIFTY,NIFTY 50,IDX,CASH,...
        for sym in ["NIFTY", "BANKNIFTY"] {
            let got = groww(row("NSE", "CASH", sym, "IDX", "", "")).expect("well formed");
            let key = kept(got).expect("must be kept");
            assert_eq!(key.kind, Kind::Index);
            assert!(key.is_sweepable(), "{sym} is one of the two swept");
        }
    }

    #[test]
    fn exchange_test_instruments_are_skipped() {
        // Real rows: 031NSETEST36DECFUT and 061NSETEST36DECFUT. Storing these
        // would put fabricated instruments beside real ones, indistinguishable
        // afterwards.
        for u in ["031NSETEST", "061NSETEST", "BSETEST01"] {
            assert_eq!(
                groww(row("NSE", "FNO", u, "FUT", "2036-11-27", ""))
                    .expect("ok")
                    .skip(),
                Some(Skip::TestInstrument),
                "{u} must be skipped"
            );
        }
    }

    #[test]
    fn bse_and_unknown_exchanges_are_skipped_not_stored() {
        // D-0017 -- NSE only.
        assert_eq!(
            groww(row("BSE", "CASH", "SENSEX", "IDX", "", ""))
                .expect("ok")
                .skip(),
            Some(Skip::ForeignExchange)
        );
        assert_eq!(
            groww(row("MCX", "COMMODITY", "GOLD", "FUT", "2026-08-05", ""))
                .expect("ok")
                .skip(),
            Some(Skip::ForeignExchange)
        );
    }

    #[test]
    fn commodity_segment_is_skipped() {
        assert_eq!(
            groww(row("NSE", "COMMODITY", "GOLD", "FUT", "2026-08-05", ""))
                .expect("ok")
                .skip(),
            Some(Skip::ForeignSegment)
        );
    }

    #[test]
    fn an_equity_decodes_and_is_stored_not_swept() {
        let got = groww(row("NSE", "CASH", "RELIANCE", "EQ", "", "")).expect("ok");
        let key = kept(got).expect("kept");
        assert_eq!(key.kind, Kind::Equity);
        assert!(!key.is_sweepable(), "D-0018: stored, not swept");
    }

    #[test]
    fn a_malformed_row_errors_rather_than_being_skipped_silently() {
        // Skipping is for rows that are VALIDLY not ours. A row we failed to
        // understand must be loud, or an instrument vanishes without trace.
        assert!(groww(row("NSE", "FNO", "NIFTY", "XX", "2026-08-04", "1")).is_err());
        assert!(groww(row("NSE", "FNO", "NIFTY", "CE", "not-a-date", "1")).is_err());
        assert!(groww(row("NSE", "FNO", "NIFTY", "CE", "2026-08-04", "abc")).is_err());
        assert!(groww(row("NSE", "FNO", "NIF TY", "FUT", "2026-08-04", "")).is_err());
    }

    #[test]
    fn a_right_length_but_non_numeric_date_part_is_refused() {
        // Distinct from the wrong-LENGTH cases below: these have exactly the
        // 4-2-2 shape, so they pass the width check and must be caught by the
        // numeric parse. Without this, a vendor emitting "20X6-08-04" would
        // reach Expiry::new with whatever a lenient parse produced.
        for bad in ["20X6-08-04", "2026-0X-04", "2026-08-0X", "----------"] {
            assert!(
                groww(row("NSE", "FNO", "NIFTY", "FUT", bad, "")).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn expiry_column_must_be_exactly_yyyy_mm_dd() {
        // A vendor that changes its date format is a loud failure, not a guess.
        for bad in [
            "2026-8-4",
            "26-08-04",
            "2026/08/04",
            "2026-08",
            "",
            "2026-08-04-01",
        ] {
            assert!(
                groww(row("NSE", "FNO", "NIFTY", "FUT", bad, "")).is_err(),
                "{bad} must be refused"
            );
        }
        // And an impossible date is refused by Expiry itself.
        assert!(groww(row("NSE", "FNO", "NIFTY", "FUT", "2026-02-31", "")).is_err());
    }

    #[test]
    fn the_real_nifty_row_decodes_from_the_columns_the_master_actually_fills() {
        // THE ROW THAT BROKE EVERYTHING. Verbatim from the master:
        //   NSE,NIFTY,NIFTY,NSE-NIFTY,NIFTY 50,IDX,CASH,,NIFTY,,,,,,,,,0,0,,0
        //         ^col3 trading_symbol            ^col10 underlying_symbol = EMPTY
        //
        // underlying_symbol is empty on ALL 4,104 Groww cash and index rows,
        // NIFTY and BANKNIFTY among them. The previous test passed "NIFTY" as
        // the underlying -- a col-3 value fed into the col-10 field -- so a
        // total decode failure on the engine's primary instrument was green.
        let real = MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "CASH",
            underlying: "", // <-- exactly as the file has it
            trading_symbol: "NIFTY",
            instrument_type: "IDX",
            // Empty series and an isin column holding the TICKER, both
            // exactly as the file has them. Neither may reach a validator.
            listing_class: "",
            isin: "NIFTY",
            expiry: "",
            strike_rupees: "",
            option_side: "",
        };
        let key = kept(groww(real).expect("the real row must decode")).expect("kept");
        assert_eq!(key.underlying.as_str(), "NIFTY");
        assert_eq!(key.segment, Segment::Index);
        assert!(
            key.is_sweepable(),
            "NIFTY must be sweepable from its real row"
        );
    }

    #[test]
    fn an_unknown_vendor_code_is_loud_never_a_silent_decline() {
        // This is the whole lesson. Treating an unrecognised code as "not ours"
        // is what let 200,460 Dhan rows disappear while reporting a routine
        // skip. A mapping bug must never look like a legitimate refusal.
        let bad_segment = MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "Z",
            underlying: "NIFTY",
            trading_symbol: "NIFTY",
            instrument_type: "INDEX",
            listing_class: "NA",
            isin: "NA",
            expiry: "",
            strike_rupees: "",
            option_side: "",
        };
        assert!(decode_master_row(Vendor::Dhan, bad_segment).is_err());
        // Groww's letters are not Dhan's, and vice versa.
        assert!(groww(bad_segment).is_err());
        let dhan_shaped_at_groww = MasterRow {
            vendor_id: "1333",
            segment: "I",
            ..bad_segment
        };
        assert!(
            groww(dhan_shaped_at_groww).is_err(),
            "Dhan codes must not silently work for Groww"
        );

        let bad_type = MasterRow {
            vendor_id: "1333",
            segment: "D",
            instrument_type: "DBT",
            ..bad_segment
        };
        assert!(decode_master_row(Vendor::Dhan, bad_type).is_err());
    }

    #[test]
    fn currency_and_commodity_are_declined_not_stored() {
        let cur = MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "C",
            underlying: "USDINR",
            trading_symbol: "USDINR",
            instrument_type: "OPTCUR",
            listing_class: "",
            isin: "",
            expiry: "2026-08-04",
            strike_rupees: "83.625",
            option_side: "CE",
        };
        assert_eq!(
            decode_master_row(Vendor::Dhan, cur).expect("ok").skip(),
            Some(Skip::ForeignSegment)
        );
    }

    #[test]
    fn a_declined_instrument_type_is_a_skip_with_a_reason() {
        // Currency derivatives are a recognised type this engine does not
        // store -- distinct from an unrecognised code, which is an error.
        let cur = MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "D",
            underlying: "USDINR",
            trading_symbol: "USDINR26AUGFUT",
            instrument_type: "FUTCUR",
            listing_class: "",
            isin: "",
            expiry: "2026-08-26",
            strike_rupees: "",
            option_side: "",
        };
        assert_eq!(
            decode_master_row(Vendor::Dhan, cur).expect("ok").skip(),
            Some(Skip::ForeignSegment)
        );
    }

    #[test]
    fn every_live_derivative_is_skipped_not_stored() {
        // The operator's rule, enforced at the decoder: a live instrument
        // master lists ONLY currently-listed contracts -- both vendors purge
        // on expiry and the earliest expiry in either is days away. Backtests
        // run on EXPIRED contracts, which come from the historical endpoints
        // and the lake. Storing the live chain adds ~148,000 contracts holding
        // a few weeks each, none ever swept.
        for (ty, strike) in [("FUT", ""), ("CE", "19450"), ("PE", "19450")] {
            assert_eq!(
                groww(row("NSE", "FNO", "NIFTY", ty, "2026-08-04", strike))
                    .expect("ok")
                    .skip(),
                Some(Skip::LiveContract),
                "{ty} must be skipped as a live contract"
            );
        }
        // Dhan's spellings too.
        for ty in ["FUTIDX", "FUTSTK", "OPTIDX", "OPTSTK"] {
            let r = MasterRow {
                vendor_id: "1333",
                exchange: "NSE",
                segment: "D",
                underlying: "NIFTY",
                trading_symbol: "NIFTY",
                instrument_type: ty,
                listing_class: "",
                isin: "",
                expiry: "2026-08-04",
                strike_rupees: "19450",
                option_side: "CE",
            };
            assert_eq!(
                decode_master_row(Vendor::Dhan, r).expect("ok").skip(),
                Some(Skip::LiveContract),
                "{ty} must be skipped"
            );
        }
    }

    #[test]
    fn dhan_reads_the_option_side_from_its_own_column() {
        // Dhan types EVERY option as OPTSTK/OPTIDX and carries call-versus-put
        // in a separate field, so both values must be read and anything else
        // must be loud -- an option whose side we cannot read is not an option
        // we can store.
        let opt = |side| MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "D",
            underlying: "NIFTY",
            trading_symbol: "NIFTY",
            instrument_type: "OPTIDX",
            listing_class: "",
            isin: "",
            expiry: "2026-08-04",
            strike_rupees: "19450",
            option_side: side,
        };
        for side in ["CE", "PE"] {
            assert_eq!(
                decode_master_row(Vendor::Dhan, opt(side))
                    .expect("ok")
                    .skip(),
                Some(Skip::LiveContract),
                "{side} must decode before being declined as live"
            );
        }
        for side in ["", "XX", "ce"] {
            assert_eq!(
                decode_master_row(Vendor::Dhan, opt(side)),
                Err(InstrumentError::Malformed),
                "{side:?} is not a side this vendor emits"
            );
        }
    }

    #[test]
    fn a_malformed_derivative_still_errors_rather_than_hiding_behind_the_skip() {
        // The expiry and strike are parsed BEFORE the skip, so a vendor that
        // starts emitting a bad date is a loud failure rather than being
        // silently swallowed by "it was live anyway".
        assert!(groww(row("NSE", "FNO", "NIFTY", "FUT", "not-a-date", "")).is_err());
        assert!(groww(row("NSE", "FNO", "NIFTY", "CE", "2026-08-04", "abc")).is_err());
        assert!(groww(row("NSE", "FNO", "NIFTY", "CE", "2026-02-31", "1")).is_err());
    }

    #[test]
    fn indices_and_equities_are_still_kept_from_both_vendors() {
        // Both name columns empty is unnameable -- an ERROR, never a skip,
        // because a row we cannot name is how an instrument silently vanishes.
        assert!(groww(row("NSE", "CASH", "", "IDX", "", "")).is_err());

        let nifty = kept(
            groww(MasterRow {
                vendor_id: "1333",
                exchange: "NSE",
                segment: "CASH",
                underlying: "",
                trading_symbol: "NIFTY",
                instrument_type: "IDX",
                listing_class: "",
                isin: "NIFTY",
                expiry: "",
                strike_rupees: "",
                option_side: "",
            })
            .expect("ok"),
        )
        .expect("kept");
        assert!(nifty.is_sweepable());

        let dhan_eq = kept(
            decode_master_row(
                Vendor::Dhan,
                MasterRow {
                    vendor_id: "1333",
                    exchange: "NSE",
                    segment: "E",
                    underlying: "RELIANCE",
                    trading_symbol: "RELIANCE INDUSTRIES LTD",
                    instrument_type: "EQUITY",
                    listing_class: "EQ",
                    isin: REAL_ISIN,
                    expiry: "",
                    strike_rupees: "",
                    option_side: "",
                },
            )
            .expect("ok"),
        )
        .expect("kept");
        assert_eq!(dhan_eq.kind, Kind::Equity);
    }

    // ---------------------------------------------------------------------
    // The equity-listing gate.
    // ---------------------------------------------------------------------

    /// A Dhan `SEGMENT=E`, `INSTRUMENT=EQUITY` row, which is what every
    /// listing on the equity segment is — share, bond or fund alike.
    ///
    /// `series` is Dhan's own `SERIES` column, which is the NSE board series
    /// and the only column this gate reads. D-0025.
    fn dhan_cash<'a>(ticker: &'a str, series: &'a str, isin: &'a str) -> MasterRow<'a> {
        MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "E",
            underlying: ticker,
            trading_symbol: "CHOLAMANDALAM IN & FIN CO",
            instrument_type: "EQUITY",
            listing_class: series,
            isin,
            expiry: "",
            strike_rupees: "",
            option_side: "",
        }
    }

    /// A Groww `CASH`/`EQ` row on a given NSE series.
    fn groww_cash<'a>(ticker: &'a str, series: &'a str, isin: &'a str) -> MasterRow<'a> {
        MasterRow {
            vendor_id: "1333",
            underlying: "",
            trading_symbol: ticker,
            listing_class: series,
            isin,
            ..row("NSE", "CASH", ticker, "EQ", "", "")
        }
    }

    #[test]
    fn the_cholafin_bond_is_declined_and_the_cholafin_share_is_kept() {
        // THE COLLISION THIS GATE EXISTS FOR. Both rows are verbatim from the
        // real Dhan master; both are NSE/E/EQUITY with ticker CHOLAFIN. The
        // BOND is at line 167146 and the SHARE at 171414, so the bond comes
        // FIRST and an insert-if-absent merge resolved CHOLAFIN to a 7.5% NCD
        // and took its tick size of 5.0 instead of the share's 10.0 --
        // silently, and dependent on nothing but file order. The bond's real
        // SERIES is `D1`; the share's is `EQ`.
        let bond = dhan_cash("CHOLAFIN", "D1", "INE121A08PJ0");
        assert_eq!(
            decode_master_row(Vendor::Dhan, bond).expect("ok").skip(),
            Some(Skip::NotEquityListing),
            "the NCD must never take the CHOLAFIN ticker"
        );

        let share = dhan_cash("CHOLAFIN", "EQ", "INE121A01024");
        let l = listing(decode_master_row(Vendor::Dhan, share).expect("ok")).expect("kept");
        assert_eq!(l.key.underlying.as_str(), "CHOLAFIN");
        assert_eq!(l.key.kind, Kind::Equity);
        assert_eq!(
            l.isin.map(|i| i.to_string()).as_deref(),
            Some("INE121A01024"),
            "the share's own ISIN travels beside the key"
        );
    }

    #[test]
    fn the_other_two_measured_ticker_captures_are_declined_too() {
        // MOTHERSON's NCD (series D1) and ELECTCAST's warrant (series W1) are
        // the other two rows that captured a live ticker, and the share of each
        // name is on series EQ. All three are NIFTY Total Market members. Every
        // series below is verbatim from the real Dhan master.
        for (ticker, series, isin) in [
            ("MOTHERSON", "D1", "INE775A08105"),
            ("ELECTCAST", "W1", "INE086A13016"),
        ] {
            assert_eq!(
                decode_master_row(Vendor::Dhan, dhan_cash(ticker, series, isin))
                    .expect("ok")
                    .skip(),
                Some(Skip::NotEquityListing),
                "{ticker} series {series} must be declined"
            );
        }
        for (ticker, isin) in [("MOTHERSON", "INE775A01035"), ("ELECTCAST", "INE086A01029")] {
            let l = listing(
                decode_master_row(Vendor::Dhan, dhan_cash(ticker, "EQ", isin)).expect("ok"),
            )
            .expect("kept");
            assert_eq!(l.isin.map(|i| i.to_string()).as_deref(), Some(isin));
        }
    }

    #[test]
    fn dhans_class_column_is_trimmed_before_it_is_read() {
        // The column is whitespace padded. Reading it untrimmed would decline
        // every genuine share in the file -- a total loss reported as a
        // routine skip, which is the failure this repository has already had
        // once.
        for padded in ["   EQ   ", " EQ", "EQ ", "EQ", "  BE  ", "BE"] {
            let l = listing(
                decode_master_row(Vendor::Dhan, dhan_cash("RELIANCE", padded, REAL_ISIN))
                    .expect("ok"),
            )
            .expect("kept");
            assert_eq!(l.key.kind, Kind::Equity, "{padded:?} must be kept");
        }
    }

    #[test]
    fn a_mutual_fund_plan_is_declined_from_both_vendors_on_one_series_alphabet() {
        // Dhan files 54 open-ended fund plans as INSTRUMENT_TYPE=ETF while its
        // OWN series column says MF; Groww carries 29 of the same ISINs under
        // series=MF and declines them. Reading the paper class kept all 54 as
        // equities -- the exact category Skip::NotEquityListing exists to
        // remove. Reading the series declines them from BOTH vendors, which is
        // the whole point of D-0025.
        for (vendor, row) in [
            (Vendor::Dhan, dhan_cash("FISTIPD3GP", "MF", "INF090I01VS3")),
            (
                Vendor::Groww,
                groww_cash("FISTIPD3GP", "MF", "INF090I01VS3"),
            ),
        ] {
            // The vendor name is bound rather than called inside the failure
            // message: a call there is a region that only runs when the
            // assertion FAILS, so it can never be covered by a passing test.
            let who = vendor.as_str();
            assert_eq!(
                decode_master_row(vendor, row).expect("ok").skip(),
                Some(Skip::NotEquityListing),
                "{who} must decline the fund plan"
            );
        }
        // And a genuine exchange-traded fund on the EQ series is still kept --
        // HDFCLIQUID carries an INF issuer prefix too, so "INF means a fund"
        // would have been the wrong rule.
        let etf = listing(
            decode_master_row(Vendor::Dhan, dhan_cash("HDFCLIQUID", "EQ", "INF179KC1JG3"))
                .expect("ok"),
        )
        .expect("kept");
        assert_eq!(etf.key.kind, Kind::Equity);
    }

    #[test]
    fn the_surveillance_and_partly_paid_equity_series_are_kept_not_called_debt() {
        // BZ (25 Groww / 38 Dhan rows), IT (2/2), SZ (1/2) and E1 (2/3) are
        // trade-for-trade, surveillance and partly-paid EQUITY. Declining them
        // under "not an equity listing" was false about 30 real shares.
        // RAJESHEXPO/INE343B01030 and HMT/INE262A01018 are verbatim from the
        // masters; the NSDL security-type digits of both are `01`, ordinary
        // equity, against `08` for the CHOLAFIN NCD.
        for series in ["BZ", "IT", "SZ", "E1"] {
            for (vendor, r) in [
                (
                    Vendor::Groww,
                    groww_cash("RAJESHEXPO", series, "INE343B01030"),
                ),
                (
                    Vendor::Dhan,
                    dhan_cash("RAJESHEXPO", series, "INE343B01030"),
                ),
            ] {
                let who = vendor.as_str();
                let l = listing(decode_master_row(vendor, r).expect("ok")).expect("kept");
                assert_eq!(
                    l.key.kind,
                    Kind::Equity,
                    "series {series} is equity at {who}"
                );
            }
        }
    }

    #[test]
    fn an_unrecognised_series_is_its_own_loud_reason_never_a_bond() {
        // THE FAILURE THIS VARIANT EXISTS FOR. Both arms of the gate used to
        // end in `_ => NotEquity`, so renaming the equity series EQ to EQX on
        // the real Dhan master silently dropped 2,438 shares -- every F&O
        // underlying among them -- while the report printed `ok` and exit 0,
        // and the only trace was one bond counter rising.
        for code in ["EQX", "XX", "eq", "es", "ETF", "ES", "DEB", "", "  "] {
            assert_eq!(
                decode_master_row(Vendor::Dhan, dhan_cash("RELIANCE", code, REAL_ISIN))
                    .expect("ok")
                    .skip(),
                Some(Skip::UnrecognisedListingClass),
                "{code:?} is not a series this engine has measured"
            );
        }
        // It is a DECLINE and not an error, because NSE mints debt series at
        // will -- but never the same decline as a bond.
        assert_ne!(Skip::UnrecognisedListingClass, Skip::NotEquityListing);
        assert!(!Skip::UnrecognisedListingClass.is_routine());
        assert!(Skip::NotEquityListing.is_routine());
        assert!(Skip::SmeBoard.is_routine());
    }

    #[test]
    fn every_measured_debt_and_fund_class_is_declined_by_name() {
        // The 7,137 rows that are on the equity segment and are not equity.
        // Both vendors, one NSE series alphabet.
        for series in [
            "N0", "N1", "SG", "GS", "MF", "IV", "Y1", "Z9", "AK", "D1", "W1", "TB", "GB", "RR",
            "P1", "SF", "ZZ",
        ] {
            for vendor in Vendor::ALL {
                let r = match vendor {
                    Vendor::Groww => groww_cash("SOMEBOND", series, REAL_ISIN),
                    _ => dhan_cash("SOMEBOND", series, REAL_ISIN),
                };
                let who = vendor.as_str();
                assert_eq!(
                    decode_master_row(vendor, r).expect("ok").skip(),
                    Some(Skip::NotEquityListing),
                    "series {series:?} must be declined at {who}"
                );
            }
        }
    }

    #[test]
    fn the_measured_series_tables_are_sorted_disjoint_and_complete() {
        // `board_of` binary-searches all three, and binary_search on an
        // unsorted array returns garbage in silence.
        for (name, list) in [
            ("EQUITY_BOARD_SERIES", EQUITY_BOARD_SERIES.as_slice()),
            ("SME_BOARD_SERIES", SME_BOARD_SERIES.as_slice()),
            ("NON_EQUITY_SERIES", NON_EQUITY_SERIES.as_slice()),
        ] {
            for w in list.windows(2) {
                assert!(w[0] < w[1], "{name} is unsorted or not unique at {w:?}");
            }
            for code in list {
                assert!(!code.is_empty(), "{name} holds an empty code");
            }
        }
        // A code in two tables would make the verdict depend on the order the
        // tables happen to be searched in.
        for a in EQUITY_BOARD_SERIES {
            assert!(!SME_BOARD_SERIES.contains(&a));
            assert!(!NON_EQUITY_SERIES.contains(&a));
        }
        for a in SME_BOARD_SERIES {
            assert!(!NON_EQUITY_SERIES.contains(&a));
        }
        // 128 distinct codes were measured across both masters.
        assert_eq!(
            EQUITY_BOARD_SERIES.len() + SME_BOARD_SERIES.len() + NON_EQUITY_SERIES.len(),
            128
        );
    }

    #[test]
    fn the_equity_board_is_kept_and_the_sme_board_is_declined_separately() {
        // EQ and BE are the equity board; SM and ST are the SME board. The SME
        // rows get their OWN reason so the decision stays visible: an SME
        // listing IS a share, it is simply not in any universe the engine
        // ranks over.
        for series in ["EQ", "BE"] {
            let r = MasterRow {
                vendor_id: "1333",
                listing_class: series,
                ..row("NSE", "CASH", "RELIANCE", "EQ", "", "")
            };
            assert_eq!(
                listing(groww(r).expect("ok")).expect("kept").key.kind,
                Kind::Equity,
                "series {series} is the equity board"
            );
        }
        // Symmetrically at BOTH vendors, since D-0025 -- 558 rows at Groww and
        // 559 at Dhan, the same paper by ISIN. Before it, Dhan kept every one
        // of them because its paper class calls an SME share `ES`.
        for series in ["SM", "ST"] {
            for (vendor, r) in [
                (Vendor::Groww, groww_cash("SOMESME", series, REAL_ISIN)),
                (Vendor::Dhan, dhan_cash("SOMESME", series, REAL_ISIN)),
            ] {
                let who = vendor.as_str();
                assert_eq!(
                    decode_master_row(vendor, r).expect("ok").skip(),
                    Some(Skip::SmeBoard),
                    "series {series} is the SME board at {who}, and says so"
                );
            }
        }
    }

    #[test]
    fn a_declined_row_carries_its_isin_as_evidence_for_the_cross_check() {
        // A merge that only compares what both vendors KEPT cannot see an
        // ELIGIBILITY disagreement. The ISIN is what makes one visible, so it
        // travels with the decline -- leniently, because the declined row is
        // declined either way.
        let d = decode_master_row(Vendor::Dhan, dhan_cash("CHOLAFIN", "D1", "INE121A08PJ0"))
            .expect("ok");
        assert_eq!(
            d,
            Decoded::Skipped(Declined {
                reason: Skip::NotEquityListing,
                isin: Isin::new("INE121A08PJ0").ok(),
            })
        );
        // An unparseable ISIN on a declined row yields no evidence and no
        // error: the row is declined and counted regardless.
        let sdl = decode_master_row(Vendor::Dhan, dhan_cash("61GJ28", "SG", "IN1520250085"))
            .expect("a declined row must never fail on its own ISIN");
        assert_eq!(
            sdl,
            Decoded::Skipped(Declined {
                reason: Skip::NotEquityListing,
                isin: None,
            })
        );
    }

    #[test]
    fn an_index_row_survives_the_gate_from_both_vendors() {
        // THE ROW THAT MUST NOT BE GATED. An index has no series at all --
        // Groww leaves the column empty, Dhan writes `NA` -- so a gate applied
        // before the instrument type is known deletes NIFTY and BANKNIFTY,
        // which is every instrument the engine exists to sweep.
        for sym in ["NIFTY", "BANKNIFTY"] {
            let g = MasterRow {
                vendor_id: "1333",
                underlying: "",
                trading_symbol: sym,
                listing_class: "",
                isin: sym,
                ..row("NSE", "CASH", sym, "IDX", "", "")
            };
            let gl = listing(groww(g).expect("ok")).expect("kept");
            assert!(gl.key.is_sweepable(), "Groww {sym} must survive the gate");
            assert_eq!(gl.isin, None, "an index has no ISIN, and none is invented");

            let d = MasterRow {
                vendor_id: "1333",
                exchange: "NSE",
                segment: "I",
                underlying: sym,
                trading_symbol: sym,
                instrument_type: "INDEX",
                listing_class: "NA",
                isin: "NA",
                expiry: "0001-01-01",
                strike_rupees: "",
                option_side: "XX",
            };
            let dl = listing(decode_master_row(Vendor::Dhan, d).expect("ok")).expect("kept");
            assert!(dl.key.is_sweepable(), "Dhan {sym} must survive the gate");
            assert_eq!(
                dl.isin, None,
                "`NA` is not an ISIN and is not parsed as one"
            );
        }
    }

    #[test]
    fn the_gate_never_sees_a_derivative_row() {
        // A live derivative carries no series either. It is declined for being
        // live, and the reason must say so rather than saying "not equity".
        let r = MasterRow {
            vendor_id: "1333",
            listing_class: "",
            isin: "",
            ..row("NSE", "FNO", "NIFTY", "FUT", "2026-08-04", "")
        };
        assert_eq!(
            groww(r).expect("ok").skip(),
            Some(Skip::LiveContract),
            "the reason must be the true one"
        );
    }

    #[test]
    fn a_kept_equity_must_carry_a_parseable_isin() {
        // Every one of the main-board rows in either master has one, so a
        // missing or malformed value means the row is not what it claims.
        for bad in ["", "NA", "INE002A01019", "INE002A0101"] {
            let r = MasterRow {
                vendor_id: "1333",
                isin: bad,
                ..row("NSE", "CASH", "RELIANCE", "EQ", "", "")
            };
            assert_eq!(
                groww(r),
                Err(InstrumentError::Malformed),
                "{bad:?} must be loud, never a quiet None"
            );
        }
    }

    #[test]
    fn the_sdl_with_the_bad_check_digit_never_reaches_the_isin_parse() {
        // IN1520250085 is the one row in either master whose check digit does
        // not verify. It is a state development loan on series `SG`, so the
        // gate declines it FIRST -- which is why the ISIN is parsed after the
        // gate and not before. Order is the whole content of this test.
        assert_eq!(
            decode_master_row(Vendor::Dhan, dhan_cash("61GJ28", "SG", "IN1520250085"))
                .expect("ok")
                .skip(),
            Some(Skip::NotEquityListing)
        );
        // And it really would have been refused, had it got that far.
        assert!(Isin::new("IN1520250085").is_err());
    }

    // ---------------------------------------------------------------------
    // The series suffix.
    // ---------------------------------------------------------------------

    #[test]
    fn a_series_suffix_is_offered_as_a_candidate_never_applied() {
        // Groww leaks internal_trading_symbol into trading_symbol on 209 of
        // the 4,080 shared ISINs. The decoder offers the stripped identity; it
        // does not adopt it, because only a second vendor's ISIN can confirm
        // that BLUECHIP-BE and BLUECHIP are one instrument.
        let r = MasterRow {
            vendor_id: "1333",
            underlying: "",
            trading_symbol: "BLUECHIP-BE",
            listing_class: "BE",
            isin: "INE657B01025",
            ..row("NSE", "CASH", "BLUECHIP-BE", "EQ", "", "")
        };
        let l = listing(groww(r).expect("ok")).expect("kept");
        assert_eq!(
            l.key.underlying.as_str(),
            "BLUECHIP-BE",
            "the key is what the vendor said, unchanged"
        );
        assert_eq!(
            l.unsuffixed.map(|k| k.underlying.as_str().to_owned()),
            Some("BLUECHIP".to_owned()),
            "and the stripped form is offered beside it"
        );
    }

    #[test]
    fn a_dash_that_is_not_the_rows_own_series_is_never_stripped() {
        // BAJAJ-AUTO is a real ticker ending in a dash. Stripping blind would
        // manufacture the collision Symbol::new refuses to manufacture.
        for (ticker, series) in [
            ("BAJAJ-AUTO", "EQ"),
            ("NAM-INDIA", "EQ"),
            ("RELIANCE", "EQ"),
            ("LOWVOL-EQ", "BE"),
        ] {
            let r = MasterRow {
                vendor_id: "1333",
                underlying: "",
                trading_symbol: ticker,
                listing_class: series,
                ..row("NSE", "CASH", ticker, "EQ", "", "")
            };
            assert_eq!(
                listing(groww(r).expect("ok")).expect("kept").unsuffixed,
                None,
                "{ticker} under series {series} must not be stripped"
            );
        }
    }

    #[test]
    fn stripping_to_nothing_is_an_error_rather_than_a_silent_no_op() {
        let r = MasterRow {
            vendor_id: "1333",
            underlying: "",
            trading_symbol: "-EQ",
            listing_class: "EQ",
            ..row("NSE", "CASH", "-EQ", "EQ", "", "")
        };
        assert_eq!(groww(r), Err(InstrumentError::Malformed));
    }

    #[test]
    fn nothing_but_a_cash_listing_is_offered_a_stripped_key() {
        // An index has no series, so there is nothing to strip and no
        // candidate to offer -- even when its ticker happens to end in a dash
        // and a word.
        let r = MasterRow {
            vendor_id: "1333",
            underlying: "",
            trading_symbol: "NIFTY-IDX",
            listing_class: "IDX",
            isin: "NIFTY-IDX",
            ..row("NSE", "CASH", "NIFTY-IDX", "IDX", "", "")
        };
        assert_eq!(
            listing(groww(r).expect("ok")).expect("kept").unsuffixed,
            None
        );
    }

    // ---------------------------------------------------------------------
    // The vendor tables.
    // ---------------------------------------------------------------------

    #[test]
    fn every_skip_reason_is_distinct_and_says_what_it_declined() {
        let all = [
            Skip::ForeignExchange,
            Skip::TestInstrument,
            Skip::ForeignSegment,
            Skip::LiveContract,
            Skip::NotEquityListing,
            Skip::SmeBoard,
            Skip::UnrecognisedListingClass,
        ];
        let rendered: Vec<&str> = all.iter().map(|s| s.reason()).collect();
        for (i, a) in rendered.iter().enumerate() {
            assert!(!a.is_empty(), "reason {i} is empty");
            for (j, b) in rendered.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "reasons {i} and {j} are the same string");
                }
            }
        }
    }

    #[test]
    fn each_vendor_names_its_own_columns_and_no_two_agree() {
        let g = Vendor::Groww.master_columns();
        let d = Vendor::Dhan.master_columns();
        // Both read the NSE board series, under each vendor's own spelling.
        // Dhan's `INSTRUMENT_TYPE` is a vendor-minted paper class and is
        // deliberately not read at all -- D-0025.
        assert_eq!(g.listing_class, "series");
        assert_eq!(d.listing_class, "SERIES");
        assert_eq!(d.instrument_type, "INSTRUMENT", "not INSTRUMENT_TYPE");
        assert!(
            [
                g.exchange,
                g.segment,
                g.underlying,
                g.trading_symbol,
                g.instrument_type,
                g.listing_class,
                g.isin,
                g.expiry,
                g.strike,
                d.exchange,
                d.segment,
                d.underlying,
                d.trading_symbol,
                d.instrument_type,
                d.listing_class,
                d.isin,
                d.expiry,
                d.strike,
            ]
            .iter()
            .all(|n| *n != "INSTRUMENT_TYPE"),
            "the measurably-wrong column must not be read by any field"
        );
        assert_eq!(g.isin, "isin");
        assert_eq!(d.isin, "ISIN");
        assert_eq!(g.option_side, None, "Groww types CE and PE directly");
        assert_eq!(d.option_side, Some("OPTION_TYPE"));
        assert_ne!(g, d);
    }

    #[test]
    fn a_vendor_set_is_a_set_and_every_vendor_has_its_own_bit() {
        let mut s = VendorSet::EMPTY;
        assert!(s.is_empty());
        for v in Vendor::ALL {
            assert!(!s.contains(v));
            s = s.with(v);
            assert!(s.contains(v));
        }
        assert!(!s.is_empty());
        assert_eq!(s, s.with(Vendor::Groww), "adding twice is adding once");
        assert_eq!(VendorSet::default(), VendorSet::EMPTY);
        // One bit each, or two vendors would be indistinguishable.
        let only_dhan = VendorSet::EMPTY.with(Vendor::Dhan);
        assert!(only_dhan.contains(Vendor::Dhan));
        assert!(!only_dhan.contains(Vendor::Groww));
    }

    // =======================================================================
    // The field-width gate — D-0033
    // =======================================================================

    /// The row from the defect: a legitimate `underlying`, and one enormous
    /// field that is scanned but never becomes the identity.
    fn wide_row<'a>(field: &str, wide: &'a str) -> MasterRow<'a> {
        let mut r = MasterRow {
            vendor_id: "1333",
            exchange: "NSE",
            segment: "CASH",
            underlying: "RELIANCE",
            trading_symbol: "RELIANCE",
            instrument_type: "EQ",
            listing_class: "EQ",
            isin: REAL_ISIN,
            expiry: "",
            strike_rupees: "",
            option_side: "",
        };
        match field {
            "exchange" => r.exchange = wide,
            "segment" => r.segment = wide,
            "underlying" => r.underlying = wide,
            "trading_symbol" => r.trading_symbol = wide,
            "instrument_type" => r.instrument_type = wide,
            "listing_class" => r.listing_class = wide,
            "isin" => r.isin = wide,
            "expiry" => r.expiry = wide,
            "strike_rupees" => r.strike_rupees = wide,
            _ => r.option_side = wide,
        }
        r
    }

    /// Every field name `over_wide` can report, in struct order.
    const FIELD_NAMES: [&str; 10] = [
        "exchange",
        "segment",
        "underlying",
        "trading_symbol",
        "instrument_type",
        "listing_class",
        "isin",
        "expiry",
        "strike_rupees",
        "option_side",
    ];

    #[test]
    fn an_over_wide_field_is_refused_whichever_field_it_is() {
        // THE DEFECT THIS PINS. `trading_symbol` only becomes the identity when
        // `underlying` is empty, so a 4 MiB `trading_symbol` beside a populated
        // `underlying` was scanned twice by TEST_MARKERS and then ACCEPTED AND
        // STORED -- the width guard in `Symbol::new` never saw it. Every field
        // is checked here, not just the two the scan reads, because the next
        // reader added below the gate must not have to remember to add a bound.
        // Bound rather than computed inside a failure message: an expression
        // there is a region only a FAILING assertion reaches, so no passing
        // test can cover it.
        let over = MAX_FIELD_BYTES + 1;
        let wide = "X".repeat(over);
        for name in FIELD_NAMES {
            let row = wide_row(name, &wide);
            assert_eq!(
                row.over_wide(),
                Some((name, over)),
                "{name} was not reported"
            );
            assert_eq!(
                decode_master_row(Vendor::Groww, row),
                Err(InstrumentError::FieldTooWide {
                    field: name,
                    len: over,
                }),
                "{name} at {over} bytes was not refused"
            );
        }
    }

    #[test]
    fn the_bound_is_the_first_byte_that_is_too_many_and_not_one_before() {
        // An off-by-one here refuses rows that decode correctly today, so the
        // exact boundary is pinned rather than the general shape.
        let at = "X".repeat(MAX_FIELD_BYTES);
        let over = "X".repeat(MAX_FIELD_BYTES + 1);
        assert_eq!(wide_row("expiry", &at).over_wide(), None);
        assert_eq!(
            wide_row("expiry", &over).over_wide(),
            Some(("expiry", MAX_FIELD_BYTES + 1))
        );
        // 64 bytes in `expiry` is still refused -- but for its CONTENT, by the
        // expiry parser, which is the bound that belongs there. The width gate
        // is not a substitute for any of the parsers below it.
        let mut r = wide_row("expiry", &at);
        r.instrument_type = "FUT";
        assert_eq!(
            decode_master_row(Vendor::Groww, r),
            Err(InstrumentError::Malformed)
        );
    }

    #[test]
    fn a_row_of_ordinary_width_passes_the_gate_untouched() {
        // The widest value in either real master, in the column it was measured
        // in: 28 bytes. If this ever fails the bound has been set below what
        // the vendors actually emit.
        const WIDEST_MEASURED: &str = "NIFTYNXT50-Aug2026-101500-CE";
        assert_eq!(WIDEST_MEASURED.len(), 28);
        assert!(WIDEST_MEASURED.len() <= MAX_FIELD_BYTES);
        let row = wide_row("trading_symbol", WIDEST_MEASURED);
        assert_eq!(row.over_wide(), None);
        assert_eq!(
            kept(decode_master_row(Vendor::Groww, row).expect("decodes"))
                .map(|k| k.underlying.as_str().to_owned()),
            Some("RELIANCE".to_owned())
        );
    }

    #[test]
    fn the_test_marker_scan_still_declines_a_real_test_listing() {
        // The gate runs BEFORE the scan, so this proves the gate did not
        // shadow it. `031NSETEST` is verbatim from the primary broker's master.
        let row = wide_row("underlying", "031NSETEST");
        assert_eq!(
            decode_master_row(Vendor::Groww, row).map(Decoded::skip),
            Ok(Some(Skip::TestInstrument))
        );
    }
}
