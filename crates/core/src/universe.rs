//! The instrument universes, as transcribed data.
//!
//! # Why these are Rust literals
//!
//! `CLAUDE.md` §2 permits no `.csv` in this repository, so a constituent list
//! cannot be tracked as a data file. The *data* is fine; the *format* is not.
//! These are transcriptions, and each names where it came from.
//!
//! # Why membership is not a field of [`crate::instrument::InstrumentKey`]
//!
//! `InstrumentKey` derives `Hash` and `Eq` over every field and is used as a
//! `HashMap` key — that is the deduplication mechanism. If membership were a
//! field, the *same contract* carrying different memberships would become two
//! different keys and dedup would break silently. Membership is therefore a
//! property looked up **from** the key, never part of it.
//!
//! # Cost
//!
//! Lookup is a binary search over a sorted array — O(log n) on 750 entries, so
//! at most 10 comparisons. Not O(1), and saying otherwise would be false; a
//! perfect hash would make it O(1) and is not worth the machinery at this size.
//! Recorded as a departure from `CLAUDE.md` golden rule 4 in
//! `docs/06-limits.md` §11, because a departure nobody wrote down is a claim
//! nobody can check. Neither list is on the sweep's per-bar path — membership
//! is asked once per instrument at merge time, never once per bar — which is
//! why O(log n) is acceptable here and would not be there.
//!
//! # What consults this
//!
//! `api::server::universe` stamps every merged cash listing with its
//! membership, and the page and the report both show it. That is not
//! decoration: [`crate::vendor::Skip::SmeBoard`] declines 1,117 real shares on
//! the ground that neither list contains an SME listing, and a claim that
//! load-bearing is checked here rather than asserted in a comment — see
//! [`of_equity`] and `no_measured_sme_ticker_belongs_to_either_universe`.

use crate::instrument::{InstrumentKey, Kind};

/// The 750 NIFTY Total Market constituents.
///
/// niftyindices.com: "750 stocks ... all stocks that are part of Nifty 500
/// and Nifty Microcap 250". Transcribed from the local lake's NSE/CASH
/// directories, every one of which matched the primary broker's equity list
/// 750/750 with zero misses. The source and its evidence lane are recorded in
/// `docs/00-charter.md` §4a, as golden rule 1 requires of any claim about an
/// instrument.
///
/// Re-measured against both real masters on 2026-08-01 under the D-0025 gate:
/// **750 of 750 resolve as a kept equity in BOTH vendors**, with agreeing
/// ISINs and zero misses.
///
/// UNVERIFIED against an NSE constituent circular: this is a SNAPSHOT, and
/// index membership is rebalanced. See `docs/06-limits.md` §11.
pub const NIFTY_TOTAL_MARKET: [&str; 750] = [
    "360ONE",
    "3MINDIA",
    "AADHARHFC",
    "AARTIDRUGS",
    "AARTIIND",
    "AARTIPHARM",
    "AAVAS",
    "ABB",
    "ABBOTINDIA",
    "ABCAPITAL",
    "ABDL",
    "ABFRL",
    "ABLBL",
    "ABREL",
    "ABSLAMC",
    "ACC",
    "ACE",
    "ACI",
    "ACMESOLAR",
    "ACUTAAS",
    "ADANIENSOL",
    "ADANIENT",
    "ADANIGREEN",
    "ADANIPORTS",
    "ADANIPOWER",
    "ADVENZYMES",
    "AEGISLOG",
    "AEGISVOPAK",
    "AEQUS",
    "AETHER",
    "AFCONS",
    "AFFLE",
    "AGARWALEYE",
    "AGL",
    "AHLUCONT",
    "AIAENG",
    "AIIL",
    "AJANTPHARM",
    "AKUMS",
    "ALIVUS",
    "ALKEM",
    "ALKYLAMINE",
    "ALOKINDS",
    "AMBER",
    "AMBUJACEM",
    "ANANDRATHI",
    "ANANTRAJ",
    "ANGELONE",
    "ANTHEM",
    "ANUP",
    "ANURAS",
    "APARINDS",
    "APLAPOLLO",
    "APLLTD",
    "APOLLO",
    "APOLLOHOSP",
    "APOLLOTYRE",
    "APTUS",
    "ARE&M",
    "ARVIND",
    "ARVINDFASN",
    "ASAHIINDIA",
    "ASHAPURMIN",
    "ASHOKA",
    "ASHOKLEY",
    "ASIANPAINT",
    "ASKAUTOLTD",
    "ASTERDM",
    "ASTRAL",
    "ASTRAMICRO",
    "ATGL",
    "ATHERENERG",
    "ATLANTAELE",
    "ATUL",
    "AUBANK",
    "AURIONPRO",
    "AUROPHARMA",
    "AVALON",
    "AVANTIFEED",
    "AVL",
    "AWFIS",
    "AWL",
    "AXISBANK",
    "AXISCADES",
    "AZAD",
    "BAJAJ-AUTO",
    "BAJAJELEC",
    "BAJAJFINSV",
    "BAJAJHFL",
    "BAJAJHLDNG",
    "BAJFINANCE",
    "BALAMINES",
    "BALKRISIND",
    "BALRAMCHIN",
    "BALUFORGE",
    "BANCOINDIA",
    "BANDHANBNK",
    "BANKBARODA",
    "BANKINDIA",
    "BATAINDIA",
    "BAYERCROP",
    "BBOX",
    "BBTC",
    "BDL",
    "BECTORFOOD",
    "BEL",
    "BELRISE",
    "BEML",
    "BERGEPAINT",
    "BHARATFORG",
    "BHARTIARTL",
    "BHARTIHEXA",
    "BHEL",
    "BIKAJI",
    "BIOCON",
    "BIRLACORPN",
    "BLACKBUCK",
    "BLS",
    "BLUEDART",
    "BLUEJET",
    "BLUESTARCO",
    "BLUESTONE",
    "BORORENEW",
    "BOSCHLTD",
    "BPCL",
    "BRIGADE",
    "BRITANNIA",
    "BSE",
    "BSOFT",
    "CAMPUS",
    "CAMS",
    "CANBK",
    "CANFINHOME",
    "CANHLIFE",
    "CAPILLARY",
    "CAPLIPOINT",
    "CARBORUNIV",
    "CARTRADE",
    "CASTROLIND",
    "CCAVENUE",
    "CCL",
    "CDSL",
    "CEATLTD",
    "CELLO",
    "CEMPRO",
    "CENTRALBK",
    "CENTURYPLY",
    "CERA",
    "CESC",
    "CGCL",
    "CGPOWER",
    "CHALET",
    "CHAMBLFERT",
    "CHENNPETRO",
    "CHOICEIN",
    "CHOLAFIN",
    "CHOLAHLDNG",
    "CIEINDIA",
    "CIPLA",
    "CLEAN",
    "CMSINFO",
    "COALINDIA",
    "COCHINSHIP",
    "COFORGE",
    "COHANCE",
    "COLPAL",
    "CONCOR",
    "CONCORDBIO",
    "COROMANDEL",
    "CORONA",
    "CPPLUS",
    "CRAFTSMAN",
    "CRAMC",
    "CREDITACC",
    "CRISIL",
    "CRIZAC",
    "CROMPTON",
    "CSBBANK",
    "CUB",
    "CUMMINSIND",
    "CUPID",
    "CYIENT",
    "DABUR",
    "DALBHARAT",
    "DATAMATICS",
    "DATAPATTNS",
    "DBL",
    "DBREALTY",
    "DCBBANK",
    "DCMSHRIRAM",
    "DEEPAKFERT",
    "DEEPAKNTR",
    "DELHIVERY",
    "DEVYANI",
    "DIACABS",
    "DIVISLAB",
    "DIXON",
    "DLF",
    "DMART",
    "DOMS",
    "DRREDDY",
    "DYNAMATECH",
    "ECLERX",
    "EDELWEISS",
    "EICHERMOT",
    "EIDPARRY",
    "EIEL",
    "EIHOTEL",
    "ELECON",
    "ELECTCAST",
    "ELGIEQUIP",
    "ELLEN",
    "EMAMILTD",
    "EMBDL",
    "EMCURE",
    "EMIL",
    "EMMVEE",
    "ENDURANCE",
    "ENGINERSIN",
    "ENRIN",
    "ENTERO",
    "EPL",
    "EQUITASBNK",
    "ERIS",
    "ESCORTS",
    "ETERNAL",
    "ETHOSLTD",
    "EUREKAFORB",
    "EXIDEIND",
    "FACT",
    "FEDERALBNK",
    "FEDFINA",
    "FIEMIND",
    "FINCABLES",
    "FINPIPE",
    "FIRSTCRY",
    "FIVESTAR",
    "FLUOROCHEM",
    "FORCEMOT",
    "FORTIS",
    "FSL",
    "GABRIEL",
    "GAEL",
    "GAIL",
    "GALLANTT",
    "GESHIP",
    "GHCL",
    "GICRE",
    "GILLETTE",
    "GLAND",
    "GLAXO",
    "GLENMARK",
    "GMDCLTD",
    "GMMPFAUDLR",
    "GMRAIRPORT",
    "GMRP&UI",
    "GNFC",
    "GODFRYPHLP",
    "GODIGIT",
    "GODREJAGRO",
    "GODREJCP",
    "GODREJIND",
    "GODREJPROP",
    "GOKEX",
    "GOKULAGRO",
    "GPIL",
    "GPPL",
    "GRANULES",
    "GRAPHITE",
    "GRASIM",
    "GRAVITA",
    "GREAVESCOT",
    "GROWW",
    "GRSE",
    "GRWRHITECH",
    "GSFC",
    "GVT&D",
    "HAL",
    "HAPPSTMNDS",
    "HAVELLS",
    "HBLENGINE",
    "HCC",
    "HCG",
    "HCLTECH",
    "HDBFS",
    "HDFCAMC",
    "HDFCBANK",
    "HDFCLIFE",
    "HEG",
    "HEMIPROP",
    "HERITGFOOD",
    "HEROMOTOCO",
    "HEXT",
    "HFCL",
    "HGINFRA",
    "HINDALCO",
    "HINDCOPPER",
    "HINDPETRO",
    "HINDUNILVR",
    "HINDZINC",
    "HOMEFIRST",
    "HONASA",
    "HONAUT",
    "HSCL",
    "HUDCO",
    "HYUNDAI",
    "ICICIAMC",
    "ICICIBANK",
    "ICICIGI",
    "ICICIPRULI",
    "ICIL",
    "IDBI",
    "IDEA",
    "IDFCFIRSTB",
    "IEX",
    "IFBIND",
    "IFCI",
    "IGIL",
    "IGL",
    "IIFL",
    "IIFLCAPS",
    "IKS",
    "IMFA",
    "INDGN",
    "INDHOTEL",
    "INDIACEM",
    "INDIAGLYCO",
    "INDIAMART",
    "INDIANB",
    "INDIASHLTR",
    "INDIGO",
    "INDIGOPNTS",
    "INDUSINDBK",
    "INDUSTOWER",
    "INFY",
    "INOXGREEN",
    "INOXINDIA",
    "INOXWIND",
    "INTELLECT",
    "IOB",
    "IOC",
    "IONEXCHANG",
    "IPCALAB",
    "IRB",
    "IRCON",
    "IRCTC",
    "IREDA",
    "IRFC",
    "ITC",
    "ITCHOTELS",
    "ITI",
    "IXIGO",
    "J&KBANK",
    "JAIBALAJI",
    "JAINREC",
    "JAMNAAUTO",
    "JAYNECOIND",
    "JBMA",
    "JINDALSAW",
    "JINDALSTEL",
    "JIOFIN",
    "JKCEMENT",
    "JKLAKSHMI",
    "JKPAPER",
    "JKTYRE",
    "JLHL",
    "JMFINANCIL",
    "JPPOWER",
    "JSFB",
    "JSL",
    "JSLL",
    "JSWCEMENT",
    "JSWDULUX",
    "JSWENERGY",
    "JSWINFRA",
    "JSWSTEEL",
    "JUBLFOOD",
    "JUBLINGREA",
    "JUBLPHARMA",
    "JUSTDIAL",
    "JWL",
    "JYOTHYLAB",
    "JYOTICNC",
    "KAJARIACER",
    "KALYANKJIL",
    "KANSAINER",
    "KARURVYSYA",
    "KAYNES",
    "KEC",
    "KEI",
    "KFINTECH",
    "KIMS",
    "KIRLOSBROS",
    "KIRLOSENG",
    "KIRLPNU",
    "KITEX",
    "KNRCON",
    "KOTAKBANK",
    "KPIGREEN",
    "KPIL",
    "KPITTECH",
    "KPRMILL",
    "KRBL",
    "KRN",
    "KSB",
    "KSCL",
    "KTKBANK",
    "LALPATHLAB",
    "LATENTVIEW",
    "LAURUSLABS",
    "LEMONTREE",
    "LENSKART",
    "LGEINDIA",
    "LICHSGFIN",
    "LICI",
    "LINDEINDIA",
    "LLOYDSENGG",
    "LLOYDSENT",
    "LLOYDSME",
    "LODHA",
    "LOTUSDEV",
    "LT",
    "LTF",
    "LTFOODS",
    "LTM",
    "LTTS",
    "LUMAXTECH",
    "LUPIN",
    "LXCHEM",
    "M&M",
    "M&MFIN",
    "MAHABANK",
    "MAHSCOOTER",
    "MAHSEAMLES",
    "MANAPPURAM",
    "MANKIND",
    "MANORAMA",
    "MANYAVAR",
    "MAPMYINDIA",
    "MARICO",
    "MARKSANS",
    "MARUTI",
    "MASTEK",
    "MAXHEALTH",
    "MAZDOCK",
    "MCX",
    "MEDANTA",
    "MEDPLUS",
    "MEESHO",
    "METROPOLIS",
    "MFSL",
    "MGL",
    "MIDHANI",
    "MINDACORP",
    "MMTC",
    "MOIL",
    "MOTHERSON",
    "MOTILALOFS",
    "MPHASIS",
    "MRF",
    "MRPL",
    "MSTCLTD",
    "MSUMI",
    "MTARTECH",
    "MUTHOOTFIN",
    "NAM-INDIA",
    "NATCOPHARM",
    "NATIONALUM",
    "NAUKRI",
    "NAVA",
    "NAVINFLUOR",
    "NAZARA",
    "NBCC",
    "NCC",
    "NEOGEN",
    "NESCO",
    "NESTLEIND",
    "NETWEB",
    "NETWORK18",
    "NEULANDLAB",
    "NEWGEN",
    "NFL",
    "NH",
    "NHPC",
    "NIACL",
    "NIVABUPA",
    "NLCINDIA",
    "NMDC",
    "NSLNISP",
    "NTPC",
    "NTPCGREEN",
    "NUVAMA",
    "NUVOCO",
    "NYKAA",
    "OBEROIRLTY",
    "OFSS",
    "OIL",
    "OLAELEC",
    "OLECTRA",
    "ONESOURCE",
    "ONGC",
    "OPTIEMUS",
    "ORIENTCEM",
    "ORKLAINDIA",
    "OSWALPUMPS",
    "PAGEIND",
    "PARADEEP",
    "PARAS",
    "PARKHOSPS",
    "PATANJALI",
    "PAYTM",
    "PCBL",
    "PCJEWELLER",
    "PERSISTENT",
    "PETRONET",
    "PFC",
    "PFIZER",
    "PFOCUS",
    "PGEL",
    "PGIL",
    "PHOENIXLTD",
    "PICCADIL",
    "PIDILITIND",
    "PIIND",
    "PINELABS",
    "PIRAMALFIN",
    "PNB",
    "PNBHOUSING",
    "PNCINFRA",
    "PNGJL",
    "POLICYBZR",
    "POLYCAB",
    "POLYMED",
    "POONAWALLA",
    "POWERGRID",
    "POWERINDIA",
    "POWERMECH",
    "PPLPHARMA",
    "PRAJIND",
    "PREMIERENE",
    "PRESTIGE",
    "PRICOLLTD",
    "PRIVISCL",
    "PRSMJOHNSN",
    "PRUDENT",
    "PTC",
    "PTCIL",
    "PURVA",
    "PVRINOX",
    "PWL",
    "QPOWER",
    "QUESS",
    "RADICO",
    "RAILTEL",
    "RAIN",
    "RAINBOW",
    "RALLIS",
    "RAMCOCEM",
    "RATEGAIN",
    "RATNAMANI",
    "RAYMONDLSL",
    "RBA",
    "RBLBANK",
    "RCF",
    "RECLTD",
    "REDINGTON",
    "REDTAPE",
    "REFEX",
    "RELAXO",
    "RELIANCE",
    "RELIGARE",
    "RENUKA",
    "RHIM",
    "RITES",
    "RKFORGE",
    "ROUTE",
    "RPOWER",
    "RRKABEL",
    "RTNINDIA",
    "RTNPOWER",
    "RUBICON",
    "RVNL",
    "SAATVIKGL",
    "SAFARI",
    "SAGILITY",
    "SAIL",
    "SAILIFE",
    "SAMHI",
    "SAMMAANCAP",
    "SANDUMA",
    "SANOFICONR",
    "SANSERA",
    "SAPPHIRE",
    "SARDAEN",
    "SAREGAMA",
    "SBFC",
    "SBICARD",
    "SBILIFE",
    "SBIN",
    "SCHAEFFLER",
    "SCHNEIDER",
    "SCI",
    "SENCO",
    "SFL",
    "SHAILY",
    "SHAKTIPUMP",
    "SHARDACROP",
    "SHAREINDIA",
    "SHILPAMED",
    "SHREECEM",
    "SHRIPISTON",
    "SHRIRAMFIN",
    "SHYAMMETL",
    "SIEMENS",
    "SIGNATURE",
    "SJVN",
    "SKFINDIA",
    "SKFINDUS",
    "SKIPPER",
    "SKYGOLD",
    "SMARTWORKS",
    "SMLMAH",
    "SOBHA",
    "SOLARINDS",
    "SONACOMS",
    "SONATSOFTW",
    "SOUTHBANK",
    "SPARC",
    "SPLPETRO",
    "SRF",
    "STAR",
    "STARCEMENT",
    "STARHEALTH",
    "STLTECH",
    "STYL",
    "STYRENIX",
    "SUBROS",
    "SUDARSCHEM",
    "SUDEEPPHRM",
    "SUMICHEM",
    "SUNDARMFIN",
    "SUNPHARMA",
    "SUNTECK",
    "SUNTV",
    "SUPREMEIND",
    "SUPRIYA",
    "SURYAROSNI",
    "SUZLON",
    "SWANCORP",
    "SWIGGY",
    "SWSOLAR",
    "SYNGENE",
    "SYRMA",
    "TANLA",
    "TARC",
    "TARIL",
    "TATACAP",
    "TATACHEM",
    "TATACOMM",
    "TATACONSUM",
    "TATAELXSI",
    "TATAINVEST",
    "TATAPOWER",
    "TATASTEEL",
    "TATATECH",
    "TBOTEK",
    "TCS",
    "TDPOWERSYS",
    "TECHM",
    "TECHNOE",
    "TEGA",
    "TEJASNET",
    "TENNIND",
    "TEXRAIL",
    "THANGAMAYL",
    "THELEELA",
    "THERMAX",
    "THOMASCOOK",
    "THYROCARE",
    "TI",
    "TIINDIA",
    "TIMETECHNO",
    "TIMKEN",
    "TIPSMUSIC",
    "TITAGARH",
    "TITAN",
    "TMB",
    "TMCV",
    "TMPV",
    "TORNTPHARM",
    "TORNTPOWER",
    "TRANSRAILL",
    "TRAVELFOOD",
    "TRENT",
    "TRIDENT",
    "TRITURBINE",
    "TRIVENI",
    "TSFINV",
    "TTML",
    "TVSMOTOR",
    "TVSSCS",
    "UBL",
    "UCOBANK",
    "UJJIVANSFB",
    "ULTRACEMCO",
    "UNIONBANK",
    "UNITDSPR",
    "UNOMINDA",
    "UPL",
    "URBANCO",
    "USHAMART",
    "UTIAMC",
    "UTLSOLAR",
    "V2RETAIL",
    "VAIBHAVGBL",
    "VARROC",
    "VBL",
    "VEDL",
    "VGUARD",
    "VIJAYA",
    "VIKRAMSOLR",
    "VIPIND",
    "VIYASH",
    "VMART",
    "VMM",
    "VOLTAMP",
    "VOLTAS",
    "VTL",
    "WAAREEENER",
    "WAAREERTL",
    "WABAG",
    "WAKEFIT",
    "WEBELSOLAR",
    "WELCORP",
    "WELENT",
    "WELSPUNLIV",
    "WESTLIFE",
    "WEWORK",
    "WHIRLPOOL",
    "WIPRO",
    "WOCKPHARMA",
    "YATHARTH",
    "YESBANK",
    "ZAGGLE",
    "ZEEL",
    "ZENSARTECH",
    "ZENTEC",
    "ZFCVINDIA",
    "ZYDUSLIFE",
    "ZYDUSWELL",
];

/// Underlyings with listed futures or options on NSE.
///
/// DERIVED from both vendor masters and cross-checked: the two brokers agree
/// exactly — 213 each, zero on either side only. Exchange test instruments
/// (`*NSETEST`) are excluded. Recorded in `docs/00-charter.md` §4a.
///
/// Re-measured 2026-08-01 under the D-0025 gate: 208 of the 213 resolve as a
/// kept equity in **both** vendors. The other five are indices, which carry no
/// ISIN and therefore no cross-vendor check at all — `NIFTY`, `BANKNIFTY` and
/// `FINNIFTY` are named identically by both masters, while `MIDCPNIFTY` and
/// `NIFTYNXT50` are Dhan's spellings for the indices Groww calls
/// `NIFTYMIDSELECT` and `NIFTYJR`. **211 of 213 are therefore confirmed by two
/// vendors and 2 rest on one**, which `docs/06-limits.md` §12 records and
/// `api::server::report` states on every run rather than leaving implied.
pub const FNO_UNDERLYINGS: [&str; 213] = [
    "360ONE",
    "ABB",
    "ABCAPITAL",
    "ADANIENSOL",
    "ADANIENT",
    "ADANIGREEN",
    "ADANIPORTS",
    "ADANIPOWER",
    "ALKEM",
    "AMBER",
    "AMBUJACEM",
    "ANGELONE",
    "APLAPOLLO",
    "APOLLOHOSP",
    "ASHOKLEY",
    "ASIANPAINT",
    "ASTRAL",
    "AUBANK",
    "AUROPHARMA",
    "AXISBANK",
    "BAJAJ-AUTO",
    "BAJAJFINSV",
    "BAJAJHLDNG",
    "BAJFINANCE",
    "BANDHANBNK",
    "BANKBARODA",
    "BANKINDIA",
    "BANKNIFTY",
    "BDL",
    "BEL",
    "BHARATFORG",
    "BHARTIARTL",
    "BHEL",
    "BIOCON",
    "BLUESTARCO",
    "BOSCHLTD",
    "BPCL",
    "BRITANNIA",
    "BSE",
    "CAMS",
    "CANBK",
    "CDSL",
    "CGPOWER",
    "CHOLAFIN",
    "CIPLA",
    "COALINDIA",
    "COCHINSHIP",
    "COFORGE",
    "COLPAL",
    "CONCOR",
    "CROMPTON",
    "CUMMINSIND",
    "DABUR",
    "DALBHARAT",
    "DELHIVERY",
    "DIVISLAB",
    "DIXON",
    "DLF",
    "DMART",
    "DRREDDY",
    "EICHERMOT",
    "ETERNAL",
    "FEDERALBNK",
    "FINNIFTY",
    "FORCEMOT",
    "FORTIS",
    "GAIL",
    "GLENMARK",
    "GMRAIRPORT",
    "GODFRYPHLP",
    "GODREJCP",
    "GODREJPROP",
    "GRASIM",
    "GVT&D",
    "HAL",
    "HAVELLS",
    "HCLTECH",
    "HDFCAMC",
    "HDFCBANK",
    "HDFCLIFE",
    "HEROMOTOCO",
    "HINDALCO",
    "HINDPETRO",
    "HINDUNILVR",
    "HINDZINC",
    "HYUNDAI",
    "ICICIBANK",
    "ICICIGI",
    "ICICIPRULI",
    "IDEA",
    "IDFCFIRSTB",
    "IEX",
    "INDHOTEL",
    "INDIANB",
    "INDIGO",
    "INDUSINDBK",
    "INDUSTOWER",
    "INFY",
    "INOXWIND",
    "IOC",
    "IREDA",
    "IRFC",
    "ITC",
    "JINDALSTEL",
    "JIOFIN",
    "JSWENERGY",
    "JSWSTEEL",
    "JUBLFOOD",
    "KALYANKJIL",
    "KAYNES",
    "KEI",
    "KFINTECH",
    "KOTAKBANK",
    "KPITTECH",
    "LAURUSLABS",
    "LICHSGFIN",
    "LICI",
    "LODHA",
    "LT",
    "LTF",
    "LTM",
    "LUPIN",
    "M&M",
    "MANAPPURAM",
    "MANKIND",
    "MARICO",
    "MARUTI",
    "MAXHEALTH",
    "MAZDOCK",
    "MCX",
    "MFSL",
    "MIDCPNIFTY",
    "MOTHERSON",
    "MOTILALOFS",
    "MPHASIS",
    "MUTHOOTFIN",
    "NAM-INDIA",
    "NATIONALUM",
    "NAUKRI",
    "NBCC",
    "NESTLEIND",
    "NHPC",
    "NIFTY",
    "NIFTYNXT50",
    "NMDC",
    "NTPC",
    "NYKAA",
    "OBEROIRLTY",
    "OFSS",
    "OIL",
    "ONGC",
    "PAGEIND",
    "PATANJALI",
    "PAYTM",
    "PERSISTENT",
    "PETRONET",
    "PFC",
    "PGEL",
    "PHOENIXLTD",
    "PIDILITIND",
    "PIIND",
    "PNB",
    "PNBHOUSING",
    "POLICYBZR",
    "POLYCAB",
    "POWERGRID",
    "POWERINDIA",
    "PREMIERENE",
    "PRESTIGE",
    "RADICO",
    "RBLBANK",
    "RECLTD",
    "RELIANCE",
    "RVNL",
    "SAIL",
    "SBICARD",
    "SBILIFE",
    "SBIN",
    "SHREECEM",
    "SHRIRAMFIN",
    "SIEMENS",
    "SOLARINDS",
    "SONACOMS",
    "SRF",
    "SUNPHARMA",
    "SUPREMEIND",
    "SUZLON",
    "SWIGGY",
    "TATACONSUM",
    "TATAELXSI",
    "TATAPOWER",
    "TATASTEEL",
    "TCS",
    "TECHM",
    "TIINDIA",
    "TITAN",
    "TMPV",
    "TORNTPHARM",
    "TRENT",
    "TVSMOTOR",
    "ULTRACEMCO",
    "UNIONBANK",
    "UNITDSPR",
    "UNOMINDA",
    "UPL",
    "VBL",
    "VEDL",
    "VMM",
    "VOLTAS",
    "WAAREEENER",
    "WIPRO",
    "YESBANK",
    "ZYDUSLIFE",
];

/// Which universes an instrument belongs to.
///
/// A bitset rather than an enum: an instrument belongs to **many** universes
/// at once — a stock can be an F&O underlying *and* a Total Market
/// constituent. Bits are append-only, exactly like the condition table: a new
/// universe takes the next free bit and invalidates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Universe(u32);

impl Universe {
    /// Belongs to no universe. Stored, never listed.
    pub const NONE: Self = Self(0);
    /// A spot index.
    pub const INDEX: Self = Self(1 << 0);
    /// Has listed futures or options.
    pub const FNO: Self = Self(1 << 1);
    /// A NIFTY Total Market constituent.
    pub const TOTAL_MARKET: Self = Self(1 << 2);

    /// Whether this set contains every bit of `other`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether this set is empty.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// The union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// The universes a cash-segment symbol belongs to.
///
/// Binary search over sorted arrays: at most ten comparisons on 750 entries.
#[must_use]
pub fn of_equity(symbol: &str) -> Universe {
    let mut u = Universe::NONE;
    if FNO_UNDERLYINGS.binary_search(&symbol).is_ok() {
        u = u.union(Universe::FNO);
    }
    if NIFTY_TOTAL_MARKET.binary_search(&symbol).is_ok() {
        u = u.union(Universe::TOTAL_MARKET);
    }
    u
}

/// The universes a merged instrument belongs to.
///
/// The entry point every caller outside this module uses, so that "which list
/// is this in" is answered in one place from the whole key rather than from a
/// bare string a caller had to remember to derive correctly. An index is
/// [`Universe::INDEX`] and is never looked up in the equity lists — a spot
/// index is not a constituent of itself, and `NIFTY` appears in
/// [`FNO_UNDERLYINGS`] as the *underlying of its options*, which is a different
/// fact from being a share.
#[must_use]
pub fn of_instrument(key: &InstrumentKey) -> Universe {
    match key.kind {
        Kind::Index => Universe::INDEX.union(of_equity(key.underlying.as_str())),
        Kind::Equity => of_equity(key.underlying.as_str()),
        // A live derivative is never stored, so it never reaches a universe.
        // Saying `NONE` is accurate; inventing membership for it would not be.
        Kind::Future { .. } | Kind::Option { .. } => Universe::NONE,
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

    #[test]
    fn both_lists_are_sorted_and_unique_so_binary_search_is_valid() {
        // binary_search on an unsorted array returns garbage silently.
        for (name, list) in [
            ("NIFTY_TOTAL_MARKET", NIFTY_TOTAL_MARKET.as_slice()),
            ("FNO_UNDERLYINGS", FNO_UNDERLYINGS.as_slice()),
        ] {
            for w in list.windows(2) {
                assert!(w[0] < w[1], "{name} is not sorted or not unique at {w:?}");
            }
        }
    }

    #[test]
    fn the_counts_are_the_measured_ones() {
        assert_eq!(NIFTY_TOTAL_MARKET.len(), 750, "niftyindices.com states 750");
        assert_eq!(FNO_UNDERLYINGS.len(), 213, "both masters name 213, exactly");
        assert!(
            FNO_UNDERLYINGS.iter().all(|s| !s.contains("NSETEST")),
            "exchange test instruments must never be a universe member"
        );
        assert!(
            NIFTY_TOTAL_MARKET.iter().all(|s| !s.contains("NSETEST")),
            "exchange test instruments must never be a universe member"
        );
    }

    #[test]
    fn no_measured_sme_ticker_belongs_to_either_universe() {
        // THE CLAIM `Skip::SmeBoard` DECLINES 1,117 REAL SHARES ON. It was a
        // sentence in a comment about a module nothing consulted; here it is a
        // check. Every ticker below is verbatim from an `SM` or `ST` row of a
        // real master -- they are the 22 rows whose series the two vendors
        // disagree about, so they are also the SME rows most likely to be
        // mis-boarded, plus two ordinary ones.
        for sme in [
            "DRONE",
            "SATKARTAR",
            "SOCL",
            "WHITEFORCE",
            "VLINFRA",
            "SUNLITE",
            "ARVINDPORT",
            "YAAP",
            "MOXSH",
            "INVICTA",
            "ARCIIL",
            "RITEZONE",
            "AILIMITED",
            "ACCORD",
        ] {
            assert!(
                of_equity(sme).is_none(),
                "{sme} is an SME listing and is in neither universe"
            );
        }
    }

    #[test]
    fn an_index_is_its_own_universe_and_a_live_derivative_is_in_none() {
        use crate::instrument::{Exchange, Expiry, OptionSide, Segment};
        use crate::price::Paisa;
        use crate::symbol::Symbol;

        let nifty = InstrumentKey::index(Exchange::Nse, "NIFTY").expect("valid");
        let u = of_instrument(&nifty);
        assert!(u.contains(Universe::INDEX));
        assert!(
            u.contains(Universe::FNO),
            "NIFTY is also the underlying of its own options"
        );
        assert!(
            !u.contains(Universe::TOTAL_MARKET),
            "an index is not a share"
        );

        let sensex = InstrumentKey::index(Exchange::Nse, "NOTANINDEXWEKNOW").expect("valid");
        assert_eq!(of_instrument(&sensex), Universe::INDEX);

        let share = InstrumentKey {
            exchange: Exchange::Nse,
            segment: Segment::Cash,
            underlying: Symbol::new("RELIANCE").expect("valid"),
            kind: Kind::Equity,
        };
        let r = of_instrument(&share);
        assert!(r.contains(Universe::FNO) && r.contains(Universe::TOTAL_MARKET));
        assert!(!r.contains(Universe::INDEX));

        // A derivative is never stored, so it is never a member of anything --
        // not even of the universe its own underlying belongs to.
        let fut = InstrumentKey {
            segment: Segment::Fno,
            kind: Kind::Future {
                expiry: Expiry::new(2026, 8, 27).expect("valid"),
            },
            ..share
        };
        assert_eq!(of_instrument(&fut), Universe::NONE);
        let opt = InstrumentKey {
            kind: Kind::Option {
                expiry: Expiry::new(2026, 8, 27).expect("valid"),
                strike: Paisa::from_raw(1_945_000),
                side: OptionSide::Call,
            },
            ..fut
        };
        assert_eq!(of_instrument(&opt), Universe::NONE);
    }

    #[test]
    fn membership_is_a_set_not_a_category() {
        // RELIANCE is both. The whole point of a bitset.
        let r = of_equity("RELIANCE");
        assert!(r.contains(Universe::FNO));
        assert!(r.contains(Universe::TOTAL_MARKET));
        assert!(!r.is_none());
    }

    #[test]
    fn a_symbol_in_neither_belongs_to_nothing() {
        let n = of_equity("NOT_A_REAL_SYMBOL");
        assert!(n.is_none());
        assert_eq!(n, Universe::NONE);
        assert_eq!(n.bits(), 0);
    }

    #[test]
    fn contains_is_a_superset_test_not_equality() {
        let both = Universe::FNO.union(Universe::TOTAL_MARKET);
        assert!(both.contains(Universe::FNO));
        assert!(both.contains(Universe::TOTAL_MARKET));
        assert!(both.contains(both));
        assert!(
            !Universe::FNO.contains(both),
            "one bit does not contain two"
        );
        assert!(Universe::NONE.contains(Universe::NONE));
        assert!(!Universe::NONE.contains(Universe::INDEX));
    }

    #[test]
    fn bits_are_distinct_powers_of_two() {
        // An append-only bitset is only safe if no two universes share a bit.
        let all = [Universe::INDEX, Universe::FNO, Universe::TOTAL_MARKET];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.bits().count_ones(), 1, "universe {i} is not a single bit");
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_eq!(a.bits() & b.bits(), 0, "universes {i} and {j} share a bit");
                }
            }
        }
    }
}
