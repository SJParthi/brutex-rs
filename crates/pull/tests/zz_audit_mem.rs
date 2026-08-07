//! TEMPORARY AUDIT REPRODUCTION — delete after reading.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use pull::archive;
use pull::csv::Columns;
use std::path::Path;

#[test]
fn the_whole_archive_is_held_in_memory_not_one_file() {
    let dir = Path::new("/tmp/bxaudit/ARCH");
    if !dir.is_dir() {
        println!("fixture absent, skipping");
        return;
    }
    let members = archive::read_dir(dir, Columns::TrueDataIndex).expect("walk");
    let rows = archive::total_rows(&members);
    println!(
        "members={} rows={} RawRow={}B -> resident row bytes ~= {} MiB",
        members.len(),
        rows,
        std::mem::size_of::<pull::fetch::RawRow>(),
        rows * std::mem::size_of::<pull::fetch::RawRow>() / (1024 * 1024)
    );
    panic!("see stdout");
}
