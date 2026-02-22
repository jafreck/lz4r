//! Byte-order I/O helpers, block-size utilities, header checksum, and compress-bound functions.
//!
//! Translated from lz4frame.c v1.10.0, lines 167–420.
//!
//! Covers:
//! - LE read/write helpers (`read_le32`, `write_le32`, `read_le64`, `write_le64`)
//! - `lz4f_compression_level_max` — mirrors `LZ4F_compressionLevel_max`
//! - `lz4f_get_block_size`        — mirrors `LZ4F_getBlockSize`
//! - `lz4f_optimal_bsid`          — mirrors `LZ4F_optimalBSID`
//! - `lz4f_header_checksum`       — mirrors `LZ4F_headerChecksum`
//! - `lz4f_compress_bound_internal` — mirrors `LZ4F_compressBound_internal`
//! - `lz4f_compress_frame_bound`  — mirrors `LZ4F_compressFrameBound`

use crate::frame::types::{
    BlockChecksum, BlockSizeId, ContentChecksum, Preferences, BF_SIZE, BH_SIZE, MAX_FH_SIZE,
};
use crate::xxhash::xxh32_oneshot;

// ─────────────────────────────────────────────────────────────────────────────
// Maximum HC compression level (lz4hc.h: LZ4HC_CLEVEL_MAX)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum allowed LZ4 HC compression level.
/// Equivalent to `LZ4HC_CLEVEL_MAX` = 12.
pub const LZ4HC_CLEVEL_MAX: i32 = 12;

// ─────────────────────────────────────────────────────────────────────────────
// Byte-order I/O helpers (lz4frame.c:187–231)
// ─────────────────────────────────────────────────────────────────────────────

/// Read a little-endian `u32` from `src` at byte `offset`.
///
/// Portable — no alignment or host-endianness assumptions.
/// Mirrors `LZ4F_readLE32` (lz4frame.c:187–195).
#[inline]
pub fn read_le32(src: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        src[offset],
        src[offset + 1],
        src[offset + 2],
        src[offset + 3],
    ])
}

/// Write a little-endian `u32` into `dst` at byte `offset`.
///
/// Mirrors `LZ4F_writeLE32` (lz4frame.c:197–204).
#[inline]
pub fn write_le32(dst: &mut [u8], offset: usize, value: u32) {
    dst[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Read a little-endian `u64` from `src` at byte `offset`.
///
/// Mirrors `LZ4F_readLE64` (lz4frame.c:206–218).
#[inline]
pub fn read_le64(src: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        src[offset],
        src[offset + 1],
        src[offset + 2],
        src[offset + 3],
        src[offset + 4],
        src[offset + 5],
        src[offset + 6],
        src[offset + 7],
    ])
}

/// Write a little-endian `u64` into `dst` at byte `offset`.
///
/// Mirrors `LZ4F_writeLE64` (lz4frame.c:220–231).
#[inline]
pub fn write_le64(dst: &mut [u8], offset: usize, value: u64) {
    dst[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

// ─────────────────────────────────────────────────────────────────────────────
// Compression level and block-size utilities (lz4frame.c:331–371)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the maximum LZ4 HC compression level (12).
///
/// Equivalent to `LZ4F_compressionLevel_max()` (lz4frame.c:331).
#[inline]
pub fn lz4f_compression_level_max() -> i32 {
    LZ4HC_CLEVEL_MAX
}

/// Returns the block byte-size for a given `BlockSizeId`.
///
/// `BlockSizeId::Default` (0) is treated as `Max64Kb`, matching
/// `LZ4F_BLOCKSIZEID_DEFAULT` in lz4frame.c:250.
/// Returns `None` for any variant that cannot be resolved (shouldn't happen
/// with well-formed inputs, but guards against future enum extensions).
///
/// Equivalent to `LZ4F_getBlockSize` (lz4frame.c:333–342).
pub fn lz4f_get_block_size(block_size_id: BlockSizeId) -> Option<usize> {
    // Mirrors C: static const size_t blockSizes[4] = { 64 KB, 256 KB, 1 MB, 4 MB }
    const BLOCK_SIZES: [usize; 4] = [
        64 * 1024,       // index 0 → Max64Kb  (ID 4)
        256 * 1024,      // index 1 → Max256Kb (ID 5)
        1024 * 1024,     // index 2 → Max1Mb   (ID 6)
        4 * 1024 * 1024, // index 3 → Max4Mb   (ID 7)
    ];

    // Normalize Default → Max64Kb (mirrors: if (blockSizeID == 0) blockSizeID = LZ4F_BLOCKSIZEID_DEFAULT)
    let id = match block_size_id {
        BlockSizeId::Default => BlockSizeId::Max64Kb,
        other => other,
    };

    let idx = match id {
        BlockSizeId::Max64Kb  => 0,
        BlockSizeId::Max256Kb => 1,
        BlockSizeId::Max1Mb   => 2,
        BlockSizeId::Max4Mb   => 3,
        BlockSizeId::Default  => return None, // unreachable after normalization above
    };

    Some(BLOCK_SIZES[idx])
}

/// Selects the smallest `BlockSizeId` sufficient to hold `src_size` bytes,
/// capped at `requested_bsid`.
///
/// Equivalent to `LZ4F_optimalBSID` (lz4frame.c:359–371).
pub fn lz4f_optimal_bsid(requested_bsid: BlockSizeId, src_size: usize) -> BlockSizeId {
    let mut proposed = BlockSizeId::Max64Kb;
    let mut max_block_size: usize = 64 * 1024;

    // Loop while requestedBSID > proposedBSID (numeric comparison on repr values)
    while (requested_bsid as u32) > (proposed as u32) {
        if src_size <= max_block_size {
            return proposed;
        }
        // Advance proposedBSID by 1 (mirrors: proposedBSID = (LZ4F_blockSizeID_t)((int)proposedBSID + 1))
        proposed = match proposed {
            BlockSizeId::Max64Kb  => BlockSizeId::Max256Kb,
            BlockSizeId::Max256Kb => BlockSizeId::Max1Mb,
            BlockSizeId::Max1Mb   => BlockSizeId::Max4Mb,
            _ => break, // safety guard; not reachable under normal inputs
        };
        max_block_size <<= 2; // mirrors: maxBlockSize <<= 2
    }

    requested_bsid
}

// ─────────────────────────────────────────────────────────────────────────────
// Header checksum (lz4frame.c:349–353)
// ─────────────────────────────────────────────────────────────────────────────

/// Computes the single-byte LZ4 frame header checksum.
///
/// Returns `(XXH32(header, 0) >> 8) & 0xFF`.
/// Equivalent to `LZ4F_headerChecksum` (lz4frame.c:349–353).
#[inline]
pub fn lz4f_header_checksum(header: &[u8]) -> u8 {
    let xxh = xxh32_oneshot(header, 0);
    ((xxh >> 8) & 0xFF) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// Compress-bound functions (lz4frame.c:379–416)
// ─────────────────────────────────────────────────────────────────────────────

/// Worst-case output byte count for compressing `src_size` bytes with `prefs`,
/// accounting for `already_buffered` bytes already held in the internal buffer.
///
/// Pass `already_buffered = 0` for one-shot (frame) calls.
///
/// Equivalent to `LZ4F_compressBound_internal` (lz4frame.c:379–404).
pub fn lz4f_compress_bound_internal(
    src_size: usize,
    prefs: &Preferences,
    already_buffered: usize,
) -> usize {
    // flush = prefsPtr->autoFlush | (srcSize==0)
    let flush = prefs.auto_flush || src_size == 0;

    // Resolve Default block-size ID → Max64Kb
    let block_id = match prefs.frame_info.block_size_id {
        BlockSizeId::Default => BlockSizeId::Max64Kb,
        id => id,
    };
    let block_size = lz4f_get_block_size(block_id).unwrap_or(64 * 1024);

    let max_buffered = block_size - 1;
    let buffered_size = already_buffered.min(max_buffered); // MIN(alreadyBuffered, maxBuffered)
    let max_src_size = src_size + buffered_size;

    let nb_full_blocks = max_src_size / block_size;
    let partial_block_size = max_src_size & (block_size - 1);
    let last_block_size = if flush { partial_block_size } else { 0 };
    let nb_blocks = nb_full_blocks + usize::from(last_block_size > 0);

    // Per-block checksum size (BFSize if block checksums enabled, else 0)
    let block_crc_size = if prefs.frame_info.block_checksum_flag == BlockChecksum::Enabled {
        BF_SIZE
    } else {
        0
    };

    // End-of-frame overhead: block terminator header + optional content checksum
    let frame_end = BH_SIZE
        + if prefs.frame_info.content_checksum_flag == ContentChecksum::Enabled {
            BF_SIZE
        } else {
            0
        };

    // Mirrors: ((BHSize + blockCRCSize) * nbBlocks) + (blockSize * nbFullBlocks) + lastBlockSize + frameEnd
    ((BH_SIZE + block_crc_size) * nb_blocks)
        + (block_size * nb_full_blocks)
        + last_block_size
        + frame_end
}

/// Returns the maximum compressed LZ4 frame size for a `src_size`-byte input.
///
/// `prefs` is optional; `None` produces zeroed preferences (no checksums,
/// default block size), then `auto_flush` is forced to `true`.
/// Adds `MAX_FH_SIZE` (max frame header = 19 bytes) on top of the internal bound.
///
/// Equivalent to `LZ4F_compressFrameBound` (lz4frame.c:406–416).
pub fn lz4f_compress_frame_bound(src_size: usize, prefs: Option<&Preferences>) -> usize {
    // Mirrors: if (preferencesPtr!=NULL) prefs = *preferencesPtr; else MEM_INIT(&prefs, 0, sizeof(prefs));
    let mut local_prefs = prefs.copied().unwrap_or_default();
    local_prefs.auto_flush = true; // mirrors: prefs.autoFlush = 1;

    // headerSize = maxFHSize (19); then add internal bound
    MAX_FH_SIZE + lz4f_compress_bound_internal(src_size, &local_prefs, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::types::{BlockMode, FrameInfo};

    // ── LE I/O helpers ───────────────────────────────────────────────────────

    #[test]
    fn le32_roundtrip() {
        let mut buf = [0u8; 4];
        write_le32(&mut buf, 0, 0xDEAD_BEEF);
        assert_eq!(read_le32(&buf, 0), 0xDEAD_BEEF);
        // Verify byte layout (little-endian)
        assert_eq!(buf, [0xEF, 0xBE, 0xAD, 0xDE]);
    }

    #[test]
    fn le32_offset() {
        let mut buf = [0u8; 8];
        write_le32(&mut buf, 4, 0x0102_0304);
        assert_eq!(read_le32(&buf, 4), 0x0102_0304);
        // First 4 bytes untouched
        assert_eq!(&buf[..4], &[0u8; 4]);
    }

    #[test]
    fn le64_roundtrip() {
        let mut buf = [0u8; 8];
        write_le64(&mut buf, 0, 0x0102_0304_0506_0708u64);
        assert_eq!(read_le64(&buf, 0), 0x0102_0304_0506_0708u64);
        // Verify little-endian byte layout
        assert_eq!(buf, [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn le64_max_value() {
        let mut buf = [0u8; 8];
        write_le64(&mut buf, 0, u64::MAX);
        assert_eq!(read_le64(&buf, 0), u64::MAX);
    }

    // ── lz4f_get_block_size ──────────────────────────────────────────────────

    /// Parity check: lz4f_get_block_size(BlockSizeId::Max64Kb) == 65536
    #[test]
    fn get_block_size_max64kb() {
        assert_eq!(lz4f_get_block_size(BlockSizeId::Max64Kb), Some(65536));
    }

    #[test]
    fn get_block_size_all_ids() {
        assert_eq!(lz4f_get_block_size(BlockSizeId::Default),  Some(65_536));
        assert_eq!(lz4f_get_block_size(BlockSizeId::Max64Kb),  Some(65_536));
        assert_eq!(lz4f_get_block_size(BlockSizeId::Max256Kb), Some(262_144));
        assert_eq!(lz4f_get_block_size(BlockSizeId::Max1Mb),   Some(1_048_576));
        assert_eq!(lz4f_get_block_size(BlockSizeId::Max4Mb),   Some(4_194_304));
    }

    // ── lz4f_optimal_bsid ───────────────────────────────────────────────────

    #[test]
    fn optimal_bsid_src_fits_64kb() {
        // 1 KB fits in 64 KB → always returns Max64Kb regardless of requested
        assert_eq!(
            lz4f_optimal_bsid(BlockSizeId::Max4Mb, 1024),
            BlockSizeId::Max64Kb,
        );
    }

    #[test]
    fn optimal_bsid_src_needs_256kb() {
        // 100 KB > 64 KB but ≤ 256 KB
        assert_eq!(
            lz4f_optimal_bsid(BlockSizeId::Max4Mb, 100_000),
            BlockSizeId::Max256Kb,
        );
    }

    #[test]
    fn optimal_bsid_requested_limits_result() {
        // Even though src needs 256 KB, requested cap is Max64Kb → return Max64Kb
        assert_eq!(
            lz4f_optimal_bsid(BlockSizeId::Max64Kb, 100_000),
            BlockSizeId::Max64Kb,
        );
    }

    #[test]
    fn optimal_bsid_exact_boundary() {
        // src == 64 KB exactly → fits in Max64Kb
        assert_eq!(
            lz4f_optimal_bsid(BlockSizeId::Max4Mb, 64 * 1024),
            BlockSizeId::Max64Kb,
        );
        // src == 64 KB + 1 → needs Max256Kb
        assert_eq!(
            lz4f_optimal_bsid(BlockSizeId::Max4Mb, 64 * 1024 + 1),
            BlockSizeId::Max256Kb,
        );
    }

    // ── lz4f_header_checksum ─────────────────────────────────────────────────

    #[test]
    fn header_checksum_is_deterministic() {
        let header = [0x60u8]; // typical FLG byte
        assert_eq!(lz4f_header_checksum(&header), lz4f_header_checksum(&header));
    }

    #[test]
    fn header_checksum_formula() {
        // Verify: result == (XXH32(data, 0) >> 8) & 0xFF
        let header = [0x60u8, 0x70u8];
        let xxh = xxh32_oneshot(&header, 0);
        assert_eq!(lz4f_header_checksum(&header), ((xxh >> 8) & 0xFF) as u8);
    }

    #[test]
    fn header_checksum_empty() {
        // Empty slice: formula still applies
        let xxh = xxh32_oneshot(&[], 0);
        assert_eq!(lz4f_header_checksum(&[]), ((xxh >> 8) & 0xFF) as u8);
    }

    // ── lz4f_compress_bound_internal ─────────────────────────────────────────

    #[test]
    fn compress_bound_internal_zero_src_no_buffered() {
        // src=0, no buffering, flush forced by src==0:
        //   nb_full_blocks=0, partial=0, last=0, nb_blocks=0
        //   frame_end = BH_SIZE(4) + 0 = 4
        //   result = 0 + 0 + 0 + 4 = 4
        let prefs = Preferences::default();
        assert_eq!(lz4f_compress_bound_internal(0, &prefs, 0), 4);
    }

    #[test]
    fn compress_bound_internal_one_full_block() {
        // src == blockSize (64KB), no checksums, auto_flush=false:
        //   flush = false | (65536==0) = false
        //   nb_full = 1, partial = 0, last = 0, nb_blocks = 1
        //   block_crc=0, frame_end=4
        //   result = (4+0)*1 + 65536*1 + 0 + 4 = 65544
        let prefs = Preferences::default();
        assert_eq!(lz4f_compress_bound_internal(65_536, &prefs, 0), 65_544);
    }

    #[test]
    fn compress_bound_internal_with_checksums() {
        // src=0, block+content checksums enabled, flush forced:
        //   nb_blocks=0, frame_end = 4 + 4 = 8, result = 8
        let prefs = Preferences {
            frame_info: FrameInfo {
                block_size_id: BlockSizeId::Max64Kb,
                block_mode: BlockMode::Linked,
                content_checksum_flag: ContentChecksum::Enabled,
                block_checksum_flag: BlockChecksum::Enabled,
                ..FrameInfo::default()
            },
            ..Preferences::default()
        };
        assert_eq!(lz4f_compress_bound_internal(0, &prefs, 0), 8);
    }

    // ── lz4f_compress_frame_bound ────────────────────────────────────────────

    /// Parity: lz4f_compress_frame_bound(0, None) matches C LZ4F_compressFrameBound(0, NULL).
    ///
    /// C trace (prefs zeroed, auto_flush=1, src=0):
    ///   block_size = 65536, flush = true, nb_blocks = 0
    ///   frame_end  = 4 (BH_SIZE + no content checksum)
    ///   internal   = 4
    ///   total      = 19 (MAX_FH_SIZE) + 4 = 23
    #[test]
    fn compress_frame_bound_zero_null_prefs() {
        assert_eq!(lz4f_compress_frame_bound(0, None), 23);
    }

    #[test]
    fn compress_frame_bound_includes_header_size() {
        // Any call adds exactly MAX_FH_SIZE (19) on top of the internal bound
        let prefs = Preferences::default();
        let internal = lz4f_compress_bound_internal(1024, &prefs, 0);
        // frame_bound forces auto_flush=true so internal is recomputed with that
        let prefs_flushed = Preferences { auto_flush: true, ..prefs };
        let expected = MAX_FH_SIZE + lz4f_compress_bound_internal(1024, &prefs_flushed, 0);
        assert_eq!(lz4f_compress_frame_bound(1024, Some(&prefs)), expected);
        // Internal bound without flush differs
        let _ = internal; // suppress unused warning
    }

    // ── lz4f_compression_level_max ───────────────────────────────────────────

    #[test]
    fn compression_level_max_is_12() {
        assert_eq!(lz4f_compression_level_max(), 12);
    }
}
