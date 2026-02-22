// Unit tests for task-019: src/frame/decompress.rs — LZ4 Frame decompression
//
// Verifies behavioural parity with lz4frame.c v1.10.0, lines 1244–2136:
//   - Context lifecycle: `lz4f_create_decompression_context`, `lz4f_free_decompression_context`,
//     `lz4f_reset_decompression_context`
//   - Header introspection: `lz4f_header_size`, `lz4f_get_frame_info`
//   - Streaming decompressor: `lz4f_decompress`
//   - Dictionary decompressor: `lz4f_decompress_using_dict`
//   - Internal dict rolling window: `Lz4FDCtx::update_dict` (exposed via public field)
//   - `DecompressOptions` struct

use lz4::frame::compress::{
    lz4f_compress_frame, lz4f_compress_frame_using_cdict,
};
use lz4::frame::cdict::Lz4FCDict;
use lz4::frame::types::Lz4FCCtx;
use lz4::frame::decompress::{
    lz4f_create_decompression_context, lz4f_decompress, lz4f_decompress_using_dict,
    lz4f_free_decompression_context, lz4f_get_frame_info, lz4f_header_size,
    lz4f_reset_decompression_context, DecompressOptions, Lz4FDCtx,
};
use lz4::frame::header::lz4f_compress_frame_bound;
use lz4::frame::types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, DecompressStage, FrameInfo,
    Lz4FError, Preferences, LZ4F_VERSION, BH_SIZE, MAX_FH_SIZE, MIN_FH_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn compress_frame_simple(src: &[u8]) -> Vec<u8> {
    let bound = lz4f_compress_frame_bound(src.len(), None);
    let mut dst = vec![0u8; bound];
    let written = lz4f_compress_frame(&mut dst, src, None).expect("compress_frame");
    dst.truncate(written);
    dst
}

fn compress_frame_with_prefs(src: &[u8], prefs: &Preferences) -> Vec<u8> {
    let bound = lz4f_compress_frame_bound(src.len(), Some(prefs));
    let mut dst = vec![0u8; bound];
    let written = lz4f_compress_frame(&mut dst, src, Some(prefs)).expect("compress_frame");
    dst.truncate(written);
    dst
}

fn repetitive_bytes(len: usize) -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog "
        .iter()
        .cycle()
        .take(len)
        .copied()
        .collect()
}

fn cycling_bytes(len: usize) -> Vec<u8> {
    (0u8..=255).cycle().take(len).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_create_decompression_context / lz4f_free_decompression_context
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: LZ4F_createDecompressionContext with LZ4F_VERSION succeeds.
#[test]
fn create_dctx_correct_version_succeeds() {
    let dctx = lz4f_create_decompression_context(LZ4F_VERSION);
    assert!(dctx.is_ok());
}

/// Parity: created context starts in GetFrameHeader stage.
#[test]
fn create_dctx_initial_stage_is_get_frame_header() {
    let dctx = lz4f_create_decompression_context(LZ4F_VERSION).unwrap();
    assert_eq!(dctx.stage, DecompressStage::GetFrameHeader);
}

/// Parity: LZ4F_createDecompressionContext rejects version != LZ4F_VERSION.
#[test]
fn create_dctx_wrong_version_returns_err() {
    assert!(lz4f_create_decompression_context(0).is_err());
    assert!(lz4f_create_decompression_context(99).is_err());
    assert!(lz4f_create_decompression_context(101).is_err());
    assert!(lz4f_create_decompression_context(u32::MAX).is_err());
}

/// Parity: LZ4F_freeDecompressionContext drops without panic.
#[test]
fn free_dctx_no_panic() {
    let dctx = lz4f_create_decompression_context(LZ4F_VERSION).unwrap();
    lz4f_free_decompression_context(dctx); // must not panic
}

/// Context can be created and freed multiple times.
#[test]
fn create_and_free_dctx_multiple_times() {
    for _ in 0..8 {
        let dctx = lz4f_create_decompression_context(LZ4F_VERSION).unwrap();
        lz4f_free_decompression_context(dctx);
    }
}

/// Created context has expected initial version stored.
#[test]
fn create_dctx_stores_version() {
    let dctx = lz4f_create_decompression_context(LZ4F_VERSION).unwrap();
    assert_eq!(dctx.version, LZ4F_VERSION);
}

/// Created context starts with empty dict, skip_checksum=false.
#[test]
fn create_dctx_initial_fields_zeroed() {
    let dctx = lz4f_create_decompression_context(LZ4F_VERSION).unwrap();
    assert!(dctx.dict_bytes.is_empty());
    assert!(!dctx.skip_checksum);
    assert_eq!(dctx.frame_remaining_size, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_reset_decompression_context
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: reset clears skip_checksum, dict, remaining_size and resets stage.
#[test]
fn reset_dctx_clears_state() {
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    dctx.skip_checksum = true;
    dctx.frame_remaining_size = 42;
    dctx.dict_bytes.extend_from_slice(b"hello world");
    lz4f_reset_decompression_context(&mut dctx);
    assert_eq!(dctx.stage, DecompressStage::GetFrameHeader);
    assert!(!dctx.skip_checksum);
    assert_eq!(dctx.frame_remaining_size, 0);
    assert!(dctx.dict_bytes.is_empty());
}

/// After reset, stage is always GetFrameHeader regardless of prior stage.
#[test]
fn reset_dctx_stage_always_get_frame_header() {
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    dctx.stage = DecompressStage::GetSuffix;
    lz4f_reset_decompression_context(&mut dctx);
    assert_eq!(dctx.stage, DecompressStage::GetFrameHeader);
}

// ─────────────────────────────────────────────────────────────────────────────
// dict_bytes rolling window (observable through public field)
// ─────────────────────────────────────────────────────────────────────────────

const MAX_DICT_SIZE: usize = 64 * 1024;

/// Parity: dict_bytes field starts empty in a new context.
#[test]
fn dctx_dict_bytes_initially_empty() {
    let dctx = Lz4FDCtx::new(LZ4F_VERSION);
    assert!(dctx.dict_bytes.is_empty());
}

/// Parity: decompress_using_dict with a large dict truncates to 64 KiB max.
/// The context dict_bytes must never exceed MAX_DICT_SIZE even when the user
/// provides a larger external dict (same as C's memcpy + pointer arithmetic).
#[test]
fn decompress_using_dict_large_dict_truncated_in_ctx() {
    let large_dict = vec![0xAAu8; 128 * 1024];
    let original = b"hello dict";
    let frame = compress_frame_simple(original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 64];
    let _ = lz4f_decompress_using_dict(&mut dctx, Some(&mut dst), &frame, &large_dict, None);
    // dict_bytes must be capped at MAX_DICT_SIZE
    assert!(
        dctx.dict_bytes.len() <= MAX_DICT_SIZE,
        "dict_bytes exceeded 64 KiB max: {}",
        dctx.dict_bytes.len()
    );
}

/// Parity: linked-block decompression does not panic regardless of dict state.
#[test]
fn decompress_linked_blocks_no_panic() {
    let original = repetitive_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Linked,
            block_size_id: BlockSizeId::Max64Kb,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    // Must not panic and must produce correct output
    let (_, dw, _) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(&dst[..dw], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_header_size
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: skippable frame header is always 8 bytes.
#[test]
fn header_size_skippable_frame_is_8() {
    // Any magic in range 0x184D2A50..0x184D2A5F is skippable
    let mut buf = [0u8; 8];
    buf[..4].copy_from_slice(&0x184D_2A50u32.to_le_bytes());
    assert_eq!(lz4f_header_size(&buf), Ok(8));
    buf[..4].copy_from_slice(&0x184D_2A5Fu32.to_le_bytes());
    assert_eq!(lz4f_header_size(&buf), Ok(8));
}

/// Parity: minimum LZ4 frame header (no content size, no dict id) = MIN_FH_SIZE = 7.
#[test]
fn header_size_minimal_frame_is_7() {
    let mut buf = [0u8; MIN_FH_SIZE];
    buf[..4].copy_from_slice(&0x184D_2204u32.to_le_bytes());
    buf[4] = 0x60; // FLG: version=01, B.Indep=1, no C.Size, no DictID
    buf[5] = 0x70; // BD: blockSizeID=7
    buf[6] = 0;    // HC — not checked here
    assert_eq!(lz4f_header_size(&buf), Ok(7));
}

/// Parity: frame with content-size flag adds 8 bytes → header = 15.
#[test]
fn header_size_with_content_size_flag() {
    let mut buf = [0u8; 20];
    buf[..4].copy_from_slice(&0x184D_2204u32.to_le_bytes());
    buf[4] = 0x68; // FLG: version=01, B.Indep=1, C.Size=1, no DictID
    buf[5] = 0x70;
    assert_eq!(lz4f_header_size(&buf), Ok(15));
}

/// Parity: frame with dict-id flag adds 4 bytes → header = 11.
#[test]
fn header_size_with_dict_id_flag() {
    let mut buf = [0u8; 20];
    buf[..4].copy_from_slice(&0x184D_2204u32.to_le_bytes());
    buf[4] = 0x61; // FLG: version=01, B.Indep=1, DictID=1
    buf[5] = 0x70;
    assert_eq!(lz4f_header_size(&buf), Ok(11));
}

/// Parity: frame with both content-size + dict-id flags = 7 + 8 + 4 = 19 (MAX_FH_SIZE).
#[test]
fn header_size_both_flags_is_max_fh_size() {
    let mut buf = [0u8; 20];
    buf[..4].copy_from_slice(&0x184D_2204u32.to_le_bytes());
    buf[4] = 0x69; // FLG: version=01, B.Indep=1, C.Size=1, DictID=1
    buf[5] = 0x70;
    assert_eq!(lz4f_header_size(&buf), Ok(MAX_FH_SIZE));
}

/// Parity: fewer than 5 bytes returns FrameHeaderIncomplete error.
#[test]
fn header_size_too_short_returns_err() {
    assert!(lz4f_header_size(&[]).is_err());
    assert!(lz4f_header_size(&[0u8; 3]).is_err());
    assert!(lz4f_header_size(&[0u8; 4]).is_err());
}

/// Parity: wrong magic number returns FrameTypeUnknown error.
#[test]
fn header_size_wrong_magic_returns_err() {
    let mut buf = [0u8; 8];
    buf[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    assert!(lz4f_header_size(&buf).is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — empty / short input
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: decompress with zero-length src returns (0, 0, MIN_FH_SIZE).
#[test]
fn decompress_empty_src_hint_is_min_fh_size() {
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let (sc, dw, hint) = lz4f_decompress(&mut dctx, None, &[], None).unwrap();
    assert_eq!(sc, 0);
    assert_eq!(dw, 0);
    assert_eq!(hint, MIN_FH_SIZE);
}

/// Parity: dst=None means no output written, but header is consumed; blocks need dst space.
#[test]
fn decompress_none_dst_header_consumed() {
    let src_data = repetitive_bytes(1024);
    let frame = compress_frame_simple(&src_data);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    // First call: dst=None, consumes at least the header bytes
    let (sc, dw, _hint) = lz4f_decompress(&mut dctx, None, &frame, None).unwrap();
    assert!(sc > 0, "some bytes should be consumed from header");
    assert_eq!(dw, 0, "no bytes written when dst=None");
}

/// Parity: decompress produces the original plaintext.
#[test]
fn decompress_round_trip_small_data() {
    let original = b"hello lz4 world!";
    let frame = compress_frame_simple(original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(hint, 0);
    assert_eq!(&dst[..dw], original.as_ref());
}

/// Parity: round-trip with repetitive (compressible) data.
#[test]
fn decompress_round_trip_repetitive_data() {
    let original = repetitive_bytes(32 * 1024);
    let frame = compress_frame_simple(&original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(hint, 0);
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: round-trip with incompressible (cycling) data.
#[test]
fn decompress_round_trip_incompressible_data() {
    let original = cycling_bytes(8 * 1024);
    let frame = compress_frame_simple(&original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, _hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: round-trip with empty source data.
#[test]
fn decompress_round_trip_empty_data() {
    let original: &[u8] = b"";
    let frame = compress_frame_simple(original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 64];
    let (sc, dw, hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, 0);
    assert_eq!(hint, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — content checksum verification
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: frame with content checksum decompresses correctly.
#[test]
fn decompress_with_content_checksum_succeeds() {
    let original = repetitive_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(hint, 0);
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: corrupted content checksum (last 4 bytes) returns ContentChecksumInvalid.
#[test]
fn decompress_bad_content_checksum_returns_err() {
    let original = repetitive_bytes(1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let mut frame = compress_frame_with_prefs(&original, &prefs);
    // Corrupt the last 4 bytes (content checksum)
    let n = frame.len();
    frame[n - 1] ^= 0xFF;
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let result = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None);
    assert!(result.is_err(), "should fail with bad content checksum");
    assert_eq!(result.unwrap_err(), Lz4FError::ContentChecksumInvalid);
}

/// Parity: skip_checksums=true skips content checksum verification.
#[test]
fn decompress_skip_checksums_ignores_bad_content_checksum() {
    let original = repetitive_bytes(1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let mut frame = compress_frame_with_prefs(&original, &prefs);
    let n = frame.len();
    frame[n - 1] ^= 0xFF; // Corrupt checksum
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let opts = DecompressOptions { skip_checksums: true, ..Default::default() };
    // Should succeed despite bad checksum when skip_checksums=true
    let result = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, Some(&opts));
    assert!(result.is_ok(), "skip_checksums must bypass checksum validation");
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — block checksum
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: frame with block checksums decompresses correctly.
#[test]
fn decompress_with_block_checksum_succeeds() {
    let original = repetitive_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_checksum_flag: BlockChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, _hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — block mode variants
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: round-trip with block-independent mode.
#[test]
fn decompress_block_independent_mode() {
    let original = repetitive_bytes(16 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (_, dw, _) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: round-trip with block-linked mode (default).
#[test]
fn decompress_block_linked_mode() {
    let original = repetitive_bytes(128 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Linked,
            block_size_id: BlockSizeId::Max64Kb,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (_, dw, _) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — various block sizes
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: round-trip with Max64Kb block size.
#[test]
fn decompress_block_size_max64kb() {
    let original = repetitive_bytes(64 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo { block_size_id: BlockSizeId::Max64Kb, ..FrameInfo::default() },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (_, dw, _) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: round-trip with Max256Kb block size.
#[test]
fn decompress_block_size_max256kb() {
    let original = repetitive_bytes(200 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo { block_size_id: BlockSizeId::Max256Kb, ..FrameInfo::default() },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (_, dw, _) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — next_src_hint semantics
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: after full frame decompression hint == 0 (frame fully consumed).
#[test]
fn decompress_hint_zero_after_complete_frame() {
    let original = repetitive_bytes(1024);
    let frame = compress_frame_simple(&original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (_, _, hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(hint, 0, "hint must be 0 when frame is fully decoded");
}

/// Parity: after empty src, hint is MIN_FH_SIZE (how many bytes needed next).
#[test]
fn decompress_hint_nonzero_when_frame_incomplete() {
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let (_, _, hint) = lz4f_decompress(&mut dctx, None, &[], None).unwrap();
    assert!(hint > 0, "hint must be >0 when frame header not yet received");
    assert_eq!(hint, MIN_FH_SIZE);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — streaming (chunked input)
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: streaming decompression in small chunks produces the same result as one-shot.
#[test]
fn decompress_streaming_chunked_input() {
    let original = repetitive_bytes(16 * 1024);
    let frame = compress_frame_simple(&original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut output = Vec::new();
    let mut dst_buf = vec![0u8; 4096];
    let mut pos = 0;
    while pos < frame.len() {
        // Feed 256 bytes at a time
        let end = (pos + 256).min(frame.len());
        let chunk = &frame[pos..end];
        let (sc, dw, _hint) =
            lz4f_decompress(&mut dctx, Some(&mut dst_buf), chunk, None).unwrap();
        output.extend_from_slice(&dst_buf[..dw]);
        pos += sc;
        if sc == 0 && dw == 0 {
            break; // No progress — frame done or need more data
        }
    }
    assert_eq!(output, original);
}

/// Parity: context state resets after a complete frame; reuse for second frame works.
#[test]
fn decompress_context_reuse_after_complete_frame() {
    let original1 = b"first frame data".to_vec();
    let original2 = b"second frame payload".to_vec();
    let frame1 = compress_frame_simple(&original1);
    let frame2 = compress_frame_simple(&original2);

    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 256];

    // First frame
    let (sc1, dw1, hint1) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame1, None).unwrap();
    assert_eq!(sc1, frame1.len());
    assert_eq!(dw1, original1.len());
    assert_eq!(hint1, 0);
    assert_eq!(&dst[..dw1], &original1[..]);

    // Second frame — context should reset automatically after frame completion
    let (sc2, dw2, hint2) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame2, None).unwrap();
    assert_eq!(sc2, frame2.len());
    assert_eq!(dw2, original2.len());
    assert_eq!(hint2, 0);
    assert_eq!(&dst[..dw2], &original2[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — error cases
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: invalid magic number returns FrameTypeUnknown.
#[test]
fn decompress_invalid_magic_returns_err() {
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    // Build a fake frame with wrong magic
    let mut buf = vec![0u8; 32];
    buf[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    buf[4] = 0x60;
    buf[5] = 0x70;
    buf[6] = 0x00; // HC
    let mut dst = vec![0u8; 64];
    let result = lz4f_decompress(&mut dctx, Some(&mut dst), &buf, None);
    assert!(result.is_err());
}

/// Parity: DecompressOptions::default has skip_checksums=false, stable_dst=false.
#[test]
fn decompress_options_default_values() {
    let opts = DecompressOptions::default();
    assert!(!opts.skip_checksums);
    assert!(!opts.stable_dst);
}

/// Parity: skip_checksum sticky once set — even if subsequent opts don't set it.
#[test]
fn decompress_skip_checksum_sticky() {
    let original = repetitive_bytes(1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let mut frame = compress_frame_with_prefs(&original, &prefs);
    let n = frame.len();
    frame[n - 1] ^= 0xFF; // Corrupt checksum

    // Set skip on first call with empty src (just to set the flag)
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let opts_skip = DecompressOptions { skip_checksums: true, ..Default::default() };
    let _ = lz4f_decompress(&mut dctx, None, &[], Some(&opts_skip));
    assert!(dctx.skip_checksum, "skip_checksum should be set sticky");

    // Second call without opts — skip_checksum remains true
    let mut dst = vec![0u8; original.len() + 64];
    let result = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None);
    assert!(result.is_ok(), "sticky skip_checksum must bypass bad checksum");
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_get_frame_info
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: get_frame_info on a valid frame header extracts FrameInfo.
/// Parity: get_frame_info extracts block_size_id from the frame header.
/// Note: compress_frame auto-selects optimal (possibly smaller) block size
/// for the given input — use data large enough to force the desired block size.
#[test]
fn get_frame_info_extracts_block_size_id() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max256Kb,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    // Use data larger than 64 KiB so that Max256Kb is not downgraded to Max64Kb
    let data = repetitive_bytes(70 * 1024);
    let frame = compress_frame_with_prefs(&data, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let (info, consumed, _hint) = lz4f_get_frame_info(&mut dctx, &frame).unwrap();
    assert_eq!(info.block_size_id, BlockSizeId::Max256Kb);
    assert!(consumed > 0);
}

/// Parity: get_frame_info on frame with content checksum flag set.
#[test]
fn get_frame_info_detects_content_checksum_flag() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(b"test", &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let (info, _consumed, _hint) = lz4f_get_frame_info(&mut dctx, &frame).unwrap();
    assert_eq!(info.content_checksum_flag, ContentChecksum::Enabled);
}

/// Parity: get_frame_info on frame with block checksum flag set.
#[test]
fn get_frame_info_detects_block_checksum_flag() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_checksum_flag: BlockChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(b"test", &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let (info, _consumed, _hint) = lz4f_get_frame_info(&mut dctx, &frame).unwrap();
    assert_eq!(info.block_checksum_flag, BlockChecksum::Enabled);
}

/// Parity: get_frame_info on incomplete src returns error.
#[test]
fn get_frame_info_incomplete_src_returns_err() {
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    // Only 4 bytes — not enough for header
    let buf = 0x184D_2204u32.to_le_bytes();
    let result = lz4f_get_frame_info(&mut dctx, &buf);
    assert!(result.is_err());
}

/// Parity: get_frame_info returns hint of BH_SIZE after header consumed.
#[test]
fn get_frame_info_hint_is_bh_size() {
    let frame = compress_frame_simple(b"hello");
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let (_info, _consumed, hint) = lz4f_get_frame_info(&mut dctx, &frame).unwrap();
    assert_eq!(hint, BH_SIZE);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress_using_dict
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: decompress_using_dict with empty dict == regular decompress.
#[test]
fn decompress_using_dict_empty_dict_same_as_regular() {
    let original = repetitive_bytes(2048);
    let frame = compress_frame_simple(&original);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, hint) =
        lz4f_decompress_using_dict(&mut dctx, Some(&mut dst), &frame, &[], None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(hint, 0);
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: decompress_using_dict round-trips data compressed with dictionary.
#[test]
fn decompress_using_dict_matches_dict_compressed_frame() {
    // Build a dictionary and compress data that references it
    let dict_data = repetitive_bytes(16 * 1024);

    // Compress using the cdict API
    let cdict = Lz4FCDict::create(&dict_data).expect("create cdict");
    let original = repetitive_bytes(4 * 1024);
    let prefs = Preferences::default();
    let bound = lz4f_compress_frame_bound(original.len(), Some(&prefs));
    let mut compressed = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let written = lz4f_compress_frame_using_cdict(
        &mut cctx, &mut compressed, &original,
        cdict.as_ref() as *const Lz4FCDict,
        Some(&prefs),
    ).expect("compress with cdict");
    compressed.truncate(written);

    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, _hint) =
        lz4f_decompress_using_dict(&mut dctx, Some(&mut dst), &compressed, &dict_data, None)
            .unwrap();
    assert_eq!(sc, compressed.len());
    assert_eq!(dw, original.len());
    assert_eq!(&dst[..dw], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — all-checksums variant (both block + content)
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: frame with both block and content checksums decompresses correctly.
#[test]
fn decompress_both_checksums_enabled() {
    let original = repetitive_bytes(8 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_checksum_flag: BlockChecksum::Enabled,
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let frame = compress_frame_with_prefs(&original, &prefs);
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; original.len() + 64];
    let (sc, dw, hint) = lz4f_decompress(&mut dctx, Some(&mut dst), &frame, None).unwrap();
    assert_eq!(sc, frame.len());
    assert_eq!(dw, original.len());
    assert_eq!(hint, 0);
    assert_eq!(&dst[..dw], &original[..]);
}

/// Parity: same result whether dst is provided with exact size or oversized buffer.
#[test]
fn decompress_dst_size_does_not_affect_correctness() {
    let original = repetitive_bytes(2048);
    let frame = compress_frame_simple(&original);

    let mut dctx1 = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst_exact = vec![0u8; original.len()];
    let (_, dw1, _) = lz4f_decompress(&mut dctx1, Some(&mut dst_exact), &frame, None).unwrap();

    let mut dctx2 = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst_large = vec![0u8; original.len() * 2];
    let (_, dw2, _) = lz4f_decompress(&mut dctx2, Some(&mut dst_large), &frame, None).unwrap();

    assert_eq!(dw1, dw2);
    assert_eq!(&dst_exact[..dw1], &dst_large[..dw2]);
}
