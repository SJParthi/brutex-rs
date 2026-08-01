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
    Append, Commit, Entry, EntryFault, EntryKey, FORMAT_VERSION, HEADER_LEN, IMAGE_LEN, MAGIC,
    MAX_ENTRIES, Manifest, ManifestError, ManifestHeader, manifest_path,
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
