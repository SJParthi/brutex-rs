# 02 — Store format, version 2

Bytes on disk. This document is the authority; the code follows it.

The version number is in the header. A format change mints a **new version**
and old files stay readable at their own stride. Nothing is ever mutated in
place — `CLAUDE.md` §3 rule 8.

**Version 1 is retired.** §10 says what it was and why it is refused rather
than decoded. Its number is never reused.

---

## 1. File layout

```
byte 0                  32768                                       EOF
  ├──── header region ────┼─── record 0 ─┼─── record 1 ─┼── … ───────┤
   2 slots × 16384 spacing    56 bytes       56 bytes
```

**Address of record *i*:**

```
ptr = base + 32768 + i * 56
```

An add, a multiply, a load. No search, no decode, no allocation.

The two numbers in that formula are not constants on the read path. They come
from `store::layout::Layout`, selected by the file's own `format_version`.

---

## 2. Header region — two slots, one write each

The header region is `slot_count × 16384` bytes. Each slot carries **64 bytes
of fields** at the start of its 16384-byte span; the rest of the span is
reserved and zero.

Commit *g* is written to slot `g % slot_count`, so consecutive commits never
touch the same slot, and a reader takes the valid slot with the highest
generation that the file's length supports.

**Why the slots are 16384 bytes apart and not adjacent.** Storage does not fail
at byte granularity. The smallest unit a device programs or a filesystem
writes back is a block or a page, and a partially programmed unit takes
everything in it. Two slots 64 bytes apart share one such unit, which makes the
redundancy nominal. Measured on the two hosts this repository builds on:

| Host | Device block | Page |
|---|---|---|
| Apple Silicon, macOS, APFS | 4096 | 16384 |
| GitHub CI runner, x86_64 Linux, ext4 | 4096 | 4096 |

16384 is the largest of those. It is a constant rather than a query of the
host: a geometry that differed between the two would make the same bytes verify
differently on each (§3 rule 5).

### One slot — 64 bytes

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 8 | `magic` | `b"BRUTEXB2"` |
| 8 | 2 | `format_version` | `2` |
| 10 | 2 | `record_stride` | `56`. Read it; never assume it. |
| 12 | 4 | `flags` | bit 0: block checksums present |
| 16 | 8 | `generation` | which commit this slot holds. Higher wins. |
| 24 | 8 | `n_valid` | **the commit counter.** See §5. |
| 32 | 8 | `first_ts_micros` | of record 0. Meaningful only when `n_valid > 0`. |
| 40 | 8 | `last_ts_micros` | of record `n_valid-1` |
| 48 | 4 | `symbol_id` | resolved from the path; a cross-check, not the index |
| 52 | 4 | `timeframe_secs` | 60 for 1-minute |
| 56 | 4 | `slot_crc` | CRC-32C over bytes 0..56 **and** 60..64 |
| 60 | 4 | reserved | zero |

The checksum covers every byte of the slot except the four it occupies, so a
flipped bit anywhere in the 64 is detected — there is no window a corruption
can land in and be called clean.

Reserved bytes are zero and stay zero. A future field takes reserved space in
a **new version**, never by reinterpreting version 2.

The 64-byte slot size and the 16384-byte slot spacing are **family** constants:
every version uses them, which is what lets a reader locate the slots before it
knows which version the file is.

---

## 3. Record — 56 bytes, seven `i64`

```rust
#[repr(C)]
pub struct Bar {
    pub ts_micros:      i64,  //  0  microseconds since Unix epoch, UTC
    pub open:           i64,  //  8  paisa
    pub high:           i64,  // 16  paisa
    pub low:            i64,  // 24  paisa
    pub close:          i64,  // 32  paisa
    pub volume:         i64,  // 40  contracts or shares; 0 is a real zero
    pub open_interest:  i64,  // 48  paisa-free count; i64::MIN means null
}

const _: () = assert!(size_of::<Bar>() == 56);
const _: () = assert!(align_of::<Bar>() == 8);
```

**Prices are paisa integers.** Never a float, at any layer, for any reason.
A float price is how a rounding difference becomes a divergent result set six
months later.

**`i64::MIN` is the open-interest null sentinel.** Zero means zero. Spot
indices carry no open interest and store the sentinel; conflating that with a
genuine zero would make a derivative series and an index series indistinguish-
able on a field that matters.

**A record has no structure that a lost write violates.** An all-zero record is
a legal flat bar whose open interest is a real zero. Nothing about the record
can tell it apart from an extent that was never written; that is §6's job, and
it is why the checksum flag is not decoration.

---

## 4. Reading — read-only mapping

The file is mapped **read-only**. Reads are pointer arithmetic against
resident pages.

**Writes never go through the mapping.** They use positional writes
(`pwrite`), which return an error on a full disk.

This is not a preference. A writable mapping that runs out of space raises
**SIGBUS** — a signal, delivered asynchronously, catchable by no language
construct in any language. Every clean disk-full halt the system has is
disabled the moment a writable mapping is introduced. The predecessor system
halted correctly on a full disk; a naive port to a writable mapping would have
been strictly worse, and that is the single most valuable thing the failure
audit produced.

A header decoder over a shared mapping copies its 64 bytes **once** before it
checks anything, so the checksum covers exactly the bytes the decoded header
carries. A copy that caught a concurrent `pwrite` mid-flight fails that
checksum rather than returning half of each image.

---

## 5. Appending — the commit counter

```
1. pwrite the new records at offset  32768 + n_valid * 56
2. pwrite the affected block checksums into the .crc sidecar
3. fsync the data, through Commit::durable_through
4. pwrite the 64-byte header slot for generation g   <- one write
5. fsync the header
```

A reader treats records `0 .. n_valid` as the whole file. Bytes past `n_valid`
are, by definition, not there yet.

Consequences:

* **A torn record is unobservable.** A crash between steps 1 and 4 leaves bytes
  on disk that no reader will look at; the next append overwrites them.
* **A torn header is unobservable.** Step 4 is one write of one self-checked
  64-byte unit, into the slot that does *not* hold the previous commit. A crash
  during it leaves a slot that fails its own checksum, and the reader takes the
  previous generation.
* **A header that outran its data loses the tail, not the file.** If the slot
  becomes durable and the records do not, the counter names records the length
  cannot support; the reader falls back to the previous generation rather than
  refusing the whole file.

`Commit::durable_through` is the byte offset step 3 must cover. This repository
performs no I/O yet, so it states the offset rather than issuing the barrier.

**Exactly one writer per file.** Two writers reading the same generation
produce the same slot, the same offset and the same record range. Nothing in
the store crate can enforce that — an exclusive lock is I/O — so the writer
takes one on the `.lock` sibling (§8) before step 1. This is a stated
precondition, not a guarantee the format provides.

---

## 6. Integrity — one checksum per block of 73 records

A block is **73 whole records = 4088 bytes**, counted in records rather than in
bytes, and anchored at the end of the header region:

```
block_of(i)  =  i / 73
block start  =  32768 + block * 4088
```

Because the block length is a whole multiple of the stride, **a record can
never begin in one block and end in the next**. Verifying a record reads one
checksum, never two, whatever its index.

The earlier byte-addressed 4096-byte block did not divide 56: 4096/56 = 73.14,
so 23 of the first 2,000 records straddled a boundary and verifying one of them
against either block checked part of a record and pronounced the whole of it
good.

**Why not the padded 4096-byte block** this document used to describe ("73
whole records with 8 bytes of slack")? It also never straddles — but a block is
a range of the file, so padding the block pads the record array, and the
address of record *i* stops being `base + 32768 + i·56` and becomes
`base + 32768 + (i/73)·4096 + (i%73)·56`: a division and two multiplies instead
of a multiply and an add. The 8 bytes are wasted either way, so the padded form
buys nothing for the cost.

### The tail block covers the committed prefix, not the nominal block

The last block of a file holds only the records the commit counter covers. Its
checksum is taken over

```
(n_valid mod 73) * 56 bytes  from the block start
```

and never over the nominal 4088 — which for 72 of every 73 file states would
name bytes past EOF. The writer recomputes the tail block's checksum on every
commit that lands in it; the reader verifies against the `n_valid` it read from
the header. Both sides derive the length from the same counter.

Each block's CRC-32C lives in a sidecar at the same **block index**:

```
bars/groww/NSE/INDEX/NIFTY/1min/2024-03.bin
bars/groww/NSE/INDEX/NIFTY/1min/2024-03.crc
```

The sidecar is indexed by `i / 73`, not by `(32768 + i*56) / 4096`.

**Why this is not optional.** A flipped bit in a raw `i64` price produces a
different price — a *plausible* one. There is no structure to violate, no
parse to fail. Without a checksum the corruption is silent and permanent, and
every downstream result derived from it is wrong in a way nobody can detect.
It is also the only thing that can tell a lost write from 73 flat bars.

A file whose `flags` bit 0 is clear carries no sidecar, and a verification
request against it is **refused** rather than answered — "verified" and "there
was nothing to verify against" are different answers.

Verification is O(1) per read: one block CRC, not a file scan. Enforced by
`docs/04-invariants.md` C-07 — sealing one block costs the same at 1×, 10× and
100× the file's record count.

**Read "O(1)" precisely.** The *operation* is constant because a block is a
fixed 4,088 bytes; the CRC inside it still reads every one of them, and always
will. Measured on an Apple M4 Pro after D-0032: 1,487.5 ns for the checksum,
1,656.0 ns for `block::seal` end to end, 20.4 ns amortised over the 73 records a
block covers. Before D-0032 the same seal cost 24,083.0 ns. `docs/06-limits.md`
§14 carries the full numbers and states what is not claimed.

---

## 7. Opening — boundary check

```
if (len - 32768) % 56 != 0 {
    // truncate to the last whole record, log loudly, continue
}
```

A file whose length does not divide by the stride was interrupted. The tail is
truncated to the last whole record and the event is logged with the byte count
discarded. Never silently, never by ignoring the remainder.

A file shorter than the header region is all tail, and is reported as such.

---

## 8. Sibling files of a month

Overlay fields — implied volatility, greeks, anything computed — do **not**
widen this record. They live in a sibling file with their own version, their
own stride, and their own commit counter, addressed by the same index *i*.

| Extension | Holds |
|---|---|
| `.bin` | the bar records |
| `.crc` | one block checksum per block index (§6) |
| `.ovl` | computed overlay fields, at their own stride |
| `.lock` | the advisory lock one writer holds for the month (§5, §9) |

```
bars/<vendor>/NSE/FNO/<contract>/1min/2024-03.bin      56-byte stride, version 2
bars/<vendor>/NSE/FNO/<contract>/1min/2024-03.ovl      its own stride, version 1
```

This keeps the base stride constant forever. A base file written in year one
is readable in year ten by arithmetic that has not changed.

Every one of those names is rendered by `store::path::StorePath` and nothing
else. The vendor is the first segment (D-0019); every segment is
case-canonical, because two segments differing only in case are two files on
ext4 and one file on APFS.

---

## 9. What this format does not solve

| Hazard | Position |
|---|---|
| Page fault on a network mount that has gone away | Uninterruptible. Keep the store on local disk. This is an operational rule, not an architectural fix. |
| Two writers on one file | Not supported. One writer per file, enforced by an advisory lock on the `.lock` sibling, and the lock is a leaf — never held while acquiring another. Nothing in `crates/store` can check it. |
| A failure coarser than 16384 bytes | Takes both header slots. No arrangement inside one file survives a dead device. |
| A symlink at a path component | Defeats vendor-prefix isolation, which is a **lexical** property of `StorePath`, not a filesystem one. The writer must open with `openat` + `O_NOFOLLOW` per component and halt naming the linked component. |
| A wrong value that is well-formed | Out of scope here. Range validation happens at the ingest boundary, before a byte is written. |

---

## 10. Version 1 — retired

Version 1 described:

* a **64-byte** header region holding **one** slot,
* fields at `n_valid` 16, `first_ts_micros` 24, `last_ts_micros` 32,
  `symbol_id` 40, `timeframe_secs` 44,
* an **8-byte** `header_crc` at offset 48 covering bytes 0..48,
* a byte-addressed **4096-byte** checksum block,
* magic `b"BRUTEXB1"`.

Every one of those numbers is different in version 2, and version 2 inserts a
`generation` field at offset 16. Decoding a version-1 file with version 2's
decoder would lift every field from the wrong offset and return plausible
integers.

So version 1 is **refused by number**, naming the reason
(`FormatError::RetiredVersion(1)`), never decoded and never reported as a
damaged header. Its number is not reused: §3 rule 8 makes the version an
append-only identifier, not a slot to be overwritten.

No version-1 file can exist — version 1 never had a reader or a writer in this
repository, only constants. The entry costs one comparison and removes the only
way this build could misread one if that assumption is ever wrong.
