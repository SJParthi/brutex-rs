# 02 — Store format, version 1

Bytes on disk. This document is the authority; the code follows it.

The version number is in the header. A format change mints a **new version**
and old files stay readable at their own stride. Nothing is ever mutated in
place.

---

## 1. File layout

```
byte 0                    64                                        EOF
  ├──────── header ────────┼─── record 0 ─┼─── record 1 ─┼── … ──────┤
           64 bytes            56 bytes       56 bytes
```

**Address of record *i*:**

```
ptr = base + 64 + i * 56
```

An add, a multiply, a load. No search, no decode, no allocation.

---

## 2. Header — 64 bytes

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 8 | `magic` | `b"BRUTEXB1"` |
| 8 | 2 | `format_version` | `1` |
| 10 | 2 | `record_stride` | `56`. Read it; never assume it. |
| 12 | 4 | `flags` | bit 0: checksums present |
| 16 | 8 | `n_valid` | **the commit counter.** See §5. |
| 24 | 8 | `first_ts_micros` | of record 0, for a cheap range reject |
| 32 | 8 | `last_ts_micros` | of record `n_valid-1` |
| 40 | 4 | `symbol_id` | resolved from the path; a cross-check, not the index |
| 44 | 4 | `timeframe_secs` | 60 for 1-minute |
| 48 | 8 | `header_crc` | over bytes 0..48 |
| 56 | 8 | reserved | zero |

Reserved bytes are zero and stay zero. A future field takes reserved space in
a **new version**, never by reinterpreting version 1.

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

const _: () = assert!(core::mem::size_of::<Bar>() == 56);
const _: () = assert!(core::mem::align_of::<Bar>() == 8);
```

**Prices are paisa integers.** Never a float, at any layer, for any reason.
A float price is how a rounding difference becomes a divergent result set six
months later.

**`i64::MIN` is the open-interest null sentinel.** Zero means zero. Spot
indices carry no open interest and store the sentinel; conflating that with a
genuine zero would make a derivative series and an index series indistinguish-
able on a field that matters.

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

---

## 5. Appending — the commit counter

```
1. pwrite the new records at offset  64 + n_valid * 56
2. fsync the data
3. store n_valid + count into the header with a RELEASE ordering
4. fsync the header
```

A reader loads `n_valid` with an **acquire** ordering and treats records
`0 .. n_valid` as the whole file. Bytes past `n_valid` are, by definition, not
there yet.

Consequence: **a torn record is unobservable.** A crash between steps 1 and 3
leaves bytes on disk that no reader will look at; the next append overwrites
them. A crash between 3 and 4 leaves a counter that may be re-published — the
same value, idempotently.

---

## 6. Integrity — one checksum per 4 KiB block

A 4 KiB block holds 73 whole records with 8 bytes of slack. Each block carries
a CRC in a sidecar file laid out at the same block index, so the record stride
stays clean.

```
bars/NSE/INDEX/NIFTY/1min/2024-03.bin
bars/NSE/INDEX/NIFTY/1min/2024-03.crc
```

**Why this is not optional.** A flipped bit in a raw `i64` price produces a
different price — a *plausible* one. There is no structure to violate, no
parse to fail. Without a checksum the corruption is silent and permanent, and
every downstream result derived from it is wrong in a way nobody can detect.

Verification is O(1) per read: one block CRC, not a file scan.

---

## 7. Opening — boundary check

```
if (len - 64) % 56 != 0 {
    // truncate to the last whole record, log loudly, continue
}
```

A file whose length does not divide by the stride was interrupted. The tail is
truncated to the last whole record and the event is logged with the byte count
discarded. Never silently, never by ignoring the remainder.

---

## 8. Enriched records are a different file

Overlay fields — implied volatility, greeks, anything computed — do **not**
widen this record. They live in a sibling file with their own version, their
own stride, and their own commit counter, addressed by the same index *i*.

```
bars/NSE/FNO/<contract>/1min/2024-03.bin      56-byte stride, version 1
bars/NSE/FNO/<contract>/1min/2024-03.ovl      its own stride, version 1
```

This keeps the base stride constant forever. A base file written in year one
is readable in year ten by arithmetic that has not changed.

---

## 9. What this format does not solve

| Hazard | Position |
|---|---|
| Page fault on a network mount that has gone away | Uninterruptible. Keep the store on local disk. This is an operational rule, not an architectural fix. |
| Two writers on one file | Not supported. One writer per file, enforced by an advisory lock, and the lock is a leaf — never held while acquiring another. |
| A wrong value that is well-formed | Out of scope here. Range validation happens at the ingest boundary, before a byte is written. |
