//! Record and block geometry: `store::geometry::*`.
//!
//! The defect these tests exist for: the checksum block was 4096 bytes and the
//! record stride is 56, and 56 does not divide 4096. A record could begin in
//! one block and end in the next, so verifying it against the block
//! `block_of` returned checked *part* of a record and pronounced the whole of
//! it good.
//!
//! The second defect: the partial tail block was not modelled anywhere, so the
//! only API that named a block's bytes named bytes past EOF for 72 of every 73
//! states a file passes through.

// A test that asserts nothing is banned, and a test that cannot fail loudly is
// a test that asserts nothing. These allow the harness to panic on a broken
// invariant instead of threading `Result` through every assertion.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use store::format::{BLOCK_LEN, FormatError, HEADER_LEN, RECORD_STRIDE, RECORDS_PER_BLOCK};
use store::layout::Layout;

/// How many indices to walk. Enough to cross 68 blocks and, under the old
/// geometry, 68 block boundaries.
const WALK: u64 = 5_000;

// ---------------------------------------------------------------------------
// The geometry that was wrong, reproduced so the fix is measured against it
// rather than against a description of it.
// ---------------------------------------------------------------------------

/// Version 1's header length, before the two-slot header.
const OLD_HEADER_LEN: u64 = 64;
/// The byte-addressed checksum block that did not divide the stride.
const OLD_BLOCK_LEN: u64 = 4096;

/// Whether record `index` straddled two blocks under the old geometry.
fn old_geometry_straddles(index: u64) -> bool {
    let start = OLD_HEADER_LEN + index * RECORD_STRIDE;
    let last = start + RECORD_STRIDE - 1;
    start / OLD_BLOCK_LEN != last / OLD_BLOCK_LEN
}

#[test]
fn the_old_geometry_straddled_and_record_145_is_the_cited_case() {
    // 4096 / 56 = 73.14..., so a record boundary and a block boundary coincide
    // only once every seven blocks.
    assert_ne!(OLD_BLOCK_LEN % RECORD_STRIDE, 0, "this is the whole defect");

    // Record 145 began at 64 + 145*56 = 8184, eight bytes before the 8192
    // boundary, and ran to 8239. Eight bytes in one block, forty-eight in the
    // next.
    let start = OLD_HEADER_LEN + 145 * RECORD_STRIDE;
    assert_eq!(start, 8_184);
    assert_eq!(start / OLD_BLOCK_LEN, 1);
    assert_eq!((start + RECORD_STRIDE - 1) / OLD_BLOCK_LEN, 2);
    assert_eq!(2 * OLD_BLOCK_LEN - start, 8, "bytes left in block 1");
    assert_eq!(RECORD_STRIDE - 8, 48, "bytes carried into block 2");
    assert!(old_geometry_straddles(145));

    // And it was not a rare accident: 23 of the first 2,000 records did it.
    let straddlers = (0..2_000).filter(|&i| old_geometry_straddles(i)).count();
    assert_eq!(straddlers, 23, "the audit counted 23; so does this");
}

#[test]
fn no_record_straddles_a_block() {
    let v2 = Layout::V2;

    // The arithmetic first, because it is what makes the walk below come out
    // the way it does rather than a coincidence of the indices chosen.
    // 56 x 73 = 4088, in both directions.
    assert_eq!(RECORD_STRIDE, 56);
    assert_eq!(RECORDS_PER_BLOCK, 73);
    assert_eq!(BLOCK_LEN, 4_088);
    assert_eq!(RECORD_STRIDE * RECORDS_PER_BLOCK, BLOCK_LEN);
    assert_eq!(BLOCK_LEN % RECORD_STRIDE, 0, "no record can straddle");
    assert_eq!(BLOCK_LEN / RECORD_STRIDE, RECORDS_PER_BLOCK);
    // The closed form `store::format` carries as a const assertion, restated
    // here so the invariant table names a test: the LAST record of a block
    // ends exactly on the block's last byte, so with the divisibility above
    // every earlier record ends strictly inside it.
    assert_eq!(
        (RECORDS_PER_BLOCK - 1) * RECORD_STRIDE + RECORD_STRIDE,
        BLOCK_LEN,
        "the last record of a block ends on the block boundary",
    );

    // The property, for every index a month file could hold and well beyond.
    for index in 0..WALK {
        let (start, end) = v2.record_byte_range(index).expect("fits");
        let block = v2.block_of(index);
        let (block_start, block_end) = v2.block_byte_range(block).expect("fits");

        assert!(
            start >= block_start && end <= block_end,
            "record {index} occupies [{start}, {end}) but block {block} covers \
             [{block_start}, {block_end}) -- verifying it would check part of it",
        );

        // Not merely inside *a* block: inside the block `block_of` names, and
        // no byte of it in the neighbours.
        if block > 0 {
            let (_, previous_end) = v2.block_byte_range(block - 1).expect("fits");
            assert!(start >= previous_end, "record {index} reaches back a block");
        }
        let (next_start, _) = v2.block_byte_range(block + 1).expect("fits");
        assert!(
            end <= next_start,
            "record {index} reaches into the next block"
        );
    }

    // The previously-cited case, spelled out. Under the new geometry record 145
    // is the last record of block 1 and ends exactly on the block boundary.
    let (start, end) = v2.record_byte_range(145).expect("fits");
    assert_eq!(v2.block_of(145), 1);
    assert_eq!(
        v2.block_byte_range(1).expect("fits"),
        (HEADER_LEN + BLOCK_LEN, HEADER_LEN + 2 * BLOCK_LEN),
    );
    assert_eq!((start, end), (40_888, 40_944));
    assert_eq!(end, HEADER_LEN + 2 * BLOCK_LEN);
    assert!(old_geometry_straddles(145), "it used to straddle");

    // Zero straddlers, against 23 in the first 2,000 before.
    let straddlers = (0..WALK)
        .filter(|&index| {
            let (start, end) = v2.record_byte_range(index).expect("fits");
            let (block_start, block_end) = v2.block_byte_range(v2.block_of(index)).expect("fits");
            start < block_start || end > block_end
        })
        .count();
    assert_eq!(straddlers, 0);
}

#[test]
fn the_block_is_counted_in_records_not_in_pages() {
    // The block length is a property of the FORMAT, not of the host. This
    // machine pages at 16384 bytes and the CI runner pages at 4096; a block
    // sized to either would make the same file verify differently on the two,
    // which breaks byte-for-byte reproducibility for a cache-locality guess.
    //
    // The only length that is a whole multiple of the stride AND of both page
    // sizes is lcm(56, 16384) = 114688 bytes -- 2048 records, 28x larger than
    // this block, so verifying one record would checksum 112 KiB. That is the
    // measurement that settles it: tracking the page size is not free, it is
    // expensive, and it buys a property the format does not need.
    assert_eq!(BLOCK_LEN % RECORD_STRIDE, 0, "a record cannot be split");
    assert_eq!(BLOCK_LEN / RECORD_STRIDE, RECORDS_PER_BLOCK);
    assert_eq!(BLOCK_LEN, 4_088);
    assert_ne!(BLOCK_LEN, 4_096, "not the CI runner's page");
    assert_ne!(BLOCK_LEN, 16_384, "not this machine's page");

    // lcm(56, 16384): the size page-tracking would actually cost.
    let lcm = 56 * 16_384 / 8;
    assert_eq!(lcm, 114_688);
    assert_eq!(lcm % 4_096, 0);
    assert_eq!(lcm / RECORD_STRIDE, 2_048);
    assert_eq!(lcm / BLOCK_LEN, 28);
}

#[test]
fn a_padded_four_kib_block_would_cost_a_division() {
    // The alternative docs/02 §6 actually described -- "a 4 KiB block holds 73
    // whole records with 8 bytes of slack" -- is a PADDED block, and a padded
    // block never straddles either. So the reason it was not taken has to be
    // stated rather than assumed, or D-0005 was overturned without an
    // argument.
    const PADDED_BLOCK: u64 = 4_096;
    assert_eq!(
        PADDED_BLOCK - RECORDS_PER_BLOCK * RECORD_STRIDE,
        8,
        "the slack the document describes",
    );

    // A block is a range of the file, so padding the block pads the record
    // array: block b starts at header + b*4096 while record 73b sits at
    // header + b*4088. The gap is 8 bytes per block, and it is real bytes.
    for block in 1..64u64 {
        let padded_start = HEADER_LEN + block * PADDED_BLOCK;
        let packed_start = HEADER_LEN + block * BLOCK_LEN;
        assert_eq!(padded_start - packed_start, block * 8);
    }

    // With the padding present, the address of record i stops being a multiply
    // and an add. That is the cost, and it is the reason: CLAUDE.md §3 rule 4
    // asks for a constant per-operation cost, and this is a strictly worse
    // constant bought with nothing -- the 8 bytes are wasted either way.
    let padded_offset = |i: u64| {
        HEADER_LEN
            + (i / RECORDS_PER_BLOCK) * PADDED_BLOCK
            + (i % RECORDS_PER_BLOCK) * RECORD_STRIDE
    };
    let v2 = Layout::V2;
    assert_eq!(padded_offset(0), v2.offset_of(0).expect("fits"));
    assert_eq!(padded_offset(72), v2.offset_of(72).expect("fits"));
    for block in 1..64u64 {
        let first = block * RECORDS_PER_BLOCK;
        assert_eq!(
            padded_offset(first) - v2.offset_of(first).expect("fits"),
            block * 8,
            "the padded form drifts by a whole slack per block",
        );
    }

    // And the waste is identical, so the padded form buys nothing at all: 8
    // bytes of every 4096 either way, because the packed block simply stops
    // 8 bytes earlier.
    assert_eq!(PADDED_BLOCK - BLOCK_LEN, 8);
}

#[test]
fn the_tail_block_covers_only_the_committed_records() {
    // The partial tail block, which was modelled nowhere. `block_byte_range`
    // is nominal and names bytes past EOF for nearly every file state;
    // `covered_byte_range` is the checksum domain and never does.
    let v2 = Layout::V2;

    // A file with exactly one record. The nominal block ends 4032 bytes past
    // the end of the file.
    let one_record = HEADER_LEN + RECORD_STRIDE;
    assert_eq!(
        v2.block_byte_range(0),
        Ok((HEADER_LEN, HEADER_LEN + BLOCK_LEN))
    );
    assert_eq!(HEADER_LEN + BLOCK_LEN - one_record, 4_032, "past EOF");
    assert_eq!(v2.covered_byte_range(0, 1), Ok((HEADER_LEN, one_record)));

    // Across every record count a month file passes through: the covered range
    // ends exactly at EOF, and every block before the tail is whole.
    let mut nominal_past_eof = 0u64;
    for n_valid in 1..=730u64 {
        let eof = HEADER_LEN + n_valid * RECORD_STRIDE;
        let blocks = v2.blocks_for(n_valid);
        let tail = blocks - 1;

        let (_, covered_end) = v2.covered_byte_range(tail, n_valid).expect("fits");
        assert_eq!(covered_end, eof, "tail block of {n_valid} records");

        let (_, nominal_end) = v2.block_byte_range(tail).expect("fits");
        if nominal_end > eof {
            nominal_past_eof += 1;
        }

        for block in 0..tail {
            assert_eq!(v2.records_in_block(block, n_valid), Ok(RECORDS_PER_BLOCK));
            assert_eq!(
                v2.covered_byte_range(block, n_valid),
                v2.block_byte_range(block),
                "a whole block's covered range is its nominal range",
            );
        }
        assert_eq!(
            v2.records_in_block(tail, n_valid),
            Ok(n_valid - tail * RECORDS_PER_BLOCK),
        );
    }
    // 72 of every 73 states -- every state except the instant an append lands
    // exactly on a block boundary.
    assert_eq!(nominal_past_eof, 720);
    assert_eq!(730 - 730 / RECORDS_PER_BLOCK, 720);
}

#[test]
fn a_block_past_the_counter_is_refused_rather_than_answered() {
    // Answering "zero bytes" would let a caller checksum nothing and report
    // the result as a verified block.
    let v2 = Layout::V2;
    assert_eq!(v2.blocks_for(0), 0);
    assert_eq!(v2.blocks_for(1), 1);
    assert_eq!(v2.blocks_for(RECORDS_PER_BLOCK), 1);
    assert_eq!(v2.blocks_for(RECORDS_PER_BLOCK + 1), 2);

    assert_eq!(
        v2.covered_byte_range(0, 0),
        Err(FormatError::BlockNotCommitted {
            block: 0,
            blocks: 0
        }),
    );
    assert_eq!(
        v2.records_in_block(1, RECORDS_PER_BLOCK),
        Err(FormatError::BlockNotCommitted {
            block: 1,
            blocks: 1
        }),
    );
    assert_eq!(
        v2.covered_byte_range(9, 10),
        Err(FormatError::BlockNotCommitted {
            block: 9,
            blocks: 1
        }),
    );
}

#[test]
fn a_full_blocks_covered_range_is_its_nominal_range_wherever_it_is_representable() {
    // The relationship at the top of u64, where it could stop holding.
    let v2 = Layout::V2;
    let last_block = (u64::MAX - HEADER_LEN - BLOCK_LEN) / BLOCK_LEN;

    for block in [0u64, 1, 73, 1_000, last_block] {
        let nominal = v2.block_byte_range(block).expect("representable");
        let n_valid = (block + 1) * RECORDS_PER_BLOCK;
        assert_eq!(
            v2.covered_byte_range(block, n_valid),
            Ok(nominal),
            "block {block} full",
        );
    }

    // Past it, both refuse rather than wrapping into a different, plausible
    // block.
    assert_eq!(
        v2.block_byte_range(last_block + 1),
        Err(FormatError::OffsetOverflow),
    );
    assert_eq!(
        v2.covered_byte_range(last_block + 1, u64::MAX),
        Err(FormatError::OffsetOverflow),
    );
}

#[test]
fn addressing_is_arithmetic_and_flat() {
    let v2 = Layout::V2;
    // Bar 400,000 costs the same as bar 0: a multiply and an add.
    assert_eq!(v2.offset_of(0).expect("fits"), HEADER_LEN);
    assert_eq!(v2.offset_of(1).expect("fits"), HEADER_LEN + RECORD_STRIDE);
    assert_eq!(v2.offset_of(400_000).expect("fits"), 22_432_768);
    for index in 0..WALK {
        let here = v2.offset_of(index).expect("fits");
        let next = v2.offset_of(index + 1).expect("fits");
        assert_eq!(next - here, RECORD_STRIDE, "step at {index}");
    }
}

#[test]
fn a_block_index_names_seventy_three_consecutive_records() {
    let v2 = Layout::V2;
    assert_eq!(v2.block_of(0), 0);
    assert_eq!(v2.block_of(72), 0, "the last record of block 0");
    assert_eq!(v2.block_of(73), 1, "the first record of block 1");
    assert_eq!(v2.block_of(145), 1, "the last record of block 1");
    assert_eq!(v2.block_of(146), 2);

    // Every block holds exactly RECORDS_PER_BLOCK indices, with no index in
    // two blocks and none in none.
    for block in 0..68u64 {
        let members = (0..WALK).filter(|&i| v2.block_of(i) == block).count();
        assert_eq!(u64::try_from(members), Ok(RECORDS_PER_BLOCK));
    }
}

#[test]
fn the_header_slots_do_not_share_a_failure_unit() {
    // The two-slot header is only redundant if a write to one slot cannot
    // damage the other, and storage fails at a block or a page, never at a
    // byte. Both hosts this repository builds on report a 4096-byte device
    // block; this machine pages at 16384 and the CI runner at 4096.
    let v2 = Layout::V2;
    let slot0 = v2.slot_offset(0);
    let slot1 = v2.slot_offset(1);
    assert_eq!(slot0, 0);
    assert_eq!(slot1, 16_384);

    for unit in [512u64, 4_096, 16_384] {
        assert_ne!(
            slot0 / unit,
            slot1 / unit,
            "the slots share one {unit}-byte failure unit",
        );
    }

    // Under the geometry this replaced -- slots at 0 and 64 -- they shared
    // every one of those units, which is what made the redundancy nominal and
    // the "disjoint byte ranges, so that is true on any device" argument
    // false.
    let old_spacing = [0u64, 64];
    for unit in [512u64, 4_096, 16_384] {
        assert_eq!(
            old_spacing[0] / unit,
            old_spacing[1] / unit,
            "the old spacing shared a {unit}-byte unit",
        );
    }

    // And the header region is exactly the slots, with nothing unaddressed.
    assert_eq!(v2.header_len(), v2.slot_count() * v2.slot_stride());
    assert_eq!(v2.header_len(), HEADER_LEN);
    assert_eq!(v2.slot_offset(2), slot0, "commit 2 returns to slot 0");
}
