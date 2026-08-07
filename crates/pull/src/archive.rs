//! Reading a vendor's CSVs off a local directory — the transport that needs no
//! socket, no token and no rate limiter.
//!
//! # Half the vendors are not APIs
//!
//! Dhan and Groww answer HTTP. `TrueData` and GDFL sell **folders of files**.
//! A descriptor that assumed every feed was an endpoint could not express them
//! at all, so [`crate::vendor::Transport`] carries both and this module is the
//! local half. It produces exactly what an HTTP source produces —
//! [`crate::fetch::RawRow`]s — so everything downstream is transport-blind.
//!
//! # `__MACOSX` is a parsing hazard, not untidiness
//!
//! `GDFL.zip` lists 24,292 entries of which **12,145 are `__MACOSX`**: one
//! `AppleDouble` stub per real file, written by macOS when it re-zips. They
//! **end in `.csv`** and they are binary. A reader that globs by extension
//! opens all 12,145 and parses resource forks as text.
//!
//! [`crate::csv::is_ghost`] is applied here, at the walk, before anything is
//! opened. It was found by getting a *count* wrong — the counting bug and the
//! parsing bug were the same bug.
//!
//! # An honest word about the cost
//!
//! **Enumerating a directory is O(files), and that is correct here.** The O(1)
//! law in `docs/07-o1-architecture.md` is about answering a *question* — "where
//! is bar N", "how many months do I hold" — without a walk. A bulk import of
//! twelve thousand contracts genuinely has to visit twelve thousand files;
//! pretending otherwise would be the false-claim shape this repository keeps
//! catching itself in.
//!
//! What is bounded: [`MAX_MEMBERS`] caps the walk, one file is open at a time,
//! and each file's rows are decoded and handed on rather than accumulated
//! across the whole directory. Peak memory is one file, not one archive.

use std::fs;
use std::path::{Path, PathBuf};

use crate::csv::{self, Columns, CsvError};
use crate::fetch::RawRow;

/// The most members one walk will visit.
///
/// `docs/07-o1-architecture.md` law 5 — bound every input at the boundary. One
/// GDFL day holds 12,132 contracts, so this is roughly four such days and still
/// refuses a directory somebody pointed at their home folder.
pub const MAX_MEMBERS: usize = 50_000;

/// Why a directory did not yield rows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArchiveError {
    /// The directory is not there, or is not a directory.
    NotADirectory {
        /// What was pointed at.
        path: PathBuf,
    },
    /// The directory could not be listed.
    Unreadable {
        /// What was pointed at.
        path: PathBuf,
        /// The operating system's own words.
        detail: String,
    },
    /// A member could not be read.
    MemberUnreadable {
        /// Which one.
        path: PathBuf,
        /// The operating system's own words.
        detail: String,
    },
    /// A member is not valid UTF-8.
    ///
    /// Usually an `AppleDouble` stub that [`csv::is_ghost`] did not catch,
    /// which makes it worth its own variant rather than a generic parse
    /// failure: it names a *different* fix.
    MemberNotText {
        /// Which one.
        path: PathBuf,
    },
    /// A member's rows did not decode.
    MemberMalformed {
        /// Which one.
        path: PathBuf,
        /// The decoder's own refusal, which names the line.
        why: CsvError,
    },
    /// More members than [`MAX_MEMBERS`].
    TooManyMembers {
        /// How many were seen before stopping.
        members: usize,
        /// The bound.
        cap: usize,
    },
    /// A member path escapes the directory it was walked from.
    ///
    /// Refused before the file is opened. A path containing a parent-directory
    /// component is the archive-extraction attack — harmless from a paid vendor
    /// and unrecoverable if it ever is not, so the check costs nothing and the
    /// absence of it costs everything.
    PathEscapes {
        /// The offending path.
        path: PathBuf,
    },
}

impl core::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::NotADirectory { ref path } => {
                write!(f, "{} is not a directory", path.display())
            }
            Self::Unreadable {
                ref path,
                ref detail,
            } => write!(f, "{} could not be listed: {detail}", path.display()),
            Self::MemberUnreadable {
                ref path,
                ref detail,
            } => write!(f, "{} could not be read: {detail}", path.display()),
            Self::MemberNotText { ref path } => write!(
                f,
                "{} is not text — most likely an AppleDouble stub that the \
                 ghost filter missed",
                path.display()
            ),
            Self::MemberMalformed { ref path, ref why } => {
                write!(f, "{}: {why}", path.display())
            }
            Self::TooManyMembers { members, cap } => write!(
                f,
                "the directory holds at least {members} members; the cap is {cap}"
            ),
            Self::PathEscapes { ref path } => write!(
                f,
                "{} escapes the directory it was walked from — refused before \
                 it was opened",
                path.display()
            ),
        }
    }
}

impl core::error::Error for ArchiveError {}

/// One member's worth of decoded rows, and where they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The file, so a refusal downstream can name it.
    pub path: PathBuf,
    /// The instrument, taken from the file name with its extensions removed.
    ///
    /// `NIFTY25SEP2525700PE.NFO.csv` becomes `NIFTY25SEP2525700PE`. Decomposing
    /// that into underlying, expiry, strike and type is the *vocabulary's* job,
    /// not this module's — a reader that also parsed contract grammar would be
    /// two things, and the second one would be wrong first.
    pub instrument: String,
    /// The rows, in file order.
    ///
    /// **File order is the only order there is.** These feeds are one-second
    /// snapshots with two to four rows sharing a second and no tiebreaker, so
    /// any re-sort destroys arrival order that was never written down.
    pub rows: Vec<RawRow>,
}

/// Every real CSV directly inside `dir`, decoded.
///
/// Ghost members are skipped silently *by design* — they are not data and
/// counting them as skipped would put 12,145 entries in a census that is meant
/// to describe bars.
///
/// # Errors
///
/// Any [`ArchiveError`]. A malformed member refuses the **whole walk**: a
/// directory that yielded some of its contracts is not a smaller import, it is
/// an import nobody can characterise afterwards.
///
/// # Cost
///
/// O(members) — a bulk import visits every file, and that is inherent. One file
/// is open at a time and peak memory is one file's rows, not the directory's.
pub fn read_dir(dir: &Path, columns: Columns) -> Result<Vec<Member>, ArchiveError> {
    if !dir.is_dir() {
        return Err(ArchiveError::NotADirectory {
            path: dir.to_path_buf(),
        });
    }
    let entries = fs::read_dir(dir).map_err(|e| ArchiveError::Unreadable {
        path: dir.to_path_buf(),
        detail: e.to_string(),
    })?;

    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ArchiveError::Unreadable {
            path: dir.to_path_buf(),
            detail: e.to_string(),
        })?;
        let path = entry.path();

        // THE GHOST FILTER, before anything is opened.
        let name = path.to_string_lossy();
        if csv::is_ghost(&name) {
            continue;
        }
        if !path.is_file() || path.extension().is_none_or(|e| e != "csv") {
            continue;
        }
        // A member must stay under the directory it was walked from. `..` in a
        // name is the extraction attack; cheap to refuse, unrecoverable if not.
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ArchiveError::PathEscapes { path });
        }
        if out.len() >= MAX_MEMBERS {
            return Err(ArchiveError::TooManyMembers {
                members: out.len(),
                cap: MAX_MEMBERS,
            });
        }

        let bytes = fs::read(&path).map_err(|e| ArchiveError::MemberUnreadable {
            path: path.clone(),
            detail: e.to_string(),
        })?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ArchiveError::MemberNotText { path: path.clone() })?;
        let rows = csv::decode(&text, columns).map_err(|why| ArchiveError::MemberMalformed {
            path: path.clone(),
            why,
        })?;

        let instrument = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .and_then(|n| n.split('.').next().map(str::to_owned))
            .unwrap_or_default();

        out.push(Member {
            path,
            instrument,
            rows,
        });
    }

    // `read_dir` yields in filesystem order, which differs between machines and
    // between runs. Sorting by path makes an import reproducible — CLAUDE.md
    // §3 rule 5, same inputs same outputs. This orders the MEMBERS, never the
    // rows inside one, whose file order carries information.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// How many rows a walk produced, across every member.
#[must_use]
pub fn total_rows(members: &[Member]) -> usize {
    members.iter().map(|m| m.rows.len()).sum()
}
