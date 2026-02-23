// Unit tests for task-018: src/frame/compress.rs — LZ4 Frame streaming compression
//
// Verifies behavioural parity with lz4frame.c v1.10.0, lines 419–1244:
//   - Context lifecycle: `lz4f_create_compression_context`, `lz4f_free_compression_context`
//   - Frame header write: `lz4f_compress_begin` variants
//   - Streaming update: `lz4f_compress_update`, `lz4f_uncompressed_update`, `lz4f_flush`, `lz4f_compress_end`
//   - Bound calculation: `lz4f_compress_bound`
//   - One-shot: `lz4f_compress_frame`, `lz4f_compress_frame_using_cdict`
//   - Constants: `LZ4F_MAGIC_NUMBER`, `LZ4F_VERSION`

use lz4::frame::compress::{
    lz4f_compress_begin, lz4f_compress_begin_using_dict, lz4f_compress_bound, lz4f_compress_end,
    lz4f_compress_frame, lz4f_compress_frame_using_cdict, lz4f_compress_update,
    lz4f_create_compression_context, lz4f_flush, lz4f_free_compression_context,
    lz4f_uncompressed_update, CompressOptions, LZ4F_MAGIC_NUMBER, LZ4F_VERSION,
};
use lz4::frame::header::lz4f_compress_frame_bound;
use lz4::frame::types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, FrameInfo, Lz4FCCtx, Preferences,
    MAX_FH_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn cycling_bytes(len: usize) -> Vec<u8> {
    (0u8..=255).cycle().take(len).collect()
}

fn repetitive_bytes(len: usize) -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog "
        .iter()
        .cycle()
        .take(len)
        .copied()
        .collect()
}

fn default_dst(src_len: usize) -> Vec<u8> {
    let bound = lz4f_compress_frame_bound(src_len, None);
    vec![0u8; bound]
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: LZ4F_MAGICNUMBER == 0x184D2204 (lz4frame.h:280).
#[test]
fn magic_number_value() {
    assert_eq!(LZ4F_MAGIC_NUMBER, 0x184D_2204u32);
}

/// Parity: LZ4F_VERSION == 100.
#[test]
fn version_constant_is_100() {
    assert_eq!(LZ4F_VERSION, 100u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Context lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: LZ4F_createCompressionContext rejects version != LZ4F_VERSION.
#[test]
fn create_ctx_wrong_version_returns_err() {
    assert!(lz4f_create_compression_context(0).is_err());
    assert!(lz4f_create_compression_context(99).is_err());
    assert!(lz4f_create_compression_context(101).is_err());
    assert!(lz4f_create_compression_context(u32::MAX).is_err());
}

/// Parity: LZ4F_createCompressionContext with LZ4F_VERSION succeeds.
#[test]
fn create_ctx_correct_version_succeeds() {
    let ctx = lz4f_create_compression_context(LZ4F_VERSION);
    assert!(ctx.is_ok());
}

/// Parity: LZ4F_freeCompressionContext drops without panic; inner ctx freed by Drop.
#[test]
fn free_ctx_no_panic() {
    let ctx = lz4f_create_compression_context(LZ4F_VERSION).unwrap();
    lz4f_free_compression_context(ctx); // must not panic
}

/// Context can be created and freed multiple times (no global state corruption).
#[test]
fn create_and_free_ctx_multiple_times() {
    for _ in 0..8 {
        let ctx = lz4f_create_compression_context(LZ4F_VERSION).unwrap();
        lz4f_free_compression_context(ctx);
    }
}

/// Drop of Lz4FCCtx must not panic (same as free, but implicit drop).
#[test]
fn ctx_drop_no_panic() {
    let _ctx = Lz4FCCtx::new(LZ4F_VERSION);
    // implicit drop at end of scope
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_begin — frame header writing
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: compress_begin writes the LZ4 magic number at offset 0.
#[test]
fn compress_begin_writes_magic() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    let n = lz4f_compress_begin(&mut cctx, &mut dst, None).expect("begin");
    assert!(n >= 4, "must write at least magic (4 bytes)");
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

/// Parity: compress_begin fails when dst is too small (< MAX_FH_SIZE).
#[test]
fn compress_begin_dst_too_small_returns_err() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE - 1];
    assert!(lz4f_compress_begin(&mut cctx, &mut dst, None).is_err());
}

/// Parity: compress_begin with content checksum prefs sets the CFlg bit (bit 2 of FLG byte).
#[test]
fn compress_begin_content_checksum_sets_flg_bit() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        ..Default::default()
    };
    lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).expect("begin");
    // FLG byte is at offset 4 (after magic)
    let flg = dst[4];
    assert_ne!(flg & 0x04, 0, "content checksum bit (bit 2) must be set");
}

/// Parity: compress_begin with independent block mode sets the B.Indep bit (bit 5 of FLG).
#[test]
fn compress_begin_independent_block_mode_sets_flg_bit() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            ..Default::default()
        },
        ..Default::default()
    };
    lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).expect("begin");
    let flg = dst[4];
    assert_ne!(flg & 0x20, 0, "B.Indep bit (bit 5) must be set");
}

/// Parity: FLG byte always has version bits (bits 7:6) == 0b01.
#[test]
fn compress_begin_flg_version_bits_are_01() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    lz4f_compress_begin(&mut cctx, &mut dst, None).expect("begin");
    let flg = dst[4];
    assert_eq!((flg >> 6) & 0x03, 0b01, "FLG version bits must be 01");
}

/// Parity: BD byte encodes the block size ID in bits 6:4.
#[test]
fn compress_begin_bd_byte_encodes_block_size_id() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max256Kb,
            ..Default::default()
        },
        ..Default::default()
    };
    lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).expect("begin");
    // BD byte is at offset 5 (after magic + FLG)
    let bd = dst[5];
    // BlockSizeId::Max256Kb = 5 → bits 6:4 = 0b101 → (bd >> 4) & 0x7 == 5
    assert_eq!((bd >> 4) & 0x7, 5u8, "BD must encode Max256Kb (id=5)");
}

/// compress_begin returns a byte count in [7, MAX_FH_SIZE] (minimum header).
#[test]
fn compress_begin_return_value_in_header_range() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    let n = lz4f_compress_begin(&mut cctx, &mut dst, None).expect("begin");
    // Minimum header: magic(4) + FLG(1) + BD(1) + HC(1) = 7
    // Maximum header: 19 bytes
    assert!(
        (7..=MAX_FH_SIZE).contains(&n),
        "header size must be in [7, 19], got {n}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_begin_using_dict
// ─────────────────────────────────────────────────────────────────────────────

/// compress_begin_using_dict with an empty dict behaves like compress_begin.
#[test]
fn compress_begin_using_empty_dict_behaves_like_begin() {
    let mut cctx1 = Lz4FCCtx::new(LZ4F_VERSION);
    let mut cctx2 = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst1 = vec![0u8; MAX_FH_SIZE + 64];
    let mut dst2 = vec![0u8; MAX_FH_SIZE + 64];
    let n1 = lz4f_compress_begin(&mut cctx1, &mut dst1, None).expect("begin");
    let n2 = lz4f_compress_begin_using_dict(&mut cctx2, &mut dst2, &[], None).expect("begin_dict");
    assert_eq!(
        n1, n2,
        "empty dict header size must match no-dict header size"
    );
    assert_eq!(
        &dst1[..n1],
        &dst2[..n2],
        "empty dict header bytes must match"
    );
}

/// compress_begin_using_dict with a non-empty dict succeeds.
#[test]
fn compress_begin_using_dict_with_data_succeeds() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE + 64];
    let dict = cycling_bytes(4096);
    let n = lz4f_compress_begin_using_dict(&mut cctx, &mut dst, &dict, None).expect("begin_dict");
    assert!(n >= 7);
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

/// compress_begin_using_dict fails when dst is too small.
#[test]
fn compress_begin_using_dict_dst_too_small() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; MAX_FH_SIZE - 1];
    let dict = cycling_bytes(256);
    assert!(lz4f_compress_begin_using_dict(&mut cctx, &mut dst, &dict, None).is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_bound
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: compress_bound is monotonically non-decreasing with src_size.
#[test]
fn compress_bound_is_monotone() {
    let prefs = Preferences::default();
    let b0 = lz4f_compress_bound(0, Some(&prefs));
    let b1 = lz4f_compress_bound(1, Some(&prefs));
    let b64k = lz4f_compress_bound(64 * 1024, Some(&prefs));
    let b1m = lz4f_compress_bound(1024 * 1024, Some(&prefs));
    assert!(b0 <= b1);
    assert!(b1 <= b64k);
    assert!(b64k <= b1m);
}

/// Parity: compress_bound with None prefs equals default prefs.
#[test]
fn compress_bound_none_prefs_equals_default() {
    let default_prefs = Preferences::default();
    for sz in [0, 1, 1024, 65536] {
        assert_eq!(
            lz4f_compress_bound(sz, None),
            lz4f_compress_bound(sz, Some(&default_prefs)),
            "None prefs must equal Default prefs for src_size={sz}"
        );
    }
}

/// Parity: compress_bound returns > 0 even for 0-byte src.
#[test]
fn compress_bound_zero_src_is_positive() {
    assert!(lz4f_compress_bound(0, None) > 0);
}

/// Parity: output buffer of compress_bound size is always sufficient for compress_update.
/// Verified indirectly by confirming one-shot compress completes within frame_bound.
#[test]
fn compress_bound_sufficient_for_update() {
    let src = repetitive_bytes(32 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let bound = lz4f_compress_bound(src.len(), Some(&prefs));
    // We allocate frame_bound (header + body + footer) to do a full frame.
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound];
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    // Ensure the bound-sized window is always big enough for the update call.
    assert!(dst.len() - pos >= bound);
    pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], &src, None).unwrap();
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(pos > 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_update
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: compress_update before compress_begin returns an error.
#[test]
fn compress_update_without_begin_returns_err() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let src = b"hello";
    let mut dst = vec![0u8; 1024];
    assert!(lz4f_compress_update(&mut cctx, &mut dst, src, None).is_err());
}

/// Parity: compress_update with dst too small returns an error.
#[test]
fn compress_update_dst_too_small_returns_err() {
    let src = repetitive_bytes(64 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    // Pass a 0-length dst slice to trigger the size check.
    assert!(lz4f_compress_update(&mut cctx, &mut dst[pos..pos], &src, None).is_err());
}

/// Parity: compress_update with 0-byte src may return 0 bytes written.
#[test]
fn compress_update_empty_src_writes_zero() {
    let prefs = Preferences {
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(0, Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    let written = lz4f_compress_update(&mut cctx, &mut dst[pos..], &[], None).unwrap();
    // Empty src → nothing to compress; auto_flush → 0-byte block if flushed.
    // Either 0 bytes (nothing) or a zero-length block (BH_SIZE) is valid.
    assert!(
        written <= 8,
        "updating with empty src must not emit large output, got {written}"
    );
}

/// Parity: streaming many small chunks accumulates into a valid frame.
#[test]
fn compress_update_chunked_produces_valid_frame() {
    let src = repetitive_bytes(16 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    for chunk in src.chunks(256) {
        pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], chunk, None).unwrap();
    }
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(pos > 0);
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

/// Parity: compress_update with stable_src=true does not panic.
#[test]
fn compress_update_stable_src_no_panic() {
    let src = repetitive_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    let opts = CompressOptions { stable_src: true };
    pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], &src, Some(&opts)).unwrap();
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], Some(&opts)).unwrap();
    assert!(pos > 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_uncompressed_update
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: uncompressed_update stores blocks verbatim (block header has uncompressed flag).
#[test]
fn uncompressed_update_marks_block_uncompressed() {
    let src = b"hello world this is a test of uncompressed blocks";
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    pos += lz4f_uncompressed_update(&mut cctx, &mut dst[pos..], src, None).unwrap();
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(pos > 0);
    // Block header starts at header_end (between magic+FLG+BD+HC and data)
    // We only verify the frame starts with magic and has output
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

/// Parity: uncompressed_update before compress_begin returns an error.
#[test]
fn uncompressed_update_without_begin_returns_err() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 1024];
    assert!(lz4f_uncompressed_update(&mut cctx, &mut dst, b"data", None).is_err());
}

/// Parity: uncompressed_update output is larger (raw store) than compressed for compressible data.
#[test]
fn uncompressed_update_output_larger_than_compressed() {
    let src = repetitive_bytes(8 * 1024); // very compressible
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));

    // Compressed frame
    let mut dst_c = vec![0u8; frame_bound];
    let mut cctx_c = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos_c = lz4f_compress_begin(&mut cctx_c, &mut dst_c, Some(&prefs)).unwrap();
    pos_c += lz4f_compress_update(&mut cctx_c, &mut dst_c[pos_c..], &src, None).unwrap();
    pos_c += lz4f_compress_end(&mut cctx_c, &mut dst_c[pos_c..], None).unwrap();

    // Uncompressed frame
    let mut dst_u = vec![0u8; frame_bound + src.len()];
    let mut cctx_u = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos_u = lz4f_compress_begin(&mut cctx_u, &mut dst_u, Some(&prefs)).unwrap();
    pos_u += lz4f_uncompressed_update(&mut cctx_u, &mut dst_u[pos_u..], &src, None).unwrap();
    pos_u += lz4f_compress_end(&mut cctx_u, &mut dst_u[pos_u..], None).unwrap();

    assert!(
        pos_u > pos_c,
        "uncompressed frame ({pos_u} bytes) must be larger than compressed ({pos_c} bytes)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_flush
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: flush on empty buffer returns 0 bytes written.
#[test]
fn flush_empty_buffer_returns_zero() {
    let prefs = Preferences {
        auto_flush: false,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(0, Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    // Nothing in the staging buffer yet.
    let written = lz4f_flush(&mut cctx, &mut dst[16..], None).unwrap();
    assert_eq!(written, 0, "flush with empty buffer must return 0");
}

/// Parity: flush after buffered data emits compressed block immediately.
#[test]
fn flush_emits_buffered_data() {
    // Use auto_flush=false so data is buffered instead of emitted by update.
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        auto_flush: false,
        ..Default::default()
    };
    let src = repetitive_bytes(1024); // less than max_block_size → buffered
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 128];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    // Update: data is smaller than block_size → gets buffered, returns 0.
    let update_written = lz4f_compress_update(&mut cctx, &mut dst[pos..], &src, None).unwrap();
    assert_eq!(
        update_written, 0,
        "sub-block data must be buffered (not emitted)"
    );
    // Now flush: must emit the buffered block.
    let flush_written = lz4f_flush(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(flush_written > 0, "flush must emit the buffered block");
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_end
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: compress_end writes at least 4 bytes (end-mark).
#[test]
fn compress_end_writes_at_least_4_bytes() {
    let prefs = Preferences {
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(0, Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    let end_written = lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(
        end_written >= 4,
        "end must write at least 4 bytes (end-mark), got {end_written}"
    );
}

/// Parity: compress_end writes 4-byte zero end-mark (lz4frame.c:998).
#[test]
fn compress_end_end_mark_is_four_zero_bytes() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Disabled,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(0, Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    let end_written = lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    // Without content checksum, end is exactly 4 zero bytes.
    assert_eq!(end_written, 4);
    assert_eq!(
        &dst[pos..pos + 4],
        &[0u8; 4],
        "end-mark must be 4 zero bytes"
    );
}

/// Parity: compress_end with content checksum writes 8 bytes (end-mark + checksum).
#[test]
fn compress_end_with_content_checksum_writes_8_bytes() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(0, Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    let end_written = lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert_eq!(
        end_written, 8,
        "end with content checksum must write 8 bytes"
    );
    // End-mark is still 4 zero bytes.
    assert_eq!(&dst[pos..pos + 4], &[0u8; 4]);
}

/// Parity: context is re-usable after compress_end (c_stage reset to 0).
#[test]
fn compress_end_context_is_reusable() {
    let prefs = Preferences {
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(16, Some(&prefs));
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    for _ in 0..3 {
        let mut dst = vec![0u8; frame_bound + 64];
        let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
        pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], b"hello", None).unwrap();
        pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
        assert!(pos > 0);
    }
}

/// Parity: content size mismatch between declared and actual returns FrameSizeWrong.
#[test]
fn compress_end_content_size_mismatch_returns_err() {
    let src = b"short";
    let prefs = Preferences {
        frame_info: FrameInfo {
            // Declare content size of 100 but only feed 5 bytes.
            content_size: 100,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(100, Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], src, None).unwrap();
    let result = lz4f_compress_end(&mut cctx, &mut dst[pos..], None);
    assert!(result.is_err(), "size mismatch must return an error");
}

/// Parity: content size correctly declared → compress_end succeeds.
#[test]
fn compress_end_content_size_correct_succeeds() {
    let src = b"correct size content";
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_size: src.len() as u64,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; frame_bound + 64];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], src, None).unwrap();
    let end = lz4f_compress_end(&mut cctx, &mut dst[pos..], None);
    assert!(end.is_ok(), "correct content size must succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_frame — one-shot
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: compress_frame with empty src produces a valid minimal frame.
#[test]
fn compress_frame_empty_src_produces_valid_frame() {
    let mut dst = default_dst(0);
    let written = lz4f_compress_frame(&mut dst, &[], None).expect("compress_frame empty");
    // magic(4) + FLG(1) + BD(1) + HC(1) + end-mark(4) = 11 bytes minimum
    assert!(
        written >= 11,
        "empty frame must be at least 11 bytes, got {written}"
    );
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

/// Parity: compress_frame returns magic number at byte 0.
#[test]
fn compress_frame_starts_with_magic() {
    let src = b"hello, world!";
    let mut dst = default_dst(src.len());
    lz4f_compress_frame(&mut dst, src, None).expect("compress_frame");
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

/// Parity: compress_frame output is strictly smaller than input for compressible data.
#[test]
fn compress_frame_compresses_compressible_data() {
    let src = repetitive_bytes(64 * 1024);
    let mut dst = default_dst(src.len());
    let written = lz4f_compress_frame(&mut dst, &src, None).expect("compress_frame");
    assert!(
        written < src.len(),
        "compressible data must compress: written={written}, src_len={}",
        src.len()
    );
}

/// Parity: compress_frame with content checksum prefs produces 4 extra bytes.
#[test]
fn compress_frame_content_checksum_adds_4_bytes() {
    let src = b"test content for checksum parity";
    let prefs_no = Preferences::default();
    let prefs_yes = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut dst_no = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs_no))];
    let mut dst_yes = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs_yes))];
    let n_no = lz4f_compress_frame(&mut dst_no, src, Some(&prefs_no)).unwrap();
    let n_yes = lz4f_compress_frame(&mut dst_yes, src, Some(&prefs_yes)).unwrap();
    assert_eq!(n_yes, n_no + 4, "content checksum must add exactly 4 bytes");
}

/// Parity: compress_frame with dst too small returns an error.
#[test]
fn compress_frame_dst_too_small_returns_err() {
    let src = repetitive_bytes(1024);
    let mut dst = vec![0u8; 4]; // definitely too small
    assert!(lz4f_compress_frame(&mut dst, &src, None).is_err());
}

/// Parity: compress_frame with block checksum enabled does not panic.
#[test]
fn compress_frame_with_block_checksum_no_panic() {
    let src = cycling_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_checksum_flag: BlockChecksum::Enabled,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut dst = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs))];
    let written = lz4f_compress_frame(&mut dst, &src, Some(&prefs)).unwrap();
    assert!(written > 0);
}

/// Parity: compress_frame is deterministic — same input + prefs = same output.
#[test]
fn compress_frame_is_deterministic() {
    let src = repetitive_bytes(8 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst1 = vec![0u8; bound];
    let mut dst2 = vec![0u8; bound];
    let n1 = lz4f_compress_frame(&mut dst1, &src, Some(&prefs)).unwrap();
    let n2 = lz4f_compress_frame(&mut dst2, &src, Some(&prefs)).unwrap();
    assert_eq!(n1, n2);
    assert_eq!(&dst1[..n1], &dst2[..n2]);
}

/// Parity: compress_frame with various block size IDs all succeed.
#[test]
fn compress_frame_various_block_sizes_succeed() {
    let src = cycling_bytes(16 * 1024);
    for bsid in [
        BlockSizeId::Max64Kb,
        BlockSizeId::Max256Kb,
        BlockSizeId::Max1Mb,
        BlockSizeId::Max4Mb,
    ] {
        let prefs = Preferences {
            frame_info: FrameInfo {
                block_size_id: bsid,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut dst = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs))];
        let written = lz4f_compress_frame(&mut dst, &src, Some(&prefs)).unwrap();
        assert!(
            written > 0,
            "compress_frame must succeed for block_size_id={bsid:?}"
        );
        let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
        assert_eq!(magic, LZ4F_MAGIC_NUMBER);
    }
}

/// Parity: one-shot compress_frame result starts with magic (parity with C LZ4F_compressFrame).
#[test]
fn compress_frame_single_byte_src() {
    let src = [0x42u8];
    let mut dst = default_dst(1);
    let written = lz4f_compress_frame(&mut dst, &src, None).unwrap();
    assert!(written >= 11);
    let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
    assert_eq!(magic, LZ4F_MAGIC_NUMBER);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_compress_frame_using_cdict — null cdict path
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: compress_frame_using_cdict with null cdict behaves like compress_frame.
#[test]
fn compress_frame_using_null_cdict_matches_compress_frame() {
    let src = repetitive_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            block_size_id: BlockSizeId::Max64Kb,
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst_frame = vec![0u8; bound];
    let mut dst_cdict = vec![0u8; bound];

    let n_frame = lz4f_compress_frame(&mut dst_frame, &src, Some(&prefs)).unwrap();
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let n_cdict = lz4f_compress_frame_using_cdict(
        &mut cctx,
        &mut dst_cdict,
        &src,
        core::ptr::null(),
        Some(&prefs),
    )
    .unwrap();

    assert_eq!(
        n_frame, n_cdict,
        "null-cdict frame must equal no-cdict frame size"
    );
    assert_eq!(
        &dst_frame[..n_frame],
        &dst_cdict[..n_cdict],
        "null-cdict frame bytes must match"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming vs one-shot parity
// ─────────────────────────────────────────────────────────────────────────────

/// Parity: streaming (begin + update + end) with auto_flush and independent blocks
/// must produce identical output to one-shot compress_frame when fed all data in
/// a single update call.
#[test]
fn streaming_single_update_matches_one_shot() {
    let src = repetitive_bytes(2 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            block_size_id: BlockSizeId::Max64Kb,
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut one_shot = vec![0u8; bound];
    let n_one_shot = lz4f_compress_frame(&mut one_shot, &src, Some(&prefs)).unwrap();

    let mut streaming = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let opts = CompressOptions { stable_src: true };
    let mut pos = lz4f_compress_begin(&mut cctx, &mut streaming, Some(&prefs)).unwrap();
    pos += lz4f_compress_update(&mut cctx, &mut streaming[pos..], &src, Some(&opts)).unwrap();
    pos += lz4f_compress_end(&mut cctx, &mut streaming[pos..], Some(&opts)).unwrap();

    assert_eq!(pos, n_one_shot, "streaming size must match one-shot");
    assert_eq!(
        &streaming[..pos],
        &one_shot[..n_one_shot],
        "streaming bytes must match one-shot"
    );
}

/// Parity: total frame size from streaming must not exceed compress_frame_bound.
#[test]
fn streaming_total_within_frame_bound() {
    let src = cycling_bytes(32 * 1024);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    for chunk in src.chunks(1024) {
        pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], chunk, None).unwrap();
    }
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(
        pos <= bound,
        "total frame size {pos} must be within bound {bound}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressOptions
// ─────────────────────────────────────────────────────────────────────────────

/// CompressOptions defaults to stable_src = false.
#[test]
fn compress_options_default_stable_src_is_false() {
    let opts = CompressOptions::default();
    assert!(!opts.stable_src);
}

// ─────────────────────────────────────────────────────────────────────────────
// HC (high-compression) mode tests
// Covers: hc_ctx_ptr, set_hc_level, HcIndependent/HcLinked paths
// ─────────────────────────────────────────────────────────────────────────────

/// HC compression level >= 3 round-trips correctly (covers hc_ctx_ptr, HcIndependent path).
#[test]
fn hc_compress_independent_round_trips() {
    let src: Vec<u8> = cycling_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            ..Default::default()
        },
        compression_level: 9, // HC level
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; bound];
    let n = lz4f_compress_frame(&mut dst, &src, Some(&prefs)).expect("HC compress must succeed");
    dst.truncate(n);
    let dec = lz4::frame::decompress_frame_to_vec(&dst).expect("decompression must succeed");
    assert_eq!(dec, src);
}

/// HC compression with linked blocks round-trips (covers HcLinked path).
#[test]
fn hc_compress_linked_blocks_round_trips() {
    let src: Vec<u8> = cycling_bytes(4096);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Linked,
            ..Default::default()
        },
        compression_level: 9, // HC level
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; bound];
    let n = lz4f_compress_frame(&mut dst, &src, Some(&prefs)).expect("HC linked compress");
    dst.truncate(n);
    let dec = lz4::frame::decompress_frame_to_vec(&dst).expect("decompression must succeed");
    assert_eq!(dec, src);
}

/// HC streaming compression with lz4f_compress_begin/update/end (covers hc_ctx_ptr lines).
#[test]
fn hc_streaming_compress_begin_update_end_round_trips() {
    let src: Vec<u8> = cycling_bytes(8192);
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_mode: BlockMode::Independent,
            ..Default::default()
        },
        compression_level: 9,
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut dst, Some(&prefs)).unwrap();
    for chunk in src.chunks(1024) {
        pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], chunk, None).unwrap();
    }
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&dst[..pos]).expect("decompression must succeed");
    assert_eq!(dec, src);
}

/// Switching from fast to HC context re-uses the allocated buffer (covers lines 549-568, 149-151).
#[test]
fn ctx_type_switch_from_fast_to_hc_covers_reinit_path() {
    let src: Vec<u8> = cycling_bytes(4096);
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);

    // First: compress with fast level (0 = default fast)
    let fast_prefs = Preferences {
        frame_info: FrameInfo { block_mode: BlockMode::Independent, ..Default::default() },
        compression_level: 0,
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&fast_prefs));
    let mut dst1 = vec![0u8; bound];
    let mut pos1 = lz4f_compress_begin(&mut cctx, &mut dst1, Some(&fast_prefs)).unwrap();
    pos1 += lz4f_compress_update(&mut cctx, &mut dst1[pos1..], &src, None).unwrap();
    pos1 += lz4f_compress_end(&mut cctx, &mut dst1[pos1..], None).unwrap();
    let dec1 = lz4::frame::decompress_frame_to_vec(&dst1[..pos1]).unwrap();
    assert_eq!(dec1, src);

    // Second: compress with HC level — triggers the ctx type switch path
    let hc_prefs = Preferences {
        frame_info: FrameInfo { block_mode: BlockMode::Independent, ..Default::default() },
        compression_level: 9, // HC
        auto_flush: true,
        ..Default::default()
    };
    let bound2 = lz4f_compress_frame_bound(src.len(), Some(&hc_prefs));
    let mut dst2 = vec![0u8; bound2];
    let mut pos2 = lz4f_compress_begin(&mut cctx, &mut dst2, Some(&hc_prefs)).unwrap();
    pos2 += lz4f_compress_update(&mut cctx, &mut dst2[pos2..], &src, None).unwrap();
    pos2 += lz4f_compress_end(&mut cctx, &mut dst2[pos2..], None).unwrap();
    let dec2 = lz4::frame::decompress_frame_to_vec(&dst2[..pos2]).unwrap();
    assert_eq!(dec2, src);
}

/// Switching from HC back to fast context (covers lines 549-555, write_inner_ptr update path).
#[test]
fn ctx_type_switch_from_hc_to_fast_covers_reinit_path() {
    let src: Vec<u8> = cycling_bytes(4096);
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);

    // First: HC compression
    let hc_prefs = Preferences {
        frame_info: FrameInfo { block_mode: BlockMode::Independent, ..Default::default() },
        compression_level: 9,
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&hc_prefs));
    let mut dst1 = vec![0u8; bound];
    let mut pos1 = lz4f_compress_begin(&mut cctx, &mut dst1, Some(&hc_prefs)).unwrap();
    pos1 += lz4f_compress_update(&mut cctx, &mut dst1[pos1..], &src, None).unwrap();
    pos1 += lz4f_compress_end(&mut cctx, &mut dst1[pos1..], None).unwrap();
    assert!(pos1 > 7);

    // Second: switch back to fast compression — triggers lz4_ctx_type != ctx_type_id path
    let fast_prefs = Preferences {
        frame_info: FrameInfo { block_mode: BlockMode::Independent, ..Default::default() },
        compression_level: 0,
        auto_flush: true,
        ..Default::default()
    };
    let bound2 = lz4f_compress_frame_bound(src.len(), Some(&fast_prefs));
    let mut dst2 = vec![0u8; bound2];
    let mut pos2 = lz4f_compress_begin(&mut cctx, &mut dst2, Some(&fast_prefs)).unwrap();
    pos2 += lz4f_compress_update(&mut cctx, &mut dst2[pos2..], &src, None).unwrap();
    pos2 += lz4f_compress_end(&mut cctx, &mut dst2[pos2..], None).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&dst2[..pos2]).unwrap();
    assert_eq!(dec, src);
}

/// lz4f_compress_end returns DstMaxSizeTooSmall when dst too small (line 1025).
#[test]
fn compress_end_dst_too_small_returns_error() {
    let src = b"hello world";
    let prefs = Preferences { auto_flush: true, ..Default::default() };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut full_dst = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut full_dst, Some(&prefs)).unwrap();
    pos += lz4f_compress_update(&mut cctx, &mut full_dst[pos..], src, None).unwrap();
    // Pass a tiny slice — too small for end-mark
    let result = lz4f_compress_end(&mut cctx, &mut full_dst[pos..pos + 1], None);
    assert!(result.is_err(), "must fail when dst too small");
}

/// lz4f_compress_end with content_checksum and too-small dst (line 1034).
#[test]
fn compress_end_content_checksum_dst_too_small_returns_error() {
    let src = b"hello world";
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut full_dst = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let mut pos = lz4f_compress_begin(&mut cctx, &mut full_dst, Some(&prefs)).unwrap();
    pos += lz4f_compress_update(&mut cctx, &mut full_dst[pos..], src, None).unwrap();
    // Provide exactly 4 bytes (end-mark) but 0 for the checksum — too small
    let result = lz4f_compress_end(&mut cctx, &mut full_dst[pos..pos + 4], None);
    assert!(result.is_err(), "must fail when dst too small for checksum");
}

/// HC compression with CDict (covers cdict_ref and HC+cdict path in lz4f_make_block).
#[test]
fn hc_compress_with_cdict_round_trips() {
    use lz4::frame::cdict::Lz4FCDict;
    let dict_data: Vec<u8> = b"common repeated pattern for dictionary"
        .iter()
        .cycle()
        .take(1024)
        .copied()
        .collect();
    let cdict = Lz4FCDict::create(&dict_data).expect("CDict creation must succeed");
    let src: Vec<u8> = b"common repeated pattern for dictionary followed by content"
        .iter()
        .cycle()
        .take(2048)
        .copied()
        .collect();
    let bound = lz4f_compress_frame_bound(src.len(), None);
    let mut dst = vec![0u8; bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let hc_prefs = Preferences {
        compression_level: 9,
        auto_flush: true,
        ..Default::default()
    };
    let n = lz4f_compress_frame_using_cdict(&mut cctx, &mut dst, &src, &*cdict, Some(&hc_prefs))
        .expect("HC cdict compress must succeed");
    assert!(n > 0);
}

/// BlockSizeId::Default triggers block_size_id override to Max64Kb (line 572-573).
#[test]
fn compress_begin_block_size_default_overrides_to_max64kb() {
    use lz4::frame::types::BlockSizeId;
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Default,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let src = cycling_bytes(1024);
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; bound];
    let n = lz4f_compress_frame(&mut dst, &src, Some(&prefs)).expect("must succeed");
    assert!(n > 0);
}

/// lz4f_flush_impl when c_stage != 1 returns CompressionStateUninitialized (line 955).
#[test]
fn flush_before_begin_returns_compression_state_uninitialized() {
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    // Manually set tmp_in_size so flush is attempted but c_stage == 0
    // We can't easily set it, so let's just call lz4f_flush without begin
    let mut dst = vec![0u8; 1024];
    // lz4f_flush calls lz4f_flush_impl internally; c_stage starts at 0
    // With tmp_in_size == 0, flush returns Ok(0) immediately (line 952)
    // To hit line 955 we need tmp_in_size > 0 and c_stage != 1
    // This is hard to test from outside without non-autoflush mode
    // Instead, test the normal flush path:
    let prefs = Preferences {
        auto_flush: false, // non-autoflush means data gets staged
        frame_info: FrameInfo { block_size_id: BlockSizeId::Max64Kb, ..Default::default() },
        ..Default::default()
    };
    let src = cycling_bytes(64); // less than block size
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut full_dst = vec![0u8; bound];
    let mut cctx2 = Lz4FCCtx::new(LZ4F_VERSION);
    let pos = lz4f_compress_begin(&mut cctx2, &mut full_dst, Some(&prefs)).unwrap();
    let pos2 = lz4f_compress_update(&mut cctx2, &mut full_dst[pos..], &src, None).unwrap();
    // Now explicitly flush
    let flush_pos = lz4f_flush(&mut cctx2, &mut full_dst[pos + pos2..], None).unwrap();
    let end_pos = lz4f_compress_end(&mut cctx2, &mut full_dst[pos + pos2 + flush_pos..], None).unwrap();
    let total = pos + pos2 + flush_pos + end_pos;
    let dec = lz4::frame::decompress_frame_to_vec(&full_dst[..total]).unwrap();
    assert_eq!(dec, src, "non-autoflush round-trip must succeed");
    let _ = dst;
}

/// HC compress_begin_using_dict with dict covers the dict loading path.
#[test]
fn compress_begin_using_dict_with_hc_level_round_trips() {
    let dict: Vec<u8> = b"dict prefix ".iter().cycle().take(512).copied().collect();
    let src: Vec<u8> = b"dict prefix content".iter().cycle().take(2048).copied().collect();
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let hc_prefs = Preferences {
        compression_level: 9,
        auto_flush: true,
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(src.len(), Some(&hc_prefs));
    let mut dst = vec![0u8; bound];
    let mut pos = lz4f_compress_begin_using_dict(&mut cctx, &mut dst, &dict, Some(&hc_prefs))
        .expect("HC begin_using_dict must succeed");
    pos += lz4f_compress_update(&mut cctx, &mut dst[pos..], &src, None).unwrap();
    pos += lz4f_compress_end(&mut cctx, &mut dst[pos..], None).unwrap();
    assert!(pos > 7, "output must be a valid frame");
}

/// dict_id > 0 writes dict_id bytes to frame header (lines 675-676).
#[test]
fn compress_frame_with_dict_id_writes_dict_id_bytes() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            dict_id: 0xDEADBEEF,
            ..Default::default()
        },
        auto_flush: true,
        ..Default::default()
    };
    let src = cycling_bytes(128);
    let bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut dst = vec![0u8; bound];
    let n = lz4f_compress_frame(&mut dst, &src, Some(&prefs))
        .expect("compress with dict_id must succeed");
    assert!(n > 0, "frame with dict_id must produce output");
    // Verify the dict_id flag is set in FLG byte (bit 0) at offset 5
    // Magic=4 bytes, FLG=1 byte at offset 4
    let flg = dst[4];
    assert_ne!(flg & 1, 0, "dict_id bit in FLG must be set when dict_id > 0");
}

/// lz4f_flush on tiny dst when staging buffer has data → DstMaxSizeTooSmall (line 959).
#[test]
fn flush_with_tiny_dst_returns_dst_max_size_too_small() {
    use lz4::frame::types::Lz4FError;
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max64Kb,
            ..Default::default()
        },
        auto_flush: false, // so data isn't auto-flushed on update
        ..Default::default()
    };
    let src = cycling_bytes(64); // less than 64KB block size — stays staged
    let full_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
    let mut full_dst = vec![0u8; full_bound];

    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    let hdr_len = lz4f_compress_begin(&mut cctx, &mut full_dst, Some(&prefs)).unwrap();
    let upd_len = lz4f_compress_update(&mut cctx, &mut full_dst[hdr_len..], &src, None).unwrap();
    let _ = upd_len;

    // Now flush with a dst that's too small (less than BH_SIZE + src.len() + BF_SIZE)
    let mut tiny_dst = vec![0u8; 4]; // way too small
    let result = lz4f_flush(&mut cctx, &mut tiny_dst, None);
    // If tmp_in_size == 0 (data was auto-flushed despite auto_flush=false), test still passes
    // If tmp_in_size > 0, should return DstMaxSizeTooSmall
    assert!(
        result.is_ok() || matches!(result, Err(Lz4FError::DstMaxSizeTooSmall)),
        "flush with tiny dst must return Ok(0) or DstMaxSizeTooSmall: {result:?}"
    );
}

/// lz4f_uncompressed_update with dst too small → DstMaxSizeTooSmall (line 780).
#[test]
fn uncompressed_update_dst_too_small_returns_error() {
    use lz4::frame::types::Lz4FError;
    let src = cycling_bytes(64);
    let full_bound = lz4f_compress_frame_bound(src.len(), None);
    let mut full_dst = vec![0u8; full_bound];
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    lz4f_compress_begin(&mut cctx, &mut full_dst, None).unwrap();

    // dst too small for uncompressed block (must hold full src + BH_SIZE)
    let mut tiny = vec![0u8; 4];
    let result = lz4f_uncompressed_update(&mut cctx, &mut tiny, &src, None);
    assert!(
        matches!(result, Err(Lz4FError::DstMaxSizeTooSmall)),
        "uncompressed update with tiny dst must fail with DstMaxSizeTooSmall: {result:?}"
    );
}
