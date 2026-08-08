//! Unit behaviour of the pull crate: `pull::unit::*`.
//!
//! # Every segment in this file is invented
//!
//! `CLAUDE.md` §8 and CI gate 1c: no literal parameter path appears in any
//! tracked file, and a test is a tracked file. `orgone`, `testenv`, `vendorone`
//! and `fieldone` are not anybody's path segments — they are chosen so that the
//! assembled paths this file asserts on are safe to publish, and so that gate
//! 1c's pattern (`/<something>/<a real environment name>/`) cannot match them.
//! A test written against the real configuration would leak it exactly as
//! effectively as a constant would.

// The same exceptions every test module in this workspace takes: a test that
// cannot panic cannot fail, and the lints that forbid panicking exist to keep
// them out of the crate, not out of its tests.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};

use brutex_core::instrument::{Exchange, Segment};
use brutex_core::symbol::Symbol;
use brutex_core::vendor::Vendor;
use store::crc::crc32c;
use store::path::{PathError, Timeframe, YearMonth};

use pull::config::{
    CONFIG_DIR, CONFIG_FILE, ConfigError, CredentialConfig, MAX_FIELDS, MAX_FILE_BYTES,
    MAX_LINE_BYTES, MAX_PATH_LEN, MAX_SEGMENT_LEN, REGION, SECRET_LIKE_LEN, SegmentFault,
    check_segment, default_config_path,
};
use pull::manifest::{
    APPEND_HEADROOM_FACTOR, Append, Commit, Entry, EntryFault, EntryKey, FORMAT_VERSION,
    HEADER_LEN, IMAGE_LEN, MAGIC, MAX_ENTRIES, Manifest, ManifestError, ManifestHeader,
    manifest_path, reservation_for,
};
use pull::rate::{
    DHAN_PER_DAY, DHAN_PER_SECOND, GROWW_PER_MINUTE, GROWW_PER_SECOND_UNVERIFIED, Governor,
    GovernorError, MAX_CEILING, MICROS_PER_DAY, MICROS_PER_MINUTE, MICROS_PER_SECOND, PoolKey,
    Pools, RequestKind, Verdict, WINDOW_COUNT, WindowSpan,
};
use pull::secret::{
    CredentialHalt, CredentialReader, ParameterStore, Secret, SecretError, SecretSource,
    SsmSecretSource,
};

/// [`HEADER_LEN`] as a length, for building test regions.
const HEADER_LEN_USIZE: usize = 32_768;
/// `store::format::SLOT_STRIDE` as a length, for reaching the second slot.
const SLOT_STRIDE_USIZE: usize = 16_384;
const _: () = assert!(HEADER_LEN_USIZE as u64 == HEADER_LEN);
const _: () = assert!(SLOT_STRIDE_USIZE * 2 == HEADER_LEN_USIZE);

// ===========================================================================
// The credential path configuration — part 1
// ===========================================================================

/// A well-formed configuration, in segments nobody uses.
const CONFIG: &str = r#"
# A comment, and a blank line above it.
org    = "orgone"
env    = "testenv"
region = "ap-south-1"

[vendor.groww]
vendor = "vendorone"
fields = ["fieldone", "fieldtwo"]

[vendor.dhan]
vendor = "vendortwo"
fields = ["fieldthree"]
"#;

/// The reference configuration with one line replaced.
fn config_without(line: &str, replacement: &str) -> String {
    assert!(
        CONFIG.contains(line),
        "the reference config has no {line:?}"
    );
    CONFIG.replace(line, replacement)
}

/// A file under the temp directory holding `body`, named after the test.
fn tmp(name: &str, body: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("brutex-pull");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(&path, body).expect("write");
    path
}

/// P-07 — a missing or unreadable configuration halts, and never defaults.
#[test]
fn credential_config_absent_halts() {
    let missing = std::env::temp_dir().join("brutex-pull-there-is-no-such-file.toml");
    let _ = std::fs::remove_file(&missing);

    let refusal = CredentialConfig::load(&missing).expect_err("a missing file must halt");
    match &refusal {
        ConfigError::Unreadable { path, kind } => {
            assert_eq!(path, &missing);
            assert_eq!(*kind, std::io::ErrorKind::NotFound);
        }
        other => panic!("expected Unreadable, got {other:?}"),
    }
    // It names the file it could not read, so this is a halt an operator can
    // act on rather than a silent absence one layer up.
    assert!(
        refusal
            .to_string()
            .contains("brutex-pull-there-is-no-such-file")
    );

    // And there is no other entry point that would have supplied a default:
    // every field of the configuration is private and the only constructors
    // are `load` and `parse`.
    let empty = CredentialConfig::parse("").expect_err("an empty file must halt");
    assert_eq!(
        empty,
        ConfigError::MissingVendor {
            vendor: Vendor::Groww.as_str()
        }
    );
}

/// P-08 — the configuration supplies path segments only.
#[test]
fn credential_config_rejects_secret_value() {
    // A value that passes the byte set and the length bound, and is still
    // plainly a pasted credential rather than a name.
    let pasted = "k7f2p9q1w8e3r5t6y0u4i2o9";
    assert_eq!(pasted.len(), SECRET_LIKE_LEN);
    let text = config_without("env    = \"testenv\"", &format!("env    = \"{pasted}\""));
    assert_eq!(
        CredentialConfig::parse(&text),
        Err(ConfigError::Segment {
            line: 4,
            key: "env",
            fault: SegmentFault::LooksLikeASecret { len: 24 },
        })
    );

    // The checks that do the real work are the length and the byte set, and
    // every credential shape this repository has had in front of it trips one
    // of them first.
    let long = "a".repeat(MAX_SEGMENT_LEN + 1);
    assert_eq!(
        check_segment(&long),
        Err(SegmentFault::TooLong {
            len: MAX_SEGMENT_LEN + 1
        })
    );
    assert_eq!(
        check_segment("AKIAIOSFODNN7EXAMPLE"),
        Err(SegmentFault::NotLowerCase { byte: b'A' })
    );
    assert_eq!(
        check_segment("eyj0.eXAt"),
        Err(SegmentFault::IllegalByte { byte: b'.' })
    );
    assert_eq!(
        check_segment("abc+def/ghi"),
        Err(SegmentFault::IllegalByte { byte: b'+' })
    );
}

/// The backstop is the first byte that is too many, and not one before.
#[test]
fn the_secret_backstop_is_the_first_byte_that_is_too_many() {
    // One byte short of the threshold: a name, however unlikely.
    let short = "k7f2p9q1w8e3r5t6y0u4i2o";
    assert_eq!(short.len(), SECRET_LIKE_LEN - 1);
    assert_eq!(check_segment(short), Ok(()));

    // At the threshold, with a digit and no separator: refused.
    let at = "k7f2p9q1w8e3r5t6y0u4i2o9";
    assert_eq!(at.len(), SECRET_LIKE_LEN);
    assert_eq!(
        check_segment(at),
        Err(SegmentFault::LooksLikeASecret {
            len: SECRET_LIKE_LEN
        })
    );

    // Long, but delimited the way a human-chosen name is.
    assert_eq!(check_segment("k7f2p9q1w8e3-5t6y0u4i2o9"), Ok(()));
    // Long, but with no digit in it at all.
    assert_eq!(check_segment("kfpqwerstyuioasdfghjklzx"), Ok(()));
}

/// Every fault a segment can carry is named, and the order is the documented
/// one.
#[test]
fn every_segment_fault_is_named() {
    assert_eq!(check_segment(""), Err(SegmentFault::Empty));
    assert_eq!(check_segment("."), Err(SegmentFault::Traversal));
    assert_eq!(check_segment(".."), Err(SegmentFault::Traversal));
    assert_eq!(
        check_segment("a/b"),
        Err(SegmentFault::IllegalByte { byte: b'/' })
    );
    assert_eq!(
        check_segment("a\\b"),
        Err(SegmentFault::IllegalByte { byte: b'\\' })
    );
    assert_eq!(
        check_segment("Field"),
        Err(SegmentFault::NotLowerCase { byte: b'F' })
    );
    assert_eq!(check_segment("field-one_2"), Ok(()));

    // An over-long segment is reported as over-long, not as whatever its first
    // byte happens to be: the checks run longest-reach first.
    let long_and_miscased = "A".repeat(MAX_SEGMENT_LEN + 1);
    assert_eq!(
        check_segment(&long_and_miscased),
        Err(SegmentFault::TooLong {
            len: MAX_SEGMENT_LEN + 1
        })
    );

    // Each fault renders something different.
    let faults = [
        SegmentFault::Empty,
        SegmentFault::TooLong { len: 99 },
        SegmentFault::Traversal,
        SegmentFault::IllegalByte { byte: b'/' },
        SegmentFault::NotLowerCase { byte: b'A' },
        SegmentFault::LooksLikeASecret { len: 24 },
    ];
    let rendered: HashSet<String> = faults.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), faults.len());
    for fault in faults {
        assert!(!fault.to_string().is_empty());
        assert!(!format!("{fault:?}").is_empty());
    }
}

/// P-12 — a path exists only for segments the configuration actually carries.
#[test]
fn the_only_path_is_the_one_the_configuration_assembles() {
    let config = CredentialConfig::parse(CONFIG).expect("the reference config parses");
    assert_eq!(config.region(), REGION);
    assert_eq!(
        config.fields(Vendor::Groww).expect("groww"),
        ["fieldone".to_owned(), "fieldtwo".to_owned()]
    );
    assert_eq!(
        config.fields(Vendor::Dhan).expect("dhan"),
        ["fieldthree".to_owned()]
    );

    let path = config
        .path_for(Vendor::Groww, "fieldtwo")
        .expect("a configured field");
    assert_eq!(path.to_string(), "/orgone/testenv/vendorone/fieldtwo");
    assert_eq!(path.field(), "fieldtwo");

    // The vendor segment is the one the file gives, not the name this build
    // knows the vendor by — that separation is why the key exists at all.
    assert_eq!(
        config
            .path_for(Vendor::Dhan, "fieldthree")
            .expect("a configured field")
            .to_string(),
        "/orgone/testenv/vendortwo/fieldthree"
    );

    // A field nobody configured is a refusal, never an assembled guess.
    assert_eq!(
        config.path_for(Vendor::Groww, "fieldthree"),
        Err(ConfigError::UnknownField {
            vendor: Vendor::Groww.as_str()
        })
    );

    // The impls this type carries are exercised, so the coverage gate is
    // measuring them rather than reporting on code nothing calls.
    let twin = config.clone();
    assert_eq!(config, twin);
    assert_eq!(path, path);
}

/// P-18 — no formatter renders a path segment. `Debug` is a redaction on the
/// path, on the whole configuration and on a vendor's table.
///
/// This test asserted the opposite until D-0036: it required
/// `format!("{config:?}")` to *contain* `orgone`, certifying the leak. The
/// segments here are invented, so what is being checked is the shape of the
/// rendering rather than the safety of these particular five bytes — a derived
/// `Debug` prints whatever is in the field, and in a real deployment that is
/// the operator's org, env and vendor path segment.
#[test]
fn no_formatter_renders_a_path_segment() {
    let config = CredentialConfig::parse(CONFIG).expect("the reference config parses");
    let path = config
        .path_for(Vendor::Groww, "fieldtwo")
        .expect("a configured field");

    let rendered = format!("{path:?}");
    for segment in ["orgone", "testenv", "vendorone"] {
        assert!(
            !rendered.contains(segment),
            "the path's Debug leaked {segment}: {rendered}"
        );
    }
    // The lengths and the shape are there, and so is the field name — the one
    // segment a halt already carries, so an operator can tell which credential
    // of a vendor's four failed.
    assert_eq!(
        rendered,
        "CredentialPath(/<org:6>/<env:7>/<vendor:9>/fieldtwo)"
    );

    let rendered = format!("{config:?}");
    for segment in ["orgone", "testenv", "vendorone", "vendortwo"] {
        assert!(
            !rendered.contains(segment),
            "the configuration's Debug leaked {segment}: {rendered}"
        );
    }
    // The region is rendered because `CLAUDE.md` §8 publishes it in the law
    // itself, and the vendor enum because `crates/core` has tracked those names
    // since its first commit. Neither is a path segment this repository is
    // keeping.
    assert_eq!(
        rendered,
        "CredentialConfig { org: <6 bytes>, env: <7 bytes>, region: ap-south-1, \
         vendors: [VendorPaths { vendor: groww, segment: <9 bytes>, \
         fields: [\"fieldone\", \"fieldtwo\"] }, \
         VendorPaths { vendor: dhan, segment: <9 bytes>, fields: [\"fieldthree\"] }] }"
    );

    // And the assembled path is still reachable, through the one audited exit.
    assert_eq!(path.to_string(), "/orgone/testenv/vendorone/fieldtwo");
}

/// P-14 — the declared path bound is tight, not merely sufficient.
#[test]
fn a_maximal_path_is_exactly_the_declared_bound() {
    // Delimited so the secret backstop does not fire; exactly at the cap.
    let widest = format!("{}-{}", "a".repeat(MAX_SEGMENT_LEN - 5), "9876");
    assert_eq!(widest.len(), MAX_SEGMENT_LEN);
    assert_eq!(check_segment(&widest), Ok(()));

    let text = format!(
        "org = \"{widest}\"\n\
         env = \"{widest}\"\n\
         region = \"{REGION}\"\n\
         [vendor.groww]\n\
         vendor = \"{widest}\"\n\
         fields = [\"{widest}\"]\n\
         [vendor.dhan]\n\
         vendor = \"{widest}\"\n\
         fields = [\"{widest}\"]\n"
    );
    let config = CredentialConfig::parse(&text).expect("maximal segments parse");
    let path = config.path_for(Vendor::Groww, &widest).expect("the field");
    assert_eq!(path.to_string().len(), MAX_PATH_LEN);
}

/// P-15 — every line this reader does not understand is a halt.
#[test]
fn every_line_this_reader_does_not_know_is_a_halt() {
    let cases: [(String, ConfigError); 13] = [
        (
            config_without("org    = \"orgone\"", "org orgone"),
            ConfigError::Unparseable { line: 3 },
        ),
        (
            config_without("org    = \"orgone\"", "org = orgone"),
            ConfigError::Unparseable { line: 3 },
        ),
        (
            config_without("org    = \"orgone\"", "org = \"org\"one\""),
            ConfigError::Unparseable { line: 3 },
        ),
        (
            config_without("[vendor.groww]", "[vendor.groww"),
            ConfigError::UnknownTable { line: 7 },
        ),
        (
            config_without("[vendor.groww]", "[something]"),
            ConfigError::UnknownTable { line: 7 },
        ),
        (
            config_without("[vendor.groww]", "[vendor.nobody]"),
            ConfigError::UnknownVendor { line: 7 },
        ),
        (
            config_without("org    = \"orgone\"", "orgg = \"orgone\""),
            ConfigError::UnknownKey { line: 3 },
        ),
        (
            config_without("vendor = \"vendorone\"", "orgg = \"orgone\""),
            ConfigError::UnknownKey { line: 8 },
        ),
        (
            config_without("org    = \"orgone\"", "org = [\"orgone\"]"),
            ConfigError::WrongShape {
                line: 3,
                key: "org",
            },
        ),
        (
            config_without(
                "fields = [\"fieldone\", \"fieldtwo\"]",
                "fields = \"fieldone\"",
            ),
            ConfigError::WrongShape {
                line: 9,
                key: "fields",
            },
        ),
        (
            config_without(
                "fields = [\"fieldone\", \"fieldtwo\"]",
                "fields = [fieldone]",
            ),
            ConfigError::Unparseable { line: 9 },
        ),
        (
            config_without("fields = [\"fieldone\", \"fieldtwo\"]", "fields = [\"a\""),
            ConfigError::Unparseable { line: 9 },
        ),
        (
            config_without("org    = \"orgone\"", "org = \"orgone\"\norg = \"orgtwo\""),
            ConfigError::DuplicateKey {
                line: 4,
                key: "org",
            },
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(CredentialConfig::parse(&text), Err(expected));
    }
}

/// A vendor table's own keys get the same treatment its file does.
#[test]
fn a_vendor_tables_own_keys_are_checked_too() {
    // Held to the same two checks the top-level keys are: the shape of the
    // value, and then the segment itself.
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "vendor = \"vendorone\"",
            "vendor = [\"vendorone\"]"
        )),
        Err(ConfigError::WrongShape {
            line: 8,
            key: "vendor"
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "vendor = \"vendorone\"",
            "vendor = \"Vendorone\""
        )),
        Err(ConfigError::Segment {
            line: 8,
            key: "vendor",
            fault: SegmentFault::NotLowerCase { byte: b'V' }
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "fields = [\"fieldone\", \"fieldtwo\"]",
            "fields = [\"fieldone\", \"field/two\"]"
        )),
        Err(ConfigError::Segment {
            line: 9,
            key: "fields",
            fault: SegmentFault::IllegalByte { byte: b'/' }
        })
    );

    // A duplicated key inside a vendor table, both of them.
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "vendor = \"vendorone\"",
            "vendor = \"vendorone\"\nvendor = \"vendorx\""
        )),
        Err(ConfigError::DuplicateKey {
            line: 9,
            key: "vendor"
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "fields = [\"fieldone\", \"fieldtwo\"]",
            "fields = [\"fieldone\"]\nfields = [\"fieldtwo\"]"
        )),
        Err(ConfigError::DuplicateKey {
            line: 10,
            key: "fields"
        })
    );

    // A line longer than the reader looks at is refused before it is parsed.
    let long = format!("org = \"{}\"", "a".repeat(MAX_LINE_BYTES));
    assert_eq!(
        CredentialConfig::parse(&long),
        Err(ConfigError::LineTooLong {
            line: 1,
            len: long.len()
        })
    );

    // More fields than the bound admits.
    let many = (0..=MAX_FIELDS)
        .map(|i| format!("\"field{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "fields = [\"fieldone\", \"fieldtwo\"]",
            &format!("fields = [{many}]")
        )),
        Err(ConfigError::TooManyFields {
            line: 9,
            count: MAX_FIELDS + 1
        })
    );
}

/// A table or a key that is absent is a halt, and it names what is missing.
#[test]
fn a_missing_table_or_key_is_a_halt() {
    assert_eq!(
        CredentialConfig::parse(&config_without("org    = \"orgone\"", "")),
        Err(ConfigError::MissingKey { key: "org" })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without("env    = \"testenv\"", "")),
        Err(ConfigError::MissingKey { key: "env" })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without("region = \"ap-south-1\"", "")),
        Err(ConfigError::MissingKey { key: "region" })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "[vendor.dhan]\nvendor = \"vendortwo\"\nfields = [\"fieldthree\"]",
            ""
        )),
        Err(ConfigError::MissingVendor {
            vendor: Vendor::Dhan.as_str()
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "[vendor.dhan]",
            "[vendor.groww]\nvendor = \"vendorx\"\nfields = [\"fieldx\"]\n[vendor.dhan]"
        )),
        Err(ConfigError::DuplicateVendor {
            vendor: Vendor::Groww.as_str()
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without("vendor = \"vendortwo\"\n", "")),
        Err(ConfigError::MissingVendorKey {
            vendor: Vendor::Dhan.as_str(),
            key: "vendor"
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without("fields = [\"fieldthree\"]", "")),
        Err(ConfigError::MissingVendorKey {
            vendor: Vendor::Dhan.as_str(),
            key: "fields"
        })
    );
    assert_eq!(
        CredentialConfig::parse(&config_without("fields = [\"fieldthree\"]", "fields = []")),
        Err(ConfigError::NoFields {
            vendor: Vendor::Dhan.as_str()
        })
    );

    // A table is closed by the NEXT table header as well as by the end of the
    // file, and an incomplete one is refused at whichever of the two arrives.
    // Removing a key from the LAST table only ever exercised the second.
    assert_eq!(
        CredentialConfig::parse(&config_without("fields = [\"fieldone\", \"fieldtwo\"]", "")),
        Err(ConfigError::MissingVendorKey {
            vendor: Vendor::Groww.as_str(),
            key: "fields"
        })
    );
}

/// P-16 — the region is checked against the one `CLAUDE.md` fixes, not merely
/// read.
#[test]
fn the_region_is_checked_rather_than_merely_read() {
    assert_eq!(
        CredentialConfig::parse(&config_without(
            "region = \"ap-south-1\"",
            "region = \"us-east-1\""
        )),
        Err(ConfigError::WrongRegion { line: 5 })
    );
}

/// P-17 — the file is bounded **at the read**, and both bounds are named.
#[test]
fn the_configuration_file_is_bounded_before_it_is_read() {
    // The bound is a number, and it is pinned. An expression nothing asserts on
    // can be edited into a different number by anything that compiles.
    assert_eq!(MAX_FILE_BYTES, 65_536);

    let good = tmp("good", CONFIG.as_bytes());
    let config = CredentialConfig::load(&good).expect("the reference config loads from disk");
    assert_eq!(config.region(), REGION);

    let over = usize::try_from(MAX_FILE_BYTES).unwrap() + 1;
    let big = tmp("big", &vec![b'#'; over]);
    assert_eq!(
        CredentialConfig::load(&big),
        Err(ConfigError::TooLarge {
            at_least: MAX_FILE_BYTES + 1
        })
    );

    // EXACTLY at the bound is not over it. Without this case the comparison's
    // sign is free: `>` and `>=` pass every other test in this file. Padded
    // with blank lines rather than with one enormous comment, so that what is
    // being tested is the file bound and not the line bound.
    let mut exact = CONFIG.as_bytes().to_vec();
    exact.resize(over - 1, b'\n');
    assert_eq!(exact.len(), usize::try_from(MAX_FILE_BYTES).unwrap());
    let at = tmp("atthebound", &exact);
    assert_eq!(
        CredentialConfig::load(&at)
            .expect("a file exactly at the bound is read")
            .region(),
        REGION
    );

    // Present, small enough, and not text. The read succeeded and the bytes are
    // not a string, which is a different arm.
    let raw = tmp("notutf8", &[0xF0, 0x28, 0x8C, 0x28]);
    match CredentialConfig::load(&raw) {
        Err(ConfigError::Unreadable { kind, .. }) => {
            assert_eq!(kind, std::io::ErrorKind::InvalidData);
        }
        other => panic!("expected an unreadable file, got {other:?}"),
    }
}

/// P-19 — anything that is not a regular file is refused by name, before it is
/// opened.
///
/// The bound used to be `metadata(path).len()`, and for a FIFO, a character
/// device or a procfs-style file that number is `0`: the check passed and the
/// read that followed was unbounded. `/dev/zero` grew the string until the
/// allocator gave up, and a FIFO blocked in `open` — the two failures the
/// comment above `MAX_FILE_BYTES` said the check existed to prevent. Now the
/// read is bounded by `take`, and a path that is not a regular file never
/// reaches it.
///
/// **There is no FIFO case here, and that is a limit rather than an omission.**
/// Creating one needs `mkfifo`, which is either an external process or a libc
/// binding, and `CLAUDE.md` §2 forbids both — so this drives the same code path
/// through the two non-regular files that can be reached from `std` alone. The
/// refusal is on `!is_file()`, which is one branch for all of them: a FIFO
/// cannot take a different path through this function than a device does.
#[test]
fn a_path_that_is_not_a_regular_file_is_refused_by_name() {
    // A directory: `metadata` succeeds, its length is not the length of
    // anything readable, and this is the one non-regular file every platform
    // this builds for has.
    let dir = std::env::temp_dir().join("brutex-pull");
    std::fs::create_dir_all(&dir).expect("mkdir");
    assert_eq!(
        CredentialConfig::load(&dir),
        Err(ConfigError::NotARegularFile { path: dir.clone() })
    );
    // It names the path, so the operator is told what is wrong rather than
    // being handed an OOM kill or a process that never returns.
    let rendered = ConfigError::NotARegularFile { path: dir }.to_string();
    assert!(rendered.contains("brutex-pull") && rendered.contains("not a regular file"));

    // A character device, where the platform has one. `/dev/zero` measures
    // `st_size == 0` and is an endless stream of NUL bytes, every one of which
    // is valid UTF-8: under the old check it was an unbounded read of an
    // infinite file.
    let zero = std::path::Path::new("/dev/zero");
    if zero.exists() {
        assert_eq!(
            CredentialConfig::load(zero),
            Err(ConfigError::NotARegularFile {
                path: zero.to_path_buf()
            })
        );
    }
}

/// The configuration path is built under the home it is handed.
#[test]
fn the_config_path_is_under_the_home_it_is_given() {
    let home = std::path::Path::new("/somewhere/else");
    let path = default_config_path(home);
    assert_eq!(
        path,
        home.join(CONFIG_DIR).join(CONFIG_FILE),
        "the path is the home, the directory and the file, and nothing else"
    );
}

/// Every refusal renders something, and no two render the same thing.
#[test]
fn every_config_error_prints_something_distinct() {
    let errors = [
        ConfigError::Unreadable {
            path: std::path::PathBuf::from("/x"),
            kind: std::io::ErrorKind::NotFound,
        },
        ConfigError::NotARegularFile {
            path: std::path::PathBuf::from("/y"),
        },
        ConfigError::TooLarge { at_least: 1 },
        ConfigError::LineTooLong { line: 1, len: 2 },
        ConfigError::Unparseable { line: 1 },
        ConfigError::UnknownTable { line: 1 },
        ConfigError::UnknownVendor { line: 1 },
        ConfigError::UnknownKey { line: 1 },
        ConfigError::DuplicateKey {
            line: 1,
            key: "org",
        },
        ConfigError::DuplicateVendor { vendor: "groww" },
        ConfigError::WrongShape {
            line: 1,
            key: "org",
        },
        ConfigError::TooManyFields { line: 1, count: 2 },
        ConfigError::MissingKey { key: "org" },
        ConfigError::MissingVendorKey {
            vendor: "groww",
            key: "vendor",
        },
        ConfigError::MissingVendor { vendor: "groww" },
        ConfigError::NoFields { vendor: "groww" },
        ConfigError::WrongRegion { line: 1 },
        ConfigError::Segment {
            line: 1,
            key: "org",
            fault: SegmentFault::Empty,
        },
        ConfigError::UnknownField { vendor: "groww" },
    ];
    let rendered: HashSet<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), errors.len(), "two refusals read the same");
    for error in &errors {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").is_empty());
        assert_eq!(error.clone(), *error);
        let _: &dyn std::error::Error = error;
    }
}

// ===========================================================================
// Reading the credential — part 2
// ===========================================================================

/// A secret source that records every read and **panics** on any write.
///
/// This is the whole of P-05's proof. The real SSM client offers
/// `put_parameter`; this double offers one too, and it cannot be called without
/// failing the test. So "no write happens" is not asserted from the absence of
/// a line in the source — it is asserted from a process that would have died.
struct Double {
    answers: RefCell<VecDeque<Result<String, SecretError>>>,
    reads: Cell<usize>,
    writes: Cell<usize>,
    decrypted: Cell<bool>,
    last_name: RefCell<String>,
}

impl Double {
    fn new(answers: Vec<Result<String, SecretError>>) -> Self {
        Self {
            answers: RefCell::new(answers.into()),
            reads: Cell::new(0),
            writes: Cell::new(0),
            decrypted: Cell::new(false),
            last_name: RefCell::new(String::new()),
        }
    }

    /// The write the real client offers and this repository never calls.
    fn put_parameter(&self, _name: &str, _value: &str) -> ! {
        self.writes.set(self.writes.get() + 1);
        panic!("a write reached the parameter store; this repository never mints a token");
    }
}

impl ParameterStore for Double {
    fn get_parameter(&self, name: &str, with_decryption: bool) -> Result<String, SecretError> {
        self.reads.set(self.reads.get() + 1);
        self.decrypted.set(with_decryption);
        name.clone_into(&mut self.last_name.borrow_mut());
        self.answers
            .borrow_mut()
            .pop_front()
            .unwrap_or(Err(SecretError::NotFound))
    }
}

/// The reference configuration and one path out of it.
fn configured() -> CredentialConfig {
    CredentialConfig::parse(CONFIG).expect("the reference config parses")
}

/// P-05 — a credential is read, never written; no token is ever minted.
#[test]
fn readonly_credentials() {
    let config = configured();
    let path = config.path_for(Vendor::Groww, "fieldone").expect("a field");
    let source = SsmSecretSource::new(Double::new(vec![Ok("a-value".to_owned())]), config.region());
    let reader = CredentialReader::new(source);

    let secret = reader.read(Vendor::Groww, &path).expect("the value");
    assert_eq!(secret.expose(), "a-value");

    let double = reader.source().client();
    assert_eq!(double.reads.get(), 1, "exactly one read");
    assert_eq!(
        double.writes.get(),
        0,
        "a whole credential read reached the store without one write"
    );

    // And the double is not a no-op: calling its write really does fail, so
    // the assertion above is a statement about the code and not about a stub
    // that would have passed either way.
    let attempted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        double.put_parameter("anything", "anything");
    }));
    assert!(attempted.is_err(), "the double must panic on a write");
    assert_eq!(double.writes.get(), 1, "the write it panicked on was seen");
}

/// P-06 — an auth failure halts the pull loudly rather than degrading.
#[test]
fn auth_halt() {
    let config = configured();
    let path = config.path_for(Vendor::Groww, "fieldone").expect("a field");
    let source = SsmSecretSource::new(
        Double::new(vec![Err(SecretError::AccessDenied)]),
        config.region(),
    );
    let reader = CredentialReader::new(source);

    let halt = reader
        .read(Vendor::Groww, &path)
        .expect_err("access denied must halt");
    assert_eq!(
        halt,
        CredentialHalt::Refused {
            vendor: Vendor::Groww,
            field: "fieldone".to_owned(),
            cause: SecretError::AccessDenied,
        }
    );
    // It names the vendor and the field, and it names neither the value nor
    // the assembled path.
    let rendered = halt.to_string();
    assert!(rendered.contains("groww") && rendered.contains("fieldone"));
    assert!(!rendered.contains("orgone") && !rendered.contains("vendorone"));

    // There is no second, quieter answer: the same call twice halts twice.
    assert!(reader.read(Vendor::Groww, &path).is_err());
}

/// A dead token is re-read once and then the pull stops. It is never minted.
#[test]
fn a_dead_token_is_re_read_once_and_then_halts() {
    let config = configured();
    let path = config.path_for(Vendor::Groww, "fieldone").expect("a field");

    // The rotation has not landed: Parameter Store still holds the value the
    // vendor just rejected.
    let dead = Secret::new("stale".to_owned()).expect("a value");
    let reader = CredentialReader::new(SsmSecretSource::new(
        Double::new(vec![Ok("stale".to_owned())]),
        config.region(),
    ));
    assert_eq!(
        reader.reread_after_rejection(Vendor::Groww, &path, &dead),
        Err(CredentialHalt::DeadToken {
            vendor: Vendor::Groww,
            field: "fieldone".to_owned(),
        })
    );
    assert_eq!(reader.source().client().reads.get(), 1);
    assert_eq!(reader.source().client().writes.get(), 0);

    // The rotation did land: the fresh value is returned and nothing halts.
    let reader = CredentialReader::new(SsmSecretSource::new(
        Double::new(vec![Ok("fresh".to_owned())]),
        config.region(),
    ));
    let fresh = reader
        .reread_after_rejection(Vendor::Groww, &path, &dead)
        .expect("a rotated value");
    assert_eq!(fresh.expose(), "fresh");

    // The re-read itself failing is that failure's halt, unchanged.
    let reader = CredentialReader::new(SsmSecretSource::new(
        Double::new(vec![Err(SecretError::Unreachable)]),
        config.region(),
    ));
    assert_eq!(
        reader.reread_after_rejection(Vendor::Groww, &path, &dead),
        Err(CredentialHalt::Refused {
            vendor: Vendor::Groww,
            field: "fieldone".to_owned(),
            cause: SecretError::Unreachable,
        })
    );
}

/// The adapter always asks for a decrypted value, and renders the whole path.
#[test]
fn the_adapter_always_asks_for_a_decrypted_value() {
    let config = configured();
    let path = config
        .path_for(Vendor::Dhan, "fieldthree")
        .expect("a field");
    let source = SsmSecretSource::new(Double::new(vec![Ok("v".to_owned())]), config.region());
    assert_eq!(source.region(), REGION);

    source.read(&path).expect("the value");
    assert!(
        source.client().decrypted.get(),
        "with_decryption is true, always: false returns the ciphertext"
    );
    assert_eq!(
        *source.client().last_name.borrow(),
        "/orgone/testenv/vendortwo/fieldthree"
    );

    // A parameter that exists and holds nothing is refused rather than passed
    // on as a credential.
    let empty = SsmSecretSource::new(Double::new(vec![Ok(String::new())]), config.region());
    assert_eq!(empty.read(&path), Err(SecretError::Empty));
    assert_eq!(Secret::new(String::new()), Err(SecretError::Empty));
}

/// A secret never prints its value, and says only how long it is.
#[test]
fn a_secret_never_prints_its_value() {
    let secret = Secret::new("hunter2-and-then-some".to_owned()).expect("a value");
    assert_eq!(secret.byte_len(), 21);
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert_eq!(rendered, "Secret(<redacted, 21 bytes>)");

    // Equality is over the value, because that is the dead-token mechanism.
    assert_eq!(secret.clone(), secret);
    assert_ne!(secret, Secret::new("something else".to_owned()).unwrap());
}

/// Every refusal and halt renders something, and no two render the same thing.
#[test]
fn every_secret_error_prints_something_distinct() {
    // `SecretError::NotDecrypted` was here until D-0036, and nothing in
    // `crates/pull/src` ever constructed it: it documented a ciphertext check
    // that did not exist. Recognising KMS ciphertext is an external fact with
    // no source recorded in `docs/00-charter.md`, so the variant went rather
    // than the doc comment staying, and `docs/06-limits.md` §19 records what
    // that leaves unprotected.
    let errors = [
        SecretError::AccessDenied,
        SecretError::NotFound,
        SecretError::Empty,
        SecretError::Unreachable,
    ];
    let rendered: HashSet<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), errors.len());
    for error in errors {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").is_empty());
        let _: &dyn std::error::Error = &error;
    }

    let halts = [
        CredentialHalt::Refused {
            vendor: Vendor::Groww,
            field: "fieldone".to_owned(),
            cause: SecretError::NotFound,
        },
        CredentialHalt::DeadToken {
            vendor: Vendor::Dhan,
            field: "fieldtwo".to_owned(),
        },
    ];
    let rendered: HashSet<String> = halts.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), halts.len());
    for halt in &halts {
        assert!(!halt.to_string().is_empty());
        assert!(!format!("{halt:?}").is_empty());
        assert_eq!(halt.clone(), *halt);
        let _: &dyn std::error::Error = halt;
    }
}

// ===========================================================================
// The manifest — part 3
// ===========================================================================

/// One entry, in the shape a NIFTY month has.
fn entry(symbol: &str, year: u16, month: u8, rows: u64, first: i64, last: i64) -> Entry {
    Entry {
        key: EntryKey {
            exchange: Exchange::Nse,
            segment: Segment::Index,
            symbol: Symbol::new(symbol).expect("a symbol"),
            timeframe: Timeframe::MINUTE_1,
            month: YearMonth::new(year, month).expect("a month"),
        },
        rows,
        first_ts_micros: first,
        last_ts_micros: last,
    }
}

/// Recomputes an image's checksum after a test has edited its bytes.
fn reseal(image: &mut [u8; IMAGE_LEN]) {
    let crc = crc32c(&image[..60]);
    image[60..64].copy_from_slice(&crc.to_le_bytes());
}

/// A header region with the given commits written into their own slots.
fn region(commits: &[Commit]) -> Vec<u8> {
    let mut bytes = vec![0u8; HEADER_LEN_USIZE];
    for commit in commits {
        let at = commit.offset as usize;
        bytes[at..at + IMAGE_LEN].copy_from_slice(&commit.bytes);
    }
    bytes
}

/// A manifest for a vendor whose file is empty — the only way to a genesis one.
fn fresh(vendor: Vendor) -> Manifest {
    Manifest::open(vendor, &[], &[]).expect("an empty file is a genesis manifest")
}

/// Builds a manifest on paper: the header region, the entry region, and the
/// in-memory state a writer would hold.
fn built(vendor: Vendor, entries: &[Entry]) -> (Vec<u8>, Vec<u8>, Manifest) {
    let mut manifest = fresh(vendor);
    let genesis = ManifestHeader::genesis(vendor)
        .commit()
        .expect("the genesis commit");
    let mut commits = vec![genesis];
    let mut data: Vec<u8> = Vec::new();
    for entry in entries {
        let append: Append = manifest.record(*entry).expect("a record");
        assert_eq!(append.offset, HEADER_LEN + append.ordinal * 64);
        assert_eq!(append.offset as usize, HEADER_LEN_USIZE + data.len());
        data.extend_from_slice(&append.bytes);
        assert_eq!(
            append.commit.durable_through as usize,
            HEADER_LEN_USIZE + data.len(),
            "the writer is told exactly how far to flush before it publishes"
        );
        commits.push(append.commit);
    }
    (region(&commits), data, manifest)
}

/// M-01 — an entry survives its own image, field for field.
#[test]
fn an_entry_round_trips_through_its_image() {
    for (exchange, segment) in [
        (Exchange::Nse, Segment::Index),
        (Exchange::Nse, Segment::Cash),
        (Exchange::Bse, Segment::Fno),
    ] {
        let mut original = entry("BANKNIFTY", 2024, 6, 7_312, 1_000, 2_000);
        original.key.exchange = exchange;
        original.key.segment = segment;
        let image = original.image();
        assert_eq!(image.len(), IMAGE_LEN);
        assert_eq!(Entry::decode(&image), Ok(original));
    }

    // The derived impls this type carries are exercised.
    let one = entry("NIFTY", 2024, 1, 1, 0, 0);
    assert_eq!(one, one);
    assert!(format!("{one:?}").contains("NIFTY"));
    assert_eq!(one.key, one.key);
    assert!(format!("{:?}", one.key).contains("Nse"));
    assert!(one.key < entry("NIFTY", 2024, 2, 1, 0, 0).key);
}

/// M-02 — the checksum covers bytes `0..60` and nothing else.
///
/// Pinned against a hardcoded number rather than against another call of the
/// same function: a checksum that agrees with itself always agrees with itself,
/// so widening or narrowing the domain would pass every round-trip test ever
/// written while making every image already on disk unreadable.
#[test]
fn the_covered_domain_is_the_image_minus_its_checksum() {
    let image = entry(
        "NIFTY",
        2024,
        6,
        7_312,
        1_717_200_000_000_000,
        1_719_791_940_000_000,
    )
    .image();
    let stored = u32::from_le_bytes(image[60..64].try_into().unwrap());
    assert_eq!(
        stored,
        crc32c(&image[..60]),
        "the stored checksum is the checksum of the first 60 bytes"
    );
    assert_ne!(
        stored,
        crc32c(&image[..]),
        "and it is NOT the checksum of all 64 — that would be a different number"
    );
    assert_ne!(stored, crc32c(&image[..56]));
}

/// M-03 — a flipped bit anywhere in an entry is detected.
#[test]
fn a_flipped_bit_in_any_entry_byte_is_detected() {
    let original = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let image = original.image();
    let mut caught = 0;
    for byte in 0..IMAGE_LEN {
        for bit in 0..8u8 {
            let mut damaged = image;
            damaged[byte] ^= 1 << bit;
            if damaged == image {
                continue;
            }
            match Entry::decode(&damaged) {
                Err(EntryFault::Checksum { .. }) => caught += 1,
                other => panic!("byte {byte} bit {bit} was not caught: {other:?}"),
            }
        }
    }
    assert_eq!(caught, IMAGE_LEN * 8, "every one of the 512 flips");
}

/// Every way an entry's bytes can fail to be an entry is named.
#[test]
fn an_entry_that_is_not_an_entry_is_named() {
    let good = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000).image();

    assert_eq!(
        Entry::decode(&good[..IMAGE_LEN - 1]),
        Err(EntryFault::TooShort { len: IMAGE_LEN - 1 })
    );

    let mut damaged = good;
    damaged[24] ^= 0xFF;
    match Entry::decode(&damaged) {
        Err(EntryFault::Checksum { stored, computed }) => assert_ne!(stored, computed),
        other => panic!("expected a checksum refusal, got {other:?}"),
    }

    // Each field, edited and re-sealed, so the checksum is not what refuses it.
    let edit = |f: &dyn Fn(&mut [u8; IMAGE_LEN])| -> EntryFault {
        let mut image = good;
        f(&mut image);
        reseal(&mut image);
        Entry::decode(&image).expect_err("this entry must be refused")
    };

    assert_eq!(edit(&|i| i[0] = 0xFF), EntryFault::SymbolNotUtf8);
    assert_eq!(
        edit(&|i| i[0..5].copy_from_slice(b"nifty")),
        EntryFault::SymbolNotCanonical
    );
    assert!(matches!(
        edit(&|i| i[0..5].copy_from_slice(b"NIF*Y")),
        EntryFault::BadSymbol(_)
    ));
    assert!(matches!(
        edit(&|i| i[0..5].copy_from_slice(&[0, 0, 0, 0, 0])),
        EntryFault::BadSymbol(_)
    ));
    assert_eq!(
        edit(&|i| i[48..52].copy_from_slice(&300u32.to_le_bytes())),
        EntryFault::UnknownTimeframe { secs: 300 }
    );
    assert_eq!(
        edit(&|i| i[54] = 13),
        EntryFault::BadMonth(PathError::MonthOutOfRange { month: 13 })
    );
    assert_eq!(
        edit(&|i| i[55] = 9),
        EntryFault::UnknownExchange { code: 9 }
    );
    assert_eq!(edit(&|i| i[56] = 9), EntryFault::UnknownSegment { code: 9 });
    assert_eq!(
        edit(&|i| i[24..32].copy_from_slice(&0u64.to_le_bytes())),
        EntryFault::EmptyEntry
    );
    assert_eq!(
        edit(&|i| i[40..48].copy_from_slice(&0i64.to_le_bytes())),
        EntryFault::TimestampsOutOfOrder {
            first: 1_000,
            last: 0
        }
    );
}

/// M-09 — the exchange and segment codes on disk are frozen.
///
/// Hardcoded, not derived from the enum: `CLAUDE.md` §3 rule 8 makes a written
/// number append-only, and a test that asked the encoder what it encodes would
/// agree with any renumbering.
#[test]
fn the_exchange_and_segment_codes_are_frozen() {
    let cases = [
        (Exchange::Nse, Segment::Index, 1u8, 1u8),
        (Exchange::Nse, Segment::Cash, 1, 2),
        (Exchange::Bse, Segment::Fno, 2, 3),
    ];
    for (exchange, segment, exchange_code, segment_code) in cases {
        let mut one = entry("NIFTY", 2024, 6, 1, 0, 0);
        one.key.exchange = exchange;
        one.key.segment = segment;
        let image = one.image();
        assert_eq!(image[55], exchange_code, "{exchange:?}");
        assert_eq!(image[56], segment_code, "{segment:?}");
        assert_eq!(Entry::decode(&image).expect("round trip"), one);
    }
}

/// M-08 — an entry's address is arithmetic, and the ordinal is bounded.
#[test]
fn the_offset_of_an_entry_is_arithmetic() {
    assert_eq!(Manifest::offset_of(0), Ok(HEADER_LEN));
    assert_eq!(Manifest::offset_of(1), Ok(HEADER_LEN + 64));
    assert_eq!(Manifest::offset_of(12_345), Ok(HEADER_LEN + 12_345 * 64));
    assert_eq!(
        Manifest::offset_of(MAX_ENTRIES),
        Ok(HEADER_LEN + MAX_ENTRIES * 64)
    );
    assert_eq!(
        Manifest::offset_of(MAX_ENTRIES + 1),
        Err(ManifestError::OrdinalOutOfRange {
            ordinal: MAX_ENTRIES + 1,
            limit: MAX_ENTRIES
        })
    );
    assert_eq!(
        Manifest::offset_of(u64::MAX),
        Err(ManifestError::OrdinalOutOfRange {
            ordinal: u64::MAX,
            limit: MAX_ENTRIES
        })
    );
}

/// M-11 — the counters are maintained on write and read without a scan.
#[test]
fn the_counters_are_maintained_on_write() {
    let mut manifest = fresh(Vendor::Groww);
    assert_eq!(manifest.entries(), 0);
    assert_eq!(manifest.keys(), 0);
    assert_eq!(manifest.total_rows(), 0);

    let june = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let july = entry("NIFTY", 2024, 7, 7_500, 3_000, 4_000);

    manifest.record(june).expect("june");
    assert_eq!(
        (manifest.entries(), manifest.keys(), manifest.total_rows()),
        (1, 1, 7_312)
    );

    manifest.record(july).expect("july");
    assert_eq!(
        (manifest.entries(), manifest.keys(), manifest.total_rows()),
        (2, 2, 14_812)
    );

    // A re-pull of an existing month is a NEW entry and the same key: entries
    // rise, keys do not, and the total moves by the difference.
    let june_again = entry("NIFTY", 2024, 6, 7_400, 1_000, 2_500);
    manifest.record(june_again).expect("june, again");
    assert_eq!(
        (manifest.entries(), manifest.keys(), manifest.total_rows()),
        (3, 2, 14_900)
    );
    assert_eq!(manifest.entry(&june.key), Some(june_again));
    assert_eq!(manifest.entry(&july.key), Some(july));

    // A key nobody recorded is absent rather than zero.
    assert_eq!(manifest.entry(&entry("SENSEX", 2024, 6, 1, 0, 0).key), None);

    // The derived impls this type carries are exercised.
    let twin = manifest.clone();
    assert_eq!(manifest, twin);
    assert!(format!("{manifest:?}").contains("NIFTY"));
    let header = manifest.header();
    assert_eq!(header, twin.header());
    assert!(format!("{header:?}").contains("Groww"));
    assert_eq!(header.n_valid, 3);
    assert_eq!(header.format_version, FORMAT_VERSION);
    assert_eq!(header.entry_stride, 64);
}

/// An entry that contradicts what a key already holds is refused, on write.
#[test]
fn a_row_count_that_went_backwards_is_refused() {
    let mut manifest = fresh(Vendor::Groww);
    manifest
        .record(entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000))
        .expect("june");

    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 6, 7_311, 1_000, 2_000)),
        Err(ManifestError::RowCountWentBackwards {
            ordinal: 1,
            previous: 7_312,
            next: 7_311
        })
    );
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 6, 7_312, 1_000, 1_999)),
        Err(ManifestError::KeyTimestampsOutOfOrder {
            ordinal: 1,
            previous: 2_000,
            next: 1_999
        })
    );
    // A refused record leaves the counters exactly as they were.
    assert_eq!(
        (manifest.entries(), manifest.keys(), manifest.total_rows()),
        (1, 1, 7_312)
    );

    // An entry that is not internally consistent never reaches the key checks.
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 8, 0, 0, 0)),
        Err(ManifestError::Entry {
            ordinal: 1,
            fault: EntryFault::EmptyEntry
        })
    );
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 8, 1, 5, 4)),
        Err(ManifestError::Entry {
            ordinal: 1,
            fault: EntryFault::TimestampsOutOfOrder { first: 5, last: 4 }
        })
    );
}

/// The row total refuses to wrap, on both the new-key and the existing-key
/// path.
#[test]
fn the_row_total_refuses_to_wrap() {
    let mut manifest = fresh(Vendor::Groww);
    manifest
        .record(entry("NIFTY", 2024, 6, u64::MAX - 1, 0, 1))
        .expect("a very large month");
    manifest
        .record(entry("NIFTY", 2024, 7, 1, 0, 1))
        .expect("one more row");
    assert_eq!(manifest.total_rows(), u64::MAX);

    // A new key would push the total past u64.
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 8, 1, 0, 1)),
        Err(ManifestError::RowTotalOverflow)
    );
    // And so would one more row on a key that already exists.
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 7, 2, 0, 1)),
        Err(ManifestError::RowTotalOverflow)
    );
}

/// A whole manifest survives a round trip through its own bytes.
#[test]
fn a_manifest_round_trips_through_its_regions() {
    let entries = [
        entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000),
        entry("NIFTY", 2024, 7, 7_500, 3_000, 4_000),
        entry("NIFTY", 2024, 6, 7_400, 1_000, 2_500),
    ];
    let (header_region, data, written) = built(Vendor::Groww, &entries);
    let read = Manifest::load(Vendor::Groww, &header_region, &data).expect("it loads");

    assert_eq!(read, written);
    assert_eq!(read.entries(), 4);
    assert_eq!(read.keys(), 3);
    assert_eq!(read.total_rows(), 7_400 + 7_310 + 7_500);
    assert_eq!(read.entry(&entries[0].key), Some(entries[3]));
    assert_eq!(read.entry(&entries[1].key), Some(entries[1]));

    // An empty manifest: the genesis commit alone, and no entries at all.
    let genesis = ManifestHeader::genesis(Vendor::Dhan)
        .commit()
        .expect("genesis");
    let empty = Manifest::load(Vendor::Dhan, &region(&[genesis]), &[]).expect("an empty manifest");
    assert_eq!(
        (empty.entries(), empty.keys(), empty.total_rows()),
        (0, 0, 0)
    );
    assert_eq!(genesis.slot, 0);
    assert_eq!(genesis.offset, 0);
    assert_eq!(genesis.durable_through, HEADER_LEN);
    assert_eq!(genesis, genesis);
    assert!(format!("{genesis:?}").contains("Dhan"));
}

/// M-04 — a torn header commit never reports a count that was not committed.
#[test]
fn a_torn_header_commit_never_reports_an_uncommitted_count() {
    let one = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let (whole, data, _) = built(Vendor::Groww, &[one]);
    // Generation 1 lives in slot 1; generation 0 is the genesis commit in slot 0.
    let published = &whole[16_384..16_384 + IMAGE_LEN];

    for prefix in 0..=IMAGE_LEN {
        let mut region = whole.clone();
        region[16_384..16_384 + IMAGE_LEN].fill(0);
        region[16_384..16_384 + prefix].copy_from_slice(&published[..prefix]);

        let manifest = Manifest::load(Vendor::Groww, &region, &data)
            .unwrap_or_else(|e| panic!("prefix {prefix} was unreadable: {e}"));
        assert!(
            manifest.entries() == 0 || manifest.entries() == 1,
            "prefix {prefix} reported {} entries",
            manifest.entries()
        );
        if prefix == IMAGE_LEN {
            assert_eq!(manifest.entries(), 1, "the whole commit is the new count");
        } else {
            assert_eq!(manifest.entries(), 0, "a partial slot is not a candidate");
        }
    }
}

/// M-05 — a header that became durable before its entries falls back one
/// generation rather than condemning the file, and says that it did.
///
/// The region-is-short case was the only one this test exercised before
/// D-0036, and it is the only one the code handled: the fall-back lived
/// entirely in `validate(capacity)`, where `capacity` is the region's *byte
/// length*. The commonest shape of the crash M-05 names leaves the length
/// right and the bytes wrong — a block allocated and never written back — and
/// that condemned the whole file while the previous generation sat intact in
/// the other slot.
#[test]
fn a_header_published_before_its_entries_falls_back_a_generation() {
    let one = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let (header_region, _, _) = built(Vendor::Groww, &[one]);

    // (a) The commit landed; the entry region is SHORT. The newest slot counts
    // an entry the region cannot support, and the previous generation is
    // intact.
    let recovered = Manifest::load(Vendor::Groww, &header_region, &[]).expect("the fallback");
    assert_eq!(recovered.entries(), 0);
    assert_eq!(recovered.total_rows(), 0);
    assert_eq!(
        recovered.degraded_reason(),
        Some(ManifestError::CounterExceedsRegion {
            n_valid: 1,
            capacity: 0
        }),
        "recovering a generation is never silent"
    );
    assert_eq!(recovered.reserved(), 0, "an empty census reserves nothing");

    // (b) The commit landed; the entry's 64 bytes are PRESENT and were never
    // written back. The region is long enough, `validate` passes, and the
    // entry's own checksum is what fails.
    let two = entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000);
    let (header_region, data, _) = built(Vendor::Groww, &[one, two]);
    let mut torn = data.clone();
    torn[IMAGE_LEN..].fill(0);
    assert_eq!(torn.len(), 2 * IMAGE_LEN, "the region is its full length");

    let recovered = Manifest::load(Vendor::Groww, &header_region, &torn)
        .expect("the previous generation describes the durable prefix");
    assert_eq!(recovered.entries(), 1);
    assert_eq!(recovered.keys(), 1);
    assert_eq!(recovered.total_rows(), 7_312);
    assert_eq!(recovered.entry(&one.key), Some(one));
    assert_eq!(recovered.entry(&two.key), None);
    match recovered.degraded_reason() {
        Some(ManifestError::Entry {
            ordinal: 1,
            fault: EntryFault::Checksum { .. },
        }) => {}
        other => panic!("the recovery must name what it stepped over, got {other:?}"),
    }

    // The same, with a garbage tail rather than a zeroed one.
    let mut garbage = data.clone();
    garbage[IMAGE_LEN..].fill(0xA5);
    assert_eq!(
        Manifest::load(Vendor::Groww, &header_region, &garbage)
            .expect("the fallback")
            .entries(),
        1
    );

    // And with a half-written one: the first 32 bytes are new, the rest is not.
    let mut half = data.clone();
    half[IMAGE_LEN + 32..].fill(0);
    assert_eq!(
        Manifest::load(Vendor::Groww, &header_region, &half)
            .expect("the fallback")
            .entries(),
        1
    );

    // (c) When NO generation survives, it is a refusal — and the refusal is the
    // newest generation's, because that is the state a writer published.
    let mut both_gone = data.clone();
    both_gone.fill(0);
    assert_eq!(
        Manifest::load(Vendor::Groww, &header_region, &both_gone),
        Err(ManifestError::Entry {
            ordinal: 0,
            fault: EntryFault::Checksum {
                stored: 0,
                computed: crc32c(&[0u8; 60])
            }
        })
    );

    // (d) And when neither generation can be supported by the region at all.
    let header = ManifestHeader {
        n_valid: 5,
        n_keys: 1,
        total_rows: 1,
        ..ManifestHeader::genesis(Vendor::Groww)
    };
    let only = header.commit().expect("a commit");
    assert_eq!(
        Manifest::load(Vendor::Groww, &region(&[only]), &[]),
        Err(ManifestError::CounterExceedsRegion {
            n_valid: 5,
            capacity: 0
        })
    );
}

/// A header slot that fails its own checksum is stepped over, and the census
/// that survives says so.
///
/// Until D-0036 `read_region` computed this refusal, stored it in a local, and
/// dropped it the moment the other slot validated. `Manifest::load` then
/// returned `Ok` on a file it had just proved was damaged: a committed month
/// vanished from the index, the row total under-reported by that month, and no
/// API a caller could reach said anything had happened. `CLAUDE.md` §4 —
/// degrade loudly and name the reason, or refuse. Never both silently.
#[test]
fn a_corrupt_header_slot_is_named_by_the_census_that_survives_it() {
    let june = entry("NIFTY", 2024, 1, 100, 1_000, 2_000);
    let july = entry("BANKNIFTY", 2024, 2, 200, 1_000, 2_000);
    let (header_region, data, whole) = built(Vendor::Groww, &[june, july]);
    assert_eq!(whole.header().generation, 2, "generation 2 is in slot 0");
    assert_eq!(whole.degraded_reason(), None, "the writer is not degraded");

    // A clean load reports both months and nothing stepped over.
    let clean = Manifest::load(Vendor::Groww, &header_region, &data).expect("a clean load");
    assert_eq!(
        (clean.entries(), clean.keys(), clean.total_rows()),
        (2, 2, 300)
    );
    assert_eq!(clean.degraded_reason(), None);

    // One bit, in the newest slot.
    let mut damaged = header_region.clone();
    damaged[40] ^= 1;
    let recovered = Manifest::load(Vendor::Groww, &damaged, &data).expect("the older generation");
    assert_eq!(
        (
            recovered.entries(),
            recovered.keys(),
            recovered.total_rows()
        ),
        (1, 1, 100),
        "the census really is smaller — which is exactly why it must be said"
    );
    match recovered.degraded_reason() {
        Some(ManifestError::SlotChecksum { stored, computed }) => assert_ne!(stored, computed),
        other => panic!("expected the slot checksum to be reported, got {other:?}"),
    }
    assert!(
        recovered
            .degraded_reason()
            .expect("a reason")
            .to_string()
            .contains("header slot checksum"),
        "the reason an operator reads names the slot"
    );
}

/// M-14 — a genesis census exists only for a file with nothing in it.
///
/// `Manifest::genesis` was public, and a writer that called it on a vendor
/// which already had a manifest produced a census that was silently wrong and
/// still loaded clean: the stale slot 0 wins on generation, the fresh
/// generation-1 commit lands in slot 1, entry 0 is overwritten, and because
/// real index months share a row count the recomputed totals still agree.
#[test]
fn the_only_genesis_is_an_empty_file() {
    // Four months of four instruments, every one 7,312 rows — which is what a
    // real set of index months looks like, not a contrivance.
    let months = [
        entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("BANKNIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("FINNIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("MIDCPNIFTY", 2024, 6, 7_312, 1_000, 2_000),
    ];
    let (header_region, data, _) = built(Vendor::Groww, &months);

    // A file that is there is opened, never started again.
    let opened = Manifest::open(Vendor::Groww, &header_region, &data).expect("it opens");
    assert_eq!(
        (opened.entries(), opened.keys(), opened.total_rows()),
        (4, 4, 4 * 7_312)
    );
    assert_eq!(opened.entry(&months[0].key), Some(months[0]));

    // A file with nothing in it is the one and only genesis.
    let empty = Manifest::open(Vendor::Dhan, &[], &[]).expect("a genesis manifest");
    assert_eq!(
        (empty.entries(), empty.keys(), empty.total_rows()),
        (0, 0, 0)
    );
    assert_eq!(empty.degraded_reason(), None);

    // Half a file is not an empty one, and is not a genesis either. Neither
    // half is quietly treated as "nothing is here yet".
    assert_eq!(
        Manifest::open(Vendor::Groww, &[], &data),
        Err(ManifestError::HeaderRegionTooShort { slots: 0, need: 2 })
    );
    assert_eq!(
        Manifest::open(Vendor::Groww, &header_region, &[]),
        Err(ManifestError::CounterExceedsRegion {
            n_valid: 4,
            capacity: 0
        }),
        "the newest generation's refusal, because that is what a writer published"
    );
}

/// C-12's mechanism — the loaded index is reserved from the **census**, not
/// from the file it came in.
///
/// `docs/07-o1-architecture.md`: *"Layer 3's guarantee is the absence of a
/// rehash, so the bound is the reservation itself."* The reservation was
/// `entries.len() / IMAGE_LEN` — the region's byte length, which is untrusted
/// input — so a one-entry census inside a region at the design ceiling
/// reserved for 2,097,152 entries and allocated 574 MB to hold one key.
#[test]
fn the_loaded_index_is_reserved_from_the_census() {
    let one = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let (header_region, mut data, _) = built(Vendor::Groww, &[one]);

    // One committed entry, in a region with room for a thousand.
    data.resize(1_000 * IMAGE_LEN, 0);
    let manifest = Manifest::load(Vendor::Groww, &header_region, &data).expect("it loads");
    assert_eq!(manifest.entries(), 1);
    assert!(
        manifest.reserved() >= 1,
        "the bound is at least the census, or the map rehashes: {}",
        manifest.reserved()
    );
    assert!(
        manifest.reserved() < 1_000,
        "the bound is the census and not the file: {}",
        manifest.reserved()
    );

    // And at a census of a thousand it does reserve for a thousand, so the
    // assertion above is about where the number comes from rather than about
    // the number being small.
    let months: Vec<Entry> = (0..1_000u16)
        .map(|i| entry("NIFTY", 2_000 + i, 6, 7_312, 1_000, 2_000))
        .collect();
    let (header_region, data, _) = built(Vendor::Groww, &months);
    let manifest = Manifest::load(Vendor::Groww, &header_region, &data).expect("it loads");
    assert_eq!(manifest.entries(), 1_000);
    assert!(manifest.reserved() >= 1_000, "{}", manifest.reserved());
}

/// The `index`th distinct month, for a census that needs more of them than a
/// literal list can carry.
///
/// `1970..=9999` is 96,360 nameable months and every one is a month
/// `store::path::YearMonth` accepts, so a census built from this is a census the
/// store could address. A key the store refuses would be measuring a shape that
/// cannot exist.
fn nth_month(index: u32) -> Entry {
    let year = u16::try_from(1_970 + index / 12).expect("a year inside u16");
    let month = u8::try_from(index % 12 + 1).expect("a month inside u8");
    entry("NIFTY", year, month, 7_312, 1_000, 2_000)
}

/// M-18 — the reservation is the census doubled, and never past the ceiling.
///
/// The arithmetic behind M-19, checked at the two ends a census cannot be built
/// at: zero, and the design ceiling. Reaching the cap through
/// [`Manifest::load`] would need a census of 1,048,576 entries — 67 MB of entry
/// bytes and a 574 MB table — which is not a thing a unit test builds, and
/// which is exactly why `reservation_for` is public rather than a closure
/// inside the walk. `CLAUDE.md` §3 rule 6.
#[test]
fn the_reservation_is_capped_at_the_design_ceiling() {
    assert_eq!(APPEND_HEADROOM_FACTOR, 2);
    let ceiling = usize::try_from(MAX_ENTRIES).expect("the ceiling inside usize");

    assert_eq!(reservation_for(0), 0, "nothing held reserves nothing");
    assert_eq!(reservation_for(1), 2);
    // 7·2^8 — the count at which `HashMap::with_capacity(n)` used to hand back
    // a table of capacity exactly `n` and no free slot at all.
    assert_eq!(reservation_for(1_792), 3_584);
    assert_eq!(reservation_for(57_344), 114_688);

    // The cap binds from half the ceiling upward, and it binds at the ceiling
    // rather than above it: the reservation never grew past what the region
    // sized version already cost.
    assert_eq!(reservation_for(MAX_ENTRIES / 2 - 1), ceiling - 2);
    assert_eq!(reservation_for(MAX_ENTRIES / 2), ceiling);
    assert_eq!(reservation_for(MAX_ENTRIES), ceiling);

    // And past half the ceiling the headroom stops being a headroom: it covers
    // every append `ManifestHeader::advance` will ever accept, so no append can
    // rehash at all. `reserved - n_valid >= MAX_ENTRIES - n_valid`.
    for n_valid in [MAX_ENTRIES / 2, MAX_ENTRIES / 2 + 1, MAX_ENTRIES - 1] {
        let still_appendable = MAX_ENTRIES - n_valid;
        let free = reservation_for(n_valid) - usize::try_from(n_valid).expect("inside usize");
        assert!(
            u64::try_from(free).expect("inside u64") >= still_appendable,
            "at {n_valid} the reservation leaves {free} free for {still_appendable} appends"
        );
    }
}

/// M-19 — a loaded index carries room for the appends that follow it, and the
/// number of them is stated rather than hoped for.
///
/// **This is the test that was missing, and its absence is the whole finding.**
/// The reservation was exactly `n_valid`, and `HashMap::with_capacity(n)` rounds
/// `n` up to `7·2^k` — so at `n = 7·2^k` it hands back a table with **zero**
/// free slots and the very next new month rebuilds all of it. 1,792 is `7·2^8`,
/// which is why this test is built at that count and not at a round one: at
/// 1,000 the rounding leaves 792 spare slots by luck and the defect is
/// invisible. `crates/pull/benches/ratio.rs` C-13 measured the cost — 5,100,585
/// ps per append at a 57,344-entry census against 95,214 ps now. D-0040.
///
/// The guarantee is `n_valid` appends, and the last assertion holds the line at
/// exactly that: one more than the headroom **does** grow the table, and this
/// test says so rather than implying the bound is unconditional.
#[test]
fn a_loaded_index_carries_headroom_for_the_appends_after_it() {
    /// `7·2^8` — a census sitting exactly on a capacity boundary.
    const CENSUS: u32 = 1_792;

    let months: Vec<Entry> = (0..CENSUS).map(nth_month).collect();
    let (header_region, data, _) = built(Vendor::Groww, &months);
    let mut manifest = Manifest::load(Vendor::Groww, &header_region, &data).expect("it loads");

    assert_eq!(manifest.entries(), u64::from(CENSUS));
    assert_eq!(manifest.keys(), u64::from(CENSUS));
    let reserved = manifest.reserved();
    assert_eq!(reserved, reservation_for(manifest.entries()));
    assert!(
        reserved >= 2 * CENSUS as usize,
        "the reservation carries the census over again: {reserved}"
    );

    // An update to a key already held costs no slot: the key count does not
    // move and neither does the table.
    let mut again = nth_month(0);
    again.rows = 7_400;
    manifest.record(again).expect("an update");
    assert_eq!(manifest.reserved(), reserved);
    assert_eq!(manifest.keys(), u64::from(CENSUS));
    assert_eq!(manifest.entries(), u64::from(CENSUS) + 1);

    // Every one of `n_valid` new keys goes in without the table moving. A
    // rehash is observable exactly here: `capacity()` changes when the table is
    // rebuilt, and it is the only thing in this type that can.
    for index in CENSUS..2 * CENSUS {
        manifest.record(nth_month(index)).expect("an append");
        assert_eq!(
            manifest.reserved(),
            reserved,
            "the table was rebuilt at append {index}, which is the O(keys) step \
             this reservation exists to remove"
        );
    }
    assert_eq!(manifest.keys(), u64::from(2 * CENSUS));

    // And the honest end of it: the headroom is `n_valid` appends, not more.
    // The table is now exactly full, and `HashMap::insert` asks for one slot
    // *before* it looks the key up — so the next `record` rebuilds the table
    // whether it is a new month or an update to one already held. That is
    // `O(n_keys)`, it is recorded as amortised in docs/06-limits.md section 23
    // rather than dressed as worst case, and this assertion is what stops the
    // bound above from being read as unconditional.
    manifest
        .record(nth_month(2 * CENSUS))
        .expect("an append past the headroom");
    assert!(
        manifest.reserved() > reserved,
        "past the headroom the table does grow, and this bound says so: {}",
        manifest.reserved()
    );
}

/// M-06 — a counter that disagrees with the entries it counts is refused.
#[test]
fn a_counter_that_disagrees_with_its_entries_is_refused() {
    let entries = [
        entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000),
    ];
    let (_, data, written) = built(Vendor::Groww, &entries);

    let lying_keys = ManifestHeader {
        n_keys: 1,
        ..written.header()
    };
    assert_eq!(
        Manifest::load(
            Vendor::Groww,
            &region(&[lying_keys.commit().expect("a commit")]),
            &data
        ),
        Err(ManifestError::KeyCountDisagrees {
            header: 1,
            entries: 2
        })
    );

    let lying_rows = ManifestHeader {
        total_rows: 1,
        ..written.header()
    };
    assert_eq!(
        Manifest::load(
            Vendor::Groww,
            &region(&[lying_rows.commit().expect("a commit")]),
            &data
        ),
        Err(ManifestError::RowTotalDisagrees {
            header: 1,
            entries: 14_622
        })
    );

    // An entry below the counter that fails its own checksum is named by
    // ordinal, not swallowed.
    let mut damaged = data.clone();
    damaged[64] ^= 0xFF;
    match Manifest::load(
        Vendor::Groww,
        &region(&[written.header().commit().expect("a commit")]),
        &damaged,
    ) {
        Err(ManifestError::Entry {
            ordinal: 1,
            fault: EntryFault::Checksum { .. },
        }) => {}
        other => panic!("expected entry 1 to be named, got {other:?}"),
    }
}

/// A key whose history goes backwards on disk is refused on load, not merely
/// on write.
#[test]
fn a_key_whose_history_goes_backwards_on_disk_is_refused() {
    let first = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let shrunk = entry("NIFTY", 2024, 6, 7_000, 1_000, 2_000);
    let earlier = entry("NIFTY", 2024, 6, 7_312, 1_000, 1_500);

    for (second, expected) in [
        (
            shrunk,
            ManifestError::RowCountWentBackwards {
                ordinal: 1,
                previous: 7_312,
                next: 7_000,
            },
        ),
        (
            earlier,
            ManifestError::KeyTimestampsOutOfOrder {
                ordinal: 1,
                previous: 2_000,
                next: 1_500,
            },
        ),
    ] {
        let mut data = Vec::new();
        data.extend_from_slice(&first.image());
        data.extend_from_slice(&second.image());
        let header = ManifestHeader {
            n_valid: 2,
            n_keys: 1,
            total_rows: second.rows,
            ..ManifestHeader::genesis(Vendor::Groww)
        };
        assert_eq!(
            Manifest::load(
                Vendor::Groww,
                &region(&[header.commit().expect("a commit")]),
                &data
            ),
            Err(expected)
        );
    }
}

/// The load-time row total refuses to wrap.
#[test]
fn the_load_time_row_total_refuses_to_wrap() {
    let mut data = Vec::new();
    data.extend_from_slice(&entry("NIFTY", 2024, 6, u64::MAX, 0, 1).image());
    data.extend_from_slice(&entry("NIFTY", 2024, 7, u64::MAX, 0, 1).image());
    let header = ManifestHeader {
        n_valid: 2,
        n_keys: 2,
        total_rows: 0,
        ..ManifestHeader::genesis(Vendor::Groww)
    };
    assert_eq!(
        Manifest::load(
            Vendor::Groww,
            &region(&[header.commit().expect("a commit")]),
            &data
        ),
        Err(ManifestError::RowTotalOverflow)
    );
}

/// M-10 — a manifest that belongs to another vendor is refused by name.
#[test]
fn a_manifest_for_another_vendor_is_refused() {
    let (header_region, data, _) = built(Vendor::Groww, &[entry("NIFTY", 2024, 6, 1, 0, 1)]);
    assert_eq!(
        Manifest::load(Vendor::Dhan, &header_region, &data),
        Err(ManifestError::VendorMismatch {
            asked: Vendor::Dhan,
            found: Vendor::Groww
        })
    );
}

/// Every way a header slot can fail to be a header is named.
#[test]
fn a_slot_that_is_not_a_header_is_named() {
    let good = ManifestHeader::genesis(Vendor::Groww).image();
    assert_eq!(
        ManifestHeader::decode(&good[..IMAGE_LEN - 1]),
        Err(ManifestError::SlotTooShort { len: IMAGE_LEN - 1 })
    );

    let edit = |f: &dyn Fn(&mut [u8; IMAGE_LEN]), seal: bool| -> ManifestError {
        let mut image = good;
        f(&mut image);
        if seal {
            reseal(&mut image);
        }
        ManifestHeader::decode(&image).expect_err("this slot must be refused")
    };

    assert_eq!(
        edit(&|i| i[0..8].copy_from_slice(b"BRUTEXB2"), true),
        ManifestError::NotAManifest
    );
    // The version field and the magic's own version byte are two statements of
    // one fact, and either one disagreeing is a refusal.
    assert_eq!(
        edit(&|i| i[8..10].copy_from_slice(&2u16.to_le_bytes()), true),
        ManifestError::UnknownVersion(2)
    );
    assert_eq!(
        edit(&|i| i[7] = b'2', true),
        ManifestError::UnknownVersion(FORMAT_VERSION)
    );
    match edit(&|i| i[24] ^= 0xFF, false) {
        ManifestError::SlotChecksum { stored, computed } => assert_ne!(stored, computed),
        other => panic!("expected a checksum refusal, got {other:?}"),
    }
    assert_eq!(
        edit(&|i| i[10..12].copy_from_slice(&32u16.to_le_bytes()), true),
        ManifestError::StrideMismatch(32)
    );
    assert_eq!(
        edit(&|i| i[48..56].copy_from_slice(b"nobody\0\0"), true),
        ManifestError::UnknownVendor
    );
    assert_eq!(
        edit(&|i| i[48] = 0xFF, true),
        ManifestError::UnknownVendor,
        "a vendor field that is not even text is the same refusal"
    );

    // A whole, well-formed slot still round-trips.
    assert_eq!(
        ManifestHeader::decode(&good),
        Ok(ManifestHeader::genesis(Vendor::Groww))
    );
    assert_eq!(&good[..8], MAGIC);
}

/// The header region is refused when it is too short, and a misplaced commit
/// is not a candidate.
#[test]
fn a_short_or_misplaced_header_region_is_refused() {
    let genesis = ManifestHeader::genesis(Vendor::Groww)
        .commit()
        .expect("genesis");
    assert_eq!(
        ManifestHeader::read_region(&[0u8; 16_384], 0, Vendor::Groww),
        Err(ManifestError::HeaderRegionTooShort { slots: 1, need: 2 })
    );
    let read = ManifestHeader::read_region(&region(&[genesis]), 0, Vendor::Groww)
        .expect("the genesis commit");
    assert_eq!(read.newest(), ManifestHeader::genesis(Vendor::Groww));
    assert_eq!(read.older(), None, "one commit is one generation");
    assert_eq!(read.stepped_over(), None, "and nothing was stepped over");
    assert_eq!(read, read);
    assert!(format!("{read:?}").contains("Groww"));
    // Nothing at all: no slot decodes, and none gave a specific reason.
    assert_eq!(
        ManifestHeader::read_region(&vec![0u8; HEADER_LEN_USIZE], 0, Vendor::Groww),
        Err(ManifestError::NoValidHeader)
    );

    // A generation-1 commit written into slot 0 is refused by position: a
    // writer that puts a commit in the wrong slot can overwrite the only
    // surviving copy of the previous one.
    let odd = ManifestHeader {
        generation: 1,
        ..ManifestHeader::genesis(Vendor::Groww)
    };
    let mut misplaced = vec![0u8; HEADER_LEN_USIZE];
    misplaced[..IMAGE_LEN].copy_from_slice(&odd.commit().expect("a commit").bytes);
    assert_eq!(
        ManifestHeader::read_region(&misplaced, 0, Vendor::Groww),
        Err(ManifestError::SlotPositionMismatch {
            expected: 1,
            found: 0
        })
    );

    // A misplaced commit beside a good one is not a candidate, and the reason
    // it was passed over is REPORTED rather than dropped on the floor.
    let mut beside = region(&[genesis]);
    beside[SLOT_STRIDE_USIZE..SLOT_STRIDE_USIZE + IMAGE_LEN]
        .copy_from_slice(&odd.commit().expect("a commit").bytes);
    // Generation 1 belongs in slot 1, so put a generation-2 commit there
    // instead: that one belongs in slot 0 and is found in slot 1.
    let even = ManifestHeader {
        generation: 2,
        ..ManifestHeader::genesis(Vendor::Groww)
    };
    beside[SLOT_STRIDE_USIZE..SLOT_STRIDE_USIZE + IMAGE_LEN]
        .copy_from_slice(&even.commit().expect("a commit").bytes);
    let read = ManifestHeader::read_region(&beside, 0, Vendor::Groww).expect("the genesis commit");
    assert_eq!(read.newest(), ManifestHeader::genesis(Vendor::Groww));
    assert_eq!(
        read.stepped_over(),
        Some(ManifestError::SlotPositionMismatch {
            expected: 0,
            found: 1
        })
    );

    // Two whole slots: the newer one wins, and the older one is KEPT — it is
    // the generation `Manifest::load` falls back to, and until D-0036 it was
    // discarded here, which is why the fall-back only ever worked for a region
    // that was physically short.
    let newer = ManifestHeader {
        generation: 1,
        ..ManifestHeader::genesis(Vendor::Groww)
    };
    let both = region(&[genesis, newer.commit().expect("a commit")]);
    let read = ManifestHeader::read_region(&both, 0, Vendor::Groww).expect("two generations");
    assert_eq!(read.newest(), newer);
    assert_eq!(read.older(), Some(ManifestHeader::genesis(Vendor::Groww)));
    assert_eq!(read.stepped_over(), None);
}

/// The counters refuse to wrap, and the bounds are named.
#[test]
fn the_header_counters_refuse_to_wrap() {
    let genesis = ManifestHeader::genesis(Vendor::Groww);

    assert_eq!(
        ManifestHeader {
            generation: u64::MAX,
            ..genesis
        }
        .advance(1, 0, 0),
        Err(ManifestError::GenerationExhausted)
    );
    assert_eq!(
        ManifestHeader {
            n_valid: u64::MAX,
            ..genesis
        }
        .advance(1, 0, 0),
        Err(ManifestError::CounterOverflow)
    );
    assert_eq!(
        ManifestHeader {
            n_valid: MAX_ENTRIES,
            ..genesis
        }
        .advance(1, 0, 0),
        Err(ManifestError::TooManyEntries {
            n_valid: MAX_ENTRIES + 1,
            limit: MAX_ENTRIES
        })
    );
    assert_eq!(
        genesis.advance(1, 2, 0),
        Err(ManifestError::KeyCountExceedsEntries {
            keys: 2,
            entries: 1
        })
    );
    assert_eq!(
        genesis.advance(1, 1, 9).expect("one entry, one key"),
        ManifestHeader {
            generation: 1,
            n_valid: 1,
            n_keys: 1,
            total_rows: 9,
            ..genesis
        }
    );

    // The same bounds guard `validate`, and `commit` refuses a counter it
    // could not address.
    assert_eq!(
        ManifestHeader {
            n_valid: MAX_ENTRIES + 1,
            ..genesis
        }
        .validate(u64::MAX),
        Err(ManifestError::TooManyEntries {
            n_valid: MAX_ENTRIES + 1,
            limit: MAX_ENTRIES
        })
    );
    assert_eq!(
        ManifestHeader {
            n_valid: 1,
            n_keys: 2,
            ..genesis
        }
        .validate(4),
        Err(ManifestError::KeyCountExceedsEntries {
            keys: 2,
            entries: 1
        })
    );
    assert_eq!(
        ManifestHeader {
            n_valid: MAX_ENTRIES + 1,
            ..genesis
        }
        .commit(),
        Err(ManifestError::TooManyEntries {
            n_valid: MAX_ENTRIES + 1,
            limit: MAX_ENTRIES
        })
    );

    // A manifest loaded at an exhausted generation still refuses to advance,
    // which is the one route `record` has into that arm.
    let exhausted = ManifestHeader {
        generation: u64::MAX,
        ..genesis
    };
    let mut manifest = Manifest::load(
        Vendor::Groww,
        &region(&[exhausted.commit().expect("a commit")]),
        &[],
    )
    .expect("an empty manifest at the last generation");
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 6, 1, 0, 1)),
        Err(ManifestError::GenerationExhausted)
    );
}

/// M-15 — every declared bound is exact: the value **at** the limit is
/// accepted and the first one past it is refused.
///
/// Written because `cargo mutants` proved it was missing. Seven boundaries in
/// this crate could have their comparison flipped from `>` to `>=` with the
/// whole suite still green, because no test ever supplied a value sitting
/// exactly on a limit — the standard
/// `pull::unit::the_secret_backstop_is_the_first_byte_that_is_too_many` already
/// set for one bound and nowhere else. X-07 and D-0036.
#[test]
fn every_declared_bound_is_exact_at_the_limit() {
    let genesis = ManifestHeader::genesis(Vendor::Groww);

    // `advance` — MAX_ENTRIES exactly is a header, one more is a refusal.
    let at = ManifestHeader {
        n_valid: MAX_ENTRIES - 1,
        ..genesis
    }
    .advance(1, 1, 1)
    .expect("exactly MAX_ENTRIES is not too many");
    assert_eq!(at.n_valid, MAX_ENTRIES);

    // `validate` — n_valid exactly at MAX_ENTRIES, and exactly at the region's
    // capacity, are both supported.
    assert_eq!(
        ManifestHeader {
            n_valid: MAX_ENTRIES,
            n_keys: MAX_ENTRIES,
            ..genesis
        }
        .validate(MAX_ENTRIES),
        Ok(())
    );
    assert_eq!(
        ManifestHeader {
            n_valid: 4,
            n_keys: 4,
            ..genesis
        }
        .validate(4),
        Ok(()),
        "a counter equal to the capacity is addressable"
    );
    assert_eq!(
        ManifestHeader {
            n_valid: 5,
            n_keys: 0,
            ..genesis
        }
        .validate(4),
        Err(ManifestError::CounterExceedsRegion {
            n_valid: 5,
            capacity: 4
        })
    );

    // `commit` — the same limit, one function later.
    assert_eq!(
        ManifestHeader {
            n_valid: MAX_ENTRIES,
            ..genesis
        }
        .commit()
        .expect("exactly MAX_ENTRIES commits")
        .durable_through,
        HEADER_LEN + MAX_ENTRIES * 64
    );

    // `n_keys > n_valid` — equal is the ordinary case of every key being new.
    assert_eq!(
        genesis.advance(1, 1, 7).expect("one entry, one key").n_keys,
        1
    );

    // The configuration's bounds, each exactly at its limit. A line exactly at
    // the bound is READ — it reaches the segment check, which is what refuses
    // it — rather than being refused for its length.
    let widest = "a".repeat(MAX_LINE_BYTES - 8);
    let line = format!("org = \"{widest}\"");
    assert_eq!(line.len(), MAX_LINE_BYTES);
    assert_eq!(
        CredentialConfig::parse(&line),
        Err(ConfigError::Segment {
            line: 1,
            key: "org",
            fault: SegmentFault::TooLong {
                len: MAX_LINE_BYTES - 8
            }
        }),
        "a line exactly at the bound is parsed, not refused for its length"
    );
    assert_eq!(
        CredentialConfig::parse(&format!("{line}a")),
        Err(ConfigError::LineTooLong {
            line: 1,
            len: MAX_LINE_BYTES + 1
        })
    );

    let exactly = (0..MAX_FIELDS)
        .map(|i| format!("\"field{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let config = CredentialConfig::parse(&config_without(
        "fields = [\"fieldone\", \"fieldtwo\"]",
        &format!("fields = [{exactly}]"),
    ))
    .expect("exactly MAX_FIELDS fields is not too many");
    assert_eq!(
        config.fields(Vendor::Groww).expect("groww").len(),
        MAX_FIELDS
    );
}

/// M-07's other half — a key that stands still is not a key that went
/// backwards, on write and on load.
#[test]
fn a_key_that_repeats_its_last_timestamp_is_accepted() {
    // On write: the same last timestamp and the same row count is a re-pull
    // that found nothing new, which is a fact to record rather than a fault.
    let mut manifest = fresh(Vendor::Groww);
    let june = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    manifest.record(june).expect("june");
    manifest.record(june).expect("june again, unchanged");
    assert_eq!(
        (manifest.entries(), manifest.keys(), manifest.total_rows()),
        (2, 1, 7_312)
    );

    // On load: the same two entries, from disk. Without this case the
    // comparison at the load-time key check can be flipped to `<=` and every
    // other test still passes.
    let (header_region, data, _) = built(Vendor::Groww, &[june, june]);
    let read = Manifest::load(Vendor::Groww, &header_region, &data).expect("it loads");
    assert_eq!(
        (read.entries(), read.keys(), read.total_rows()),
        (2, 1, 7_312)
    );
    assert_eq!(read.degraded_reason(), None);
}

/// One manifest per vendor, named after the vendor.
#[test]
fn a_manifest_is_named_after_its_vendor() {
    let root = std::path::Path::new("/data");
    assert_eq!(
        manifest_path(root, Vendor::Groww),
        std::path::Path::new("/data/manifest/groww.man")
    );
    assert_eq!(
        manifest_path(root, Vendor::Dhan),
        std::path::Path::new("/data/manifest/dhan.man")
    );
}

/// Every refusal renders something, and no two render the same thing.
#[test]
fn every_manifest_error_prints_something_distinct() {
    let errors = [
        ManifestError::OrdinalOutOfRange {
            ordinal: 1,
            limit: 2,
        },
        ManifestError::SlotTooShort { len: 1 },
        ManifestError::NotAManifest,
        ManifestError::UnknownVersion(9),
        ManifestError::StrideMismatch(9),
        ManifestError::SlotChecksum {
            stored: 1,
            computed: 2,
        },
        ManifestError::UnknownVendor,
        ManifestError::VendorMismatch {
            asked: Vendor::Groww,
            found: Vendor::Dhan,
        },
        ManifestError::SlotPositionMismatch {
            expected: 1,
            found: 0,
        },
        ManifestError::HeaderRegionTooShort { slots: 1, need: 2 },
        ManifestError::NoValidHeader,
        ManifestError::TooManyEntries {
            n_valid: 1,
            limit: 2,
        },
        ManifestError::CounterExceedsRegion {
            n_valid: 1,
            capacity: 0,
        },
        ManifestError::KeyCountExceedsEntries {
            keys: 2,
            entries: 1,
        },
        ManifestError::CounterOverflow,
        ManifestError::GenerationExhausted,
        ManifestError::RowTotalOverflow,
        ManifestError::Entry {
            ordinal: 1,
            fault: EntryFault::EmptyEntry,
        },
        ManifestError::RowCountWentBackwards {
            ordinal: 1,
            previous: 2,
            next: 1,
        },
        ManifestError::KeyTimestampsOutOfOrder {
            ordinal: 1,
            previous: 2,
            next: 1,
        },
        ManifestError::KeyCountDisagrees {
            header: 1,
            entries: 2,
        },
        ManifestError::RowTotalDisagrees {
            header: 1,
            entries: 2,
        },
    ];
    let rendered: HashSet<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), errors.len(), "two refusals read the same");
    for error in errors {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").is_empty());
        let _: &dyn std::error::Error = &error;
    }

    let faults = [
        EntryFault::TooShort { len: 1 },
        EntryFault::Checksum {
            stored: 1,
            computed: 2,
        },
        EntryFault::SymbolNotUtf8,
        EntryFault::BadSymbol(brutex_core::error::InstrumentError::Malformed),
        EntryFault::SymbolNotCanonical,
        EntryFault::UnknownExchange { code: 9 },
        EntryFault::UnknownSegment { code: 9 },
        EntryFault::UnknownTimeframe { secs: 300 },
        EntryFault::BadMonth(PathError::MonthOutOfRange { month: 13 }),
        EntryFault::EmptyEntry,
        EntryFault::TimestampsOutOfOrder { first: 2, last: 1 },
    ];
    let rendered: HashSet<String> = faults.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), faults.len());
    for fault in faults {
        assert!(!fault.to_string().is_empty());
        assert!(!format!("{fault:?}").is_empty());
        let _: &dyn std::error::Error = &fault;
    }
}

// ===========================================================================
// The adaptive rate governor — `pull::rate`
//
// Not one of these tests sleeps, and not one of them reads a clock. Time is an
// argument to `admit`, so every boundary below is asserted at the exact
// microsecond rather than near it. D-0037.
// ===========================================================================

/// The secondary vendor's governor, exactly as `docs/00-charter.md` §4 states
/// it: 5 per second, 100,000 per day, and **no** per-minute governor.
fn dhan_governor() -> Governor {
    Governor::new(Some(DHAN_PER_SECOND), None, Some(DHAN_PER_DAY)).expect("the charter's figures")
}

/// A governor bounded on one span only, so a test can isolate that span.
fn only(span: WindowSpan, ceiling: u32) -> Governor {
    let (per_second, per_minute, per_day) = match span {
        WindowSpan::Second => (Some(ceiling), None, None),
        WindowSpan::Minute => (None, Some(ceiling), None),
        WindowSpan::Day => (None, None, Some(ceiling)),
    };
    Governor::new(per_second, per_minute, per_day).expect("a ceiling within bounds")
}

/// P-20 — the first request of a pull is admitted, and it costs exactly one
/// permit.
///
/// The bucket starts full because a pull that has not started has spent
/// nothing. Starting it empty would make every pull wait for a budget nobody
/// had consumed, which is the shape of a rate limiter that is wrong in the safe
/// direction and therefore never noticed.
#[test]
fn the_first_request_of_a_pull_is_admitted() {
    let mut governor = dhan_governor();

    // Before anything: the allowance is the published ceiling and the bucket is
    // full, in both live spans.
    assert_eq!(governor.permitted(WindowSpan::Second), Some(5));
    assert_eq!(governor.ceiling(WindowSpan::Second), Some(5));
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Second),
        Some(5 * MICROS_PER_SECOND)
    );
    assert_eq!(governor.permitted(WindowSpan::Day), Some(100_000));
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Day),
        Some(100_000 * MICROS_PER_DAY)
    );
    // The charter records no per-minute governor for this vendor, and absence
    // is `None` rather than a very large number.
    assert_eq!(governor.permitted(WindowSpan::Minute), None);
    assert_eq!(governor.ceiling(WindowSpan::Minute), None);
    assert_eq!(governor.credit_micro_permits(WindowSpan::Minute), None);
    assert_eq!(governor.cursor_micros(), 0);

    assert_eq!(governor.admit(0), Verdict::Admit);

    // One permit charged against every live span, and nothing charged against
    // the span that has no bound.
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Second),
        Some(4 * MICROS_PER_SECOND)
    );
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Day),
        Some(99_999 * MICROS_PER_DAY)
    );
    assert_eq!(governor.credit_micro_permits(WindowSpan::Minute), None);
    assert_eq!(governor.cursor_micros(), 0);
}

/// P-21 — a window saturates **at** its allowance and refuses the next one.
///
/// The boundary is asserted from both sides in the same test, because a limiter
/// that admits four of five and a limiter that admits six of five both pass a
/// test that only checks one side.
#[test]
fn a_window_saturates_exactly_at_its_allowance_and_not_one_over() {
    let mut governor = only(WindowSpan::Second, 5);

    for issued in 0..5u32 {
        assert_eq!(
            governor.admit(0),
            Verdict::Admit,
            "request {issued} of the allowance must be admitted"
        );
    }
    assert_eq!(governor.credit_micro_permits(WindowSpan::Second), Some(0));

    // One over. The refusal names the span and the exact wait: the bucket is
    // empty, one permit costs 1,000,000 micro-permits and the window earns 5 of
    // them per microsecond.
    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );
    // A refusal charges nothing, so asking twice gives the same answer.
    assert_eq!(governor.credit_micro_permits(WindowSpan::Second), Some(0));
    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );
}

/// P-22 — a refusal halves the allowance and drains the bucket, and the
/// published ceiling is untouched.
///
/// The multiplicative half of AIMD. Every live span backs off, because a
/// vendor's throttle response names none of them.
#[test]
fn a_refusal_halves_every_allowance_and_drains_every_bucket() {
    let mut governor = Governor::new(Some(8), Some(500), Some(100_000)).expect("within bounds");
    assert_eq!(governor.admit(0), Verdict::Admit);

    governor.record_throttled();

    assert_eq!(governor.permitted(WindowSpan::Second), Some(4));
    assert_eq!(governor.permitted(WindowSpan::Minute), Some(250));
    assert_eq!(governor.permitted(WindowSpan::Day), Some(50_000));
    // The ceiling is a published figure and a back-off is not evidence about
    // it. Only the allowance moves.
    assert_eq!(governor.ceiling(WindowSpan::Second), Some(8));
    assert_eq!(governor.ceiling(WindowSpan::Minute), Some(500));
    assert_eq!(governor.ceiling(WindowSpan::Day), Some(100_000));
    // The bucket is drained as well: the budget the governor believed in has
    // just been disproven, and leaving credit behind would let the caller spend
    // it immediately after being told it does not exist.
    assert_eq!(governor.credit_micro_permits(WindowSpan::Second), Some(0));
    assert_eq!(governor.credit_micro_permits(WindowSpan::Minute), Some(0));
    assert_eq!(governor.credit_micro_permits(WindowSpan::Day), Some(0));

    // And the next request is refused by the span that must wait longest — the
    // day, at 86,400,000,000 / 50,000 microseconds — not by the first span
    // looked at.
    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Day,
            wait_micros: 1_728_000,
        }
    );
}

/// P-23 — sustained success walks the allowance back up one permit at a time,
/// and stops **exactly** at the published ceiling.
///
/// The additive half of AIMD, and the reason a refusal is expensive: the halving
/// above cost four permits in one step, and every one of them has to be earned
/// back separately.
#[test]
fn sustained_success_walks_up_one_permit_at_a_time_and_stops_at_the_ceiling() {
    let mut governor = only(WindowSpan::Second, 8);
    governor.record_throttled();
    assert_eq!(governor.permitted(WindowSpan::Second), Some(4));

    let mut walk = Vec::new();
    for _ in 0..4 {
        governor.record_success();
        walk.push(governor.permitted(WindowSpan::Second));
    }
    assert_eq!(walk, vec![Some(5), Some(6), Some(7), Some(8)]);

    // The ceiling is a wall, not a target that can be overshot. Twenty more
    // successes do not buy a ninth permit.
    for _ in 0..20 {
        governor.record_success();
        assert_eq!(governor.permitted(WindowSpan::Second), Some(8));
    }
    assert_eq!(governor.ceiling(WindowSpan::Second), Some(8));

    // The refusal drained the bucket as well as halving the allowance, so the
    // recovered allowance is not spendable until it has been earned: at the
    // instant of the refusal there is nothing to spend.
    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 125_000,
        }
    );

    // One second of earning at the recovered allowance fills the bucket, and
    // nine requests at that instant are eight admits and a refusal: the
    // allowance is what the bucket is sized from.
    let mut admitted = 0u32;
    for _ in 0..9 {
        if governor.admit(MICROS_PER_SECOND) == Verdict::Admit {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 8);
}

/// P-24 — the allowance never reaches zero, however many refusals arrive.
///
/// Zero is an absorbing state: no request is admitted, so no success is
/// observed, so nothing raises it again. A governor that can reach it can stop
/// a pull permanently on one bad minute, and the failure looks exactly like a
/// hung process.
#[test]
fn the_allowance_never_reaches_zero_however_many_refusals() {
    let mut governor = only(WindowSpan::Second, 5);

    let mut walk = Vec::new();
    for _ in 0..12 {
        governor.record_throttled();
        walk.push(governor.permitted(WindowSpan::Second).expect("bounded"));
    }
    assert_eq!(walk, vec![2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert!(walk.iter().all(|permitted| *permitted >= 1));

    // And one permit per second is a rate, not a stop: after one second's worth
    // of earning at the floor, a request is admitted again.
    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: MICROS_PER_SECOND,
        }
    );
    assert_eq!(governor.admit(MICROS_PER_SECOND), Verdict::Admit);
    // Which is what lets it climb back out.
    governor.record_success();
    assert_eq!(governor.permitted(WindowSpan::Second), Some(2));
}

/// P-25 — a drained window earns back at exactly the permitted rate, and is
/// whole again after one whole span.
///
/// This is what "the window rolled over" means for a token bucket, asserted at
/// the microsecond on both sides rather than described.
#[test]
fn a_drained_window_earns_back_at_the_permitted_rate_and_is_whole_after_one_span() {
    let mut governor = only(WindowSpan::Second, 5);
    for _ in 0..5 {
        assert_eq!(governor.admit(0), Verdict::Admit);
    }

    // One permit costs 1,000,000 micro-permits and the window earns 5 per
    // microsecond, so the next one arrives at 200,000 µs and not at 199,999.
    assert_eq!(
        governor.admit(199_999),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 1,
        }
    );
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Second),
        Some(999_995)
    );
    assert_eq!(governor.admit(200_000), Verdict::Admit);
    assert_eq!(governor.credit_micro_permits(WindowSpan::Second), Some(0));

    // A whole span of quiet restores the whole allowance and no more than it.
    let later = 200_000 + MICROS_PER_SECOND;
    assert_eq!(
        {
            let mut admitted = 0u32;
            for _ in 0..6 {
                if governor.admit(later) == Verdict::Admit {
                    admitted += 1;
                }
            }
            admitted
        },
        5
    );
    // Idle time does not accumulate past a full bucket: ten more seconds of
    // quiet still buys exactly five.
    let much_later = later + 10 * MICROS_PER_SECOND;
    let mut admitted = 0u32;
    for _ in 0..8 {
        if governor.admit(much_later) == Verdict::Admit {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 5);
}

/// P-26 — when two spans disagree, the one that must wait longest denies, it
/// says so by name, and **no** span is charged.
///
/// A per-minute budget can be exhausted while the per-second budget is wide
/// open. Charging as the spans are walked would let a request the minute refuses
/// still consume a second-window permit, so a throttled pull would drain the
/// short span it was never allowed to use.
#[test]
fn the_span_that_waits_longest_denies_and_nothing_is_charged() {
    let mut governor = Governor::new(Some(5), Some(6), None).expect("within bounds");

    // Five at one instant empties the second window and leaves the minute
    // window one permit.
    for _ in 0..5 {
        assert_eq!(governor.admit(0), Verdict::Admit);
    }
    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );

    // A second later the second window is full again. The minute window held
    // one permit and earned six micro-permits per microsecond for a second, so
    // it holds 66,000,000 — one permit and a tenth — and the admission spends
    // the permit.
    assert_eq!(governor.admit(MICROS_PER_SECOND), Verdict::Admit);
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Second),
        Some(4 * MICROS_PER_SECOND)
    );
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Minute),
        Some(6_000_000)
    );

    // Now the spans disagree: four whole permits sitting in the second window,
    // a tenth of one in the minute window. The minute denies, it is named, and
    // its wait is the 54,000,000 micro-permit shortfall earned at six per
    // microsecond.
    assert_eq!(
        governor.admit(MICROS_PER_SECOND),
        Verdict::Deny {
            span: WindowSpan::Minute,
            wait_micros: 9_000_000,
        }
    );
    // The free span kept its four permits: a denial charges nothing anywhere.
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Second),
        Some(4 * MICROS_PER_SECOND)
    );
}

/// P-26 — a tie between two spans goes to the shorter one, deterministically.
///
/// Two spans can demand exactly the same wait. Which one is reported must not
/// depend on iteration luck, or the same input would produce two different
/// refusals across runs and `CLAUDE.md` §3 rule 5 would be broken by a message.
#[test]
fn a_tie_between_two_spans_is_broken_by_the_shorter_one() {
    // After one halving: 1 per second and 60 per minute. A drained second
    // window waits 1,000,000 / 1 µs; a drained minute window waits
    // 60,000,000 / 60 µs. The same number.
    let mut governor = Governor::new(Some(2), Some(120), None).expect("within bounds");
    governor.record_throttled();
    assert_eq!(governor.permitted(WindowSpan::Second), Some(1));
    assert_eq!(governor.permitted(WindowSpan::Minute), Some(60));

    assert_eq!(
        governor.admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: MICROS_PER_SECOND,
        }
    );
    // Asked a hundred times, it answers the same way a hundred times.
    for _ in 0..100 {
        assert_eq!(
            governor.admit(0),
            Verdict::Deny {
                span: WindowSpan::Second,
                wait_micros: MICROS_PER_SECOND,
            }
        );
    }
}

/// P-27 — the wait a denial reports is the smallest one that clears it.
///
/// Both halves are asserted: one microsecond earlier is still a refusal, and
/// the reported instant is an admission. A wait that is merely sufficient makes
/// a pull slower than the vendor requires and nothing ever reports it.
#[test]
fn a_denial_names_the_exact_wait_that_clears_it() {
    // Seven per minute: one permit costs 60,000,000 micro-permits earned at 7
    // per microsecond, which is 8,571,428.57… — a wait that must round up.
    let mut governor = only(WindowSpan::Minute, 7);
    for _ in 0..7 {
        assert_eq!(governor.admit(0), Verdict::Admit);
    }

    let Verdict::Deny { span, wait_micros } = governor.admit(0) else {
        panic!("the eighth request must be refused");
    };
    assert_eq!(span, WindowSpan::Minute);
    assert_eq!(wait_micros, 8_571_429);
    // Rounded up, never down: one microsecond less does not pay for the permit.
    assert_eq!(7 * (wait_micros - 1), 59_999_996);
    assert!(7 * wait_micros >= MICROS_PER_MINUTE);

    assert_eq!(
        governor.admit(wait_micros - 1),
        Verdict::Deny {
            span: WindowSpan::Minute,
            wait_micros: 1,
        }
    );
    assert_eq!(governor.admit(wait_micros), Verdict::Admit);
}

/// P-28 — a clock that goes backwards grants nothing, and cannot panic.
///
/// An NTP step, a resumed laptop and a virtualised counter all produce a `now`
/// below the last one. A cursor that followed the clock down would hand out the
/// whole regression as free capacity the moment the clock came back, which is a
/// rate limiter that a suspended process turns off.
#[test]
fn a_clock_that_goes_backwards_grants_nothing() {
    let mut governor = only(WindowSpan::Second, 5);
    for _ in 0..5 {
        assert_eq!(governor.admit(10 * MICROS_PER_SECOND), Verdict::Admit);
    }
    assert_eq!(governor.cursor_micros(), 10 * MICROS_PER_SECOND);

    // A full second backwards. No panic, no credit, and the cursor holds its
    // high-water mark.
    assert_eq!(
        governor.admit(9 * MICROS_PER_SECOND),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );
    assert_eq!(governor.credit_micro_permits(WindowSpan::Second), Some(0));
    assert_eq!(governor.cursor_micros(), 10 * MICROS_PER_SECOND);

    // The discriminating assertion. 200,000 µs past the high-water mark buys
    // exactly ONE permit. Had the cursor followed the clock down to 9 s, the
    // same instant would have looked like 1,200,000 µs of elapsed time and
    // handed back a full bucket of five.
    assert_eq!(
        governor.admit(10 * MICROS_PER_SECOND + 200_000),
        Verdict::Admit
    );
    assert_eq!(
        governor.admit(10 * MICROS_PER_SECOND + 200_000),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );
}

/// P-29 — a clock at the far end of `u64` does not wrap, and does not grant
/// more than a full bucket.
///
/// The first `admit` of a governor's life sees an elapsed time of the caller's
/// whole clock reading, and `elapsed · permitted` overflows `u64` long before
/// `u64::MAX` microseconds. Saturation is only safe because the clamp to a full
/// bucket makes it unobservable, and that is what is checked here rather than
/// assumed.
#[test]
fn a_clock_at_the_far_end_of_u64_does_not_wrap() {
    let mut governor = dhan_governor();

    assert_eq!(governor.admit(u64::MAX), Verdict::Admit);
    assert_eq!(governor.cursor_micros(), u64::MAX);
    assert_eq!(
        governor.credit_micro_permits(WindowSpan::Second),
        Some(4 * MICROS_PER_SECOND)
    );

    // Exactly a full bucket, not a wrapped one: four more admits and then a
    // refusal, at the same instant.
    for _ in 0..4 {
        assert_eq!(governor.admit(u64::MAX), Verdict::Admit);
    }
    assert_eq!(
        governor.admit(u64::MAX),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );

    // Time cannot advance past the end of the clock, so the cursor stays and
    // the refusal stands.
    assert_eq!(
        governor.admit(u64::MAX - 1),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 200_000,
        }
    );
    assert_eq!(governor.cursor_micros(), u64::MAX);

    // The widest product this build can be asked for — the day span at
    // MAX_CEILING — is a full bucket and not a wrap.
    let widest = only(WindowSpan::Day, MAX_CEILING);
    assert_eq!(
        widest.credit_micro_permits(WindowSpan::Day),
        Some(u64::from(MAX_CEILING) * MICROS_PER_DAY)
    );
    assert_eq!(
        widest.credit_micro_permits(WindowSpan::Day),
        Some(86_400_000_000_000_000)
    );
}

/// P-30 — one request kind's pool is exhausted while the other is untouched,
/// and a refusal on one moves only that one.
///
/// `docs/00-charter.md` §4 records the primary vendor's published per-second
/// figure as applying to *a different endpoint group*, which is the charter
/// saying that endpoint groups carry separate budgets. A historical backfill
/// must therefore not be able to starve a live quote, and a `429` on the
/// backfill must not slow a live feed the vendor never complained about.
#[test]
fn one_request_kinds_pool_is_exhausted_while_the_other_is_untouched() {
    let mut pools = Pools::new(
        Vendor::Groww,
        only(WindowSpan::Second, GROWW_PER_SECOND_UNVERIFIED),
        only(WindowSpan::Second, GROWW_PER_SECOND_UNVERIFIED),
    );
    assert_eq!(pools.vendor(), Vendor::Groww);
    assert_eq!(
        pools.key(RequestKind::Historical),
        PoolKey {
            vendor: Vendor::Groww,
            kind: RequestKind::Historical,
        }
    );
    assert_eq!(pools.key(RequestKind::Live).to_string(), "groww/live");
    assert_eq!(
        pools.key(RequestKind::Historical).to_string(),
        "groww/historical"
    );

    // Drain the backfill's pool completely.
    for _ in 0..8 {
        assert_eq!(
            pools.governor_mut(RequestKind::Historical).admit(0),
            Verdict::Admit
        );
    }
    assert_eq!(
        pools.governor_mut(RequestKind::Historical).admit(0),
        Verdict::Deny {
            span: WindowSpan::Second,
            wait_micros: 125_000,
        }
    );

    // The live pool has not moved at all.
    assert_eq!(
        pools
            .governor(RequestKind::Live)
            .credit_micro_permits(WindowSpan::Second),
        Some(8 * MICROS_PER_SECOND)
    );
    assert_eq!(
        pools.governor_mut(RequestKind::Live).admit(0),
        Verdict::Admit
    );

    // And a throttle on one kind halves that kind's allowance alone.
    pools
        .governor_mut(RequestKind::Historical)
        .record_throttled();
    assert_eq!(
        pools
            .governor(RequestKind::Historical)
            .permitted(WindowSpan::Second),
        Some(4)
    );
    assert_eq!(
        pools
            .governor(RequestKind::Live)
            .permitted(WindowSpan::Second),
        Some(8)
    );
}

/// P-31 — the same script produces the same verdicts and the same final state,
/// every time.
///
/// `CLAUDE.md` §3 rule 5. The governor holds no clock, no randomness and no
/// hidden state, so two runs of one script are indistinguishable — which is
/// also what makes a failing run reproducible from the script alone.
#[test]
fn the_same_script_gives_the_same_verdicts_and_the_same_state() {
    // (instant, whether the vendor throttled the request that was admitted)
    let script: [(u64, bool); 13] = [
        (0, false),
        (0, false),
        (0, false),
        (0, false),
        (0, false),
        (0, false),
        (MICROS_PER_SECOND, true),
        (MICROS_PER_SECOND, false),
        (MICROS_PER_SECOND, false),
        (2 * MICROS_PER_SECOND, false),
        (500 * MICROS_PER_SECOND, false),
        (500 * MICROS_PER_SECOND, true),
        (500 * MICROS_PER_SECOND + 1, false),
    ];

    let run = |script: &[(u64, bool)]| {
        let mut governor = Governor::new(
            Some(DHAN_PER_SECOND),
            Some(GROWW_PER_MINUTE),
            Some(DHAN_PER_DAY),
        )
        .expect("within bounds");
        let mut verdicts = Vec::new();
        for (now, throttled) in script {
            let verdict = governor.admit(*now);
            verdicts.push(verdict);
            if verdict == Verdict::Admit {
                if *throttled {
                    governor.record_throttled();
                } else {
                    governor.record_success();
                }
            }
        }
        (governor, verdicts)
    };

    let (first_state, first_verdicts) = run(&script);
    let (second_state, second_verdicts) = run(&script);

    assert_eq!(first_verdicts, second_verdicts);
    assert_eq!(first_state, second_state);

    // Not vacuous: the script really does exercise both arms.
    assert!(first_verdicts.contains(&Verdict::Admit));
    assert!(
        first_verdicts
            .iter()
            .any(|verdict| matches!(verdict, Verdict::Deny { .. })),
        "a script with no refusal proves nothing about a rate limiter"
    );
    // And the final state is a real one rather than the initial one.
    assert_ne!(
        first_state.permitted(WindowSpan::Second),
        first_state.ceiling(WindowSpan::Second)
    );
}

/// P-32 — every declared rate bound is exact at the limit, and absence is not
/// zero.
///
/// The value sitting *on* the bound is accepted and the first one past it is
/// refused, naming the span. `MAX_CEILING` is what keeps `ceiling · day` inside
/// a `u64`, so a ceiling that slipped past it would not be a policy failure but
/// an arithmetic one.
#[test]
fn every_declared_rate_bound_is_exact_at_the_limit() {
    // On the bound, on every span.
    for span in WindowSpan::ALL {
        let governor = only(span, MAX_CEILING);
        assert_eq!(governor.ceiling(span), Some(MAX_CEILING));
        assert_eq!(governor.permitted(span), Some(MAX_CEILING));
    }

    // One past it, on every span, naming that span.
    assert_eq!(
        Governor::new(Some(MAX_CEILING + 1), None, None),
        Err(GovernorError::CeilingOutOfRange {
            span: WindowSpan::Second,
            ceiling: MAX_CEILING + 1,
            limit: MAX_CEILING,
        })
    );
    assert_eq!(
        Governor::new(None, Some(MAX_CEILING + 1), None),
        Err(GovernorError::CeilingOutOfRange {
            span: WindowSpan::Minute,
            ceiling: MAX_CEILING + 1,
            limit: MAX_CEILING,
        })
    );
    assert_eq!(
        Governor::new(Some(1), Some(1), Some(u32::MAX)),
        Err(GovernorError::CeilingOutOfRange {
            span: WindowSpan::Day,
            ceiling: u32::MAX,
            limit: MAX_CEILING,
        })
    );

    // Zero is refused rather than read as "no window". Absence is `None`, and
    // reading a configuration mistake as absence would turn a bound into an
    // unbounded pull.
    assert_eq!(
        Governor::new(Some(0), None, None),
        Err(GovernorError::CeilingIsZero {
            span: WindowSpan::Second,
        })
    );
    assert_eq!(
        Governor::new(None, Some(0), None),
        Err(GovernorError::CeilingIsZero {
            span: WindowSpan::Minute,
        })
    );
    assert_eq!(
        Governor::new(None, None, Some(0)),
        Err(GovernorError::CeilingIsZero {
            span: WindowSpan::Day,
        })
    );
    // One is the smallest ceiling there is, and it is accepted.
    assert_eq!(
        Governor::new(Some(1), None, None)
            .expect("one is a ceiling")
            .permitted(WindowSpan::Second),
        Some(1)
    );
}

/// P-33 — the published figures in `pull::rate` are the ones
/// `docs/00-charter.md` §4 records, and no others.
///
/// Frozen against hardcoded numbers rather than re-derived, for the reason
/// `pull::unit::the_exchange_and_segment_codes_are_frozen` gives: a constant
/// checked against itself is checked against nothing. A vendor figure changes
/// by amending the charter, and this test is what makes that amendment visible
/// here.
#[test]
fn the_published_vendor_figures_are_the_ones_the_charter_records() {
    assert_eq!(DHAN_PER_SECOND, 5);
    assert_eq!(DHAN_PER_DAY, 100_000);
    assert_eq!(GROWW_PER_MINUTE, 500);
    assert_eq!(GROWW_PER_SECOND_UNVERIFIED, 8);

    // The charter's row for the secondary vendor reads "5/s, 100,000/day, no
    // per-minute governor". All three halves of that survive construction.
    let dhan = dhan_governor();
    assert_eq!(dhan.ceiling(WindowSpan::Second), Some(5));
    assert_eq!(dhan.ceiling(WindowSpan::Day), Some(100_000));
    assert_eq!(dhan.ceiling(WindowSpan::Minute), None);

    // The primary vendor's row reads "500 requests per minute. No daily quota."
    let groww = Governor::new(
        Some(GROWW_PER_SECOND_UNVERIFIED),
        Some(GROWW_PER_MINUTE),
        None,
    )
    .expect("within bounds");
    assert_eq!(groww.ceiling(WindowSpan::Minute), Some(500));
    assert_eq!(groww.ceiling(WindowSpan::Day), None);
    assert_eq!(groww.ceiling(WindowSpan::Second), Some(8));
}

/// P-34 — a governor's whole state is a fixed-size struct: no allocation, no
/// history, no growth.
///
/// `Copy` is the proof rather than the convenience — a `Copy` type cannot own
/// an allocation, so its state is exactly its `size_of`, and a governor that
/// has admitted ten thousand requests is byte-for-byte the same size as one
/// that has admitted none.
#[test]
fn a_governor_holds_no_allocation_and_no_history() {
    fn only_copy_types_pass<T: Copy>(_: &T) {}

    let fresh = dhan_governor();
    only_copy_types_pass(&fresh);
    assert!(size_of::<Governor>() <= 128, "{}", size_of::<Governor>());
    assert_eq!(WINDOW_COUNT, 3);
    assert_eq!(WindowSpan::ALL.len(), WINDOW_COUNT);

    let mut worked = fresh;
    for tick in 0..10_000u64 {
        let _ = worked.admit(tick * 1_000);
        worked.record_success();
    }
    assert_eq!(size_of_val(&worked), size_of_val(&fresh));
    assert_eq!(size_of_val(&worked), size_of::<Governor>());
    // Ten thousand requests later it is still a value, not a handle: a copy
    // taken here diverges from the original rather than sharing with it.
    let mut copy = worked;
    for _ in 0..20 {
        let _ = copy.admit(10_000_000);
    }
    assert_ne!(
        copy.credit_micro_permits(WindowSpan::Second),
        worked.credit_micro_permits(WindowSpan::Second)
    );
    assert_eq!(size_of_val(&copy), size_of::<Governor>());
}

/// A vendor that publishes no bound at all is admitted without limit, and that
/// is said in the constructor call rather than defaulted into.
///
/// Three `None`s is a statement. It is the only way to reach this state, so it
/// cannot be arrived at by a missing configuration — which is the distinction
/// `CLAUDE.md` §4 draws between degrading loudly and degrading silently.
#[test]
fn a_governor_with_no_published_bound_admits_everything() {
    let mut governor = Governor::new(None, None, None).expect("no bound is a legal shape");
    for _ in 0..1_000 {
        assert_eq!(governor.admit(0), Verdict::Admit);
    }
    for span in WindowSpan::ALL {
        assert_eq!(governor.ceiling(span), None);
        assert_eq!(governor.permitted(span), None);
        assert_eq!(governor.credit_micro_permits(span), None);
    }
    // Feedback on a governor with nothing to govern is a no-op, not a panic.
    governor.record_success();
    governor.record_throttled();
    assert_eq!(governor.permitted(WindowSpan::Second), None);
    assert_eq!(governor.admit(0), Verdict::Admit);
    assert_eq!(governor.cursor_micros(), 0);
}

/// The span lengths, their order and their names are frozen.
///
/// A span whose length drifted would move every bound in this module at once,
/// silently, and the arithmetic would still look right.
#[test]
fn the_span_lengths_and_their_names_are_frozen() {
    assert_eq!(MICROS_PER_SECOND, 1_000_000);
    assert_eq!(MICROS_PER_MINUTE, 60_000_000);
    assert_eq!(MICROS_PER_DAY, 86_400_000_000);
    assert_eq!(WindowSpan::Second.len_micros(), MICROS_PER_SECOND);
    assert_eq!(WindowSpan::Minute.len_micros(), MICROS_PER_MINUTE);
    assert_eq!(WindowSpan::Day.len_micros(), MICROS_PER_DAY);
    // Shortest first — the order a tie is broken in.
    assert_eq!(
        WindowSpan::ALL,
        [WindowSpan::Second, WindowSpan::Minute, WindowSpan::Day]
    );
    assert!(WindowSpan::Second < WindowSpan::Minute);
    assert!(WindowSpan::Minute < WindowSpan::Day);

    let names: Vec<String> = WindowSpan::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(names, vec!["second", "minute", "day"]);

    assert_eq!(
        RequestKind::ALL,
        [RequestKind::Live, RequestKind::Historical]
    );
    let kinds: Vec<String> = RequestKind::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(kinds, vec!["live", "historical"]);
    assert!(RequestKind::Live < RequestKind::Historical);
}

/// Every rate refusal reads differently, and each one is an `Error`.
///
/// The same standard `pull::unit::every_manifest_error_prints_something_distinct`
/// sets for the manifest: two refusals that render identically are one refusal
/// an operator cannot act on.
#[test]
fn every_rate_refusal_reads_differently() {
    let refusals = [
        GovernorError::CeilingIsZero {
            span: WindowSpan::Second,
        },
        GovernorError::CeilingIsZero {
            span: WindowSpan::Minute,
        },
        GovernorError::CeilingIsZero {
            span: WindowSpan::Day,
        },
        GovernorError::CeilingOutOfRange {
            span: WindowSpan::Second,
            ceiling: 2,
            limit: 1,
        },
        GovernorError::CeilingOutOfRange {
            span: WindowSpan::Minute,
            ceiling: 2,
            limit: 1,
        },
        GovernorError::CeilingOutOfRange {
            span: WindowSpan::Day,
            ceiling: 2,
            limit: 1,
        },
    ];
    let rendered: HashSet<String> = refusals.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), refusals.len(), "two refusals read the same");
    for refusal in refusals {
        assert!(!refusal.to_string().is_empty());
        assert!(!format!("{refusal:?}").is_empty());
        let _: &dyn std::error::Error = &refusal;
    }
    // The refusal names the span in words, so a log line is readable without
    // the enum in front of it.
    assert!(
        GovernorError::CeilingIsZero {
            span: WindowSpan::Day
        }
        .to_string()
        .contains("per-day")
    );
    assert!(
        GovernorError::CeilingOutOfRange {
            span: WindowSpan::Second,
            ceiling: 9,
            limit: 8,
        }
        .to_string()
        .contains("per-second")
    );

    // A verdict is inspectable too — a refusal nobody can print is a refusal
    // nobody can report.
    assert!(!format!("{:?}", Verdict::Admit).is_empty());
    assert!(
        !format!(
            "{:?}",
            Verdict::Deny {
                span: WindowSpan::Minute,
                wait_micros: 7,
            }
        )
        .is_empty()
    );
    assert!(
        !format!(
            "{:?}",
            PoolKey {
                vendor: Vendor::Dhan,
                kind: RequestKind::Live,
            }
        )
        .is_empty()
    );
    assert_eq!(
        PoolKey {
            vendor: Vendor::Dhan,
            kind: RequestKind::Live,
        }
        .to_string(),
        "dhan/live"
    );
}

/// P-35 — the governor converges downward onto a rate the vendor honours, and
/// back up when it stops refusing.
///
/// The operator's requirement in one test: *auto-decremented* when the vendor
/// pushes back, *auto-incremented* when it stops. The vendor here honours 3 per
/// second while publishing 8 — the exact shape `docs/00-charter.md` §4 warns
/// about with its **UNVERIFIED** per-second row.
#[test]
fn the_allowance_converges_onto_the_rate_the_vendor_actually_honours() {
    const HONOURED: u32 = 3;
    let mut governor = only(WindowSpan::Second, GROWW_PER_SECOND_UNVERIFIED);
    let mut issued_this_second = 0u32;

    for tick in 0..600u64 {
        let now = tick * 100_000;
        if now % MICROS_PER_SECOND == 0 {
            issued_this_second = 0;
        }
        if governor.admit(now) == Verdict::Admit {
            issued_this_second += 1;
            if issued_this_second > HONOURED {
                governor.record_throttled();
            } else {
                governor.record_success();
            }
        }
    }

    // It settled at or below what the vendor honours, and never at zero.
    let settled = governor.permitted(WindowSpan::Second).expect("bounded");
    assert!(
        (1..=HONOURED).contains(&settled),
        "settled at {settled}, outside 1..={HONOURED}"
    );
    // It never passed the published ceiling on the way.
    assert!(settled <= GROWW_PER_SECOND_UNVERIFIED);

    // And when the vendor stops refusing, it walks back up to the published
    // ceiling and stops there.
    for _ in 0..GROWW_PER_SECOND_UNVERIFIED {
        governor.record_success();
    }
    assert_eq!(
        governor.permitted(WindowSpan::Second),
        Some(GROWW_PER_SECOND_UNVERIFIED)
    );
}

// ===========================================================================
// The session window and the calendar — `crates/pull/src/session.rs`
//
// Every number asserted below was computed independently of the code under
// test before it was written down, and the two exhaustive walks re-derive
// the calendar from a second, unrelated definition rather than from the one
// they are checking. A test that calls the implementation twice and compares
// it to itself proves only that it is deterministic.
// ===========================================================================

use pull::session::{
    BARS_PER_REGULAR_SESSION, Cadence, Day, DropCensus, DropReason, IST_OFFSET_SECS, IstMoment,
    MAX_DAY_NUMBER, MAX_YEAR, MIN_YEAR, SECS_PER_DAY, SECS_PER_MINUTE, SESSION_CLOSE_MINUTE,
    SESSION_OPEN_MINUTE, SessionError, Window,
};

/// A date, or a panic naming it. Tests only.
fn d(year: u16, month: u8, day: u8) -> Day {
    Day::new(year, month, day).expect("a date that exists")
}

/// The epoch second of IST midnight on a date — **computed here from a second
/// definition**, not from `Day::days_from_epoch`.
///
/// This is the deliberately naive year-by-year, month-by-month count. It is
/// slow and it is obviously correct, which is exactly what a check on a
/// closed-form formula has to be.
fn naive_days_from_epoch(year: u32, month: u8, day: u8) -> u32 {
    let leap = |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let lengths: [u32; 12] = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut total = 0_u32;
    for y in MIN_YEAR..year {
        total += if leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        total += lengths[usize::from(m) - 1];
    }
    total + u32::from(day) - 1
}

/// The epoch second of `hh:mm:ss` IST on a date, from the naive counter.
fn ist_epoch(year: u16, month: u8, day: u8, hh: i64, mm: i64, ss: i64) -> i64 {
    i64::from(naive_days_from_epoch(u32::from(year), month, day)) * SECS_PER_DAY - IST_OFFSET_SECS
        + hh * 3_600
        + mm * SECS_PER_MINUTE
        + ss
}

/// The constants are the charter's numbers, and they are pinned to each other.
///
/// `docs/00-charter.md` §3: 09:15 inclusive, 15:30 exclusive, last bar 15:29,
/// 375 bars. An operator asserted 15:40; the exchange says 15:30. If any one
/// of these four drifts, the other three make it visible here.
#[test]
fn the_session_constants_are_the_charters_numbers() {
    assert_eq!(SESSION_OPEN_MINUTE, 555, "09:15 is minute 555 of the day");
    assert_eq!(SESSION_CLOSE_MINUTE, 930, "15:30 is minute 930");
    assert_eq!(SESSION_OPEN_MINUTE, 9 * 60 + 15);
    assert_eq!(SESSION_CLOSE_MINUTE, 15 * 60 + 30);
    assert_eq!(BARS_PER_REGULAR_SESSION, 375);
    assert_eq!(
        SESSION_CLOSE_MINUTE - SESSION_OPEN_MINUTE,
        BARS_PER_REGULAR_SESSION,
        "the two bounds and the bar count are one statement"
    );
    // The last bar OPENS at 15:29. 15:40 would be 385 bars, not 375.
    assert_eq!(SESSION_CLOSE_MINUTE - 1, 15 * 60 + 29);
    assert_eq!((15 * 60 + 40) - SESSION_OPEN_MINUTE, 385);

    assert_eq!(IST_OFFSET_SECS, 19_800, "+05:30, fixed, no daylight saving");
    assert_eq!(IST_OFFSET_SECS, 5 * 3_600 + 30 * 60);
    assert_eq!(SECS_PER_DAY, 86_400);
    assert_eq!(SECS_PER_MINUTE, 60);
    assert_eq!((MIN_YEAR, MAX_YEAR), (1_970, 9_999));
}

/// The day-count ceiling in `session.rs` is `u32::MAX` spelled as a literal,
/// because `i64::from` is not const-stable. This is the check the source
/// cannot make about itself.
#[test]
fn the_day_count_ceiling_is_exactly_u32_max() {
    assert_eq!(i64::from(u32::MAX), 4_294_967_295);
    // And the largest day count the calendar can actually name is far below
    // it — the gap between the two is what `Day::from_days` still refuses.
    assert_eq!(MAX_DAY_NUMBER, 2_932_896);
    assert!(i64::from(MAX_DAY_NUMBER) < i64::from(u32::MAX));
}

/// **The exhaustive one.** All 2,932,897 day counts this build can name, in
/// both directions, against a second definition of the Gregorian calendar.
///
/// Not a sample. `Day::days_from_epoch` and `Day::from_days` are the closed-form
/// civil-calendar pair, and the only honest check on a closed form is every
/// input it accepts. Running under a debug profile this also proves that no
/// intermediate in either direction overflows or underflows, because a debug
/// build panics on both — which is the whole reason the walk is not run only
/// in release.
///
/// It checks, for every one of the 2,932,897 days:
///   * `from_days` names a date, and `days_from_epoch` names the count back;
///   * the date matches an independent year-by-year, month-by-month count;
///   * the count is strictly monotonic in calendar order;
///   * `succ` advances by exactly one day count, everywhere except the last;
///   * `year_month` succeeds — the "unreachable in practice" error, checked
///     against practice rather than asserted.
#[test]
fn the_calendar_round_trips_every_day_this_build_can_name() {
    // The naive counter is O(years) per call, so it is advanced incrementally
    // here rather than called 2.9 million times: it is re-derived from scratch
    // only at each year boundary, and stepped by one in between. The
    // comparison is still against a definition that shares nothing with the
    // formula under test.
    let mut expect_year: u16 = 1970;
    let mut expect_month: u8 = 1;
    let mut expect_day: u8 = 1;
    let leap = |y: u16| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let length = |y: u16, m: u8| -> u8 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if leap(y) {
                    29
                } else {
                    28
                }
            }
            other => panic!("month {other}"),
        }
    };

    let mut walked: u64 = 0;
    let mut leap_days_seen: u32 = 0;
    let mut previous: Option<Day> = None;

    for days in 0..=MAX_DAY_NUMBER {
        let got = Day::from_days(days).expect("inside the nameable range");

        // 1. It is the date the independent counter says it is.
        assert_eq!(
            (got.year(), got.month(), got.day()),
            (expect_year, expect_month, expect_day),
            "day {days} is not what a day-by-day walk says it is"
        );

        // 2. And the count comes back.
        assert_eq!(
            got.days_from_epoch(),
            days,
            "{got} did not round-trip its day count"
        );

        // 3. Rendering is fixed width and re-parses by eye.
        let text = got.to_string();
        assert_eq!(text.len(), 10, "{text} is not YYYY-MM-DD");

        // 4. `year_month` — the refusal the source calls unreachable.
        let ym = got
            .year_month()
            .expect("a validated Day is inside YearMonth's bounds");
        assert_eq!((ym.year(), ym.month()), (got.year(), got.month()));

        // 5. Monotonic, and `succ` steps by exactly one.
        if let Some(before) = previous {
            assert!(before < got, "{before} is not before {got}");
            assert_eq!(
                before.days_from_epoch() + 1,
                days,
                "{before} to {got} is not one day"
            );
            assert_eq!(before.succ().expect("not the last day"), got);
        }
        previous = Some(got);

        if (expect_month, expect_day) == (2, 29) {
            leap_days_seen += 1;
        }
        walked += 1;

        // Advance the independent counter.
        if expect_day < length(expect_year, expect_month) {
            expect_day += 1;
        } else if expect_month < 12 {
            expect_month += 1;
            expect_day = 1;
        } else {
            expect_year += 1;
            expect_month = 1;
            expect_day = 1;
        }
    }

    assert_eq!(walked, 2_932_897, "the walk did not cover the whole range");
    assert_eq!(
        previous.expect("at least one day"),
        d(9999, 12, 31),
        "the walk did not end on the last nameable date"
    );
    // 1970..=9999. Multiples of 4 in it: 1972, 1976 ... 9996, which is 2,007.
    // Of those, 80 are multiples of 100 and only 20 of those 80 are multiples
    // of 400, so 60 centuries are NOT leap years: 2,007 − 60 = 1,947.
    //
    // This number was written down as 1,948 first, from arithmetic done by
    // hand, and the exhaustive walk is what caught it. That is the whole
    // argument for walking every day instead of sampling.
    assert_eq!(leap_days_seen, 1_947, "the leap days do not add up");
    let independently = (MIN_YEAR..=MAX_YEAR)
        .filter(|y| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)))
        .count();
    assert_eq!(u32::try_from(independently).expect("small"), leap_days_seen);
}

/// The century boundaries, which is where naive leap logic dies.
///
/// 2000 IS a leap year — divisible by 400. 2100 is NOT — divisible by 100 and
/// not by 400. A `year % 4 == 0` rule gets 2100 wrong; a `year % 100 != 0`
/// rule gets 2000 wrong. Both are checked here from both directions.
#[test]
fn the_century_boundaries_are_where_naive_leap_logic_dies() {
    // Divisible by 400 — February has 29 days.
    for year in [2000_u16, 2400, 2800, 3200, 3600, 4000, 8000, 9600] {
        assert!(
            Day::new(year, 2, 29).is_ok(),
            "{year} is divisible by 400 and IS a leap year"
        );
        assert_eq!(d(year, 2, 28).succ().expect("next"), d(year, 2, 29));
        assert_eq!(d(year, 2, 29).succ().expect("next"), d(year, 3, 1));
    }

    // Divisible by 100, not by 400 — February has 28 days.
    for year in [2100_u16, 2200, 2300, 2500, 2600, 2700, 9700, 9800, 9900] {
        assert_eq!(
            Day::new(year, 2, 29),
            Err(SessionError::DayOutOfRange {
                day: 29,
                month_len: 28
            }),
            "{year} is divisible by 100 but not 400 and is NOT a leap year"
        );
        assert_eq!(d(year, 2, 28).succ().expect("next"), d(year, 3, 1));
    }

    // The ordinary cases either side, so the rule is not merely "centuries".
    assert!(Day::new(2024, 2, 29).is_ok(), "2024 is a leap year");
    assert!(Day::new(2023, 2, 29).is_err(), "2023 is not");
    assert!(Day::new(1996, 2, 29).is_ok());
    assert!(Day::new(9996, 2, 29).is_ok(), "the last leap year in range");
    assert!(Day::new(9999, 2, 29).is_err());

    // 1900 is NOT a leap year, and this build cannot be asked: it is below
    // MIN_YEAR, so the question is refused before the leap rule is consulted.
    // Stated rather than skipped — the 1900 case is UNREACHABLE here, not
    // passing.
    assert_eq!(
        Day::new(1900, 2, 29),
        Err(SessionError::YearOutOfRange { year: 1900 }),
        "1900 is refused as a year, not as a non-leap February"
    );
    assert_eq!(
        Day::new(1900, 2, 28),
        Err(SessionError::YearOutOfRange { year: 1900 }),
        "and so is every other date in it"
    );

    // Day counts either side of a century that is not a leap year.
    assert_eq!(
        d(2100, 2, 28).days_from_epoch() + 1,
        d(2100, 3, 1).days_from_epoch()
    );
    // And either side of one that is.
    assert_eq!(
        d(2000, 2, 28).days_from_epoch() + 2,
        d(2000, 3, 1).days_from_epoch()
    );
}

/// `Day::new` accepts exactly the dates that exist — checked against an
/// independent month-length table over every month and every plausible day.
#[test]
fn a_date_that_does_not_exist_is_refused_with_the_length_that_month_has() {
    let table = |y: u16, m: u8| -> u8 {
        let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => 0,
        }
    };
    for year in [1970_u16, 1999, 2000, 2023, 2024, 2100, 2400, 9999] {
        for month in 0..=13_u8 {
            for day in 0..=32_u8 {
                let got = Day::new(year, month, day);
                if month == 0 || month > 12 {
                    assert_eq!(
                        got,
                        Err(SessionError::MonthOutOfRange { month }),
                        "{year}-{month}-{day}"
                    );
                    continue;
                }
                let len = table(year, month);
                if day == 0 || day > len {
                    assert_eq!(
                        got,
                        Err(SessionError::DayOutOfRange {
                            day,
                            month_len: len
                        }),
                        "{year}-{month}-{day} must be refused and must name {len}"
                    );
                } else {
                    let ok = got.expect("that date exists");
                    assert_eq!((ok.year(), ok.month(), ok.day()), (year, month, day));
                }
            }
        }
    }

    // The year bound, on both sides of both ends.
    assert_eq!(
        Day::new(1969, 12, 31),
        Err(SessionError::YearOutOfRange { year: 1969 })
    );
    assert!(Day::new(1970, 1, 1).is_ok(), "the first nameable date");
    assert!(Day::new(9999, 12, 31).is_ok(), "the last nameable date");
    assert_eq!(
        Day::new(10_000, 1, 1),
        Err(SessionError::YearOutOfRange { year: 10_000 })
    );
    assert_eq!(
        Day::new(u16::MAX, 1, 1),
        Err(SessionError::YearOutOfRange { year: 65_535 })
    );
    assert_eq!(
        Day::new(0, 1, 1),
        Err(SessionError::YearOutOfRange { year: 0 })
    );
}

/// `succ` at every boundary that matters, including the one that must refuse.
#[test]
fn succ_advances_the_month_the_year_and_february_and_refuses_at_the_end() {
    // Inside a month.
    assert_eq!(d(2024, 3, 14).succ().expect("next"), d(2024, 3, 15));
    // February in a leap year, and in one that is not.
    assert_eq!(d(2024, 2, 28).succ().expect("next"), d(2024, 2, 29));
    assert_eq!(d(2024, 2, 29).succ().expect("next"), d(2024, 3, 1));
    assert_eq!(d(2023, 2, 28).succ().expect("next"), d(2023, 3, 1));
    // Every month end in a leap year and in a common one.
    for year in [2023_u16, 2024] {
        for month in 1..=11_u8 {
            // The last day of the month, found by asking which days exist —
            // an independent route to the length `succ` has to agree with.
            let len = (1..=31_u8)
                .rfind(|&x| Day::new(year, month, x).is_ok())
                .expect("every month has at least one day");
            assert_eq!(
                d(year, month, len).succ().expect("next"),
                d(year, month + 1, 1),
                "{year}-{month:02} does not roll into the next month"
            );
        }
        assert_eq!(d(year, 12, 31).succ().expect("next"), d(year + 1, 1, 1));
    }
    // The year end.
    assert_eq!(d(2024, 12, 31).succ().expect("next"), d(2025, 1, 1));
    assert_eq!(d(1970, 12, 31).succ().expect("next"), d(1971, 1, 1));
    // And the end of the range: a refusal, never a wrap to 1970.
    assert_eq!(d(9999, 12, 31).succ(), Err(SessionError::NoNextDay));
    assert!(
        d(9999, 12, 30).succ().is_ok(),
        "the day before it still advances"
    );
    assert_eq!(d(9999, 11, 30).succ().expect("next"), d(9999, 12, 1));
    assert_eq!(
        SessionError::NoNextDay.to_string(),
        "there is no day after 9999-12-31"
    );
}

/// `from_days` refuses, in both profiles, the counts that used to make it
/// overflow. **This is the fault that panicked in debug and wrapped in
/// release**, giving a pure function two answers depending on how it was built.
#[test]
fn from_days_refuses_the_counts_that_overflow_the_epoch_shift() {
    // The nameable range, at both ends.
    assert_eq!(Day::from_days(0).expect("day zero"), d(1970, 1, 1));
    assert_eq!(
        Day::from_days(MAX_DAY_NUMBER).expect("the last"),
        d(9999, 12, 31)
    );

    // One past it: a year, named.
    assert_eq!(
        Day::from_days(MAX_DAY_NUMBER + 1),
        Err(SessionError::YearOutOfRange { year: 10_000 })
    );
    assert_eq!(
        Day::from_days(4_000_000),
        Err(SessionError::YearOutOfRange { year: 12_921 })
    );

    // The last count whose epoch shift still fits u32 — refused for its year,
    // and the year it names is enormous but real.
    assert_eq!(
        Day::from_days(4_294_247_827),
        Err(SessionError::YearOutOfRange { year: 11_759_221 })
    );
    // And the first that does not fit. BEFORE THE FIX this line panicked in a
    // debug build with "attempt to add with overflow", and in a release build
    // returned Err(YearOutOfRange { year: 1969 }) — a wrapped, plausible,
    // wrong answer.
    assert_eq!(
        Day::from_days(4_294_247_828),
        Err(SessionError::DayCountOutOfRange {
            days: 4_294_247_828
        })
    );
    assert_eq!(
        Day::from_days(u32::MAX),
        Err(SessionError::DayCountOutOfRange { days: u32::MAX })
    );
    assert_eq!(i64::from(u32::MAX) - 719_468 + 1, 4_294_247_828);
}

/// The `BeforeEpoch` boundary is the **IST instant**, not the sign of the
/// input — and the module used to claim otherwise.
#[test]
fn the_before_epoch_boundary_is_the_ist_instant_not_the_sign() {
    // Exactly 1970-01-01 00:00:00 IST. Negative, and accepted.
    let first = IstMoment::from_epoch_secs(-19_800).expect("IST midnight, day zero");
    assert_eq!(first.day(), d(1970, 1, 1));
    assert_eq!(first.minute_of_day(), 0);
    assert_eq!(first.second_of_minute(), 0);
    assert_eq!(-IST_OFFSET_SECS, -19_800);

    // One second before it: refused.
    assert_eq!(
        IstMoment::from_epoch_secs(-19_801),
        Err(SessionError::BeforeEpoch { secs: -19_801 })
    );

    // And a NEGATIVE second that is not refused, which is the case the doc
    // used to get wrong: -1 UTC is 05:29:59 IST on 1970-01-01.
    let minus_one = IstMoment::from_epoch_secs(-1).expect("still 1970-01-01 in IST");
    assert_eq!(minus_one.day(), d(1970, 1, 1));
    assert_eq!(minus_one.minute_of_day(), 5 * 60 + 29);
    assert_eq!(minus_one.second_of_minute(), 59);

    // Zero, for completeness: 05:30:00 IST.
    let zero = IstMoment::from_epoch_secs(0).expect("the epoch itself");
    assert_eq!(zero.day(), d(1970, 1, 1));
    assert_eq!(zero.minute_of_day(), 5 * 60 + 30);

    // The far end, which cannot overflow the add.
    assert_eq!(
        IstMoment::from_epoch_secs(i64::MIN),
        Err(SessionError::BeforeEpoch { secs: i64::MIN })
    );
    assert_eq!(
        IstMoment::from_epoch_secs(i64::MIN + 1),
        Err(SessionError::BeforeEpoch { secs: i64::MIN + 1 })
    );
    assert!(
        SessionError::BeforeEpoch { secs: -19_801 }
            .to_string()
            .contains("-19801")
    );
}

/// **The silent-wrong-answer fault.** A day count that does not fit `u32` was
/// truncated and came back as a plausible date.
#[test]
fn the_day_count_that_does_not_fit_u32_is_refused_not_truncated() {
    // 371,086,500,594,600 is IST day 4,294,982,646, which is 11761233-01-30.
    // That day count is 2^32 + 15,350, and `as u32` kept the low 32 bits:
    // BEFORE THE FIX this returned Ok(2012-01-11 00:00:00) — not a refusal,
    // not a panic, a date an operator would believe.
    let forged = 371_086_500_594_600_i64;
    assert_eq!(
        IstMoment::from_epoch_secs(forged),
        Err(SessionError::TimestampOutOfRange { secs: forged }),
        "a truncated day count must be a refusal, never a plausible date"
    );
    // The arithmetic that made it dangerous, stated so the case cannot be
    // mistaken for an arbitrary large number.
    let day_number = (forged + IST_OFFSET_SECS) / SECS_PER_DAY;
    assert_eq!(day_number, 4_294_982_646);
    assert_eq!(day_number - (i64::from(u32::MAX) + 1), 15_350);
    assert_eq!(
        Day::from_days(15_350).expect("the date it forged"),
        d(2012, 1, 11)
    );

    // The last second whose day count still fits u32, and the first that does
    // not. Both are refusals now, by two different internal routes, and an
    // operator cannot tell them apart — which is correct, the column is not
    // epoch seconds either way.
    let fits = (i64::from(u32::MAX) + 1) * SECS_PER_DAY - 1 - IST_OFFSET_SECS;
    assert_eq!(fits, 371_085_174_354_599);
    assert_eq!((fits + IST_OFFSET_SECS) / SECS_PER_DAY, i64::from(u32::MAX));
    assert_eq!(
        IstMoment::from_epoch_secs(fits),
        Err(SessionError::TimestampOutOfRange { secs: fits })
    );
    assert_eq!(
        IstMoment::from_epoch_secs(fits + 1),
        Err(SessionError::TimestampOutOfRange { secs: fits + 1 })
    );

    // Both internal routes to that refusal are live rather than one of them
    // being an arm no input reaches. `fits` produces day count u32::MAX, which
    // the calendar refuses for its epoch shift; the last nameable second plus
    // one produces day count 2,932,897, which it refuses for its year.
    assert_eq!(
        Day::from_days(u32::MAX),
        Err(SessionError::DayCountOutOfRange { days: u32::MAX })
    );
    let past_the_end = 253_402_281_000_i64;
    assert_eq!(
        (past_the_end + IST_OFFSET_SECS) / SECS_PER_DAY,
        i64::from(MAX_DAY_NUMBER) + 1
    );
    assert_eq!(
        Day::from_days(MAX_DAY_NUMBER + 1),
        Err(SessionError::YearOutOfRange { year: 10_000 })
    );
    assert_eq!(
        IstMoment::from_epoch_secs(past_the_end),
        Err(SessionError::TimestampOutOfRange { secs: past_the_end })
    );
}

/// The `checked_add(19_800)` overflow, on both sides of the exact second.
#[test]
fn the_ist_offset_add_overflows_at_one_exact_second_and_is_checked() {
    let last_that_adds = i64::MAX - IST_OFFSET_SECS;
    assert_eq!(last_that_adds, 9_223_372_036_854_756_007);
    // It does not overflow — and it is still refused, for being past 9999.
    assert_eq!(
        IstMoment::from_epoch_secs(last_that_adds),
        Err(SessionError::TimestampOutOfRange {
            secs: last_that_adds
        })
    );
    // One more, and the add itself overflows. Same refusal, no panic, no wrap.
    assert_eq!(
        IstMoment::from_epoch_secs(last_that_adds + 1),
        Err(SessionError::TimestampOutOfRange {
            secs: last_that_adds + 1
        })
    );
    assert_eq!(
        IstMoment::from_epoch_secs(i64::MAX),
        Err(SessionError::TimestampOutOfRange { secs: i64::MAX })
    );
    assert_eq!(last_that_adds.checked_add(IST_OFFSET_SECS), Some(i64::MAX));
    assert_eq!((last_that_adds + 1).checked_add(IST_OFFSET_SECS), None);
}

/// The last epoch second this build can name, and the first it cannot.
#[test]
fn the_nameable_epoch_seconds_end_at_one_exact_second() {
    // 9999-12-31 23:59:59 IST.
    let last = 253_402_280_999_i64;
    assert_eq!(
        last,
        (i64::from(MAX_DAY_NUMBER) + 1) * SECS_PER_DAY - 1 - IST_OFFSET_SECS
    );
    let at = IstMoment::from_epoch_secs(last).expect("the last nameable second");
    assert_eq!(at.day(), d(9999, 12, 31));
    assert_eq!(at.minute_of_day(), 23 * 60 + 59);
    assert_eq!(at.second_of_minute(), 59);

    assert_eq!(
        IstMoment::from_epoch_secs(last + 1),
        Err(SessionError::TimestampOutOfRange { secs: last + 1 }),
        "one second later is 10000-01-01 IST and cannot be named"
    );
    // And the second before the last is ordinary.
    let before = IstMoment::from_epoch_secs(last - 1).expect("still 9999");
    assert!(before < at, "IstMoment orders chronologically");
    assert_eq!(before.second_of_minute(), 58);
}

/// The four exact seconds that define the session window. 09:15:00 is in,
/// 09:14:59 is out, 15:29:59 is in, 15:30:00 is out.
///
/// The date is **2025-02-01, a Saturday**, deliberately: the charter records it
/// as a full 375-bar Union Budget session, so a filter with a weekend rule in
/// it would fail this test rather than pass a weekday one.
#[test]
fn the_four_seconds_that_define_the_session_window() {
    let saturday = d(2025, 2, 1);
    let window = Window::new(saturday, saturday).expect("one day");

    // The literals were computed away from this code; the naive counter agrees.
    assert_eq!(ist_epoch(2025, 2, 1, 9, 15, 0), 1_738_381_500);
    assert_eq!(saturday.days_from_epoch(), 20_120);

    let cases = [
        (
            1_738_381_499_i64,
            Some(DropReason::BeforeSessionOpen),
            554,
            59,
        ),
        (1_738_381_500, None, 555, 0),
        (1_738_403_999, None, 929, 59),
        (
            1_738_404_000,
            Some(DropReason::AtOrAfterSessionClose),
            930,
            0,
        ),
    ];
    for (secs, expect, minute, second) in cases {
        let at = IstMoment::from_epoch_secs(secs).expect("a real second");
        assert_eq!(at.day(), saturday, "{secs}");
        assert_eq!(at.minute_of_day(), minute, "{secs}");
        assert_eq!(at.second_of_minute(), second, "{secs}");
        assert_eq!(
            at.in_regular_session(),
            expect.is_none(),
            "in_regular_session disagrees with the verdict at {secs}"
        );
        assert_eq!(
            window
                .verdict(secs, Cadence::Minute)
                .expect("a real second"),
            expect,
            "{secs}"
        );
    }

    // 15:30:00 is a DROP, not a keep. `<= 15:30` would admit a bar the
    // exchange never traded, and 15:40 would admit ten more minutes of them.
    assert_eq!(
        window
            .verdict(ist_epoch(2025, 2, 1, 15, 40, 0), Cadence::Minute)
            .expect("real"),
        Some(DropReason::AtOrAfterSessionClose),
        "the operator's asserted 15:40 close is outside the exchange's session"
    );
}

/// The whole Saturday, second by second: exactly 375 minute-bars survive, and
/// the count is the charter's number rather than a coincidence.
#[test]
fn a_saturday_is_a_full_session_because_there_is_no_weekend_rule() {
    let saturday = d(2025, 2, 1);
    let window = Window::new(saturday, saturday).expect("one day");
    let midnight = ist_epoch(2025, 2, 1, 0, 0, 0);
    assert_eq!(midnight, 1_738_348_200);

    let mut census = DropCensus::new();
    let mut kept_minutes = 0_u32;
    // Every minute of the day, at its exact opening second.
    for minute in 0..1_440_i64 {
        let secs = midnight + minute * SECS_PER_MINUTE;
        match window
            .verdict(secs, Cadence::Minute)
            .expect("a real second")
        {
            None => kept_minutes += 1,
            Some(why) => census.count(why),
        }
    }
    assert_eq!(
        kept_minutes, BARS_PER_REGULAR_SESSION,
        "a Saturday must yield a full 375-bar session — 2025-02-01 did"
    );
    assert_eq!(census.of(DropReason::BeforeSessionOpen), 555);
    assert_eq!(census.of(DropReason::AtOrAfterSessionClose), 1_440 - 930);
    assert_eq!(census.total(), 1_440 - 375);
    assert_eq!(census.total() + kept_minutes, 1_440);

    // And every day of that week behaves identically — Sunday included. If a
    // weekday were consulted anywhere, these seven would not agree.
    for day_of_month in 26..=31_u8 {
        let day = d(2025, 1, day_of_month);
        let one = Window::new(day, day).expect("one day");
        let base = ist_epoch(2025, 1, day_of_month, 0, 0, 0);
        let kept = (0..1_440_i64)
            .filter(|m| {
                one.verdict(base + m * SECS_PER_MINUTE, Cadence::Minute)
                    .expect("real")
                    .is_none()
            })
            .count();
        assert_eq!(
            kept, 375,
            "2025-01-{day_of_month:02} is not the same shape as every other day"
        );
    }
}

/// Every second of one day is classified, and the classification changes at
/// exactly two seconds. 86,400 inputs, no gaps and no double-counting.
#[test]
fn every_second_of_a_day_falls_on_exactly_one_side_of_the_session() {
    let day = d(2024, 2, 29); // a leap day, for good measure
    let window = Window::new(day, day).expect("one day");
    let midnight = ist_epoch(2024, 2, 29, 0, 0, 0);

    let mut kept = 0_u32;
    let mut before = 0_u32;
    let mut after = 0_u32;
    let mut transitions = 0_u32;
    let mut previous: Option<Option<DropReason>> = None;
    for offset in 0..SECS_PER_DAY {
        let verdict = window
            .verdict(midnight + offset, Cadence::Minute)
            .expect("a real second");
        match verdict {
            None => kept += 1,
            Some(DropReason::BeforeSessionOpen) => before += 1,
            Some(DropReason::AtOrAfterSessionClose) => after += 1,
            Some(other) => panic!("a bar inside its own window was dropped as {other}"),
        }
        if previous.is_some_and(|p| p != verdict) {
            transitions += 1;
        }
        previous = Some(verdict);
    }
    assert_eq!(kept, 375 * 60, "375 minutes of seconds are inside");
    assert_eq!(before, 555 * 60);
    assert_eq!(after, (1_440 - 930) * 60);
    assert_eq!(kept + before + after, 86_400, "every second was classified");
    assert_eq!(transitions, 2, "the verdict changes at exactly two seconds");
}

/// A daily bar is exempt from the intraday window — and a daily bar outside
/// the operator's dates is still dropped.
#[test]
fn a_daily_bar_is_exempt_from_the_session_filter() {
    let day = d(2012, 1, 11);
    let window = Window::new(day, day).expect("one day");

    // IST midnight, which is what the doctest's 1,326,220,200 actually is.
    let midnight = 1_326_220_200_i64;
    assert_eq!(midnight, ist_epoch(2012, 1, 11, 0, 0, 0));
    let at = IstMoment::from_epoch_secs(midnight).expect("real");
    assert_eq!(at.minute_of_day(), 0, "IST midnight exactly, not 09:15");
    assert!(
        !at.in_regular_session(),
        "midnight is outside the intraday session, which is the whole point"
    );

    assert_eq!(
        window.verdict(midnight, Cadence::Daily).expect("real"),
        None,
        "a daily bar at IST midnight must survive"
    );
    assert_eq!(
        window.verdict(midnight, Cadence::Minute).expect("real"),
        Some(DropReason::BeforeSessionOpen),
        "the same second at minute cadence is a drop — the exemption is real"
    );

    // Every minute of the day survives at daily cadence. An unconditional
    // intraday filter would drop 1,065 of the 1,440.
    let survived = (0..1_440_i64)
        .filter(|m| {
            window
                .verdict(midnight + m * SECS_PER_MINUTE, Cadence::Daily)
                .expect("real")
                .is_none()
        })
        .count();
    assert_eq!(survived, 1_440, "no daily stamp is ever out of session");

    // But a daily bar outside the operator's dates is still dropped. Exempt
    // from the session is not exempt from the window.
    assert_eq!(
        window
            .verdict(midnight - SECS_PER_DAY, Cadence::Daily)
            .expect("real"),
        Some(DropReason::BeforeWindow)
    );
    assert_eq!(
        window
            .verdict(midnight + SECS_PER_DAY, Cadence::Daily)
            .expect("real"),
        Some(DropReason::AfterWindow)
    );
    // The two cadences are distinguishable, and they read differently — a
    // `Cadence` that compared equal to itself only would exempt everything.
    assert_ne!(Cadence::Daily, Cadence::Minute);
    assert_ne!(
        format!("{:?}", Cadence::Minute),
        format!("{:?}", Cadence::Daily)
    );
}

/// A bar that is not minute-aligned is KEPT, and its offset is visible.
///
/// This pins current behaviour rather than endorsing it: `verdict` classifies
/// by minute, so 09:15:30 is inside the 09:15 minute. The misalignment is not
/// silently lost — `second_of_minute` reports it — but it is not a
/// `DropReason` either, and this test is what makes changing that deliberate.
#[test]
fn a_bar_that_is_not_minute_aligned_is_kept_and_its_offset_is_visible() {
    let day = d(2025, 2, 1);
    let window = Window::new(day, day).expect("one day");

    for second in 1..60_i64 {
        let secs = ist_epoch(2025, 2, 1, 9, 15, second);
        let at = IstMoment::from_epoch_secs(secs).expect("real");
        assert_eq!(at.minute_of_day(), SESSION_OPEN_MINUTE);
        assert_eq!(
            at.second_of_minute(),
            u32::try_from(second).expect("under 60"),
            "the offset must survive to the caller"
        );
        assert!(at.in_regular_session());
        assert_eq!(
            window.verdict(secs, Cadence::Minute).expect("real"),
            None,
            "a misaligned bar inside the session is kept, not dropped"
        );
    }

    // And a misaligned bar in the last minute is kept right up to 15:29:59,
    // while 15:30:00 — perfectly aligned — is not.
    assert_eq!(
        window
            .verdict(ist_epoch(2025, 2, 1, 15, 29, 59), Cadence::Minute)
            .expect("real"),
        None
    );
    assert_eq!(
        window
            .verdict(ist_epoch(2025, 2, 1, 15, 30, 0), Cadence::Minute)
            .expect("real"),
        Some(DropReason::AtOrAfterSessionClose)
    );
    // A misaligned bar one second before the open is still out.
    assert_eq!(
        window
            .verdict(ist_epoch(2025, 2, 1, 9, 14, 59), Cadence::Minute)
            .expect("real"),
        Some(DropReason::BeforeSessionOpen)
    );
}

/// `in_regular_session` and `verdict` are the same rule, on every minute of a
/// day. Two spellings of one bound is how a bound drifts.
#[test]
fn in_regular_session_agrees_with_the_verdict_on_every_minute() {
    let day = d(2024, 6, 3);
    let window = Window::new(day, day).expect("one day");
    let midnight = ist_epoch(2024, 6, 3, 0, 0, 0);
    let mut inside = 0_u32;
    for minute in 0..1_440_i64 {
        let secs = midnight + minute * SECS_PER_MINUTE;
        let at = IstMoment::from_epoch_secs(secs).expect("real");
        let by_moment = at.in_regular_session();
        let by_verdict = window
            .verdict(secs, Cadence::Minute)
            .expect("real")
            .is_none();
        assert_eq!(by_moment, by_verdict, "minute {minute} is classified twice");
        if by_moment {
            inside += 1;
        }
    }
    assert_eq!(inside, BARS_PER_REGULAR_SESSION);
}

/// The vendor's non-inclusive `toDate`: the wire date is the day after the
/// operator's last day, everywhere, including across every boundary.
#[test]
fn the_wire_to_date_is_the_day_after_the_operators_last_day() {
    let window = Window::new(d(2022, 1, 8), d(2022, 2, 8)).expect("a window");
    assert_eq!(window.to().to_string(), "2022-02-08");
    assert_eq!(window.wire_to().expect("next").to_string(), "2022-02-09");
    assert_eq!(window.from().to_string(), "2022-01-08");
    assert_eq!(window.to_string(), "2022-01-08..=2022-02-08");
    assert_eq!(window.days(), 32, "both ends included");

    // Across a month end, a year end and a leap day.
    for (to, expect) in [
        (d(2024, 1, 31), "2024-02-01"),
        (d(2024, 2, 28), "2024-02-29"),
        (d(2023, 2, 28), "2023-03-01"),
        (d(2024, 12, 31), "2025-01-01"),
        (d(2100, 2, 28), "2100-03-01"),
    ] {
        let w = Window::new(d(1970, 1, 1), to).expect("a window");
        assert_eq!(w.wire_to().expect("next").to_string(), expect);
    }

    // And the one window that has no wire date: it is refused, not wrapped
    // back to 1970, which would silently ask the vendor for the wrong range.
    let end = Window::new(d(9999, 12, 30), d(9999, 12, 31)).expect("a window");
    assert_eq!(end.wire_to(), Err(SessionError::NoNextDay));
    assert_eq!(
        Window::new(d(9999, 12, 30), d(9999, 12, 30))
            .expect("a window")
            .wire_to()
            .expect("next"),
        d(9999, 12, 31),
        "the day before still has one"
    );
}

/// An inclusive window survives the vendor's exclusive `toDate`: the extra day
/// the wire fetches comes back, and is dropped as **after the window** rather
/// than as an out-of-session bar.
#[test]
fn an_inclusive_window_survives_the_vendors_exclusive_to_date() {
    let window = Window::new(d(2022, 1, 8), d(2022, 2, 8)).expect("a window");
    let wire = window.wire_to().expect("next");

    // The operator's last day is inside; the wire day is not.
    assert!(window.contains(window.to()), "the last day is included");
    assert!(!window.contains(wire), "the wire day is not the operator's");

    // A bar at 09:15 on the operator's last day: kept.
    assert_eq!(
        window
            .verdict(ist_epoch(2022, 2, 8, 9, 15, 0), Cadence::Minute)
            .expect("real"),
        None,
        "without wire_to's +1 this bar would never have been fetched at all"
    );
    // The same bar on the extra day the wire brings back: dropped, and the
    // reason names the window rather than the session.
    assert_eq!(
        window
            .verdict(ist_epoch(2022, 2, 9, 9, 15, 0), Cadence::Minute)
            .expect("real"),
        Some(DropReason::AfterWindow)
    );
    // Even out-of-session bars on the extra day report the window, because the
    // window is checked first. Only one of the two reasons tells an operator
    // what happened.
    assert_eq!(
        window
            .verdict(ist_epoch(2022, 2, 9, 3, 0, 0), Cadence::Minute)
            .expect("real"),
        Some(DropReason::AfterWindow),
        "the window is checked before the session, deliberately"
    );
    assert_eq!(
        window
            .verdict(ist_epoch(2022, 1, 7, 3, 0, 0), Cadence::Minute)
            .expect("real"),
        Some(DropReason::BeforeWindow)
    );
}

/// The window's own arithmetic: inclusive at both ends, refused backwards,
/// and `days` costs the same for one day as for the whole range.
#[test]
fn a_window_is_inclusive_at_both_ends_and_refuses_to_run_backwards() {
    let one = Window::new(d(2024, 3, 15), d(2024, 3, 15)).expect("a one-day window");
    assert_eq!(one.days(), 1, "the commonest resume shape");
    assert!(one.contains(d(2024, 3, 15)));
    assert!(!one.contains(d(2024, 3, 14)));
    assert!(!one.contains(d(2024, 3, 16)));

    let all = Window::new(d(1970, 1, 1), d(9999, 12, 31)).expect("everything");
    assert_eq!(all.days(), 2_932_897, "every day this build can name");
    assert_eq!(all.days(), MAX_DAY_NUMBER + 1);

    // Backwards by one day, one month and one year — refused, never swapped.
    for (from, to) in [
        (d(2024, 3, 15), d(2024, 3, 14)),
        (d(2024, 3, 15), d(2024, 2, 15)),
        (d(2024, 3, 15), d(2023, 3, 15)),
        (d(9999, 12, 31), d(1970, 1, 1)),
    ] {
        assert_eq!(
            Window::new(from, to),
            Err(SessionError::WindowRunsBackwards { from, to }),
            "{from}..={to}"
        );
    }
    let backwards = SessionError::WindowRunsBackwards {
        from: d(2022, 2, 8),
        to: d(2022, 1, 8),
    };
    assert_eq!(
        backwards.to_string(),
        "window 2022-02-08..=2022-01-08 runs backwards"
    );

    // `contains` at both edges of a multi-day window.
    let month = Window::new(d(2024, 2, 1), d(2024, 2, 29)).expect("february");
    assert_eq!(month.days(), 29, "2024 is a leap year");
    assert!(month.contains(d(2024, 2, 1)) && month.contains(d(2024, 2, 29)));
    assert!(!month.contains(d(2024, 1, 31)) && !month.contains(d(2024, 3, 1)));
}

/// `Day` compares in calendar order, which is what a window comparison relies
/// on. Field order is load-bearing and a reordering would pass every other
/// test in this file.
#[test]
fn day_ordering_is_calendar_ordering() {
    assert!(d(2024, 1, 2) < d(2024, 2, 1), "month outranks day");
    assert!(d(2023, 12, 31) < d(2024, 1, 1), "year outranks month");
    assert!(d(2024, 3, 14) < d(2024, 3, 15));
    assert_eq!(d(2024, 3, 15), d(2024, 3, 15));
    let mut sorted = [
        d(2024, 3, 15),
        d(1970, 1, 1),
        d(9999, 12, 31),
        d(2024, 2, 29),
    ];
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            d(1970, 1, 1),
            d(2024, 2, 29),
            d(2024, 3, 15),
            d(9999, 12, 31)
        ]
    );
    // And the ordering agrees with the day count, which is the other spelling.
    for pair in sorted.windows(2) {
        assert!(pair[0].days_from_epoch() < pair[1].days_from_epoch());
    }
    let mut seen = HashSet::new();
    assert!(seen.insert(d(2024, 3, 15)));
    assert!(!seen.insert(d(2024, 3, 15)), "Hash agrees with Eq");
}

/// The census total is exactly the sum of its reasons — at zero, at one of
/// each, and at a mixed count.
#[test]
fn the_census_total_is_the_sum_of_its_reasons() {
    let reasons = [
        DropReason::BeforeSessionOpen,
        DropReason::AtOrAfterSessionClose,
        DropReason::BeforeWindow,
        DropReason::AfterWindow,
    ];

    // At zero.
    let empty = DropCensus::new();
    assert_eq!(empty.total(), 0);
    assert!(empty.is_empty());
    assert_eq!(empty, DropCensus::default());
    for r in reasons {
        assert_eq!(empty.of(r), 0);
    }

    // One of each: the total is four, and each slot is its own.
    let mut one = DropCensus::new();
    for r in reasons {
        one.count(r);
    }
    assert_eq!(one.total(), 4);
    assert!(!one.is_empty());
    for r in reasons {
        assert_eq!(one.of(r), 1, "{r} did not land in its own slot");
    }

    // A mixed count: 7 + 11 + 13 + 17. The reasons must not bleed into each
    // other, which a single shared counter would hide.
    let mut mixed = DropCensus::new();
    let counts = [7_u32, 11, 13, 17];
    for (r, n) in reasons.into_iter().zip(counts) {
        for _ in 0..n {
            mixed.count(r);
        }
    }
    let sum: u32 = reasons.iter().map(|&r| mixed.of(r)).sum();
    assert_eq!(
        mixed.total(),
        sum,
        "the total is not the sum of its reasons"
    );
    assert_eq!(mixed.total(), 48);
    for (r, n) in reasons.into_iter().zip(counts) {
        assert_eq!(mixed.of(r), n, "{r}");
    }

    // The invariant holds at every step of a walk, not only at the end.
    let mut running = DropCensus::new();
    for (i, r) in reasons.into_iter().cycle().take(37).enumerate() {
        running.count(r);
        let sum: u32 = reasons.iter().map(|&x| running.of(x)).sum();
        assert_eq!(running.total(), sum, "after {} drops", i + 1);
        assert_eq!(running.total(), u32::try_from(i + 1).expect("small"));
    }
}

/// Every drop reason prints something, and no two print the same thing. A
/// census an operator cannot read is a census that says one number.
#[test]
fn every_drop_reason_reads_differently() {
    let reasons = [
        DropReason::BeforeSessionOpen,
        DropReason::AtOrAfterSessionClose,
        DropReason::BeforeWindow,
        DropReason::AfterWindow,
    ];
    let rendered: HashSet<String> = reasons.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), reasons.len(), "two reasons read the same");
    for r in reasons {
        assert_eq!(r.to_string(), r.label(), "Display is the label");
        assert!(!r.label().is_empty());
        assert!(!format!("{r:?}").is_empty());
    }
    assert_eq!(
        DropReason::AtOrAfterSessionClose.label(),
        "at or after the session close",
        "the label says AT is a drop, because the close is exclusive"
    );
}

/// Every session refusal prints something, no two print the same thing, and
/// each carries the value it refused.
#[test]
fn every_session_error_prints_something_distinct() {
    let errors = [
        SessionError::BeforeEpoch { secs: -19_801 },
        SessionError::TimestampOutOfRange {
            secs: 253_402_281_000,
        },
        SessionError::DayCountOutOfRange { days: u32::MAX },
        SessionError::YearOutOfRange { year: 10_000 },
        SessionError::MonthOutOfRange { month: 13 },
        SessionError::DayOutOfRange {
            day: 29,
            month_len: 28,
        },
        SessionError::NoNextDay,
        SessionError::WindowRunsBackwards {
            from: d(2022, 2, 8),
            to: d(2022, 1, 8),
        },
    ];
    let rendered: HashSet<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(rendered.len(), errors.len(), "two refusals read the same");
    for error in errors {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").is_empty());
        let _: &dyn std::error::Error = &error;
    }
    // The refused value is IN the message, not summarised out of it.
    assert!(
        errors[0].to_string().contains("-19801"),
        "an operator must not need a hex dump: {}",
        errors[0]
    );
    assert!(errors[1].to_string().contains("253402281000"));
    assert!(errors[2].to_string().contains("4294967295"));
    assert!(errors[3].to_string().contains("10000"));
    assert!(errors[3].to_string().contains("1970"));
    assert!(errors[3].to_string().contains("9999"));
    assert!(errors[5].to_string().contains("29") && errors[5].to_string().contains("28"));
}

/// A timestamp that is not a timestamp is a REFUSAL from `verdict`, never a
/// drop. A drop is a bar this engine declined; a bar it could not read is the
/// vendor or the decoder being wrong, and counting it as a drop would bury it.
#[test]
fn an_unreadable_timestamp_is_refused_by_the_verdict_not_counted_as_a_drop() {
    let window = Window::new(d(2022, 1, 8), d(2022, 2, 8)).expect("a window");
    for bad in [
        i64::MIN,
        i64::MAX,
        -19_801,
        253_402_281_000,
        371_086_500_594_600,
    ] {
        let got = window.verdict(bad, Cadence::Minute);
        assert!(got.is_err(), "{bad} must be refused, not dropped");
        assert_eq!(
            got,
            IstMoment::from_epoch_secs(bad).map(|_| None),
            "the verdict must pass the moment's own refusal through unchanged"
        );
        // And at daily cadence too — the exemption is from the session, not
        // from being a readable timestamp.
        assert!(window.verdict(bad, Cadence::Daily).is_err());
    }
}

/// `year_month` is the store's month, and the refusal the source calls
/// unreachable is unreachable because the two bounds are identical — checked
/// over every year and month, not argued.
#[test]
fn year_month_is_the_stores_month_and_its_refusal_is_unreachable() {
    for year in MIN_YEAR..=MAX_YEAR {
        for month in 1..=12_u8 {
            let year = u16::try_from(year).expect("inside u16");
            let day = d(year, month, 1);
            let ym = day.year_month().expect("Day's bounds are YearMonth's");
            assert_eq!((ym.year(), ym.month()), (year, month));
            assert_eq!(ym, YearMonth::new(year, month).expect("the same bounds"));
        }
    }
    // The bounds are the same on both sides, which is *why* it is unreachable.
    assert!(YearMonth::new(1970, 1).is_ok() && Day::new(1970, 1, 1).is_ok());
    assert!(YearMonth::new(9999, 12).is_ok() && Day::new(9999, 12, 31).is_ok());
    assert_eq!(
        YearMonth::new(1969, 12),
        Err(PathError::YearOutOfRange { year: 1969 })
    );
    assert_eq!(
        Day::new(1969, 12, 31),
        Err(SessionError::YearOutOfRange { year: 1969 })
    );
    assert_eq!(
        YearMonth::new(10_000, 1),
        Err(PathError::YearOutOfRange { year: 10_000 })
    );
    // A Day carrying a month YearMonth would refuse cannot be built at all.
    assert!(Day::new(2024, 0, 1).is_err() && YearMonth::new(2024, 0).is_err());
    assert!(Day::new(2024, 13, 1).is_err() && YearMonth::new(2024, 13).is_err());
}

/// The module's own doctest number, checked rather than believed: 1,326,220,200
/// is IST **midnight**, not 09:15. The file once asserted the latter.
#[test]
fn the_daily_stamp_in_the_doctest_is_ist_midnight_exactly() {
    let secs = 1_326_220_200_i64;
    let local = secs + IST_OFFSET_SECS;
    assert_eq!(local, 1_326_240_000);
    assert_eq!(local % SECS_PER_DAY, 0, "the remainder is ZERO — midnight");
    assert_eq!(local / SECS_PER_DAY, 15_350);
    assert_eq!(Day::from_days(15_350).expect("real"), d(2012, 1, 11));

    let at = IstMoment::from_epoch_secs(secs).expect("real");
    assert_eq!(at.day().to_string(), "2012-01-11");
    assert_eq!((at.minute_of_day(), at.second_of_minute()), (0, 0));

    // 09:15 IST on the same day is 33,300 seconds later, and 03:45 UTC.
    let open = IstMoment::from_epoch_secs(secs + 33_300).expect("real");
    assert_eq!(9 * 3_600 + 15 * SECS_PER_MINUTE, 33_300);
    assert_eq!(open.minute_of_day(), SESSION_OPEN_MINUTE);
    assert_eq!(open.day(), d(2012, 1, 11));
    assert_eq!((secs + 33_300) % SECS_PER_DAY, 13_500, "03:45:00 UTC");
}

/// The `>` in `from_epoch_secs`'s day-count guard and `Day::from_days`'s own
/// ceiling refuse the same boundary with the same error.
///
/// This is why `cargo mutants` reports one surviving mutant on that line and
/// why it is an **equivalent** mutant rather than a missing test: at day count
/// `u32::MAX` exactly, the guard firing and the calendar refusing are
/// indistinguishable from outside. Asserted here so the claim in the source is
/// an executed fact. Checked over every second of the boundary day and both
/// neighbouring days — 259,200 of them — so it is a property, not a spot check.
#[test]
fn the_guard_and_the_calendar_refuse_the_same_boundary_identically() {
    // The boundary day: day count exactly u32::MAX.
    let base = i64::from(u32::MAX) * SECS_PER_DAY - IST_OFFSET_SECS;
    for offset in -SECS_PER_DAY..(2 * SECS_PER_DAY) {
        let secs = base + offset;
        assert_eq!(
            IstMoment::from_epoch_secs(secs),
            Err(SessionError::TimestampOutOfRange { secs }),
            "every second across the boundary is the same refusal, so no test \
             can tell `>` from `>=` here"
        );
    }

    // And the two routes, called directly, produce refusals that this function
    // maps onto that one variant.
    assert_eq!(
        Day::from_days(u32::MAX),
        Err(SessionError::DayCountOutOfRange { days: u32::MAX }),
        "the route `>` takes at the boundary"
    );
    // Every bound the guard could be given lies here, and both sides of each
    // refuse — which is why no choice of constant kills the mutant.
    for candidate in [
        MAX_DAY_NUMBER + 1,
        MAX_DAY_NUMBER + 2,
        4_000_000,
        4_294_247_827,
        4_294_247_828,
        u32::MAX - 1,
        u32::MAX,
    ] {
        assert!(
            Day::from_days(candidate).is_err(),
            "day count {candidate} must be refused by the calendar itself"
        );
        let at_that_day = i64::from(candidate) * SECS_PER_DAY - IST_OFFSET_SECS;
        assert_eq!(
            IstMoment::from_epoch_secs(at_that_day),
            Err(SessionError::TimestampOutOfRange { secs: at_that_day }),
            "and so must the timestamp that lands on it"
        );
    }
}

// ───────────────────────────── the vendor fetch ─────────────────────────────

/// The UTC epoch second of an IST wall-clock time on a day.
///
/// Computed from the calendar rather than written as a literal: a hardcoded
/// epoch second is a number nobody can check by reading, and the first draft
/// of these tests had one that was wrong by most of a day.
fn ist(day: pull::session::Day, hour: i64, minute: i64, second: i64) -> i64 {
    i64::from(day.days_from_epoch()) * 86_400 - pull::session::IST_OFFSET_SECS
        + hour * 3_600
        + minute * 60
        + second
}

/// The `zip` trap, refused whole rather than truncated.
///
/// A vendor returning 375 opens and 374 volumes would, under `zip`, yield 374
/// perfectly valid-looking bars and a manifest recording a complete window.
/// That is data loss with a green checkmark and nothing downstream can detect
/// it, because there is no later point at which the missing row is noticeable.
#[test]
fn seven_arrays_that_disagree_refuse_the_whole_window() {
    use pull::fetch::{FetchError, ParallelArrays, RawWindow};

    let short = ParallelArrays {
        open: vec![100, 101, 102],
        high: vec![100, 101, 102],
        low: vec![100, 101, 102],
        close: vec![100, 101, 102],
        volume: vec![10, 11],
        timestamp: vec![0, 60, 120],
        open_interest: Vec::new(),
    };
    let why = RawWindow::decode(&short).expect_err("the arrays disagree");
    assert_eq!(
        why,
        FetchError::LengthDisagreement {
            open: 3,
            high: 3,
            low: 3,
            close: 3,
            volume: 2,
            timestamp: 3,
            open_interest: 0,
        },
        "every length is named, so an operator sees WHICH field was truncated"
    );

    let text = why.to_string();
    assert!(
        text.contains("volume 2") && text.contains("open 3"),
        "the message carries the disagreement — got {text:?}"
    );
}

/// Open interest absent and open interest zero are different facts.
#[test]
fn absent_open_interest_is_the_null_sentinel_and_zero_is_zero() {
    use pull::fetch::{ParallelArrays, RawWindow};

    let without = ParallelArrays {
        open: vec![100],
        high: vec![100],
        low: vec![100],
        close: vec![100],
        volume: vec![1],
        timestamp: vec![0],
        open_interest: Vec::new(),
    };
    let with_zero = ParallelArrays {
        open_interest: vec![0],
        ..without.clone()
    };

    let a = RawWindow::decode(&without).expect("a legal window");
    let b = RawWindow::decode(&with_zero).expect("a legal window");

    assert_eq!(
        a.rows.first().expect("one row").open_interest,
        None,
        "the vendor did not send the field"
    );
    assert_eq!(
        b.rows.first().expect("one row").open_interest,
        Some(0),
        "the vendor measured zero, which is a measurement"
    );
}

/// W1: a vendor stamping IST wall-clock seconds is not read as UTC.
///
/// This is the fault that STORED a wrong answer rather than refusing — 45 bars
/// at times the exchange never traded, passing every validation. The encoding
/// is now dispatched from the vendor descriptor, so the two readings of the
/// same integer land on different instants and a test can see the difference.
#[test]
fn the_timestamp_encoding_is_dispatched_never_assumed() {
    use pull::fetch::{BarRequest, RawRow, RawWindow, land};
    use pull::session::{Cadence, Day, Window};
    use pull::vendor::{PriceScale, TimestampEncoding};

    // 2026-08-07 09:15:00 IST is 03:45:00 UTC.
    let day_of = pull::session::Day::new(2026, 8, 7).expect("a real date");
    let utc_open = ist(day_of, 9, 15, 0);
    let row = RawRow {
        timestamp: utc_open,
        open: 100,
        high: 100,
        low: 100,
        close: 100,
        volume: 1,
        open_interest: None,
    };
    let raw = RawWindow { rows: vec![row] };
    let day = Day::new(2026, 8, 7).expect("a real date");
    let request = BarRequest {
        instrument_id: String::new(),
        window: Window::new(day, day).expect("one day"),
        cadence: Cadence::Minute,
    };

    let as_utc = land(
        &raw,
        &request,
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Paisa,
    )
    .expect("a legal window");

    let as_ist = land(
        &raw,
        &request,
        TimestampEncoding::IstDateTimeText,
        PriceScale::Paisa,
    )
    .expect("a legal window");

    assert_ne!(
        as_utc.bars.first().map(|b| b.ts_micros),
        as_ist.bars.first().map(|b| b.ts_micros),
        "the same integer under two encodings must NOT land on the same \
         instant — if it does, the dispatch is decorative and W1 is back"
    );
    assert_eq!(
        as_utc.bars.len() + as_utc.census.total() as usize,
        1,
        "every row is either a bar or a counted drop, never neither"
    );
}

/// Rupees become paisa exactly, and a price that would overflow refuses.
#[test]
fn a_rupee_price_becomes_paisa_and_an_overflow_refuses() {
    use pull::fetch::{BarRequest, FetchError, RawRow, RawWindow, land};
    use pull::session::{Cadence, Day, Window};
    use pull::vendor::{PriceScale, TimestampEncoding};

    let day = Day::new(2026, 8, 7).expect("a real date");
    let request = BarRequest {
        instrument_id: String::new(),
        window: Window::new(day, day).expect("one day"),
        cadence: Cadence::Minute,
    };
    let at = ist(day, 9, 15, 0);

    let cheap = RawWindow {
        rows: vec![RawRow {
            timestamp: at,
            open: 250,
            high: 250,
            low: 250,
            close: 250,
            volume: 1,
            open_interest: None,
        }],
    };
    let landed = land(
        &cheap,
        &request,
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Rupees,
    )
    .expect("a legal window");
    assert_eq!(
        landed.bars.first().map(|b| b.open),
        Some(25_000),
        "250 rupees is 25,000 paisa — multiplied once, never rounded here"
    );

    let huge = RawWindow {
        rows: vec![RawRow {
            timestamp: at,
            open: i64::MAX,
            high: i64::MAX,
            low: i64::MAX,
            close: i64::MAX,
            volume: 1,
            open_interest: None,
        }],
    };
    let why = land(
        &huge,
        &request,
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Rupees,
    )
    .expect_err("i64::MAX rupees cannot become paisa");
    assert!(
        matches!(why, FetchError::PriceRefused { field: "open", .. }),
        "refused by field name, not silently wrapped — got {why:?}"
    );
}

/// The vendor's exclusive range end is converted at exactly one site.
#[test]
fn the_wire_end_honours_the_vendors_inclusivity() {
    use pull::fetch::wire_end;
    use pull::session::Day;
    use pull::vendor::RangeEnd;

    let last = Day::new(2026, 8, 5).expect("a real date");

    assert_eq!(
        wire_end(last, RangeEnd::Inclusive).expect("no successor needed"),
        last,
        "an inclusive vendor takes the operator's date unchanged"
    );
    assert_eq!(
        wire_end(last, RangeEnd::Exclusive).expect("the day after exists"),
        Day::new(2026, 8, 6).expect("a real date"),
        "an exclusive vendor takes the day AFTER — the correction the operator \
         never has to know about, made in one place so it cannot drift"
    );

    let end_of_time = Day::new(9999, 12, 31).expect("a real date");
    assert!(
        wire_end(end_of_time, RangeEnd::Exclusive).is_err(),
        "past 9999-12-31 there is no successor, and it refuses rather than wrapping"
    );
}

/// Every row is accounted for: a bar, or a drop with a named reason.
#[test]
fn every_row_is_either_a_bar_or_a_counted_drop() {
    use pull::fetch::{BarRequest, RawRow, RawWindow, land};
    use pull::session::{Cadence, Day, Window};
    use pull::vendor::{PriceScale, TimestampEncoding};

    let day = Day::new(2026, 8, 7).expect("a real date");
    let request = BarRequest {
        instrument_id: String::new(),
        window: Window::new(day, day).expect("one day"),
        cadence: Cadence::Minute,
    };

    // 09:14:59 IST (out), 09:15:00 (in), 15:29:59 (in), 15:30:00 (out) —
    // both sides of both boundaries, plus a bar on the day before the window.
    // Both sides of both session boundaries, plus a bar on the day before.
    let secs = [
        ist(day, 9, 14, 59),
        ist(day, 9, 15, 0),
        ist(day, 15, 29, 59),
        ist(day, 15, 30, 0),
        ist(day, 9, 15, 0) - 86_400,
    ];
    let rows = secs
        .iter()
        .map(|&t| RawRow {
            timestamp: t,
            open: 100,
            high: 100,
            low: 100,
            close: 100,
            volume: 1,
            open_interest: None,
        })
        .collect();

    let landed = land(
        &RawWindow { rows },
        &request,
        TimestampEncoding::EpochSecondsUtc,
        PriceScale::Paisa,
    )
    .expect("a legal window");

    assert_eq!(
        landed.bars.len() + landed.census.total() as usize,
        secs.len(),
        "rows in equals bars out plus drops counted — a row that vanished \
         without a reason is indistinguishable from one the vendor never sent"
    );
    assert!(!landed.bars.is_empty(), "the in-session bars survived");
    assert!(
        !landed.census.is_empty(),
        "the out-of-session ones were counted"
    );
}

// ─────────────── the vendor CSV decoder, against real shapes ───────────────

/// `TrueData`'s index layout, verbatim from the operator's 2022 archive.
#[test]
fn truedata_index_rows_decode_to_paisa_and_utc() {
    use pull::csv::{Columns, decode};

    // Copied byte-for-byte from NSE_IDX_TICK_20221003.zip -> BANKNIFTY.csv.
    let body = "20221003,09:07:41,38444.90,0,0\n\
                20221003,09:15:01,38445.65,0,0\n\
                20221003,15:31:42,38029.65,0,0\n";
    let rows = decode(body, Columns::TrueDataIndex).expect("the real shape");
    assert_eq!(rows.len(), 3, "five fields, no header, three data rows");

    assert_eq!(
        rows.first().map(|r| r.close),
        Some(3_844_490),
        "38444.90 rupees is 3,844,490 paisa — exact, never a float"
    );
    assert!(
        rows.windows(2).all(|w| w[0].timestamp < w[1].timestamp),
        "09:07:41 < 09:15:01 < 15:31:42 after the IST-to-UTC conversion"
    );
    // 09:07:41 is PRE-OPEN and 15:31:42 is POST-CLOSE. The decoder keeps both;
    // deciding is the session filter's job, and it counts them by reason.
    assert_eq!(
        rows.last()
            .map(|r| r.timestamp)
            .zip(rows.first().map(|r| r.timestamp)),
        Some((rows[2].timestamp, rows[0].timestamp)),
        "the decoder judges nothing — it decodes"
    );
}

/// GDFL's layout, and the date trap that would shift every bar by months.
#[test]
fn gdfl_dates_are_day_first_and_the_header_is_skipped() {
    use pull::csv::{Columns, decode};
    use pull::session::IstMoment;

    // Verbatim from GFDLNFO_TICK_01072025/Futures/-III/FINNIFTY-III.NFO.csv.
    let body = "Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest\n\
                FINNIFTY-III.NFO,01/07/2025,09:16:16,27674,0,0,0,0,65,65\n";
    let rows = decode(body, Columns::Gdfl).expect("the real shape");
    assert_eq!(rows.len(), 1, "the header row is skipped, not decoded");

    let at = IstMoment::from_epoch_secs(rows[0].timestamp).expect("a real instant");
    assert_eq!(
        (at.day().year(), at.day().month(), at.day().day()),
        (2025, 7, 1),
        "01/07/2025 is 1 JULY, not 7 January — reading it the other way shifts \
         every bar by months and produces a file that is internally consistent \
         and completely wrong"
    );
    assert_eq!(at.minute_of_day(), 9 * 60 + 16, "09:16 IST");
    assert_eq!(rows[0].close, 2_767_400, "27674 rupees in paisa");
}

/// One vendor, two segments, two column counts — the shape that breaks a
/// per-vendor layout.
#[test]
fn the_same_vendor_emits_different_column_counts_per_segment() {
    use pull::csv::{Columns, decode};

    let index_row = "20221003,09:15:01,38445.65,0,0";
    let futures_row = "20221003,09:15:01,38445.65,0,0,0,0,0,0";

    assert_eq!(Columns::TrueDataIndex.count(), 5);
    assert_eq!(Columns::TrueDataFutures.count(), 9);

    assert!(decode(index_row, Columns::TrueDataIndex).is_ok());
    assert!(decode(futures_row, Columns::TrueDataFutures).is_ok());

    // Crossed shapes must REFUSE, never silently read a price as a volume.
    let wrong =
        decode(index_row, Columns::TrueDataFutures).expect_err("five fields cannot be nine");
    assert!(
        wrong.to_string().contains("5 fields, expected 9"),
        "named by count so the operator sees which layout was assumed — got {wrong}"
    );
    assert!(decode(futures_row, Columns::TrueDataIndex).is_err());
}

/// The 12,145 ghosts that would be parsed as CSVs.
#[test]
fn macosx_ghosts_are_not_data() {
    use pull::csv::is_ghost;

    assert!(is_ghost(
        "__MACOSX/GFDLNFO_TICK_01072025/Options/._NIFTY.NFO.csv"
    ));
    assert!(is_ghost("GFDLNFO_TICK_01072025/Options/._NIFTY.NFO.csv"));
    assert!(is_ghost("GFDLNFO_TICK_01072025/.DS_Store"));
    assert!(!is_ghost(
        "GFDLNFO_TICK_01072025/Options/NIFTY25SEP2525700PE.NFO.csv"
    ));
    assert!(
        !is_ghost("NSE_IDX_TICK_20221003.zip"),
        "a real nested archive is not a ghost"
    );
}

/// Every way a line can be wrong refuses the whole file, by name.
#[test]
fn a_malformed_line_refuses_the_file_rather_than_the_line() {
    use pull::csv::{Columns, decode};

    let cases: [(&str, &str); 5] = [
        ("20221003,09:15:01,38445.65,0", "4 fields"),
        (
            "2022-10-03,09:15:01,38445.65,0,0",
            "a dashed date in a compact field",
        ),
        ("20221003,9:15:01,38445.65,0,0", "a one-digit hour"),
        ("20221003,25:00:00,38445.65,0,0", "hour 25"),
        ("20221003,09:15:01,38445.657,0,0", "a third decimal place"),
    ];
    for (line, why) in cases {
        assert!(
            decode(line, Columns::TrueDataIndex).is_err(),
            "{why} must refuse — a file missing an arbitrary subset of its rows \
             is not a shorter file, it is a wrong one"
        );
    }

    // CRLF is tolerated: `lines()` strips \n and leaves \r, which would turn
    // the last field into a number that does not parse.
    assert!(
        decode("20221003,09:15:01,38445.65,0,0\r\n", Columns::TrueDataIndex).is_ok(),
        "a Windows line ending is a file property, not a data fault"
    );
}

// ─────────────────────── the local-archive transport ───────────────────────

/// Ghosts are skipped at the walk, before anything is opened.
#[test]
fn the_walk_skips_ghosts_and_decodes_the_rest() {
    use pull::archive::{read_dir, total_rows};
    use pull::csv::Columns;

    let dir =
        std::env::temp_dir().join(format!("brutex-archive-{}-{}", std::process::id(), line!()));
    std::fs::create_dir_all(&dir).expect("a scratch dir");

    // Two real members, one AppleDouble ghost that ENDS IN .csv and is binary,
    // and one file that is not a CSV at all.
    std::fs::write(
        dir.join("NIFTY-I.NFO.csv"),
        "Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest\n\
         NIFTY-I.NFO,01/07/2025,09:16:16,27674,0,0,0,0,65,65\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("BANKNIFTY-I.NFO.csv"),
        "Ticker,Date,Time,LTP,BuyPrice,BuyQty,SellPrice,SellQty,LTQ,OpenInterest\n\
         BANKNIFTY-I.NFO,01/07/2025,09:16:17,55000.50,0,0,0,0,1,1\n",
    )
    .expect("write");
    // The hazard: binary, and named `.csv`.
    std::fs::write(dir.join("._NIFTY-I.NFO.csv"), [0x00u8, 0x05, 0x16, 0x07]).expect("write");
    std::fs::write(dir.join("README.txt"), "not data").expect("write");

    let members = read_dir(&dir, Columns::Gdfl).expect("the ghost is skipped, not parsed");
    assert_eq!(
        members.len(),
        2,
        "two real members — the AppleDouble stub would have failed as UTF-8 if \
         it had been opened, and 12,145 of them sit in the operator's GDFL zip"
    );
    assert_eq!(total_rows(&members), 2);
    assert_eq!(
        members
            .iter()
            .map(|m| m.instrument.as_str())
            .collect::<Vec<_>>(),
        ["BANKNIFTY-I", "NIFTY-I"],
        "sorted by path so an import is reproducible — filesystem order differs \
         between machines and CLAUDE.md §3 rule 5 requires same in, same out"
    );
    assert_eq!(
        members
            .first()
            .and_then(|m| m.rows.first())
            .map(|r| r.close),
        Some(5_500_050),
        "55000.50 rupees is 5,500,050 paisa"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A malformed member refuses the whole walk.
#[test]
fn one_bad_member_refuses_the_whole_walk() {
    use pull::archive::{ArchiveError, read_dir};
    use pull::csv::Columns;

    let dir =
        std::env::temp_dir().join(format!("brutex-archive-{}-{}", std::process::id(), line!()));
    std::fs::create_dir_all(&dir).expect("a scratch dir");
    std::fs::write(dir.join("GOOD.csv"), "20221003,09:15:01,38445.65,0,0\n").expect("write");
    std::fs::write(dir.join("BAD.csv"), "20221003,09:15:01,38445.65\n").expect("write");

    let why = read_dir(&dir, Columns::TrueDataIndex).expect_err("three fields is not five");
    assert!(
        matches!(why, ArchiveError::MemberMalformed { .. }),
        "named by member and line — a directory that yielded SOME of its \
         contracts is not a smaller import, it is one nobody can characterise \
         afterwards. Got {why:?}"
    );
    assert!(
        why.to_string().contains("BAD.csv"),
        "the message names the file"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A path that is not a directory refuses by name.
#[test]
fn a_missing_directory_refuses_rather_than_yielding_nothing() {
    use pull::archive::{ArchiveError, read_dir};
    use pull::csv::Columns;

    let nowhere = std::env::temp_dir().join("brutex-no-such-dir-ever");
    let why = read_dir(&nowhere, Columns::Gdfl).expect_err("it does not exist");
    assert!(
        matches!(why, ArchiveError::NotADirectory { .. }),
        "an absent folder is a refusal, never an empty successful import — the \
         two look identical downstream and only one of them is true"
    );
}

// ───────────────────── folding snapshots, and the ladder ─────────────────────

/// The collision that produced zero bars from 354,675 real rows.
#[test]
fn snapshots_sharing_a_second_fold_into_one_bar() {
    use pull::fold::{Bucket, fold};
    use store::format::Bar;

    let snap = |s: i64, p: i64| Bar {
        ts_micros: s * 1_000_000,
        open: p,
        high: p,
        low: p,
        close: p,
        volume: 1,
        open_interest: i64::MIN,
    };
    // Four rows sharing 09:16:16, exactly what GDFL ships, then one later.
    let raw = [
        snap(0, 100),
        snap(0, 130),
        snap(0, 90),
        snap(0, 110),
        snap(59, 105),
    ];

    let bars = fold(&raw, Bucket::MINUTE).expect("non-decreasing");
    assert_eq!(bars.len(), 1, "five snapshots in one minute become ONE bar");
    assert_eq!(bars[0].ts_micros, 0, "stamped at the START of its bucket");
    assert_eq!(bars[0].open, 100, "first in FILE order, not sorted");
    assert_eq!(bars[0].high, 130);
    assert_eq!(bars[0].low, 90);
    assert_eq!(bars[0].close, 105, "last in file order");
    assert_eq!(bars[0].volume, 5, "summed");
    assert!(
        bars.windows(2).all(|w| w[0].ts_micros < w[1].ts_micros),
        "output timestamps strictly increase — which is what the store demands \
         and what raw snapshots could not give it"
    );
}

/// Out of order refuses rather than sorting.
#[test]
fn an_out_of_order_snapshot_refuses_rather_than_being_sorted() {
    use pull::fold::{Bucket, FoldError, fold};
    use store::format::Bar;

    let snap = |s: i64| Bar {
        ts_micros: s * 1_000_000,
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: 0,
        open_interest: i64::MIN,
    };
    let why = fold(&[snap(10), snap(5)], Bucket::MINUTE).expect_err("5 follows 10");
    assert!(
        matches!(why, FoldError::OutOfOrder { at: 1, .. }),
        "named by position — sorting would invent an order these rows do not \
         have and would change which price became the open. Got {why:?}"
    );
}

/// Bucket width is runtime data, not a fixed set.
#[test]
fn the_bucket_width_is_a_runtime_value() {
    use pull::fold::{Bucket, fold};
    use store::format::Bar;

    let snap = |s: i64| Bar {
        ts_micros: s * 1_000_000,
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: 1,
        open_interest: i64::MIN,
    };
    let raw: Vec<Bar> = (0..300).map(snap).collect();

    for (secs, want) in [(1_u32, 300_usize), (5, 60), (60, 5), (300, 1)] {
        let b = Bucket::of_secs(secs).expect("non-zero");
        let bars = fold(&raw, b).expect("in order");
        assert_eq!(bars.len(), want, "{secs}s buckets over 300 seconds");
    }
    assert!(
        Bucket::of_secs(0).is_none(),
        "zero divides by zero — refused, not clamped"
    );
}

/// The ladder cannot be skipped, and a failure does not advance it.
#[test]
fn the_ladder_refuses_to_skip_a_rung() {
    use pull::fold::{Grain, LADDER, Ladder, Segment};

    assert_eq!(LADDER.len(), 6, "3 segments x 2 granularities");

    // Daily across every segment BEFORE any minute work: the cheap pass first.
    assert_eq!(
        LADDER.map(|s| (s.segment, s.grain)),
        [
            (Segment::Spot, Grain::Daily),
            (Segment::Futures, Grain::Daily),
            (Segment::Options, Grain::Daily),
            (Segment::Spot, Grain::Minute),
            (Segment::Futures, Grain::Minute),
            (Segment::Options, Grain::Minute),
        ],
        "spot before futures before options; daily before minute"
    );

    let mut l = Ladder::new();
    assert_eq!(l.next(), Some(LADDER[0]), "the cheapest rung first");

    // A FAILED stage does not advance. This is the whole point.
    for _ in 0..5 {
        l.record(false);
    }
    assert_eq!(
        l.next(),
        Some(LADDER[0]),
        "five failures later it is still on spot/daily — a partial success \
         that advanced would carry its gap into a stage 375 times larger"
    );
    assert_eq!(l.completed(), 0);

    // Clean stages advance, one rung each.
    for (i, rung) in LADDER.iter().enumerate() {
        assert_eq!(l.next().as_ref(), Some(rung), "rung {i}");
        l.record(true);
    }
    assert!(l.finished());
    assert_eq!(l.next(), None, "nothing left to climb");
    assert_eq!(l.completed(), 6);

    // Recording past the end does not overflow.
    l.record(true);
    assert_eq!(l.completed(), 6);
}

// ──────────────── dynamic selection, and the automatic work list ────────────

/// Any size, 1 to all of them — the thing three fixed buttons could not do.
#[test]
fn a_selection_is_any_size_not_one_of_three_universes() {
    use pull::work::Selection;

    assert_eq!(Selection::of(["NSE-NIFTY"]).len(), 1, "one instrument");
    assert_eq!(Selection::of(["A", "B", "C"]).len(), 3, "three");
    assert!(
        Selection::of(Vec::<String>::new()).is_empty(),
        "zero is legal"
    );

    let many: Vec<String> = (0..785).map(|i| format!("SYM{i}")).collect();
    assert_eq!(Selection::of(many).len(), 785, "all of them");

    // A duplicate would pull the same window twice and the second write would
    // be refused as not following the first — a failure caused entirely by the
    // caller's list, so it is removed rather than passed on.
    assert_eq!(
        Selection::of(["NSE-NIFTY", "NSE-NIFTY", "NSE-BANKNIFTY"]).len(),
        2,
        "duplicates collapse"
    );
    assert_eq!(
        Selection::of(["", "A", ""]).len(),
        1,
        "empty names are not instruments"
    );
}

/// The automatic side: re-running a complete store does nothing.
#[test]
fn gap_detection_makes_a_rerun_free_and_a_resume_exact() {
    use pull::work::{Cell, Selection, gaps};
    use std::collections::HashSet;
    use store::path::{Timeframe, YearMonth};

    let months = [
        YearMonth::new(2025, 6).expect("real"),
        YearMonth::new(2025, 7).expect("real"),
    ];
    let pick = Selection::of(["NSE-NIFTY", "NSE-BANKNIFTY", "NSE-FINNIFTY"]);
    let want = pick.cells(&months, Timeframe::MINUTE_1);
    assert_eq!(want.len(), 6, "3 instruments x 2 months");

    // Nothing held — everything is work.
    let none = HashSet::new();
    let all_work = gaps(&want, &none);
    assert_eq!(all_work.missing.len(), 6);
    assert_eq!(all_work.held, 0);
    assert!(!all_work.is_complete());

    // Interrupted after four: the resume is exactly the remaining two, with no
    // progress variable involved — the store's own census is the memory.
    let held: HashSet<Cell> = want.iter().take(4).cloned().collect();
    let resumed = gaps(&want, &held);
    assert_eq!(resumed.missing.len(), 2, "exactly what is left");
    assert_eq!(resumed.held, 4, "and it SAYS how many were already there");
    assert_eq!(
        resumed.requested(),
        6,
        "held plus missing is what was asked"
    );

    // Complete — a re-run contacts no vendor at all.
    let everything: HashSet<Cell> = want.iter().cloned().collect();
    let again = gaps(&want, &everything);
    assert!(again.is_complete(), "same inputs, no work, nothing fetched");
    assert_eq!(again.held, 6);

    // Add one instrument: only the new one is work.
    let bigger = Selection::of([
        "NSE-NIFTY",
        "NSE-BANKNIFTY",
        "NSE-FINNIFTY",
        "NSE-MIDCPNIFTY",
    ]);
    let grown = gaps(&bigger.cells(&months, Timeframe::MINUTE_1), &everything);
    assert_eq!(
        grown.missing.len(),
        2,
        "only the new instrument's two months"
    );
    assert!(
        grown
            .missing
            .iter()
            .all(|c| c.instrument == "NSE-MIDCPNIFTY"),
        "and nothing else is re-fetched"
    );
}

/// Cells are instrument-major so a bar file is opened once, not once a month.
#[test]
fn cells_are_ordered_instrument_major() {
    use pull::work::Selection;
    use store::path::{Timeframe, YearMonth};

    let months: Vec<_> = (1..=3)
        .map(|m| YearMonth::new(2025, m).expect("real"))
        .collect();
    let cells = Selection::of(["AAA", "BBB"]).cells(&months, Timeframe::MINUTE_1);

    assert_eq!(cells.len(), 6);
    assert!(
        cells[..3].iter().all(|c| c.instrument == "AAA"),
        "all of AAA's months before BBB starts — month-major order would open \
         and close every bar file once per month instead of once"
    );
    assert!(cells[3..].iter().all(|c| c.instrument == "BBB"));
}

// ===========================================================================
// The manifest writer — part 8
//
// `Manifest::image` produces a whole file and `Manifest::open_image` reads one
// back. Every test below goes through the READER THAT ALREADY EXISTED:
// `Manifest::load`, `ManifestHeader::read_region`, `Entry::decode` and both
// CRC-32C checks are untouched by the change that added the writer. A writer
// that needed the reader relaxed would be the wrong writer, so the round trip
// is the proof and not the convenience.
// ===========================================================================

/// M-20 — a whole manifest survives its own image, through the reader
/// unchanged.
///
/// Zero entries, one entry, several, and a key recorded twice: every counter,
/// the generation, and every entry in order.
#[test]
fn a_whole_manifest_survives_its_own_image() {
    // Zero entries — a first ingest for a vendor, which is the state a file
    // that does not exist yet is in. `Manifest::open_image` of nothing is the
    // only door to it; there is no `Manifest::empty`, and D-0036 is why.
    let empty = Manifest::open_image(Vendor::Dhan, &[]).expect("no file is a genesis census");
    assert_eq!(
        empty,
        fresh(Vendor::Dhan),
        "the same census `open` hands out"
    );
    let image = empty.image();
    assert_eq!(
        image.len(),
        HEADER_LEN_USIZE,
        "an empty census is the header region and nothing after it"
    );
    let read = Manifest::open_image(Vendor::Dhan, &image).expect("an empty census reloads");
    assert_eq!(read, empty);
    assert_eq!((read.entries(), read.keys(), read.total_rows()), (0, 0, 0));
    assert_eq!(read.degraded_reason(), None);
    assert_eq!(read.header().generation, 0);

    // One entry. Generation 1 lives in slot 1, so the image exercises the
    // second slot as well as the first.
    let one = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let mut single = fresh(Vendor::Groww);
    single.record(one).expect("one month");
    let image = single.image();
    assert_eq!(image.len(), HEADER_LEN_USIZE + IMAGE_LEN);
    assert_eq!(
        Manifest::open_image(Vendor::Groww, &image).expect("one month reloads"),
        single
    );

    // Several, including a key recorded twice: four entries, three keys.
    let months = [
        entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000),
        entry("NIFTY", 2024, 7, 7_500, 3_000, 4_000),
        entry("NIFTY", 2024, 6, 7_400, 1_000, 2_500),
    ];
    let mut written = fresh(Vendor::Groww);
    for month in &months {
        written.record(*month).expect("a record");
    }
    let image = written.image();

    let read = Manifest::open_image(Vendor::Groww, &image).expect("four months reload");
    assert_eq!(
        read, written,
        "every counter, the generation and every entry"
    );
    assert_eq!(read.entries(), 4);
    assert_eq!(read.keys(), 3);
    assert_eq!(read.total_rows(), 7_400 + 7_310 + 7_500);
    assert_eq!(read.header().generation, 4);
    assert_eq!(read.header(), written.header());
    assert_eq!(read.degraded_reason(), None, "a fresh image is not damaged");
    assert_eq!(
        read.entry(&months[0].key),
        Some(months[3]),
        "the newest wins"
    );
    assert_eq!(read.entry(&months[1].key), Some(months[1]));
    assert_eq!(read.entry(&months[2].key), Some(months[2]));

    // Idempotence — `CLAUDE.md` §3 rule 5, byte for byte. The image of a
    // manifest that came from an image is the same bytes, which an emission in
    // `HashMap` iteration order could not promise.
    assert_eq!(read.image(), image);

    // `open_image` is the split and nothing else: the same bytes handed to the
    // reader that already existed give the same census.
    assert_eq!(
        Manifest::open(
            Vendor::Groww,
            &image[..HEADER_LEN_USIZE],
            &image[HEADER_LEN_USIZE..]
        )
        .expect("the same bytes, split by hand"),
        read
    );
}

/// M-21 — the image puts every byte at the address the reader computes for it.
///
/// The round trip proves the file is *readable*; this proves it is readable for
/// the right reason rather than because the writer and the reader share a
/// mistake. The header slot is at `generation % SLOT_COUNT`, entry `i` is at
/// `Manifest::offset_of(i)`, and the whole length is the `durable_through` the
/// commit already published.
#[test]
fn the_image_puts_every_byte_where_the_reader_looks_for_it() {
    let months = [
        entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000),
        entry("NIFTY", 2024, 7, 7_500, 3_000, 4_000),
    ];
    let mut manifest = fresh(Vendor::Groww);
    for month in &months {
        manifest.record(*month).expect("a record");
    }
    let image = manifest.image();
    let commit = manifest.header().commit().expect("the published commit");

    // Generation 3 belongs in slot 1, and the slot it does not belong in is
    // left zero — which decodes as "not a manifest" and costs the reader a
    // candidate rather than a fault.
    assert_eq!(commit.slot, 1);
    assert_eq!(commit.offset, SLOT_STRIDE_USIZE as u64);
    assert_eq!(
        &image[SLOT_STRIDE_USIZE..SLOT_STRIDE_USIZE + IMAGE_LEN],
        &commit.bytes[..],
        "the header slot is the commit the writer would have issued"
    );
    assert!(
        image[..SLOT_STRIDE_USIZE].iter().all(|b| *b == 0),
        "the other slot is zero, not a stale generation"
    );
    assert!(
        image[SLOT_STRIDE_USIZE + IMAGE_LEN..HEADER_LEN_USIZE]
            .iter()
            .all(|b| *b == 0),
        "and the rest of the slot's 16,384-byte spacing is zero"
    );
    assert_eq!(&image[..MAGIC.len()], &[0u8; 8], "slot 0 carries no magic");
    assert_eq!(
        &image[SLOT_STRIDE_USIZE..SLOT_STRIDE_USIZE + MAGIC.len()],
        &MAGIC[..],
        "and slot 1 carries the manifest magic, not a bar file's"
    );

    // Entry `i` is at `HEADER_LEN + i·64`, and holds exactly that entry's own
    // image — checksum included, because the reader verifies it.
    for (ordinal, month) in months.iter().enumerate() {
        let at = HEADER_LEN_USIZE + ordinal * IMAGE_LEN;
        assert_eq!(
            at as u64,
            Manifest::offset_of(ordinal as u64).expect("an ordinal in range"),
            "entry {ordinal} sits where the arithmetic says"
        );
        assert_eq!(
            &image[at..at + IMAGE_LEN],
            &month.image()[..],
            "entry {ordinal} is its own image, in the order it was recorded"
        );
        assert_eq!(Entry::decode(&image[at..at + IMAGE_LEN]), Ok(*month));
    }

    // The length is the offset the commit already told a writer to flush
    // through. One number, not two that have to agree.
    assert_eq!(image.len() as u64, commit.durable_through);
    assert_eq!(image.len() as u64, HEADER_LEN + manifest.entries() * 64);
    assert_eq!(image.len(), HEADER_LEN_USIZE + 3 * IMAGE_LEN);

    // # The design ceiling is arithmetic here, not a census that was built
    //
    // A manifest at MAX_ENTRIES is a 134,250,496-byte image beside a ~285 MB
    // index and a ~168 MB log, which is not a thing a unit test builds — the
    // same reason `the_reservation_is_capped_at_the_design_ceiling` gives for
    // its own ceiling. What IS checked is that the writer's length is the
    // header's `durable_through`, asserted above at a census that exists, and
    // that the same expression at the ceiling is exact and refuses one past it.
    // `CLAUDE.md` §3 rule 6: this is a bound, and the part of it that is not
    // measured is named rather than implied.
    let genesis = ManifestHeader::genesis(Vendor::Groww);
    let at_ceiling = ManifestHeader {
        n_valid: MAX_ENTRIES,
        n_keys: MAX_ENTRIES,
        ..genesis
    }
    .commit()
    .expect("exactly MAX_ENTRIES commits")
    .durable_through;
    assert_eq!(at_ceiling, HEADER_LEN + MAX_ENTRIES * 64);
    assert_eq!(
        at_ceiling, 134_250_496,
        "134 MB, the figure MAX_ENTRIES names"
    );
    assert_eq!(
        ManifestHeader {
            n_valid: MAX_ENTRIES + 1,
            ..genesis
        }
        .commit(),
        Err(ManifestError::TooManyEntries {
            n_valid: MAX_ENTRIES + 1,
            limit: MAX_ENTRIES
        }),
        "one past the ceiling has no image, because it has no commit"
    );
}

/// M-22 — a half-installed image is refused by name, never believed.
///
/// The image is one buffer and is therefore atomic as a value; installing it is
/// the caller's `write, flush, rename`, and this crate performs no I/O and
/// cannot do it. So the load-bearing claim is the other one: **every truncation
/// of a whole image is refused, and no prefix is ever read as a smaller
/// census.** The header region is at the front, so a prefix publishes a counter
/// over entries that are not there.
#[test]
fn a_half_installed_image_is_refused_by_name() {
    let months = [
        entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000),
        entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000),
        entry("NIFTY", 2024, 7, 7_500, 3_000, 4_000),
    ];
    let mut manifest = fresh(Vendor::Groww);
    for month in &months {
        manifest.record(*month).expect("a record");
    }
    let whole = manifest.image();
    assert_eq!(whole.len(), HEADER_LEN_USIZE + 3 * IMAGE_LEN);

    // Truncated in the MIDDLE of the last entry: the region holds two whole
    // entries and the header counts three.
    assert_eq!(
        Manifest::open_image(Vendor::Groww, &whole[..whole.len() - 32]),
        Err(ManifestError::CounterExceedsRegion {
            n_valid: 3,
            capacity: 2
        })
    );

    // Truncated by a WHOLE entry, which is the shape that would otherwise look
    // like a perfectly good two-entry file.
    assert_eq!(
        Manifest::open_image(Vendor::Groww, &whole[..whole.len() - IMAGE_LEN]),
        Err(ManifestError::CounterExceedsRegion {
            n_valid: 3,
            capacity: 2
        })
    );

    // Shorter than HEADER_LEN — the header region itself did not land.
    assert_eq!(
        Manifest::open_image(Vendor::Groww, &whole[..HEADER_LEN_USIZE - 1]),
        Err(ManifestError::HeaderRegionTooShort { slots: 1, need: 2 })
    );
    assert_eq!(
        Manifest::open_image(Vendor::Groww, &whole[..1]),
        Err(ManifestError::HeaderRegionTooShort { slots: 0, need: 2 })
    );

    // Every prefix, exhaustively, so the three cases above are a description of
    // the whole space rather than three points in it. Nothing between an empty
    // file and the whole image loads.
    assert!(
        Manifest::open_image(Vendor::Groww, &whole[..0]).is_ok(),
        "no file at all is a first ingest, and that is not damage"
    );
    for cut in 1..whole.len() {
        match Manifest::open_image(Vendor::Groww, &whole[..cut]) {
            Err(ManifestError::HeaderRegionTooShort { need: 2, .. }) => {
                assert!(cut < HEADER_LEN_USIZE, "cut {cut} is a whole header");
            }
            Err(ManifestError::CounterExceedsRegion { n_valid: 3, .. }) => {
                assert!(cut >= HEADER_LEN_USIZE, "cut {cut} has no header region");
            }
            other => panic!("prefix of {cut} bytes was not refused by name: {other:?}"),
        }
    }
    assert!(
        Manifest::open_image(Vendor::Groww, &whole).is_ok(),
        "and the whole thing does load"
    );

    // A header slot that landed corrupt is the other half: its own CRC-32C
    // catches it, and there is no older generation in a freshly imaged file to
    // fall back to, so it refuses rather than reporting a stale census.
    let mut corrupt = whole.clone();
    let at = manifest.header().commit().expect("a commit").offset as usize;
    // Byte 40 of a slot is the row total: inside the checksum's domain, and
    // past the magic and the version, so the checksum is what refuses it.
    corrupt[at + 40] ^= 0xFF;
    match Manifest::open_image(Vendor::Groww, &corrupt) {
        Err(ManifestError::SlotChecksum { stored, computed }) => {
            assert_ne!(stored, computed, "the refusal names both numbers");
        }
        other => panic!("a corrupt header slot must be refused by name, got {other:?}"),
    }
}

/// M-23 — the image carries the entry LOG, not the index over it.
///
/// The entry region on disk is a log: `n_valid` entries in the order they were
/// committed, of which the index keeps the newest per key. A writer that
/// emitted the index would compact one month's history away on every whole-file
/// replacement — `CLAUDE.md` §3 rule 8 — and would emit it in `HashMap`
/// iteration order, which is rule 5. The reader refuses that order outright,
/// which is what makes both of those a test rather than a comment.
#[test]
fn the_log_is_the_entry_region_in_order() {
    assert_eq!(
        size_of::<Entry>(),
        80,
        "the per-entry cost of the log, as the Manifest doc states it"
    );

    let june = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let june_again = entry("NIFTY", 2024, 6, 7_400, 1_000, 2_500);
    let mut manifest = fresh(Vendor::Groww);
    manifest.record(june).expect("june");
    manifest.record(june_again).expect("june, again");
    assert_eq!(
        (manifest.entries(), manifest.keys(), manifest.total_rows()),
        (2, 1, 7_400)
    );

    let image = manifest.image();
    assert_eq!(
        image.len(),
        HEADER_LEN_USIZE + 2 * IMAGE_LEN,
        "two entries on disk for one key — the older one is not compacted away"
    );
    assert_eq!(
        &image[HEADER_LEN_USIZE..HEADER_LEN_USIZE + IMAGE_LEN],
        &june.image()[..],
        "the first entry recorded is the first entry written"
    );
    assert_eq!(
        &image[HEADER_LEN_USIZE + IMAGE_LEN..],
        &june_again.image()[..]
    );

    let read = Manifest::open_image(Vendor::Groww, &image).expect("it reloads");
    assert_eq!(read, manifest);
    assert_eq!(read.entries(), 2, "both entries came back");
    assert_eq!(read.keys(), 1, "and they are still one key");
    assert_eq!(read.entry(&june.key), Some(june_again));

    // The order is load-bearing, and the reader is what enforces it: the same
    // two entries the other way round are the row count going backwards.
    let mut swapped = image.clone();
    swapped[HEADER_LEN_USIZE..HEADER_LEN_USIZE + IMAGE_LEN].copy_from_slice(&june_again.image());
    swapped[HEADER_LEN_USIZE + IMAGE_LEN..].copy_from_slice(&june.image());
    assert_eq!(
        Manifest::open_image(Vendor::Groww, &swapped),
        Err(ManifestError::RowCountWentBackwards {
            ordinal: 1,
            previous: 7_400,
            next: 7_312
        }),
        "a log emitted in any order but the recorded one is refused by the reader"
    );

    // And the same fact on the write side, so a caller cannot record it in the
    // first place.
    assert_eq!(
        manifest.record(entry("NIFTY", 2024, 6, 7_311, 1_000, 2_500)),
        Err(ManifestError::RowCountWentBackwards {
            ordinal: 2,
            previous: 7_400,
            next: 7_311
        })
    );
    assert_eq!(
        manifest.image(),
        image,
        "a refused record left the census, and its bytes, untouched"
    );
}

/// M-24 — a census that loaded degraded images as its repair, and that is the
/// one documented inequality in the round trip.
///
/// `Manifest::image` writes the recovered generation whole into a clean file,
/// so the value that loads back differs from the one that produced it in
/// exactly one field: the damage it named is gone. Asserted rather than left as
/// prose, because "equal except for" is the kind of claim that rots.
#[test]
fn a_degraded_census_images_as_its_repair() {
    let one = entry("NIFTY", 2024, 6, 7_312, 1_000, 2_000);
    let two = entry("BANKNIFTY", 2024, 6, 7_310, 1_000, 2_000);
    let (header_region, data, _) = built(Vendor::Groww, &[one, two]);

    // The commit for entry 1 landed; entry 1's own bytes never were written
    // back. The generation below it describes the durable prefix.
    let mut torn = data.clone();
    torn[IMAGE_LEN..].fill(0);
    let recovered = Manifest::load(Vendor::Groww, &header_region, &torn).expect("the fallback");
    assert_eq!(recovered.entries(), 1);
    assert!(
        recovered.degraded_reason().is_some(),
        "the recovery names what it stepped over"
    );

    let repaired =
        Manifest::open_image(Vendor::Groww, &recovered.image()).expect("the repaired file reloads");
    assert_eq!(
        repaired.degraded_reason(),
        None,
        "the damage the recovery named is not in the file it wrote"
    );
    assert_eq!(
        repaired.header(),
        recovered.header(),
        "and nothing else moved: same generation, same counters"
    );
    assert_eq!(repaired.entry(&one.key), Some(one));
    assert_eq!(repaired.entry(&two.key), None, "the lost month stays lost");
    assert_ne!(
        repaired, recovered,
        "the reason is a real difference, not a rounding of one"
    );
    assert_eq!(
        repaired.image(),
        recovered.image(),
        "the bytes are identical; only the reason the value carries differs"
    );
}

// ───────────────────────── the census lock ─────────────────────────

/// Two runs cannot install a census at once.
///
/// Measured before this lock existed: two POSTs at the same instant over two
/// folders sharing no files wrote **40 bar files** and left a census holding
/// **20 entries — one run only**. 6,433 bars, 48.3% of everything written and
/// fsync-ed, invisible to every page. Both receipts said `STORED` and both said
/// *"balances: yes"*, because each run's own books balanced perfectly.
#[test]
fn a_second_census_install_is_refused_while_the_first_holds_the_lock() {
    use std::fs::OpenOptions;

    let root = std::env::temp_dir().join(format!(
        "brutex-census-lock-{}-{}",
        std::process::id(),
        line!()
    ));
    let dir = root.join("manifest");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let census = dir.join("dhan.man");
    let lock_path = census.with_extension("man.lock");

    // Hold the lock the way a running pull would.
    let holder = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .expect("lock file");
    holder.try_lock().expect("first taker always wins");

    // A second install must REFUSE, not queue and not overwrite.
    let second = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .expect("lock file");
    assert!(
        second.try_lock().is_err(),
        "the second taker must be refused. Without this the loser's whole \
         census is overwritten by a rename that never saw it, and its receipt \
         still reads 'every row accounted for'."
    );

    // Once the first releases, the second may proceed — a refusal, not a ban.
    drop(holder);
    let third = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .expect("lock file");
    assert!(
        third.try_lock().is_ok(),
        "the lock is released with the file — a pull that finished must not \
         block the next one"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// **A census must be readable, not only probeable.**
///
/// `Manifest::entry` answers *is this key held*, which only helps a caller that
/// already knows which key to ask about. `crates/api`'s coverage grid did not:
/// it built keys out of the instrument master and probed those, every probe
/// missed, and the page reported `0 of 200 held` beside `62,978 rows`. See
/// D-0048. `held_keys` is how a caller asks *what is here* instead.
///
/// The order is a `HashMap`'s and is deliberately not asserted — it is not
/// stable between runs, and `api::census::held_series` sorts for exactly that
/// reason. What is asserted is the set.
#[test]
fn a_manifest_reports_every_key_it_holds_and_not_one_it_does_not() {
    let mut manifest = Manifest::open(Vendor::Dhan, &[], &[]).expect("a genesis manifest");
    assert_eq!(
        manifest.held_keys().count(),
        0,
        "an empty census holds nothing, and says so rather than being unaskable"
    );

    let one = entry("NIFTY", 2026, 7, 8_250, 1, 2);
    let two = entry("BANKNIFTY", 2026, 7, 7_100, 1, 2);
    let three = entry("NIFTY", 2026, 8, 6_000, 1, 2);
    for e in [one, two, three] {
        manifest.record(e).expect("records");
    }

    let held: std::collections::HashSet<EntryKey> = manifest.held_keys().copied().collect();
    assert_eq!(held.len(), 3, "three distinct keys: {held:?}");
    for e in [one, two, three] {
        assert!(
            held.contains(&e.key),
            "{:?} was recorded and is not reported",
            e.key
        );
    }
    // A month that was never recorded is absent from the report, exactly as it
    // is absent from a probe.
    let absent = entry("NIFTY", 2026, 6, 1, 1, 2).key;
    assert!(!held.contains(&absent));
    assert_eq!(manifest.entry(&absent), None, "and the probe agrees");

    // RE-RECORDING A KEY DOES NOT DUPLICATE IT. The entry region is an
    // append-only log and the index keeps the newest per key, so a second
    // month-two write is one key with a new row count, not two keys.
    manifest
        .record(entry("NIFTY", 2026, 7, 9_000, 1, 3))
        .expect("records the update");
    assert_eq!(
        manifest.held_keys().count(),
        3,
        "an update is a new entry and the same key"
    );
    assert_eq!(manifest.entry(&one.key).expect("held").rows, 9_000);
    assert_eq!(manifest.entries(), 4, "four entries in the log");
    assert_eq!(manifest.keys(), 3, "three keys in the index");
}
