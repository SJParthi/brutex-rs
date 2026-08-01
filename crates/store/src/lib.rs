//! The fixed-stride bar file: bytes on disk, addressed by arithmetic.
//!
//! `docs/02-store-format.md` is the authority for every byte here.
//!
//! # What is here
//!
//! | Module | Owns |
//! |---|---|
//! | [`crc`] | CRC-32C, the one checksum this store computes |
//! | [`mod@format`] | the record, the shared constants, and every error |
//! | [`layout`] | one version's geometry, and the dispatch that selects it |
//! | [`header`] | the two-slot header and the single-write commit |
//! | [`block`] | the per-block checksum, over the committed prefix |
//! | [`path`] | the only way a store path is built |
//!
//! # Six properties the types enforce rather than document
//!
//! 1. **A record never straddles a checksum block.** The block length is a
//!    whole multiple of the record stride, so verifying a record reads exactly
//!    one block's checksum — never one of the two blocks its bytes fell into.
//!    See [`layout`].
//! 2. **The stride is never a global constant on the read path.** It comes
//!    from [`layout::Layout`], which comes from the file's own
//!    `format_version`. An unrecognised version is refused by number, and a
//!    retired one by name; nothing falls back to the current stride. See
//!    [`layout::Layout::for_version`].
//! 3. **A geometry is never mutated in place.** The header region, the field
//!    offsets, the checksum and the block length all changed, so this is
//!    format **version 2** with magic `BRUTEXB2`. Version 1's number is
//!    retired, never reused — `CLAUDE.md` §3 rule 8. See
//!    [`format::RETIRED_VERSIONS`].
//! 4. **A header update is one write of one self-checked unit.** There is no
//!    API that can update the counter, the timestamp and the checksum
//!    separately, because [`header::Commit`] carries one offset and one
//!    buffer. See [`header`].
//! 5. **Redundant header slots sit in different failure units.** Storage does
//!    not fail at byte granularity, so the two slots are a whole page apart
//!    rather than adjacent. See [`format::SLOT_STRIDE`].
//! 6. **A vendor cannot write outside its own prefix, lexically.** The first
//!    path segment is a `Vendor`, not a string, and every other segment is
//!    checked and case-canonical before a path exists. What that does *not*
//!    cover — a symlink — is stated in [`path::StorePath::to_path_buf`]
//!    rather than implied away.

#![forbid(unsafe_code)]

pub mod block;
pub mod crc;
pub mod format;
pub mod header;
pub mod layout;
pub mod path;
