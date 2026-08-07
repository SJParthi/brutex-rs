//! Running the binary itself.
//!
//! `crates/api/src/main.rs` is the one file in this crate that a unit test
//! cannot enter: `cargo test` compiles the binary with its own harness and
//! never calls `main`. Every other line of the server therefore lives in the
//! library, and this test exists to cover the call that is left — by actually
//! running the process, with a command that terminates.
//!
//! It is not a smoke test. `docs/04-invariants.md` X-06 requires 100% line and
//! branch coverage on every crate, with no omit list, and an untested `main`
//! is the difference between a gate that holds and a gate with one exception
//! in it. The coverage harness collects from child processes, so the run below
//! is measured exactly like an in-process test.

// The same exceptions every test module in this workspace takes: a test that
// cannot panic cannot fail, and the lints that forbid panicking exist to keep
// them out of the SERVER, not out of its tests.
#![allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]

use std::process::Command;

const GROWW_HEAD: &str = "exchange,segment,underlying_symbol,trading_symbol,instrument_type,\
                          series,isin,expiry_date,strike_price\n";
const DHAN_HEAD: &str = "EXCH_ID,SEGMENT,ISIN,INSTRUMENT,UNDERLYING_SYMBOL,SYMBOL_NAME,\
                         SERIES,SM_EXPIRY_DATE,STRIKE_PRICE,OPTION_TYPE\n";

/// A directory holding the given vendor masters, named after the test.
///
/// A `None` deletes that vendor's file, so a test can drive the
/// vendor-was-never-read path deliberately rather than by accident.
fn masters(name: &str, groww: Option<&str>, dhan: Option<&str>) -> std::path::PathBuf {
    // THE PROCESS ID IS PART OF THE NAME. A fixed name in the shared temporary
    // directory is a fixture two concurrent test processes both claim, and
    // `crates/api/src/scratch.rs` records the intermittent failure that came
    // of it. This file is an integration test and cannot see that module, so
    // it spells the same rule out.
    let dir = std::env::temp_dir().join(format!("brutex-{}-binary-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (file, body) in [("groww_instruments.csv", groww), ("dhan_scrip.csv", dhan)] {
        let path = dir.join(file);
        match body {
            Some(text) => std::fs::write(&path, text).expect("write"),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    dir
}

/// Runs the binary once and returns its exit code, stdout and stderr.
fn run(dir: &std::path::Path, arg: &str) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_api"))
        .arg(arg)
        .env("BRUTEX_MASTERS", dir)
        .output()
        .expect("the binary must run");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_binary_reports_what_it_read_and_exits_zero() {
    // BOTH masters present and agreeing, which is the only shape that may
    // exit zero. The measured decline is a real bond on series `N2`.
    let dir = masters(
        "report",
        Some(&format!(
            "{GROWW_HEAD}\
             NSE,CASH,,NIFTY,IDX,,NIFTY,,\n\
             NSE,CASH,,RELIANCE,EQ,EQ,INE002A01018,,\n\
             NSE,CASH,,SOMEBOND,EQ,N2,INE121A08PJ0,,\n"
        )),
        Some(&format!(
            "{DHAN_HEAD}\
             NSE,I,NA,INDEX,NIFTY,NIFTY,NA,0001-01-01,,\n\
             NSE,E,INE002A01018,EQUITY,RELIANCE,RELIANCE INDUSTRIES LTD,EQ,,,\n\
             NSE,E,INE121A08PJ0,EQUITY,SOMEBOND,SOME BOND,N2,,,\n"
        )),
    );
    let (code, text, _) = run(&dir, "report");

    assert_eq!(code, Some(0), "a clean read exits zero: {text}");
    assert!(text.starts_with("ok\n"), "{text}");
    assert!(text.contains("groww: 2 kept"), "{text}");
    assert!(text.contains("dhan: 2 kept"), "{text}");
    assert!(text.contains("not an equity listing 1"), "{text}");
    assert!(text.contains("merged: 2 instruments"), "{text}");
    assert!(text.contains("0 isin conflicts"), "{text}");
    assert!(text.contains("0 eligibility conflicts"), "{text}");
}

#[test]
fn the_binary_exits_non_zero_when_a_vendor_was_never_read() {
    // The status line, the exit code and `/health` all used to report success
    // beside `dhan: UNAVAILABLE`, so every machine check went green while half
    // the universe had never been opened. D-0026.
    let dir = masters(
        "unavailable",
        Some(&format!("{GROWW_HEAD}NSE,CASH,,NIFTY,IDX,,NIFTY,,\n")),
        None,
    );
    let (code, text, _) = run(&dir, "report");
    assert_eq!(code, Some(3), "DEGRADED, not success: {text}");
    assert!(text.starts_with("DEGRADED\n"), "{text}");
    assert!(text.contains("dhan: UNAVAILABLE"), "{text}");
    // The work still happened and the output is still real -- it is the
    // universe that is refused, not the run.
    assert!(text.contains("groww: 1 kept"), "{text}");
    assert!(text.contains("merged: 1 instruments"), "{text}");
}

#[test]
fn the_binary_refuses_an_argument_it_does_not_understand() {
    // Exit 2, and a usage line on stderr. A binary that silently started a
    // server on a typo is a binary whose typos are found in production.
    let dir = std::env::temp_dir();
    let (code, _, err) = run(&dir, "--serv");
    assert_eq!(code, Some(2), "a misuse is not a failure");
    assert!(err.contains("unknown argument"), "{err}");
    assert!(err.contains("usage:"), "{err}");
}
