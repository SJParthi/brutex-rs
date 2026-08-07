//! The step that makes a new month's **name** durable: `store::durability::*`.
//!
//! # The step this file is about
//!
//! `BarFile::open_or_create` on a month that does not exist yet writes the
//! header region, `fsync`s the file, and then `fsync`s the **directory**. The
//! last of those three is the one nobody thinks about: without it the bars can
//! be on stable storage inside a file the directory does not yet mention after
//! a crash, which is the same as not having them.
//!
//! Every other test of this crate drives that step down its success path, so
//! the refusal it carries had never run. A refusal that has never run is a
//! refusal nobody has read since it was written, and this one guards the one
//! failure that is invisible afterwards.
//!
//! # How the failure is arranged, and what it assumes
//!
//! A directory with the write and execute bits and **not** the read bit
//! accepts a file being created inside it and refuses to be opened. That is
//! exactly the gap between "the bars were written" and "the directory flush
//! succeeded", and it needs no full disk and no injected fault.
//!
//! **UNIX ONLY, and it assumes this process is not root.** Root bypasses the
//! permission bits, would open the directory anyway, and the test would fail
//! on its own assertion rather than pass silently. CI and the operator's
//! machine both run as an ordinary user.
//!
//! # What this file does NOT prove
//!
//! That an `fsync` reached the platter. Nothing in this repository measures
//! that, and `docs/06-limits.md` says so. What is proven here is that when the
//! host refuses the flush, the refusal is **returned and named** rather than
//! swallowed — which is the part this crate controls.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use brutex_core::vendor::Vendor;
use store::file::{Action, BarFile, StoreError};
use store::path::{FileKind, PathParts, StorePath, Timeframe, YearMonth};

/// Distinguishes two scratch trees taken in the same process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary directory that removes itself.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut root = std::env::temp_dir();
        root.push(format!("{}-{tag}-{serial}", std::process::id()));
        fs::create_dir_all(&root).expect("a scratch root");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leaked scratch directory must never fail a test run.
        drop(fs::remove_dir_all(&self.root));
    }
}

/// The month under test.
fn bars_path() -> StorePath<'static> {
    StorePath::new(PathParts {
        vendor: Vendor::Groww,
        exchange: "NSE",
        segment: "INDEX",
        symbol: "NIFTY",
        timeframe: Timeframe::MINUTE_1,
        month: YearMonth::new(2024, 6).expect("June 2024"),
        file: FileKind::Bars,
    })
    .expect("a legal path")
}

/// A directory that will not open refuses the create, in the host's own words.
///
/// The bars file and the header region are written first — the failure is the
/// directory flush that publishes the file's NAME, and it is returned rather
/// than treated as best effort.
#[cfg(unix)]
#[test]
fn a_directory_flush_the_host_refuses_is_returned_and_named() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("NOFLUSH");
    let path = bars_path();
    let file = path.to_path_buf(&scratch.root);
    let dir = file
        .parent()
        .expect("a rendered store path always has parents")
        .to_path_buf();
    fs::create_dir_all(&dir).expect("the month's directory");

    // Write and execute, and NOT read: a file may be created inside, and the
    // directory itself may not be opened.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o300)).expect("close the directory");
    let refused = BarFile::open_or_create(&scratch.root, path, 7);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("reopen the directory");

    let refused = refused.expect_err(
        "the directory cannot be opened, so its flush cannot be issued, so the \
         new file's name is not durable and this must not report success",
    );
    assert_eq!(
        refused,
        StoreError::Denied {
            path: dir.clone(),
            action: Action::Open,
        },
        "refused by name, carrying the DIRECTORY and the operation — not the \
         bar file, which opened fine, and not a generic I/O error"
    );

    let text = refused.to_string();
    assert!(
        text.contains(&dir.display().to_string()),
        "the refusal names the directory an operator has to fix — {text}"
    );

    // The same month with the directory open succeeds, so the refusal is about
    // the permission bits and not about the fixture.
    let opened = BarFile::open_or_create(&scratch.root, bars_path(), 7)
        .expect("the same month, with the directory readable");
    assert_eq!(opened.records(), 0, "a fresh month holds no records");
}
