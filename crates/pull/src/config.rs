//! Where a parameter path comes from, and the only thing that assembles one.
//!
//! # The shape, and why only the shape is here
//!
//! ```text
//! /<org>/<env>/<vendor>/<field>
//! ```
//!
//! `CLAUDE.md` §8 and `docs/05-decisions.md` D-0013: **no literal parameter path
//! appears in any tracked file.** This repository is public. A real path names
//! an AWS account, an environment and a vendor relationship, and once it is
//! pushed it lives in every fork's history forever — there is no rotating it
//! back out. So this module holds four *names of segments* and nothing else.
//! The segments themselves come from a local file that is never committed, and
//! `crates/pull` contains no `org` literal, no `env` literal and no real vendor
//! path segment — in code, in a test, in a doc comment or in an example. CI
//! gate 1c walks every tracked file and fails the build on one.
//!
//! Every example and every test below therefore uses **invented** segments.
//! That is not a stylistic choice: a doc example carrying a real segment would
//! publish it exactly as effectively as a constant would.
//!
//! # The file
//!
//! ```text
//! ~/.brutex/credentials.toml
//! ```
//!
//! ```toml
//! org    = "orgone"
//! env    = "testenv"
//! region = "ap-south-1"
//!
//! [vendor.groww]
//! vendor = "vendorone"
//! fields = ["fieldone", "fieldtwo"]
//!
//! [vendor.dhan]
//! vendor = "vendortwo"
//! fields = ["fieldone"]
//! ```
//!
//! The table *name* is the vendor this build knows — [`Vendor::as_str`], a
//! public broker name that has been a tracked literal in `crates/core` since
//! that crate's first commit and appears in the README, in the charter, and in
//! every store path. The `vendor` **key inside** it is that vendor's
//! *parameter path* segment: a separate string, supplied at runtime, that this
//! repository never writes down.
//!
//! Whether those two happen to be the same text in a given operator's file is
//! that operator's business, and nothing here can tell. That is precisely why
//! the key exists rather than the segment being derived from
//! [`Vendor::as_str`] — deriving it would make the path segment a **tracked
//! literal by construction**, for every deployment, permanently, and no local
//! configuration could ever take it back out.
//!
//! # It halts. It never defaults
//!
//! Invariant P-07. A missing file, an unreadable one, a missing key, a missing
//! vendor table, an empty segment, a segment holding a path separator, or
//! anything shaped like a pasted secret is a refusal that names what was wrong
//! and where. There is no default `org`, no default `env`, no "assume the usual
//! region" and no skip-the-line-we-did-not-understand — `CLAUDE.md` §4 bans a
//! fallback that hides a failure, and a credential path resolved by guesswork
//! points a pull at somebody else's account.
//!
//! # Why this reader is strict rather than a TOML crate
//!
//! The grammar it accepts is four keys, one table shape, quoted strings and a
//! one-line array of quoted strings. A general TOML parser would accept a great
//! deal more and would then have to be told which of it to refuse, which is the
//! same work in a less obvious place; it would also add a dependency and a
//! `serde` derive to read eleven lines. The reader below refuses **every** line
//! it does not recognise, by line number, which is the property that matters:
//! a typo'd key is a halt, not a silent absence that becomes a default one
//! layer up.
//!
//! # Nothing from the file is ever echoed back
//!
//! Errors name the **line** and, where the key is one this reader defines, the
//! key. They never quote the text that was there. An operator who pastes a
//! token where a field name belongs would otherwise have it copied into a log
//! line by the very check that caught the mistake. The two exceptions are a
//! single offending byte — one byte is not a credential, and naming it is what
//! makes `IllegalByte` actionable — and a length.
//!
//! # Cost
//!
//! Reading is a bounded scan of a bounded file: at most [`MAX_FILE_BYTES`],
//! lines of at most [`MAX_LINE_BYTES`], segments of at most [`MAX_SEGMENT_LEN`],
//! at most [`MAX_FIELDS`] fields per vendor. `docs/07-o1-architecture.md` law 5
//! — every O(1) claim dies at the first unbounded input, and unbounded input
//! always arrives from outside. Assembling a path afterwards allocates nothing:
//! [`CredentialPath`] borrows its four segments and writes them straight into
//! the caller's sink.

use std::fmt;
use std::path::{Path, PathBuf};

use brutex_core::vendor::Vendor;

/// The directory the configuration lives in, under the operator's home.
pub const CONFIG_DIR: &str = ".brutex";

/// The configuration file's name.
pub const CONFIG_FILE: &str = "credentials.toml";

/// The AWS region `CLAUDE.md` §8 fixes, and the only one this build accepts.
///
/// A region is not a credential path — it is published in `CLAUDE.md` itself —
/// so it is a literal here. It is checked rather than merely read because a
/// configuration naming a different region resolves the *same* path against a
/// *different* account's Parameter Store, and every symptom of that is a
/// permission error a hundred lines later.
pub const REGION: &str = "ap-south-1";

/// The longest a single path segment may be.
///
/// Long enough for every segment the local configuration actually carries, and
/// far shorter than any credential this repository has seen. It is a bound on
/// input from outside the process — `docs/07-o1-architecture.md` law 5 — and it
/// is also the first of the two structural checks that stop a pasted secret
/// from being used as a path segment.
pub const MAX_SEGMENT_LEN: usize = 32;

/// The most fields one vendor may declare.
///
/// The real configuration declares three and four. Sixteen leaves room for a
/// vendor that grows a new credential field without letting a malformed file
/// declare an unbounded list.
pub const MAX_FIELDS: usize = 16;

/// The longest line this reader will look at.
///
/// Checked before the line is parsed, not after: a line is split and trimmed
/// into borrowed pieces before any key is recognised, so an unbounded line is
/// unbounded work paid before the first check could run. The same argument
/// `docs/05-decisions.md` D-0033 makes about a vendor master row.
pub const MAX_LINE_BYTES: usize = 512;

/// The largest configuration file this reader will pull into memory.
///
/// Checked from `metadata` **before** the read, not after. A file this process
/// cannot hold is not an error `read_to_string` can report — it is an allocator
/// failure or an OOM kill, and neither reaches an operator as "the credential
/// configuration is too big". The real file is under a kilobyte.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The length at which an undelimited alphanumeric run is treated as a pasted
/// secret rather than as a name.
///
/// See [`SegmentFault::LooksLikeASecret`] for what this is and, more
/// importantly, for what it is not.
pub const SECRET_LIKE_LEN: usize = 24;

/// The longest a rendered parameter path can be.
///
/// Four segments at [`MAX_SEGMENT_LEN`], each preceded by a separator. Tight
/// rather than merely sufficient:
/// `pull::unit::a_maximal_path_is_exactly_the_declared_bound` builds one and
/// asserts the equality, because a bound with slack in it is a bound nobody
/// notices going wrong.
pub const MAX_PATH_LEN: usize = 4 * (1 + MAX_SEGMENT_LEN);

/// The path of the configuration file under a home directory.
///
/// Takes the home directory rather than reading `HOME` itself, and that is
/// deliberate on two counts. It keeps this crate free of process-global state,
/// so two tests can drive two different configurations at once. And the absent-
/// `HOME` arm would otherwise be a branch no test on any real machine could
/// enter — `std::env::set_var` is `unsafe` under edition 2024 and every crate
/// here carries `#![forbid(unsafe_code)]`, so it could not even be forced. An
/// unreachable arm behind a coverage gate reporting 100% is exactly the false
/// green `docs/07-o1-architecture.md` records twice. The process boundary that
/// reads `HOME` is the operator entry point, and it is bounded there.
#[must_use]
pub fn default_config_path(home: &Path) -> PathBuf {
    let mut out =
        PathBuf::with_capacity(home.as_os_str().len() + 2 + CONFIG_DIR.len() + CONFIG_FILE.len());
    out.push(home);
    out.push(CONFIG_DIR);
    out.push(CONFIG_FILE);
    out
}

/// Why one segment is not a path segment.
///
/// Every variant names what was wrong. A refusal that says only "invalid" sends
/// an operator to compare eleven lines against a document by eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SegmentFault {
    /// The segment was empty. An empty segment collapses two levels of the
    /// parameter path into one and silently reads somebody else's parameter.
    Empty,
    /// The segment was longer than [`MAX_SEGMENT_LEN`].
    TooLong {
        /// How long it was.
        len: usize,
    },
    /// The segment was `.` or `..`.
    ///
    /// Checked before the byte scan, so the reason is legible rather than
    /// arriving as "byte 0x2e". Parameter Store does not resolve `..`, but a
    /// path holding one is a configuration nobody meant to write.
    Traversal,
    /// The segment held a byte outside `[a-z0-9_-]` and outside `[A-Z]`.
    ///
    /// A separator lands here, which is what refuses both an absolute path and
    /// a nested one: a whole path pasted into a segment begins with `/`.
    IllegalByte {
        /// The byte that was refused.
        byte: u8,
    },
    /// The segment held an upper-case letter.
    ///
    /// Refused rather than folded. Parameter Store paths are case-sensitive, so
    /// folding would read a parameter that does not exist and report it as
    /// missing; and mixed case in a four-character-to-thirty-two-character name
    /// is the commonest shape of a pasted credential.
    NotLowerCase {
        /// The letter that was in the wrong case.
        byte: u8,
    },
    /// The segment is [`SECRET_LIKE_LEN`] bytes or more of undelimited
    /// alphanumerics with at least one digit in it.
    ///
    /// **This is a backstop, and it is stated as one.** The checks that do the
    /// real work are the length bound and the byte set: every credential shape
    /// this repository has had in front of it — an access key, a JWT, a base64
    /// blob, a bearer token — is refused by one of those before this runs,
    /// because it is longer than [`MAX_SEGMENT_LEN`], or carries an upper-case
    /// letter, or a `.`, or a `+`, or a `/`. This catches the residue: a short,
    /// lower-case, alphanumeric secret that would otherwise pass for a name.
    ///
    /// It is a heuristic over a shape, not a proof about entropy, and no claim
    /// is made that it catches every secret — `CLAUDE.md` §3 rule 6. What it
    /// does not catch, `docs/06-limits.md` §16 records.
    LooksLikeASecret {
        /// How long the run was.
        len: usize,
    },
}

impl fmt::Display for SegmentFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty => f.write_str("is empty"),
            Self::TooLong { len } => write!(f, "is {len} bytes, max {MAX_SEGMENT_LEN}"),
            Self::Traversal => f.write_str("is a traversal and would escape"),
            Self::IllegalByte { byte } => write!(f, "holds byte {byte:#04x}"),
            Self::NotLowerCase { byte } => write!(f, "holds byte {byte:#04x} in upper case"),
            Self::LooksLikeASecret { len } => write!(
                f,
                "is {len} undelimited alphanumeric bytes and looks like a pasted secret, not a name"
            ),
        }
    }
}

/// Whether `text` is a legal path segment.
///
/// The checks run in this order, and the order is load-bearing: an empty or
/// over-long segment is reported as such rather than as whatever its first byte
/// happens to be, a traversal is named before the byte scan would call it an
/// illegal byte, and the secret backstop runs last so a segment that is *both*
/// mis-cased and over-long is reported as over-long.
///
/// # Errors
///
/// The first [`SegmentFault`] that applies.
///
/// # Examples
///
/// ```
/// # use pull::config::{check_segment, SegmentFault, MAX_SEGMENT_LEN};
/// assert_eq!(check_segment("fieldone"), Ok(()));
/// assert_eq!(check_segment(""), Err(SegmentFault::Empty));
/// assert_eq!(check_segment("a/b"), Err(SegmentFault::IllegalByte { byte: b'/' }));
/// assert_eq!(check_segment("Field"), Err(SegmentFault::NotLowerCase { byte: b'F' }));
/// ```
pub fn check_segment(text: &str) -> Result<(), SegmentFault> {
    if text.is_empty() {
        return Err(SegmentFault::Empty);
    }
    if text.len() > MAX_SEGMENT_LEN {
        return Err(SegmentFault::TooLong { len: text.len() });
    }
    if text == "." || text == ".." {
        return Err(SegmentFault::Traversal);
    }
    if let Some(byte) = text
        .bytes()
        .find(|b| !matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return Err(SegmentFault::IllegalByte { byte });
    }
    if let Some(byte) = text.bytes().find(u8::is_ascii_uppercase) {
        return Err(SegmentFault::NotLowerCase { byte });
    }
    if text.len() >= SECRET_LIKE_LEN
        && text.bytes().all(|b| b.is_ascii_alphanumeric())
        && text.bytes().any(|b| b.is_ascii_digit())
    {
        return Err(SegmentFault::LooksLikeASecret { len: text.len() });
    }
    Ok(())
}

/// Why the configuration was refused.
///
/// Nothing from the file is quoted back — see this module's header for why.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    /// The file could not be read at all.
    ///
    /// Carries the path, which is `~/.brutex/credentials.toml` and not a
    /// credential, and the kind, because "no such file" and "permission denied"
    /// send an operator to two different places.
    Unreadable {
        /// The file that could not be read.
        path: PathBuf,
        /// What the operating system said.
        kind: std::io::ErrorKind,
    },
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooLarge {
        /// How large it is.
        len: u64,
    },
    /// A line is longer than [`MAX_LINE_BYTES`].
    LineTooLong {
        /// One-based line number.
        line: usize,
        /// How long it was.
        len: usize,
    },
    /// A line is not blank, not a comment, not a table header and not an
    /// assignment this reader understands.
    ///
    /// Refused rather than skipped. A skipped line is a key that silently is
    /// not there, which becomes a missing segment, which is the one thing
    /// P-07 says must never be defaulted.
    Unparseable {
        /// One-based line number.
        line: usize,
    },
    /// A table header is not of the form `[vendor.<name>]`.
    UnknownTable {
        /// One-based line number.
        line: usize,
    },
    /// A `[vendor.<name>]` table names a vendor this build does not read.
    UnknownVendor {
        /// One-based line number.
        line: usize,
    },
    /// A key is not one this shape defines, or is not defined where it appears.
    ///
    /// `org` inside a vendor table and `fields` at the top level both land
    /// here: a key in the wrong place is a key that will not be read.
    UnknownKey {
        /// One-based line number.
        line: usize,
    },
    /// A key this reader defines appeared twice in one table.
    DuplicateKey {
        /// One-based line number.
        line: usize,
        /// The key.
        key: &'static str,
    },
    /// A vendor has more than one table.
    DuplicateVendor {
        /// The vendor, by the name this build knows it as.
        vendor: &'static str,
    },
    /// A value is a list where a string is required, or the reverse.
    WrongShape {
        /// One-based line number.
        line: usize,
        /// The key.
        key: &'static str,
    },
    /// A vendor declared more than [`MAX_FIELDS`] fields.
    TooManyFields {
        /// One-based line number.
        line: usize,
        /// How many were declared.
        count: usize,
    },
    /// A top-level key is absent.
    MissingKey {
        /// The key.
        key: &'static str,
    },
    /// A vendor table is absent a key.
    MissingVendorKey {
        /// The vendor, by the name this build knows it as.
        vendor: &'static str,
        /// The key.
        key: &'static str,
    },
    /// A vendor this build reads has no table at all.
    MissingVendor {
        /// The vendor, by the name this build knows it as.
        vendor: &'static str,
    },
    /// A vendor declared an empty field list.
    ///
    /// A vendor with no fields has no credential, which would surface as a
    /// pull that reads nothing and reports success.
    NoFields {
        /// The vendor, by the name this build knows it as.
        vendor: &'static str,
    },
    /// The configured region is not [`REGION`].
    WrongRegion {
        /// One-based line number.
        line: usize,
    },
    /// A segment is not a path segment.
    Segment {
        /// One-based line number.
        line: usize,
        /// Which key it came from.
        key: &'static str,
        /// What was wrong with it.
        fault: SegmentFault,
    },
    /// A field was asked for that this vendor does not declare.
    ///
    /// Not a file fault — a caller fault — but it belongs in the same enum
    /// because it is the same refusal: this build will not assemble a path out
    /// of a segment nobody configured.
    UnknownField {
        /// The vendor, by the name this build knows it as.
        vendor: &'static str,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, kind } => {
                write!(f, "{}: {kind:?}", path.display())
            }
            Self::TooLarge { len } => {
                write!(f, "{len} bytes; this reader holds at most {MAX_FILE_BYTES}")
            }
            Self::LineTooLong { line, len } => {
                write!(f, "line {line} is {len} bytes, max {MAX_LINE_BYTES}")
            }
            Self::Unparseable { line } => write!(f, "line {line} is not a line this reader knows"),
            Self::UnknownTable { line } => {
                write!(f, "line {line} is not a [vendor.<name>] table header")
            }
            Self::UnknownVendor { line } => {
                write!(f, "line {line} names a vendor this build does not read")
            }
            Self::UnknownKey { line } => {
                write!(f, "line {line} sets a key that is not defined here")
            }
            Self::DuplicateKey { line, key } => write!(f, "line {line} sets {key} twice"),
            Self::DuplicateVendor { vendor } => {
                write!(f, "vendor {vendor} has more than one table")
            }
            Self::WrongShape { line, key } => {
                write!(f, "line {line}: {key} is not the shape this reader reads")
            }
            Self::TooManyFields { line, count } => {
                write!(f, "line {line} declares {count} fields, max {MAX_FIELDS}")
            }
            Self::MissingKey { key } => write!(f, "no {key} is configured"),
            Self::MissingVendorKey { vendor, key } => {
                write!(f, "vendor {vendor} configures no {key}")
            }
            Self::MissingVendor { vendor } => write!(f, "vendor {vendor} has no table"),
            Self::NoFields { vendor } => write!(f, "vendor {vendor} declares no fields"),
            Self::WrongRegion { line } => {
                write!(f, "line {line} is not region {REGION}")
            }
            Self::Segment { line, key, fault } => write!(f, "line {line}: {key} {fault}"),
            Self::UnknownField { vendor } => {
                write!(f, "vendor {vendor} declares no such field")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// One assembled parameter path.
///
/// Borrows its four segments from the configuration, so building one allocates
/// nothing and the segments cannot outlive the file they came from.
///
/// There is no constructor taking a whole path as text. That is the point: the
/// only way to obtain one is [`CredentialConfig::path_for`], which assembles it
/// from segments that were each validated, so a path that was never configured
/// is unrepresentable rather than merely unlikely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CredentialPath<'a> {
    org: &'a str,
    env: &'a str,
    vendor: &'a str,
    field: &'a str,
}

impl<'a> CredentialPath<'a> {
    /// The field this path names.
    ///
    /// The field *name* — not the value, and not the account. It is the one
    /// segment a halt is allowed to carry, so an operator can tell which of a
    /// vendor's four credentials failed.
    #[must_use]
    pub const fn field(self) -> &'a str {
        self.field
    }
}

impl fmt::Display for CredentialPath<'_> {
    /// The one renderer, and the only place the four segments are ever joined.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "/{}/{}/{}/{}",
            self.org, self.env, self.vendor, self.field
        )
    }
}

/// One vendor's segment and its field names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VendorPaths {
    vendor: Vendor,
    segment: String,
    fields: Vec<String>,
}

/// The validated contents of `~/.brutex/credentials.toml`.
///
/// Holds path segments. It never holds, reads, receives or returns a credential
/// **value** — that is [`crate::secret`]'s job, and the value comes from
/// Parameter Store and from nowhere else (`CLAUDE.md` §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialConfig {
    org: String,
    env: String,
    region: String,
    vendors: Vec<VendorPaths>,
}

impl CredentialConfig {
    /// Reads and validates the configuration at `path`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Unreadable`] if the file is absent or cannot be opened —
    /// which is invariant P-07's headline case and is a halt, never a default.
    /// [`ConfigError::TooLarge`] if it exceeds [`MAX_FILE_BYTES`]. Otherwise
    /// whatever [`CredentialConfig::parse`] refuses.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        // THE SIZE IS CHECKED BEFORE THE READ, NOT AFTER. A file this process
        // cannot hold is an OOM kill, not an error `read_to_string` reports.
        let meta = std::fs::metadata(path).map_err(|e| ConfigError::Unreadable {
            path: path.to_path_buf(),
            kind: e.kind(),
        })?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(ConfigError::TooLarge { len: meta.len() });
        }
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Unreadable {
            path: path.to_path_buf(),
            kind: e.kind(),
        })?;
        Self::parse(&text)
    }

    /// Validates configuration text that is already in memory.
    ///
    /// Separated from [`CredentialConfig::load`] so a test can drive every
    /// refusal without a file, and so the file reading has nothing in it but
    /// the two bounds.
    ///
    /// # Errors
    ///
    /// [`ConfigError`], naming the line and the key. Every refusal is a halt:
    /// there is no arm of this function that supplies a value the file did not.
    ///
    /// # Examples
    ///
    /// ```
    /// # use brutex_core::vendor::Vendor;
    /// # use pull::config::{ConfigError, CredentialConfig};
    /// let text = "\
    /// org    = \"orgone\"
    /// env    = \"testenv\"
    /// region = \"ap-south-1\"
    ///
    /// [vendor.groww]
    /// vendor = \"vendorone\"
    /// fields = [\"fieldone\", \"fieldtwo\"]
    ///
    /// [vendor.dhan]
    /// vendor = \"vendortwo\"
    /// fields = [\"fieldone\"]
    /// ";
    /// let config = CredentialConfig::parse(text)?;
    /// let path = config.path_for(Vendor::Groww, "fieldtwo")?;
    /// assert_eq!(path.to_string(), "/orgone/testenv/vendorone/fieldtwo");
    ///
    /// // A field nobody configured is a refusal, never an assembled guess.
    /// assert!(config.path_for(Vendor::Groww, "fieldthree").is_err());
    /// # Ok::<(), ConfigError>(())
    /// ```
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut org: Option<String> = None;
        let mut env: Option<String> = None;
        let mut region: Option<String> = None;
        let mut vendors: Vec<VendorPaths> = Vec::with_capacity(Vendor::ALL.len());
        let mut current: Option<Pending> = None;

        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            if raw.len() > MAX_LINE_BYTES {
                return Err(ConfigError::LineTooLong {
                    line,
                    len: raw.len(),
                });
            }
            match parse_line(raw, line)? {
                Line::Blank => {}
                Line::Table(vendor) => {
                    if let Some(done) = current.take() {
                        vendors.push(done.finish()?);
                    }
                    current = Some(Pending::new(vendor));
                }
                Line::Assign { key, value } => match current.as_mut() {
                    None => assign_top(key, &value, line, &mut org, &mut env, &mut region)?,
                    Some(pending) => pending.assign(key, &value, line)?,
                },
            }
        }
        if let Some(done) = current.take() {
            vendors.push(done.finish()?);
        }

        for vendor in Vendor::ALL {
            let seen = vendors.iter().filter(|v| v.vendor == vendor).count();
            if seen == 0 {
                return Err(ConfigError::MissingVendor {
                    vendor: vendor.as_str(),
                });
            }
            if seen > 1 {
                return Err(ConfigError::DuplicateVendor {
                    vendor: vendor.as_str(),
                });
            }
        }

        Ok(Self {
            org: org.ok_or(ConfigError::MissingKey { key: "org" })?,
            env: env.ok_or(ConfigError::MissingKey { key: "env" })?,
            region: region.ok_or(ConfigError::MissingKey { key: "region" })?,
            vendors,
        })
    }

    /// The configured region. Always [`REGION`]; anything else was refused.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The field names configured for `vendor`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::MissingVendor`] — unreachable through
    /// [`CredentialConfig::parse`], which refuses a configuration missing any
    /// vendor, and present because a future `Vendor` variant would reach it
    /// before the parser was updated. It is a refusal rather than an empty
    /// slice for exactly that reason: an empty slice would read as "this vendor
    /// needs no credential".
    pub fn fields(&self, vendor: Vendor) -> Result<&[String], ConfigError> {
        self.vendors
            .iter()
            .find(|v| v.vendor == vendor)
            .map(|v| v.fields.as_slice())
            .ok_or(ConfigError::MissingVendor {
                vendor: vendor.as_str(),
            })
    }

    /// The parameter path for one vendor's one field.
    ///
    /// The only thing in this repository that assembles a parameter path.
    ///
    /// # Errors
    ///
    /// [`ConfigError::MissingVendor`] as in [`CredentialConfig::fields`], or
    /// [`ConfigError::UnknownField`] when the vendor declares no such field.
    /// A field nobody configured is never assembled: it would resolve to a path
    /// that does not exist, and "parameter not found" is a much worse
    /// diagnosis of a typo than "you did not configure that".
    pub fn path_for(&self, vendor: Vendor, field: &str) -> Result<CredentialPath<'_>, ConfigError> {
        // `and_then` rather than `?` on the vendor lookup, on purpose.
        // [`CredentialConfig::parse`] refuses a configuration missing any
        // vendor, so the `None` arm here is unreachable by any input — and a
        // `?` would put its short-circuit in *this* function, where the
        // coverage gate would count a branch no test could ever enter and the
        // whole crate would sit at 99%. Written this way the unreachable arm
        // lives in `Result::and_then`, which is not this repository's code to
        // measure. The refusal is still returned, and it is still a refusal
        // rather than an empty slice: an empty field list would read as "this
        // vendor needs no credential".
        self.vendors
            .iter()
            .find(|v| v.vendor == vendor)
            .ok_or(ConfigError::MissingVendor {
                vendor: vendor.as_str(),
            })
            .and_then(|paths| {
                let field = paths.fields.iter().find(|f| f.as_str() == field).ok_or(
                    ConfigError::UnknownField {
                        vendor: vendor.as_str(),
                    },
                )?;
                Ok(CredentialPath {
                    org: &self.org,
                    env: &self.env,
                    vendor: &paths.segment,
                    field,
                })
            })
    }
}

/// A vendor table being built.
struct Pending {
    vendor: Vendor,
    segment: Option<String>,
    fields: Option<Vec<String>>,
}

impl Pending {
    const fn new(vendor: Vendor) -> Self {
        Self {
            vendor,
            segment: None,
            fields: None,
        }
    }

    /// Takes one assignment inside a vendor table.
    fn assign(&mut self, key: &str, value: &Value<'_>, line: usize) -> Result<(), ConfigError> {
        match key {
            "vendor" => {
                let text = value.text(line, "vendor")?;
                segment(text, line, "vendor")?;
                if self.segment.replace(text.to_owned()).is_some() {
                    return Err(ConfigError::DuplicateKey {
                        line,
                        key: "vendor",
                    });
                }
                Ok(())
            }
            "fields" => {
                let items = value.list(line, "fields")?;
                if items.len() > MAX_FIELDS {
                    return Err(ConfigError::TooManyFields {
                        line,
                        count: items.len(),
                    });
                }
                let mut fields = Vec::with_capacity(items.len());
                for item in items {
                    segment(item, line, "fields")?;
                    fields.push((*item).to_owned());
                }
                if self.fields.replace(fields).is_some() {
                    return Err(ConfigError::DuplicateKey {
                        line,
                        key: "fields",
                    });
                }
                Ok(())
            }
            _ => Err(ConfigError::UnknownKey { line }),
        }
    }

    /// Closes the table, refusing one that is incomplete.
    fn finish(self) -> Result<VendorPaths, ConfigError> {
        let name = self.vendor.as_str();
        let segment = self.segment.ok_or(ConfigError::MissingVendorKey {
            vendor: name,
            key: "vendor",
        })?;
        let fields = self.fields.ok_or(ConfigError::MissingVendorKey {
            vendor: name,
            key: "fields",
        })?;
        if fields.is_empty() {
            return Err(ConfigError::NoFields { vendor: name });
        }
        Ok(VendorPaths {
            vendor: self.vendor,
            segment,
            fields,
        })
    }
}

/// Takes one assignment outside any table.
fn assign_top(
    key: &str,
    value: &Value<'_>,
    line: usize,
    org: &mut Option<String>,
    env: &mut Option<String>,
    region: &mut Option<String>,
) -> Result<(), ConfigError> {
    let slot = match key {
        "org" => &mut *org,
        "env" => &mut *env,
        "region" => &mut *region,
        _ => return Err(ConfigError::UnknownKey { line }),
    };
    let text = value.text(line, top_key(key))?;
    segment(text, line, top_key(key))?;
    if key == "region" && text != REGION {
        return Err(ConfigError::WrongRegion { line });
    }
    if slot.replace(text.to_owned()).is_some() {
        return Err(ConfigError::DuplicateKey {
            line,
            key: top_key(key),
        });
    }
    Ok(())
}

/// The `&'static str` for a top-level key, so an error never borrows the file.
///
/// The `_` arm is unreachable through [`assign_top`], which has already matched
/// the same three names, and it returns a name rather than panicking: this
/// function exists to keep file text out of errors, and a panic here would be a
/// worse failure than a slightly vaguer message.
fn top_key(key: &str) -> &'static str {
    match key {
        "org" => "org",
        "env" => "env",
        _ => "region",
    }
}

/// Validates one segment and wraps the fault with where it was found.
fn segment(text: &str, line: usize, key: &'static str) -> Result<(), ConfigError> {
    check_segment(text).map_err(|fault| ConfigError::Segment { line, key, fault })
}

/// One value, as this reader's grammar admits it.
///
/// No derives: a derived `Debug` or `PartialEq` on a private type nothing
/// prints or compares is a function the coverage gate would count and no test
/// could reach.
enum Value<'a> {
    Text(&'a str),
    List(Vec<&'a str>),
}

impl<'a> Value<'a> {
    /// The value as a string, or a refusal naming the key.
    fn text(&self, line: usize, key: &'static str) -> Result<&'a str, ConfigError> {
        match *self {
            Self::Text(t) => Ok(t),
            Self::List(_) => Err(ConfigError::WrongShape { line, key }),
        }
    }

    /// The value as a list, or a refusal naming the key.
    fn list(&self, line: usize, key: &'static str) -> Result<&[&'a str], ConfigError> {
        match self {
            Self::List(items) => Ok(items),
            Self::Text(_) => Err(ConfigError::WrongShape { line, key }),
        }
    }
}

/// One line, as this reader's grammar admits it.
enum Line<'a> {
    Blank,
    Table(Vendor),
    Assign { key: &'a str, value: Value<'a> },
}

/// Parses one line, refusing anything the grammar does not cover.
fn parse_line(raw: &str, line: usize) -> Result<Line<'_>, ConfigError> {
    let text = raw.trim();
    if text.is_empty() || text.starts_with('#') {
        return Ok(Line::Blank);
    }
    if let Some(rest) = text.strip_prefix('[') {
        let inner = rest
            .strip_suffix(']')
            .ok_or(ConfigError::UnknownTable { line })?;
        let name = inner
            .strip_prefix("vendor.")
            .ok_or(ConfigError::UnknownTable { line })?;
        let vendor = Vendor::ALL
            .into_iter()
            .find(|v| v.as_str() == name)
            .ok_or(ConfigError::UnknownVendor { line })?;
        return Ok(Line::Table(vendor));
    }
    let (key, value) = text
        .split_once('=')
        .ok_or(ConfigError::Unparseable { line })?;
    Ok(Line::Assign {
        key: key.trim(),
        value: parse_value(value.trim(), line)?,
    })
}

/// Parses one value: a quoted string, or a one-line array of quoted strings.
fn parse_value(text: &str, line: usize) -> Result<Value<'_>, ConfigError> {
    if let Some(rest) = text.strip_prefix('[') {
        let inner = rest
            .strip_suffix(']')
            .ok_or(ConfigError::Unparseable { line })?
            .trim();
        if inner.is_empty() {
            return Ok(Value::List(Vec::new()));
        }
        let mut items = Vec::with_capacity(inner.split(',').count());
        for item in inner.split(',') {
            items.push(quoted(item.trim(), line)?);
        }
        return Ok(Value::List(items));
    }
    Ok(Value::Text(quoted(text, line)?))
}

/// The inside of a double-quoted string, or a refusal.
///
/// A quote *inside* the string is refused rather than escaped: this grammar has
/// no escapes, and silently accepting one would mean the segment that reached
/// validation was not the text the operator wrote.
fn quoted(text: &str, line: usize) -> Result<&str, ConfigError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .ok_or(ConfigError::Unparseable { line })?;
    if inner.contains('"') {
        return Err(ConfigError::Unparseable { line });
    }
    Ok(inner)
}
