//! Crash and corruption behaviour of the header: `store::fault::*`.
//!
//! The defect these tests exist for: publishing a commit wrote `n_valid`,
//! `last_ts_micros` and `header_crc` as three separate stores. A crash between
//! any two left a header whose checksum disagreed with its own counter, and a
//! header that fails its checksum condemns the **whole file** rather than the
//! tail. D-0004's guarantee that a torn record is unobservable did not extend
//! to a torn header.
//!
//! The second defect: the reader took the highest-generation slot, checked
//! only that one, and returned an error for the whole file when it failed —
//! reproducing the outcome the design exists to abolish, with the check moved
//! from the checksum to the file length. The intact previous commit sat in the
//! other slot and was never consulted.
//!
//! What is exercised here is the 32768-byte header region of a file image, not
//! a file: this crate performs no I/O yet. Every byte tested is a byte that
//! would be on disk, and the write under test is the single `pwrite` a writer
//! will issue, so the crash window is modelled exactly. **No test here kills a
//! real process or cuts real power**, and the 65 prefixes walked below are 65
//! points of a much larger space of partial outcomes.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use store::block;
use store::format::{
    Bar, FLAG_CHECKSUMS, FormatError, HEADER_LEN, RECORD_LEN, RECORD_STRIDE, SLOT_LEN,
};
use store::header::{Commit, Header};
use store::layout::Layout;
use store::path::FileKind;

/// Bytes of header region in a version-2 file.
const REGION_LEN: usize = 32_768;
/// Bytes from one header slot to the next, as a `usize`.
const SLOT_STRIDE_LEN: usize = 16_384;

/// The open of the first bar of 2024-06-03, in microseconds.
const T0: i64 = 1_717_386_300_000_000;
/// One minute, in microseconds.
const MINUTE: i64 = 60_000_000;

/// The first and last timestamps of `count` one-minute bars starting at index
/// `from`.
const fn batch(from: i64, count: i64) -> (i64, i64) {
    (T0 + from * MINUTE, T0 + (from + count - 1) * MINUTE)
}

/// Applies the first `prefix` bytes of a commit — a write torn at that point.
fn apply(region: &mut [u8], commit: &Commit, prefix: usize) {
    let at = usize::try_from(commit.offset).expect("offset is in range");
    region
        .iter_mut()
        .skip(at)
        .zip(commit.bytes.iter().take(prefix))
        .for_each(|(dst, src)| *dst = *src);
}

/// A region with three commits already published, and the header of the last.
///
/// Generation 0 → slot 0 (empty), 1 → slot 1 (100 records), 2 → slot 0 (150).
/// So slot 0 holds generation 2 and slot 1 holds generation 1: the previous
/// commit is intact in the slot the next commit will not touch.
fn three_commits() -> (Vec<u8>, Header) {
    let mut region = vec![0u8; REGION_LEN];

    let genesis = Header::genesis(7, 60, FLAG_CHECKSUMS);
    apply(&mut region, &genesis.commit().expect("v2"), SLOT_LEN);

    let (first_from, first_to) = batch(0, 100);
    let first = genesis.advance(100, first_from, first_to).expect("fits");
    apply(&mut region, &first.commit().expect("v2"), SLOT_LEN);

    let (second_from, second_to) = batch(100, 50);
    let second = first.advance(50, second_from, second_to).expect("fits");
    apply(&mut region, &second.commit().expect("v2"), SLOT_LEN);

    let read = Header::read_region(&region, file_len(200)).expect("three whole commits");
    assert_eq!(read, second);
    assert_eq!(read.generation, 2);
    assert_eq!(read.n_valid, 150);
    // The range the header advertises is the range the records occupy -- the
    // field that no method used to set at all.
    assert_eq!(read.first_ts_micros, T0);
    assert_eq!(read.last_ts_micros, second_to);
    (region, second)
}

/// A file length that can hold `records` records.
const fn file_len(records: u64) -> u64 {
    HEADER_LEN + records * RECORD_STRIDE
}

// A private `bar_bytes` helper used to live here, spelling the seven
// little-endian fields out again. It was a SECOND DEFINITION of the record
// format -- precisely what `Bar::image`'s own doc comment forbids -- and while
// it existed nothing in the repository called `Bar::image` at all. Deleted by
// D-0039: this file now builds record bytes with the encoder the format
// document is the authority for, so a drift between the two is no longer
// possible because there is only one.

#[test]
fn a_torn_header_commit_never_reports_an_uncommitted_count() {
    let (committed, previous) = three_commits();
    let (from, to) = batch(150, 25);
    let next = previous.advance(25, from, to).expect("fits");
    let commit = next.commit().expect("v2");

    // Generation 3 goes to slot 1, which holds generation 1 -- never slot 0,
    // which holds the newest committed state. That is the whole argument, and
    // the two slots are a whole page apart so the write cannot reach slot 0.
    assert_eq!(commit.slot, 1);
    assert_eq!(commit.offset, 16_384);

    let mut published_from = None;
    for prefix in 0..=SLOT_LEN {
        let mut region = committed.clone();
        apply(&mut region, &commit, prefix);

        let read = Header::read_region(&region, file_len(200))
            .unwrap_or_else(|e| panic!("a torn write at byte {prefix} lost the file: {e}"));

        // The reader sees the new commit exactly when the slot it landed in
        // holds the whole image, and the previous one otherwise. There is no
        // third answer -- no count that was never committed, and no header
        // whose counter and checksum disagree.
        let landed_whole = region[SLOT_STRIDE_LEN..SLOT_STRIDE_LEN + SLOT_LEN] == commit.bytes[..];
        if landed_whole {
            assert_eq!(read, next, "whole image at prefix {prefix}");
            assert_eq!(read.n_valid, 175);
            published_from.get_or_insert(prefix);
        } else {
            assert_eq!(read, previous, "partial image at prefix {prefix}");
            assert_eq!(read.n_valid, 150);
            assert_eq!(read.generation, 2);
        }
        assert!(
            read.n_valid == 150 || read.n_valid == 175,
            "prefix {prefix} produced a count nobody committed: {}",
            read.n_valid,
        );
    }

    // The checksum sits at bytes 56..60 and the four bytes after it are
    // reserved zeros, which the previous image also held -- so the commit
    // becomes durable the moment its checksum is down, at 60 bytes, and not
    // before.
    assert_eq!(published_from, Some(60));
}

#[test]
fn commit_counter_publishes_last() {
    // docs/04-invariants.md S-03: a reader never observes a record beyond
    // n_valid. The counter is published last, so the reader's horizon is
    // always exactly what some completed commit published -- never a byte
    // further, whatever is already sitting on the disk past it.
    let v2 = Layout::V2;
    let (committed, previous) = three_commits();
    let previous_commit = previous.commit().expect("v2");
    let (from, to) = batch(150, 25);
    let next = previous.advance(25, from, to).expect("fits");
    let commit = next.commit().expect("v2");

    // Step 1 of docs/02 §5 writes the records at the offset the OLD counter
    // names, so they are past the horizon the whole time they are landing.
    assert_eq!(v2.offset_of(previous.n_valid), Ok(file_len(150)));
    assert_eq!(previous_commit.durable_through, file_len(150));
    assert_eq!(commit.durable_through, file_len(175));

    for prefix in 0..=SLOT_LEN {
        let mut region = committed.clone();
        apply(&mut region, &commit, prefix);
        // The file is long enough for all 175 records the whole time: the
        // bytes are there, and the counter is still what decides.
        let read = Header::read_region(&region, file_len(175)).expect("never lost");
        let horizon = v2.offset_of(read.n_valid).expect("fits");
        let published = if read.n_valid == next.n_valid {
            commit.durable_through
        } else {
            previous_commit.durable_through
        };
        assert_eq!(
            horizon, published,
            "at prefix {prefix} the reader's horizon was not a published one",
        );
    }
}

#[test]
fn kill_between_write_and_commit() {
    // docs/04-invariants.md S-04. The writer pwrites 25 records and then the
    // header slot; power is lost in between, with the header page reaching the
    // platter first -- which is the LIKELY ordering, because page 0 is
    // re-dirtied on every commit and is the hottest writeback candidate in the
    // file.
    let v2 = Layout::V2;
    let (committed, previous) = three_commits();
    let (from, to) = batch(150, 25);
    let next = previous.advance(25, from, to).expect("fits");
    let commit = next.commit().expect("v2");

    // The commit names the offset the records must have reached first, so a
    // writer cannot claim it did not know which barrier to take.
    assert_eq!(commit.durable_through, file_len(175));

    let mut region = committed.clone();
    apply(&mut region, &commit, SLOT_LEN);

    // Case A -- the records never extended the file. The header is whole and
    // says 175; the bytes say 150. The reader returns generation 2. It does
    // NOT condemn the file, which is what losing a month to save a tail looks
    // like, and it does not return 25 records of whatever was there.
    let read = Header::read_region(&region, file_len(150)).expect("the previous commit survives");
    assert_eq!(read, previous);
    assert_eq!((read.generation, read.n_valid), (2, 150));

    // Case B -- the file WAS extended and the extent reads as zeros. The
    // length supports the count, so the counter alone cannot catch it.
    let read = Header::read_region(&region, file_len(175)).expect("the header is whole");
    assert_eq!(read.n_valid, 175);

    // And an all-zero record passes every structural check a record has: it
    // is a legal flat bar, and its open interest is a REAL zero rather than
    // the null sentinel, so a spot-index file would quietly acquire
    // derivative-shaped records.
    let ghost = Bar::default();
    assert!(ghost.ohlc_is_sane());
    assert!(!ghost.oi_is_null());
    assert_eq!(ghost.oi(), Some(0));

    // The block checksum is the only thing that can say those bytes are not
    // the bytes that were committed. Record 174 lives in block 2, which under
    // a counter of 175 covers 29 records.
    let tail = v2.block_of(174);
    assert_eq!(tail, 2);
    assert_eq!(v2.records_in_block(tail, 175), Ok(29));

    let written = vec![0x5Au8; 29 * 56];
    let sealed = block::seal(v2, 175, tail, &written).expect("the writer sealed it");
    assert_eq!(block::verify(&read, v2, tail, &written, sealed), Ok(()));

    let lost = vec![0u8; written.len()];
    let computed = block::seal(v2, 175, tail, &lost).expect("same length");
    assert_eq!(
        block::verify(&read, v2, tail, &lost, sealed),
        Err(FormatError::BlockChecksum {
            block: tail,
            stored: sealed,
            computed,
        }),
        "a lost write must be named, not read as 29 flat bars",
    );
}

#[test]
fn bitflip_detected() {
    // docs/04-invariants.md S-06. A flipped bit in a raw i64 price yields a
    // DIFFERENT, PLAUSIBLE price -- there is no parse to fail and no structure
    // to violate. Every bit of every byte of a block, walked.
    let v2 = Layout::V2;
    let (from, to) = batch(0, 2);
    let header = Header::genesis(7, 60, FLAG_CHECKSUMS)
        .advance(2, from, to)
        .expect("fits");

    let real = Bar {
        ts_micros: T0,
        open: 2_333_870,
        high: 2_333_870,
        low: 2_308_370,
        close: 2_310_955,
        volume: 0,
        open_interest: i64::MIN,
    };
    let next = Bar {
        ts_micros: T0 + MINUTE,
        ..real
    };
    // Through `Bar::image` -- the one encoder `docs/02-store-format.md` §3 is
    // the authority for. Not a local copy of it.
    let mut bytes = real.image().to_vec();
    bytes.extend(next.image());
    assert_eq!(bytes.len(), 112);
    assert_eq!(real.image().len(), RECORD_LEN);
    assert_eq!(Bar::decode(&bytes), Ok(real), "the encoder's own inverse");

    let sealed = block::seal(v2, header.n_valid, 0, &bytes).expect("one partial block");
    assert_eq!(block::verify(&header, v2, 0, &bytes, sealed), Ok(()));

    for byte in 0..bytes.len() {
        for bit in 0..8u32 {
            let mut damaged = bytes.clone();
            damaged[byte] ^= 1u8 << bit;
            let refusal = block::verify(&header, v2, 0, &damaged, sealed);
            assert!(
                matches!(refusal, Err(FormatError::BlockChecksum { block: 0, .. })),
                "bit {bit} of byte {byte} was not detected: {refusal:?}",
            );
        }
    }

    // A file that declares no checksums cannot have a block verified, and
    // says so rather than answering "fine".
    let unchecked = Header { flags: 0, ..header };
    assert!(!unchecked.checksums_present());
    assert_eq!(
        block::verify(&unchecked, v2, 0, &bytes, sealed),
        Err(FormatError::ChecksumsAbsent),
    );

    // And a block offered at a length the geometry does not give it is
    // refused rather than trimmed to fit.
    assert_eq!(
        block::seal(v2, header.n_valid, 0, &bytes[..111]),
        Err(FormatError::BlockLengthMismatch {
            block: 0,
            len: 111,
            need: 112,
        }),
    );
}

#[test]
fn a_single_bit_flip_in_any_header_byte_is_detected() {
    let (committed, _) = three_commits();

    // Slot 0 holds generation 2 -- the newest commit. Damage it anywhere and
    // the reader must fall back to generation 1, never to a mixture.
    for byte in 0..SLOT_LEN {
        for bit in 0..8u32 {
            let mut region = committed.clone();
            region[byte] ^= 1u8 << bit;

            let read = Header::read_region(&region, file_len(200)).unwrap_or_else(|e| {
                panic!("bit {bit} of byte {byte} took the whole file down: {e}")
            });
            assert_eq!(
                read.generation, 1,
                "bit {bit} of byte {byte} was not detected",
            );
            assert_eq!(read.n_valid, 100);
        }
    }
}

#[test]
fn a_damaged_slot_is_not_repaired_by_a_second_write_to_the_same_slot() {
    // Two commits in a row cannot land in the same slot, so the corrupted copy
    // of the newest state is never the only copy.
    let (_, header) = three_commits();
    let mut generation = header;
    let mut slots = Vec::new();
    for step in 0..8i64 {
        let (from, to) = batch(150 + step, 1);
        generation = generation.advance(1, from, to).expect("fits");
        slots.push(generation.commit().expect("v2").slot);
    }
    assert_eq!(slots, vec![1, 0, 1, 0, 1, 0, 1, 0]);
    for pair in slots.windows(2) {
        assert_ne!(pair[0], pair[1], "consecutive commits shared a slot");
    }
}

#[test]
fn a_second_writer_at_the_same_generation_collides_on_one_slot() {
    // The crash table is conditioned on exactly one writer, and this crate
    // cannot enforce that: an exclusive lock is I/O. Measured rather than
    // assumed, so the precondition documented on Header::commit is not
    // decoration and the limit is not a guess.
    let (_, header) = three_commits();
    let (a_from, a_to) = batch(150, 25);
    let (b_from, b_to) = batch(150, 40);
    let a = header
        .advance(25, a_from, a_to)
        .expect("fits")
        .commit()
        .expect("v2");
    let b = header
        .advance(40, b_from, b_to)
        .expect("fits")
        .commit()
        .expect("v2");

    assert_eq!(
        a.slot, b.slot,
        "two writers from one generation take one slot"
    );
    assert_eq!(a.offset, b.offset);
    assert_ne!(a.header.n_valid, b.header.n_valid);
    assert_ne!(a.bytes, b.bytes);
    // Their record writes begin at the same byte, too.
    assert_eq!(Layout::V2.offset_of(header.n_valid), Ok(file_len(150)));
    assert_eq!(a.durable_through, file_len(175));
    assert_eq!(b.durable_through, file_len(190));

    // The lock that has to prevent it is a named sibling of the month, formed
    // by the one path renderer rather than invented at a call site.
    assert_eq!(FileKind::Lock.extension(), ".lock");
}

#[test]
fn a_commit_names_the_bytes_that_must_be_durable_first() {
    let genesis = Header::genesis(7, 60, FLAG_CHECKSUMS);
    assert_eq!(genesis.commit().expect("v2").durable_through, file_len(0));

    let (from, to) = batch(0, 100);
    let after = genesis.advance(100, from, to).expect("fits");
    assert_eq!(after.commit().expect("v2").durable_through, file_len(100));

    // A counter whose data would end past u64 is refused rather than wrapped:
    // a wrapped barrier names a byte offset that is inside the file.
    let absurd = Header {
        n_valid: u64::MAX,
        ..genesis
    };
    assert_eq!(absurd.commit(), Err(FormatError::OffsetOverflow));
}

#[test]
fn both_slots_damaged_refuses_rather_than_guessing() {
    // Every copy of the header is gone. There is no state to fall back to that
    // would not be invented, so this refuses and says so.
    assert_eq!(
        Header::read_region(&vec![0u8; REGION_LEN], file_len(200)),
        Err(FormatError::NoValidHeader),
    );

    let (mut region, _) = three_commits();
    region[0] ^= 0xFF;
    region[SLOT_STRIDE_LEN] ^= 0xFF;
    assert_eq!(
        Header::read_region(&region, file_len(200)),
        Err(FormatError::NoValidHeader),
    );

    // A region too short to hold one slot is the same answer.
    assert_eq!(
        Header::read_region(&[0u8; 8], file_len(200)),
        Err(FormatError::NoValidHeader),
    );
}

#[test]
fn a_short_header_region_is_refused_rather_than_answered_from_a_stale_slot() {
    // Handing read_region less than the header region used to return whichever
    // commit happened to fit -- generation 2 with 150 records, and Ok, with
    // the 25 records committed since simply gone. It slipped the position
    // guard whenever the surviving slot's generation was even, so half the
    // time.
    let (region, _) = three_commits();

    assert_eq!(
        Header::read_region(&region[..SLOT_STRIDE_LEN], file_len(200)),
        Err(FormatError::HeaderRegionTooShort { slots: 1, need: 2 }),
    );
    assert_eq!(
        Header::read_region(&region[..SLOT_LEN], file_len(200)),
        Err(FormatError::HeaderRegionTooShort { slots: 0, need: 2 }),
    );
    assert!(Header::read_region(&region, file_len(200)).is_ok());
}

#[test]
fn record_bytes_never_audition_as_a_header_slot() {
    // The region a caller passes may be the whole read-only mapping, which is
    // the usage the module endorses. Without a bound, every aligned run of
    // record bytes that happened to begin BRUTEXB would be a candidate.
    let (committed, header) = three_commits();
    let (from, to) = batch(150, 900);
    let impostor = header
        .advance(900, from, to)
        .expect("fits")
        .commit()
        .expect("v2");
    assert_eq!(impostor.header.generation, 3, "a HIGHER generation");

    let mut file = vec![0u8; REGION_LEN + 4 * SLOT_STRIDE_LEN];
    file[..REGION_LEN].copy_from_slice(&committed);
    // Placed in the record area, one slot stride past the header region.
    file[REGION_LEN..REGION_LEN + SLOT_LEN].copy_from_slice(&impostor.bytes);

    let read = Header::read_region(&file, file_len(2_000)).expect("the header region is intact");
    assert_eq!((read.generation, read.n_valid), (2, 150));

    // And when the header region really is blank, the answer is "no header
    // survived" -- not a position complaint about bytes that were never
    // header at all, which would condemn a file for the wrong reason.
    let mut blank = vec![0u8; REGION_LEN + 4 * SLOT_STRIDE_LEN];
    blank[REGION_LEN..REGION_LEN + SLOT_LEN].copy_from_slice(&impostor.bytes);
    assert_eq!(
        Header::read_region(&blank, file_len(2_000)),
        Err(FormatError::NoValidHeader),
    );
}

#[test]
fn a_commit_found_in_the_wrong_slot_is_refused() {
    // A writer that puts a commit in the wrong slot overwrites the only
    // surviving copy of the previous one. That is a bug, not a state to
    // tolerate, so it is named.
    let mut region = vec![0u8; REGION_LEN];
    let genesis = Header::genesis(7, 60, FLAG_CHECKSUMS);
    apply(&mut region, &genesis.commit().expect("v2"), SLOT_LEN);

    let (from, to) = batch(0, 100);
    let first = genesis.advance(100, from, to).expect("fits");
    let commit = first.commit().expect("v2");
    assert_eq!(commit.slot, 1);

    // Same bytes, wrong slot.
    region
        .iter_mut()
        .zip(commit.bytes)
        .for_each(|(dst, src)| *dst = src);
    assert_eq!(
        Header::read_region(&region, file_len(200)),
        Err(FormatError::SlotPositionMismatch {
            expected: 1,
            found: 0,
        }),
    );
}

#[test]
fn a_counter_claiming_more_than_the_file_holds_falls_back_to_the_last_supported_commit() {
    // Trusting the counter over the length reads past the end and returns
    // whatever bytes were there -- plausible integers, silently wrong. But
    // refusing the WHOLE FILE for it loses a month of bars to a tail that a
    // perfectly self-consistent previous commit already describes.
    let (mut region, header) = three_commits();
    let (from, to) = batch(150, 1_000_000);
    let overreach = header.advance(1_000_000, from, to).expect("fits");
    apply(&mut region, &overreach.commit().expect("v2"), SLOT_LEN);

    let read = Header::read_region(&region, file_len(200)).expect("generation 2 is intact");
    assert_eq!(read, header);
    assert_eq!((read.generation, read.n_valid), (2, 150));

    // When the file is long enough, the newest commit is what is returned --
    // the fallback is a fallback, not a ceiling.
    assert_eq!(
        Header::read_region(&region, file_len(1_000_150)).expect("supported"),
        overreach,
    );

    // And when NO slot is supported, the refusal is returned rather than a
    // fabricated success or a misleading NoValidHeader.
    assert_eq!(
        Header::read_region(&region, file_len(10)),
        Err(FormatError::CounterExceedsFile),
    );

    // The same answer when there is only ONE slot left to fall back from:
    // damage slot 0 so generation 2 is gone, leaving generation 3 alone and
    // unsupported. The refusal is still the counter's, not "no header".
    region[0] ^= 0xFF;
    assert_eq!(
        Header::read_region(&region, file_len(200)),
        Err(FormatError::CounterExceedsFile),
    );
    assert_eq!(
        Header::read_region(&region, file_len(1_000_150))
            .expect("the surviving slot is supported")
            .n_valid,
        1_000_150,
    );
}

#[test]
fn ragged_tail_truncates_loudly() {
    // docs/04-invariants.md S-07, and docs/02 §7. A file whose length does not
    // divide by the stride was interrupted mid-record. The remainder is
    // reported so it can be logged with a byte count -- never ignored, never
    // rounded away, and never counted as a record.
    let v2 = Layout::V2;
    for whole in 0..5u64 {
        let clean = file_len(whole);
        assert_eq!(v2.ragged_tail_bytes(clean), 0, "{whole} whole records");
        assert_eq!(v2.capacity_for(clean), whole);
        for torn in 1..RECORD_STRIDE {
            assert_eq!(
                v2.ragged_tail_bytes(clean + torn),
                torn,
                "{torn} bytes into record {whole}",
            );
            assert_eq!(
                v2.capacity_for(clean + torn),
                whole,
                "the torn record must not be counted",
            );
        }
    }
    // A file shorter than a header region is all tail.
    assert_eq!(v2.ragged_tail_bytes(30), 30);
    assert_eq!(v2.capacity_for(30), 0);
    assert_eq!(v2.ragged_tail_bytes(HEADER_LEN - 1), HEADER_LEN - 1);
}

#[test]
fn a_file_too_short_for_its_header_region_is_refused() {
    let (region, _) = three_commits();
    assert_eq!(
        Header::read_region(&region, HEADER_LEN - 1),
        Err(FormatError::TooShortForHeader),
    );
    assert_eq!(
        Header::read_region(&region, HEADER_LEN),
        Err(FormatError::CounterExceedsFile),
        "the header region fits, the 150 records do not",
    );
}

#[test]
fn the_counter_and_the_generation_refuse_to_wrap() {
    // A wrapped generation makes the OLDEST slot win, which silently
    // un-commits every record published since. A wrapped counter hides
    // records. Both are refusals.
    let exhausted = Header {
        generation: u64::MAX,
        ..Header::genesis(7, 60, FLAG_CHECKSUMS)
    };
    assert_eq!(
        exhausted.advance(1, T0, T0),
        Err(FormatError::GenerationExhausted),
    );

    let full = Header {
        n_valid: u64::MAX,
        ..Header::genesis(7, 60, FLAG_CHECKSUMS)
    };
    assert_eq!(full.advance(1, T0, T0), Err(FormatError::CounterOverflow));
    assert!(
        full.advance(0, 0, 0).is_ok(),
        "appending nothing is not an overflow"
    );
}

#[test]
fn a_batch_that_does_not_follow_the_committed_records_is_refused() {
    // A re-pull landing on top of newer data is well-formed bytes that no
    // checksum can catch afterwards, so it is caught here or never.
    let (_, header) = three_commits();
    assert_eq!(header.n_valid, 150);

    // Backwards within the batch itself.
    let (from, to) = batch(150, 25);
    assert_eq!(
        header.advance(25, to, from),
        Err(FormatError::TimestampsOutOfOrder {
            previous: to,
            next: from,
        }),
    );

    // Beginning exactly on the last committed bar, which would duplicate it.
    assert_eq!(
        header.advance(1, header.last_ts_micros, header.last_ts_micros),
        Err(FormatError::TimestampsOutOfOrder {
            previous: header.last_ts_micros,
            next: header.last_ts_micros,
        }),
    );

    // Beginning before it.
    let (older_from, older_to) = batch(100, 5);
    assert_eq!(
        header.advance(5, older_from, older_to),
        Err(FormatError::TimestampsOutOfOrder {
            previous: header.last_ts_micros,
            next: older_from,
        }),
    );

    // The next minute is fine, and it moves only the last timestamp.
    let (next_from, next_to) = batch(150, 25);
    let after = header.advance(25, next_from, next_to).expect("follows");
    assert_eq!(after.first_ts_micros, T0, "record 0 never moves");
    assert_eq!(after.last_ts_micros, next_to);
    assert_eq!(after.n_valid, 175);

    // An EMPTY file has nothing for a batch to follow, so the ordering guard
    // does not apply to it -- not even when the header still carries a stale
    // last timestamp ahead of the batch. A guard that fired here would refuse
    // the first append to every new file.
    let empty = Header {
        last_ts_micros: T0 + 999 * MINUTE,
        ..Header::genesis(7, 60, FLAG_CHECKSUMS)
    };
    assert_eq!(empty.n_valid, 0);
    let opened = empty
        .advance(1, T0, T0)
        .expect("an empty file follows nothing");
    assert_eq!(opened.first_ts_micros, T0);
    assert_eq!(opened.last_ts_micros, T0);
    assert_eq!(opened.n_valid, 1);

    // An empty commit carries both timestamps forward untouched.
    let republished = header.advance(0, 0, 0).expect("no records is not an error");
    assert_eq!(republished.first_ts_micros, header.first_ts_micros);
    assert_eq!(republished.last_ts_micros, header.last_ts_micros);
    assert_eq!(republished.n_valid, header.n_valid);
    assert_eq!(republished.generation, header.generation + 1);
}

#[test]
fn a_header_whose_advertised_range_runs_backwards_is_refused() {
    // Only a hand-written struct can reach this state, because `advance`
    // refuses to produce it -- but a hand-written struct is exactly what a
    // corrupt file decodes into once its checksum happens to agree.
    let backwards = Header {
        n_valid: 10,
        first_ts_micros: T0 + MINUTE,
        last_ts_micros: T0,
        ..Header::genesis(7, 60, FLAG_CHECKSUMS)
    };
    assert_eq!(
        backwards.validate(Layout::V2, file_len(10)),
        Err(FormatError::TimestampsOutOfOrder {
            previous: T0 + MINUTE,
            next: T0,
        }),
    );

    // A single record: first and last name the same instant, which is a range
    // that does not run backwards. Refusing it would condemn every file that
    // holds exactly one bar.
    let one = Header {
        n_valid: 1,
        first_ts_micros: T0,
        last_ts_micros: T0,
        ..Header::genesis(7, 60, FLAG_CHECKSUMS)
    };
    assert_eq!(one.validate(Layout::V2, file_len(1)), Ok(()));

    // An empty file says nothing about its range, so nothing is checked.
    let empty = Header {
        first_ts_micros: T0 + MINUTE,
        last_ts_micros: T0,
        ..Header::genesis(7, 60, FLAG_CHECKSUMS)
    };
    assert_eq!(empty.validate(Layout::V2, file_len(10)), Ok(()));
}
