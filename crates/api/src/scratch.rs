//! Test-only temporary paths, unique to the process that asks for one.
//!
//! # The failure this exists to remove
//!
//! `master::tests::a_master_larger_than_this_reader_holds_is_refused_before_it_is_read`
//! failed intermittently — about one run in three — with
//! `cleanup: Os { code: 2, kind: NotFound }` on the line that deletes its own
//! fixture. The fixture path was
//! `std::env::temp_dir().join("brutex-master-toobig.csv")`: **one fixed name in
//! a directory shared by every process on the machine.**
//!
//! The test creates a 256 MiB sparse file at that path, calls `load` twice —
//! once over the bound and once at it, and the second call reads all 256 MiB —
//! and then deletes it. Any second process running the same test binary
//! deletes the file inside that window. Two concurrent `cargo test` runs, a
//! `cargo llvm-cov` run beside a `cargo test`, or two agents working the same
//! checkout are all enough, and it was reproduced deliberately: two copies of
//! the test binary started together, one failed and one passed.
//!
//! The assertions were never wrong. `MAX_MASTER_BYTES + 1` is refused and
//! `MAX_MASTER_BYTES` exactly is not, and both are still asserted unchanged.
//! What was wrong was a fixture that two processes could both claim.
//!
//! # The rule
//!
//! Every temporary path this crate's tests touch goes through [`path`], which
//! stamps the process id into the name. Two processes therefore never name the
//! same file, and a test remains free to delete what it created — which is the
//! only way `remove_file` can be an assertion rather than a hope.
//!
//! The process id is not a random number and is not meant to be: it is unique
//! among *live* processes, which is exactly the set that can collide. A stale
//! file from a dead process with the same id is overwritten by `File::create`,
//! which truncates.

use std::path::PathBuf;

/// A temporary path no other live process will name.
///
/// `name` should describe the fixture, not the crate — the `brutex-` prefix
/// and the process id are added here so every caller gets both.
pub(crate) fn path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("brutex-{}-{name}", std::process::id()))
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
    fn two_names_differ_and_both_carry_this_process_and_never_the_other() {
        let a = path("alpha");
        let b = path("beta");
        assert_ne!(a, b, "two fixtures must not share one path");
        let pid = std::process::id().to_string();
        for p in [&a, &b] {
            let text = p.to_string_lossy().into_owned();
            assert!(
                text.contains(&pid),
                "a fixture that does not name its process can be deleted by another: {text}"
            );
            assert!(
                text.contains("brutex-"),
                "and it must be identifiable: {text}"
            );
        }
        assert_eq!(
            a.parent(),
            Some(std::env::temp_dir().as_path()),
            "fixtures live in the temporary directory and nowhere else"
        );
        // The same name asked for twice is the same path — a test that writes
        // then reads must reach its own file.
        assert_eq!(path("alpha"), a);
    }
}
