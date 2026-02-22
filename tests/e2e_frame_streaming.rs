//! E2E Test Suite 05: Frame Streaming API
//!
//! Validates the LZ4 Frame streaming compress/decompress pipeline:
//! - Streaming compression context lifecycle (create → begin → update → end → free)
//! - Multiple chunk compression via compressUpdate
//! - Frame decompression state machine
//! - compressBound calculation
//! - Flush behavior
//! - get_frame_info
//! - Multiple concatenated frames
//! - Small buffer decompression (1-byte at a time)
//! - Context creation/destruction (RAII)
//!
//! These tests verify that the Rust port correctly implements the LZ4 Frame
//! streaming API for both compression and decompression.

extern crate lz4;

use lz4::frame::{
    lz4f_compress_begin, lz4f_compress_bound, lz4f_compress_end, lz4f_compress_frame,
    lz4f_compress_update, lz4f_create_compression_context, lz4f_create_decompression_context,
    lz4f_decompress, lz4f_free_compression_context, lz4f_get_frame_info, Preferences,
};
use lz4::frame::header::lz4f_compress_frame_bound;

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: Streaming compress → decompress roundtrip (single update)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_streaming_single_update_roundtrip() {
    // 8 KB data - ensure we have enough repetitions
    let original = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(150);
    assert!(original.len() >= 8000);

    // Create compression context
    let mut cctx = lz4f_create_compression_context(100).expect("create cctx");

    // Allocate output buffer with appropriate bound using preferences
    let prefs = Preferences::default();
    let max_dst_size = lz4f_compress_frame_bound(original.len(), Some(&prefs));
    let mut compressed = vec![0u8; max_dst_size];
    let mut total_compressed = 0;

    // Begin compression (writes frame header)
    let header_size = lz4f_compress_begin(&mut cctx, &mut compressed, None)
        .expect("compress begin should succeed");
    total_compressed += header_size;

    // Single update (writes compressed blocks)
    let update_size = lz4f_compress_update(
        &mut cctx,
        &mut compressed[total_compressed..],
        &original,
        None,
    )
    .expect("compress update should succeed");
    total_compressed += update_size;

    // End compression (writes end mark and optional checksum)
    let end_size = lz4f_compress_end(&mut cctx, &mut compressed[total_compressed..], None)
        .expect("compress end should succeed");
    total_compressed += end_size;

    // Free compression context
    lz4f_free_compression_context(cctx);

    assert!(total_compressed > 0, "compressed output should be non-empty");
    compressed.truncate(total_compressed);

    // Decompress
    let mut dctx = lz4f_create_decompression_context(100).expect("create dctx");
    let mut decompressed = vec![0u8; original.len() + 1024];
    let mut total_decompressed = 0;
    let mut src_pos = 0;

    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..],
            None,
        )
        .expect("frame decompression should succeed");

        src_pos += src_read;
        total_decompressed += dst_written;

        if src_pos >= compressed.len() {
            break;
        }
    }

    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: Streaming compress → decompress roundtrip (multiple updates)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_streaming_multi_chunk_roundtrip() {
    // 64 KB data fed in 16 KB chunks (4 updates)
    let chunk_size = 16 * 1024;
    let chunk1 = b"AAAA".repeat(4 * 1024);
    let chunk2 = b"BBBB".repeat(4 * 1024);
    let chunk3 = b"CCCC".repeat(4 * 1024);
    let chunk4 = b"DDDD".repeat(4 * 1024);
    let chunks = [&chunk1[..], &chunk2[..], &chunk3[..], &chunk4[..]];

    // Create compression context
    let mut cctx = lz4f_create_compression_context(100).expect("create cctx");

    // Allocate output buffer
    let max_dst_size = lz4f_compress_frame_bound(chunk_size * 4, None);
    let mut compressed = vec![0u8; max_dst_size];
    let mut total_compressed = 0;

    // Begin compression
    let header_size = lz4f_compress_begin(&mut cctx, &mut compressed, None)
        .expect("compress begin should succeed");
    total_compressed += header_size;

    // Feed 4 chunks
    for chunk in &chunks {
        let update_size = lz4f_compress_update(
            &mut cctx,
            &mut compressed[total_compressed..],
            chunk,
            None,
        )
        .expect("compress update should succeed");
        total_compressed += update_size;
    }

    // End compression
    let end_size = lz4f_compress_end(&mut cctx, &mut compressed[total_compressed..], None)
        .expect("compress end should succeed");
    total_compressed += end_size;

    // Free compression context
    lz4f_free_compression_context(cctx);

    compressed.truncate(total_compressed);

    // Decompress
    let mut dctx = lz4f_create_decompression_context(100).expect("create dctx");
    let mut decompressed = vec![0u8; chunk_size * 4 + 1024];
    let mut total_decompressed = 0;
    let mut src_pos = 0;

    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..],
            None,
        )
        .expect("frame decompression should succeed");

        src_pos += src_read;
        total_decompressed += dst_written;

        if src_pos >= compressed.len() {
            break;
        }
    }

    // Concatenate original chunks for comparison
    let original: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: compressBound returns adequate size before compressBegin
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_bound_adequate() {
    let input_size = 65536;

    // Get bound for streaming compression (includes overhead for updates)
    let bound = lz4f_compress_bound(input_size, None);

    // Bound must be greater than input size (accounts for frame overhead + worst case)
    assert!(
        bound > input_size,
        "compress bound {bound} should be > input size {input_size}"
    );

    // Verify that compressing with this bound succeeds
    let original = b"Test data ".repeat(6554); // ~65540 bytes
    let original = &original[..input_size]; // truncate to exactly input_size

    let mut cctx = lz4f_create_compression_context(100).expect("create cctx");
    
    // Use compress_frame_bound to get total size including header
    let total_bound = lz4f_compress_frame_bound(input_size, None);
    let mut compressed = vec![0u8; total_bound];
    let mut total = 0;

    let header = lz4f_compress_begin(&mut cctx, &mut compressed, None).expect("begin");
    total += header;

    let update = lz4f_compress_update(&mut cctx, &mut compressed[total..], original, None)
        .expect("update");
    total += update;

    let end = lz4f_compress_end(&mut cctx, &mut compressed[total..], None).expect("end");
    total += end;

    lz4f_free_compression_context(cctx);

    // Total compressed should fit within total_bound
    assert!(
        total <= total_bound,
        "compressed size {total} should fit within total bound {total_bound}"
    );
    
    // And the update portion should respect the bound for updates
    assert!(
        update <= bound,
        "update size {update} should fit within compress_bound {bound}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: Streaming compress with flush
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_streaming_compress_with_flush() {
    // 4 KB input, flush after each update
    let original = b"Flush test data. ".repeat(240);
    assert!(original.len() >= 4000);

    // Create preferences with auto_flush enabled
    let prefs = Preferences {
        auto_flush: true,
        ..Default::default()
    };

    let mut cctx = lz4f_create_compression_context(100).expect("create cctx");
    let max_dst_size = lz4f_compress_frame_bound(original.len(), Some(&prefs));
    let mut compressed = vec![0u8; max_dst_size];
    let mut total = 0;

    let header = lz4f_compress_begin(&mut cctx, &mut compressed, Some(&prefs)).expect("begin");
    total += header;

    let update = lz4f_compress_update(&mut cctx, &mut compressed[total..], &original, None)
        .expect("update");
    total += update;

    let end = lz4f_compress_end(&mut cctx, &mut compressed[total..], None).expect("end");
    total += end;

    lz4f_free_compression_context(cctx);

    compressed.truncate(total);

    // Decompress and verify
    let mut dctx = lz4f_create_decompression_context(100).expect("create dctx");
    let mut decompressed = vec![0u8; original.len() + 1024];
    let mut total_decompressed = 0;
    let mut src_pos = 0;

    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..],
            None,
        )
        .expect("decompress");

        src_pos += src_read;
        total_decompressed += dst_written;

        if src_pos >= compressed.len() {
            break;
        }
    }

    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: Frame decompression state machine: get frame info
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_get_frame_info() {
    // Create a valid LZ4 frame
    let original = b"Frame info test data. ".repeat(50);

    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];

    let compressed_size = lz4f_compress_frame(&mut compressed, &original, None)
        .expect("frame compression should succeed");
    compressed.truncate(compressed_size);

    // Create decompression context
    let mut dctx = lz4f_create_decompression_context(100).expect("create dctx");

    // Get frame info
    let (frame_info, _src_read, _hint) =
        lz4f_get_frame_info(&mut dctx, &compressed).expect("get frame info should succeed");

    // Verify frame info is valid (block_size_id should be non-Default or a valid value)
    // The content_size might be 0 (unknown) if not stored in frame header
    // Just verify we got a valid FrameInfo struct without panicking
    let _ = frame_info.block_size_id;
    let _ = frame_info.block_mode;
    let _ = frame_info.content_checksum_flag;
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: Multiple frames in sequence
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_multiple_frames_concatenated() {
    // Create two independent frames
    let data1 = b"First frame data. ".repeat(50);
    let data2 = b"Second frame data. ".repeat(50);

    let bound1 = lz4f_compress_frame_bound(data1.len(), None);
    let bound2 = lz4f_compress_frame_bound(data2.len(), None);

    let mut frame1 = vec![0u8; bound1];
    let mut frame2 = vec![0u8; bound2];

    let size1 = lz4f_compress_frame(&mut frame1, &data1, None).expect("compress frame 1");
    let size2 = lz4f_compress_frame(&mut frame2, &data2, None).expect("compress frame 2");

    frame1.truncate(size1);
    frame2.truncate(size2);

    // Concatenate frames
    let mut concatenated = Vec::new();
    concatenated.extend_from_slice(&frame1);
    concatenated.extend_from_slice(&frame2);

    // Decompress both frames with a single context
    // The decompression context should automatically handle multiple frames
    let mut dctx = lz4f_create_decompression_context(100).expect("create dctx");
    let mut decompressed = vec![0u8; data1.len() + data2.len() + 1024];
    let mut total_decompressed = 0;
    let mut src_pos = 0;

    loop {
        if src_pos >= concatenated.len() {
            break;
        }

        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &concatenated[src_pos..],
            None,
        )
        .expect("decompress");

        src_pos += src_read;
        total_decompressed += dst_written;

        if src_read == 0 && dst_written == 0 {
            break;
        }
    }

    // Both frames should be decompressed
    let expected = [&data1[..], &data2[..]].concat();
    assert_eq!(total_decompressed, expected.len());
    assert_eq!(&decompressed[..total_decompressed], &expected[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7: Streaming decompress with undersized dst buffer (1-byte at a time)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decompress_one_byte_at_a_time() {
    // Compress a small frame
    let original = b"Small test data for byte-by-byte decompression.";

    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];

    let compressed_size = lz4f_compress_frame(&mut compressed, original, None)
        .expect("frame compression should succeed");
    compressed.truncate(compressed_size);

    // Decompress with 1-byte buffer
    let mut dctx = lz4f_create_decompression_context(100).expect("create dctx");
    let mut decompressed = Vec::new();
    let mut src_pos = 0;

    loop {
        if src_pos >= compressed.len() {
            break;
        }

        let mut one_byte_buf = [0u8; 1];
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut one_byte_buf),
            &compressed[src_pos..],
            None,
        )
        .expect("decompress should succeed");

        src_pos += src_read;
        if dst_written > 0 {
            decompressed.extend_from_slice(&one_byte_buf[..dst_written]);
        }

        // Prevent infinite loop if no progress
        if src_read == 0 && dst_written == 0 {
            break;
        }
    }

    assert_eq!(decompressed.len(), original.len());
    assert_eq!(&decompressed[..], original);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 8: Context create/drop with default allocator (RAII test)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_context_create_drop() {
    // Create compression context
    let cctx = lz4f_create_compression_context(100).expect("create cctx");
    // Use it minimally
    let _version = cctx.version;
    // Drop it (RAII)
    lz4f_free_compression_context(cctx);

    // Create decompression context
    let dctx = lz4f_create_decompression_context(100).expect("create dctx");
    // Use it minimally
    let _version = dctx.version;
    // Drop happens automatically (RAII)
    drop(dctx);

    // No panic = success
}
