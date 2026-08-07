//! TEMPORARY audit scratch — delete after the run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use brutex_core::vendor::Vendor;
use store::file::BarFile;
use store::path::{FileKind, PathParts, StorePath, Timeframe, YearMonth};

fn root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("brutex-audit-{}-{tag}", std::process::id()));
    p
}

fn parts(symbol: &'static str, y: u16, m: u8) -> PathParts<'static> {
    PathParts {
        vendor: Vendor::Dhan,
        exchange: "NSE",
        segment: "INDEX",
        symbol,
        timeframe: Timeframe::MINUTE_1,
        month: YearMonth::new(y, m).unwrap(),
        file: FileKind::Bars,
    }
}

#[test]
fn a_read_only_caller_creates_the_month_it_asked_about() {
    let root = root("read");
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "nothing on disk before the call");

    let path = StorePath::new(parts("NEVERPULLED", 1970, 1)).unwrap();
    let file = BarFile::open_or_create(&root, path, 12345).expect("opened");
    assert_eq!(file.records(), 0, "a month nobody pulled reads as empty");

    let bin = root.join("bars/dhan/NSE/INDEX/NEVERPULLED/1min/1970-01.bin");
    let lock = root.join("bars/dhan/NSE/INDEX/NEVERPULLED/1min/1970-01.lock");
    println!(
        "CREATED {} bytes at {}",
        std::fs::metadata(&bin).unwrap().len(),
        bin.display()
    );
    println!("CREATED lock {} exists={}", lock.display(), lock.exists());

    let path2 = StorePath::new(parts("NEVERPULLED", 1970, 1)).unwrap();
    let second = BarFile::open_or_create(&root, path2, 12345);
    println!("SECOND CONCURRENT READER -> {second:?}");
    assert!(second.is_err());

    drop(file);

    let mut bytes = 0u64;
    for m in 1..=12u8 {
        let p = StorePath::new(parts("NEVERPULLED", 2001, m)).unwrap();
        let f = BarFile::open_or_create(&root, p, 12345).expect("opened");
        bytes += 32_768;
        drop(f);
    }
    println!("TWELVE READ REQUESTS MATERIALISED {bytes} BYTES OF EMPTY MONTHS");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn december_bars_are_accepted_into_the_june_file() {
    let root = root("month");
    let _ = std::fs::remove_dir_all(&root);
    let path = StorePath::new(parts("NIFTY", 2024, 6)).unwrap();
    let mut file = BarFile::open_or_create(&root, path, 7).expect("opened");
    let december = 1_733_130_900_000_000i64; // 2024-12-02 09:15 UTC
    let batch = [store::format::Bar {
        ts_micros: december,
        open: 100,
        high: 100,
        low: 100,
        close: 100,
        volume: 0,
        open_interest: store::format::OI_NULL,
    }];
    let out = file.append(&batch);
    println!("APPEND OF A DECEMBER BAR INTO 2024-06.bin -> {out:?}");
    println!("HEADER NOW {:?}", file.header());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_short_genesis_file_is_condemned_forever() {
    let root = root("short");
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("bars/dhan/NSE/INDEX/NIFTY/1min");
    std::fs::create_dir_all(&dir).unwrap();
    // A crash part-way through `initialise`: 20,000 zero bytes, not 0 and not
    // 32,768.
    std::fs::write(dir.join("2024-06.bin"), vec![0u8; 20_000]).unwrap();
    let path = StorePath::new(parts("NIFTY", 2024, 6)).unwrap();
    let out = BarFile::open_or_create(&root, path, 7);
    println!("A TORN GENESIS (20,000 zero bytes) -> {out:?}");
    let _ = std::fs::remove_dir_all(&root);
}
