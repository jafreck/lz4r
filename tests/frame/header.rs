// Unit tests for task-016: src/frame/header.rs — Frame header utilities
//
// Verifies parity with lz4frame.c v1.10.0, lines 167–420:
//   - LE byte-order helpers (`read_le32`, `write_le32`, `read_le64`, `write_le64`)
//   - `lz4f_compression_level_max` → LZ4F_compressionLevel_max
//   - `lz4f_get_block_size`        → LZ4F_getBlockSize
//   - `lz4f_optimal_bsid`          → LZ4F_optimalBSID
//   - `lz4f_header_checksum`        → LZ4F_headerChecksum
//   - `lz4f_compress_bound_internal` → LZ4F_compressBound_internal
//   - `lz4f_compress_frame_bound`  → LZ4F_compressFrameBound

use lz4::frame::header::{
    lz4f_compress_bound_internal, lz4f_compress_frame_bound, lz4f_compression_level_max,
    lz4f_get_block_size, lz4f_header_checksum, lz4f_optimal_bsid, read_le32, read_le64,
    write_le32, write_le64, LZ4HC_CLEVEL_MAX,
};
use lz4::frame::types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, FrameInfo, Preferences, BF_SIZE,
    BH_SIZE, MAX_FH_SIZE,
};
use lz4::xxhash::xxh32_oneshot;

// ---------------------------------------------------------------------------
// LE I/O helpers — read_le32 / write_le32
// ---------------------------------------------------------------------------

/// write_le32 then read_le32 must round-trip any u32 value.
#[test]
fn le32_roundtrip_zero() {
    let mut buf = [0u8; 4];
    write_le32(&mut buf, 0, 0);
    assert_eq!(read_le32(&buf, 0), 0);
}

#[test]
fn le32_roundtrip_max() {
    let mut buf = [0u8; 4];
    write_le32(&mut buf, 0, u32::MAX);
    assert_eq!(read_le32(&buf, 0), u32::MAX);
}

/// Parity: LZ4F_writeLE32 produces little-endian layout (LSB first).
#[test]
fn le32_byte_layout() {
    let mut buf = [0u8; 4];
    write_le32(&mut buf, 0, 0xDEAD_BEEF);
    // LSB first
    assert_eq!(buf, [0xEF, 0xBE, 0xAD, 0xDE]);
}

/// Value 0x01020304 maps to bytes [0x04, 0x03, 0x02, 0x01].
#[test]
fn le32_byte_layout_ascending() {
    let mut buf = [0u8; 4];
    write_le32(&mut buf, 0, 0x0102_0304);
    assert_eq!(buf, [0x04, 0x03, 0x02, 0x01]);
}

/// Offset parameter must shift write and read independently within a larger buffer.
#[test]
fn le32_at_nonzero_offset() {
    let mut buf = [0xFFu8; 8];
    write_le32(&mut buf, 4, 0x1234_5678);
    assert_eq!(read_le32(&buf, 4), 0x1234_5678);
    // First 4 bytes must be untouched
    assert_eq!(&buf[..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
}

/// read_le32 at offset 0 on a manually constructed buffer returns the correct value.
#[test]
fn le32_read_from_known_bytes() {
    let buf = [0x78u8, 0x56, 0x34, 0x12];
    assert_eq!(read_le32(&buf, 0), 0x1234_5678);
}

// ---------------------------------------------------------------------------
// LE I/O helpers — read_le64 / write_le64
// ---------------------------------------------------------------------------

#[test]
fn le64_roundtrip_zero() {
    let mut buf = [0u8; 8];
    write_le64(&mut buf, 0, 0);
    assert_eq!(read_le64(&buf, 0), 0);
}

#[test]
fn le64_roundtrip_max() {
    let mut buf = [0u8; 8];
    write_le64(&mut buf, 0, u64::MAX);
    assert_eq!(read_le64(&buf, 0), u64::MAX);
}

/// Parity: LZ4F_writeLE64 produces little-endian layout.
#[test]
fn le64_byte_layout() {
    let mut buf = [0u8; 8];
    write_le64(&mut buf, 0, 0x0102_0304_0506_0708u64);
    assert_eq!(buf, [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
}

#[test]
fn le64_at_nonzero_offset() {
    let mut buf = [0xAAu8; 16];
    write_le64(&mut buf, 8, 0xDEAD_BEEF_CAFE_BABEu64);
    assert_eq!(read_le64(&buf, 8), 0xDEAD_BEEF_CAFE_BABEu64);
    // First 8 bytes untouched
    assert_eq!(&buf[..8], &[0xAA; 8]);
}

#[test]
fn le64_read_from_known_bytes() {
    let buf = [0x01u8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(read_le64(&buf, 0), 1u64);
}

// ---------------------------------------------------------------------------
// lz4f_compression_level_max (LZ4F_compressionLevel_max)
// ---------------------------------------------------------------------------

/// Parity: must return exactly 12 (LZ4HC_CLEVEL_MAX).
#[test]
fn compression_level_max_equals_12() {
    assert_eq!(lz4f_compression_level_max(), 12);
    assert_eq!(lz4f_compression_level_max(), LZ4HC_CLEVEL_MAX);
}

#[test]
fn compression_level_max_is_deterministic() {
    assert_eq!(lz4f_compression_level_max(), lz4f_compression_level_max());
}

// ---------------------------------------------------------------------------
// lz4f_get_block_size (LZ4F_getBlockSize)
// ---------------------------------------------------------------------------

/// Parity: Default block size ID must resolve to 64 KB (same as Max64Kb).
/// C: `if (blockSizeID == 0) blockSizeID = LZ4F_BLOCKSIZEID_DEFAULT`
#[test]
fn get_block_size_default_resolves_to_64kb() {
    assert_eq!(lz4f_get_block_size(BlockSizeId::Default), Some(64 * 1024));
}

/// Parity: Max64Kb → 65536 bytes.
#[test]
fn get_block_size_max64kb() {
    assert_eq!(lz4f_get_block_size(BlockSizeId::Max64Kb), Some(65_536));
}

/// Parity: Max256Kb → 262144 bytes.
#[test]
fn get_block_size_max256kb() {
    assert_eq!(lz4f_get_block_size(BlockSizeId::Max256Kb), Some(262_144));
}

/// Parity: Max1Mb → 1048576 bytes.
#[test]
fn get_block_size_max1mb() {
    assert_eq!(lz4f_get_block_size(BlockSizeId::Max1Mb), Some(1_048_576));
}

/// Parity: Max4Mb → 4194304 bytes.
#[test]
fn get_block_size_max4mb() {
    assert_eq!(lz4f_get_block_size(BlockSizeId::Max4Mb), Some(4_194_304));
}

/// Block sizes must form the geometric sequence: 64KB × 4^n.
#[test]
fn get_block_size_geometric_sequence() {
    let kb64 = lz4f_get_block_size(BlockSizeId::Max64Kb).unwrap();
    let kb256 = lz4f_get_block_size(BlockSizeId::Max256Kb).unwrap();
    let mb1 = lz4f_get_block_size(BlockSizeId::Max1Mb).unwrap();
    let mb4 = lz4f_get_block_size(BlockSizeId::Max4Mb).unwrap();
    assert_eq!(kb256, kb64 * 4);
    assert_eq!(mb1, kb256 * 4);
    assert_eq!(mb4, mb1 * 4);
}

// ---------------------------------------------------------------------------
// lz4f_optimal_bsid (LZ4F_optimalBSID)
// ---------------------------------------------------------------------------

/// Parity: 0 bytes fits in Max64Kb regardless of requested ID.
#[test]
fn optimal_bsid_zero_src() {
    assert_eq!(lz4f_optimal_bsid(BlockSizeId::Max4Mb, 0), BlockSizeId::Max64Kb);
}

/// 1 byte fits in Max64Kb.
#[test]
fn optimal_bsid_one_byte() {
    assert_eq!(lz4f_optimal_bsid(BlockSizeId::Max4Mb, 1), BlockSizeId::Max64Kb);
}

/// Exactly 64 KB fits in Max64Kb (boundary: src <= maxBlockSize returns proposed).
#[test]
fn optimal_bsid_exactly_64kb() {
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max4Mb, 64 * 1024),
        BlockSizeId::Max64Kb
    );
}

/// 64 KB + 1 byte does NOT fit in Max64Kb → returns Max256Kb.
#[test]
fn optimal_bsid_just_over_64kb() {
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max4Mb, 64 * 1024 + 1),
        BlockSizeId::Max256Kb
    );
}

/// 256 KB fits in Max256Kb.
#[test]
fn optimal_bsid_exactly_256kb() {
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max4Mb, 256 * 1024),
        BlockSizeId::Max256Kb
    );
}

/// 256 KB + 1 → needs Max1Mb.
#[test]
fn optimal_bsid_just_over_256kb() {
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max4Mb, 256 * 1024 + 1),
        BlockSizeId::Max1Mb
    );
}

/// 1 MB + 1 → needs Max4Mb.
#[test]
fn optimal_bsid_just_over_1mb() {
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max4Mb, 1024 * 1024 + 1),
        BlockSizeId::Max4Mb
    );
}

/// Parity: requested cap limits the result even when src would require a larger block.
/// C: while (requestedBSID > proposedBSID) — once equal, loop terminates.
#[test]
fn optimal_bsid_requested_caps_result() {
    // src needs Max256Kb but cap is Max64Kb → return Max64Kb
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max64Kb, 100_000),
        BlockSizeId::Max64Kb
    );
    // src needs Max1Mb but cap is Max256Kb
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max256Kb, 300_000),
        BlockSizeId::Max256Kb
    );
}

/// Very large src with Max4Mb requested → returns Max4Mb.
#[test]
fn optimal_bsid_large_src_max4mb() {
    assert_eq!(
        lz4f_optimal_bsid(BlockSizeId::Max4Mb, 100 * 1024 * 1024),
        BlockSizeId::Max4Mb
    );
}

// ---------------------------------------------------------------------------
// lz4f_header_checksum (LZ4F_headerChecksum)
// ---------------------------------------------------------------------------

/// Parity: result == (XXH32(header, 0) >> 8) & 0xFF — formula must hold for any input.
#[test]
fn header_checksum_formula_parity() {
    let header = [0x60u8, 0x70u8];
    let xxh = xxh32_oneshot(&header, 0);
    assert_eq!(lz4f_header_checksum(&header), ((xxh >> 8) & 0xFF) as u8);
}

/// Empty slice must still apply the formula without panic.
#[test]
fn header_checksum_empty_input() {
    let xxh = xxh32_oneshot(&[], 0);
    assert_eq!(lz4f_header_checksum(&[]), ((xxh >> 8) & 0xFF) as u8);
}

/// Single byte input.
#[test]
fn header_checksum_single_byte() {
    let header = [0x60u8]; // typical FLG byte
    let xxh = xxh32_oneshot(&header, 0);
    assert_eq!(lz4f_header_checksum(&header), ((xxh >> 8) & 0xFF) as u8);
}

/// Must be deterministic.
#[test]
fn header_checksum_is_deterministic() {
    let header = [0x68u8, 0x70u8, 0x00u8];
    assert_eq!(lz4f_header_checksum(&header), lz4f_header_checksum(&header));
}

/// Different headers must (generally) produce different checksums.
#[test]
fn header_checksum_different_inputs_differ() {
    let h1 = lz4f_header_checksum(&[0x60u8]);
    let h2 = lz4f_header_checksum(&[0x61u8]);
    // Not guaranteed by the formula, but empirically true for these bytes
    // (this validates that distinct FLG bytes produce distinct HC bytes)
    assert_ne!(h1, h2, "distinct FLG bytes should produce distinct header checksums");
}

/// Larger header (FLG + BD + optional content size bytes).
#[test]
fn header_checksum_larger_header() {
    // FLG=0x68 (version=01, B.Indep=1, C.Size=1), BD=0x70, content_size=8 bytes
    let header = [0x68u8, 0x70, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let xxh = xxh32_oneshot(&header, 0);
    let expected = ((xxh >> 8) & 0xFF) as u8;
    assert_eq!(lz4f_header_checksum(&header), expected);
}

// ---------------------------------------------------------------------------
// lz4f_compress_bound_internal (LZ4F_compressBound_internal)
// ---------------------------------------------------------------------------

/// Parity: src=0, default prefs → result = BH_SIZE (4) — zero-src forces flush,
/// no blocks, only end-of-frame overhead.
#[test]
fn compress_bound_internal_zero_src() {
    let prefs = Preferences::default();
    // flush = auto_flush | (src==0) = true; nb_blocks=0; frame_end=BH_SIZE=4
    assert_eq!(lz4f_compress_bound_internal(0, &prefs, 0), BH_SIZE);
}

/// Parity: one full block (64 KB), no checksums, no flush →
///   nb_full=1, partial=0, last=0, nb_blocks=1
///   result = (BH_SIZE + 0) * 1 + block_size * 1 + 0 + BH_SIZE
///          = 4 + 65536 + 4 = 65544
#[test]
fn compress_bound_internal_one_full_block_no_checksum() {
    let prefs = Preferences::default(); // auto_flush=false, no checksums
    assert_eq!(lz4f_compress_bound_internal(65_536, &prefs, 0), 65_544);
}

/// With content checksum enabled, frame_end = BH_SIZE + BF_SIZE = 8.
/// src=0 → result = 8.
#[test]
fn compress_bound_internal_zero_src_content_checksum() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    assert_eq!(
        lz4f_compress_bound_internal(0, &prefs, 0),
        BH_SIZE + BF_SIZE // 4 + 4 = 8
    );
}

/// With block checksum enabled, each block adds BF_SIZE overhead.
/// src=65536, no auto_flush → nb_blocks=1, block_crc=BF_SIZE=4
///   result = (BH_SIZE + BF_SIZE) + block_size + BH_SIZE
///          = 8 + 65536 + 4 = 65548
#[test]
fn compress_bound_internal_one_full_block_with_block_checksum() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_checksum_flag: BlockChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    assert_eq!(
        lz4f_compress_bound_internal(65_536, &prefs, 0),
        (BH_SIZE + BF_SIZE) + 65_536 + BH_SIZE // 8 + 65536 + 4 = 65548
    );
}

/// Both checksums enabled, src=0 → result = BH_SIZE + BF_SIZE = 8.
#[test]
fn compress_bound_internal_both_checksums_zero_src() {
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

/// auto_flush=true treats partial blocks as emitted.
/// src=1 byte, Max64Kb, auto_flush=true →
///   nb_full=0, partial=1, last=1, nb_blocks=1
///   result = (BH_SIZE)*1 + 0 + 1 + BH_SIZE = 4+1+4 = 9
#[test]
fn compress_bound_internal_auto_flush_partial_block() {
    let prefs = Preferences {
        auto_flush: true,
        ..Preferences::default()
    };
    // nb_full=0, partial=1, last=1, nb_blocks=1
    // ((BH+0)*1) + (block_size*0) + 1 + BH_SIZE = 4 + 0 + 1 + 4 = 9
    assert_eq!(lz4f_compress_bound_internal(1, &prefs, 0), 9);
}

/// already_buffered is capped at max_buffered = block_size - 1.
/// already_buffered > block_size-1 is treated as block_size-1.
#[test]
fn compress_bound_internal_already_buffered_capped() {
    let prefs = Preferences::default(); // Max64Kb, no flush, no checksums
    let block_size = 64 * 1024usize;
    let max_buffered = block_size - 1;
    // Capping: already_buffered > max_buffered is treated as max_buffered
    let result_capped = lz4f_compress_bound_internal(0, &prefs, max_buffered + 100);
    let result_at_max = lz4f_compress_bound_internal(0, &prefs, max_buffered);
    assert_eq!(result_capped, result_at_max);
}

/// already_buffered shifts bytes into the block calculation.
/// src=0, already_buffered=1, auto_flush=false → flush forced by src==0
///   buffered=1, max_src=1, nb_full=0, partial=1, last=1, nb_blocks=1
///   result = (BH_SIZE)*1 + 0 + 1 + BH_SIZE = 9
#[test]
fn compress_bound_internal_with_already_buffered() {
    let prefs = Preferences::default();
    // src==0 → flush=true; max_src=0+1=1; partial=1; last=1; nb_blocks=1
    assert_eq!(lz4f_compress_bound_internal(0, &prefs, 1), 9);
}

/// Default block size ID must be treated as Max64Kb (block_size = 65536).
#[test]
fn compress_bound_internal_default_block_id_treated_as_64kb() {
    let prefs_default = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Default,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let prefs_64kb = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    assert_eq!(
        lz4f_compress_bound_internal(1024, &prefs_default, 0),
        lz4f_compress_bound_internal(1024, &prefs_64kb, 0)
    );
}

// ---------------------------------------------------------------------------
// lz4f_compress_frame_bound (LZ4F_compressFrameBound)
// ---------------------------------------------------------------------------

/// Parity: LZ4F_compressFrameBound(0, NULL) must return 23.
/// C trace: auto_flush=1, prefs zeroed, src=0 → internal=4, total=19+4=23.
#[test]
fn compress_frame_bound_zero_null_prefs_is_23() {
    assert_eq!(lz4f_compress_frame_bound(0, None), 23);
}

/// Result must always be MAX_FH_SIZE (19) bytes more than the internal bound
/// computed with auto_flush=true.
#[test]
fn compress_frame_bound_adds_max_fh_size() {
    let prefs = Preferences::default();
    let prefs_flushed = Preferences { auto_flush: true, ..prefs };
    let internal = lz4f_compress_bound_internal(1024, &prefs_flushed, 0);
    assert_eq!(lz4f_compress_frame_bound(1024, Some(&prefs)), MAX_FH_SIZE + internal);
}

/// Frame bound with None prefs equals frame bound with zeroed prefs (after auto_flush forced).
#[test]
fn compress_frame_bound_none_equals_default_prefs() {
    let default_prefs = Preferences::default();
    assert_eq!(
        lz4f_compress_frame_bound(4096, None),
        lz4f_compress_frame_bound(4096, Some(&default_prefs))
    );
}

/// Content checksum prefs must increase the bound by BF_SIZE = 4.
#[test]
fn compress_frame_bound_content_checksum_increases_bound() {
    let prefs_no_cksum = Preferences::default();
    let prefs_cksum = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let without = lz4f_compress_frame_bound(1024, Some(&prefs_no_cksum));
    let with_cksum = lz4f_compress_frame_bound(1024, Some(&prefs_cksum));
    assert_eq!(with_cksum, without + BF_SIZE);
}

/// auto_flush in caller prefs doesn't matter — lz4f_compress_frame_bound forces it.
#[test]
fn compress_frame_bound_ignores_caller_auto_flush() {
    let prefs_no_flush = Preferences { auto_flush: false, ..Preferences::default() };
    let prefs_flush = Preferences { auto_flush: true, ..Preferences::default() };
    // Both should produce the same result since frame_bound forces auto_flush=true
    assert_eq!(
        lz4f_compress_frame_bound(1024, Some(&prefs_no_flush)),
        lz4f_compress_frame_bound(1024, Some(&prefs_flush))
    );
}

/// Larger src produces larger frame bound (monotonicity).
#[test]
fn compress_frame_bound_monotone_with_src_size() {
    let prefs = Preferences::default();
    let small = lz4f_compress_frame_bound(0, Some(&prefs));
    let medium = lz4f_compress_frame_bound(65_536, Some(&prefs));
    let large = lz4f_compress_frame_bound(4 * 1024 * 1024, Some(&prefs));
    assert!(small <= medium);
    assert!(medium <= large);
}
