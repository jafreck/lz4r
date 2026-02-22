//! E2E Test Suite 03: HC API
//!
//! Tests the High Compression (HC) API for both one-shot and streaming modes.
//! These tests validate that HC-compressed data can be decompressed correctly
//! using the standard block decompression API.

use lz4::hc::{
    compress_hc, compress_hc_continue, reset_stream_hc, Lz4StreamHc,
    LZ4HC_CLEVEL_DEFAULT, LZ4HC_CLEVEL_MAX, LZ4HC_CLEVEL_MIN,
};
use lz4::{lz4_compress_default, compress_bound, lz4_decompress_safe};

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: compress_HC default level (9) roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_default_level_roundtrip() {
    // 2 KB repetitive ASCII text
    let original = b"Hello LZ4 HC! ".repeat(150); // ~2100 bytes
    let src_size = original.len() as i32;
    
    // Allocate destination buffer
    let mut compressed = vec![0u8; compress_bound(src_size) as usize];
    
    // Compress with HC level 9 (default)
    let compressed_size = unsafe {
        compress_hc(
            original.as_ptr(),
            compressed.as_mut_ptr(),
            src_size,
            compressed.len() as i32,
            LZ4HC_CLEVEL_DEFAULT,
        )
    };
    
    assert!(compressed_size > 0, "HC compression failed");
    assert!(compressed_size < src_size, "HC should compress repetitive data");
    
    // Decompress using standard block API
    let mut decompressed = vec![0u8; original.len()];
    let result = lz4_decompress_safe(
        &compressed[..compressed_size as usize],
        &mut decompressed,
    );
    
    assert!(result.is_ok(), "Decompression failed: {:?}", result);
    assert_eq!(
        result.unwrap(),
        original.len(),
        "Decompressed size mismatch"
    );
    assert_eq!(decompressed, original, "Decompressed data mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: compress_HC minimum level (1) roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_minimum_level_roundtrip() {
    // 1 KB input
    let original = b"The quick brown fox jumps over the lazy dog. ".repeat(23); // ~1035 bytes
    let src_size = original.len() as i32;
    
    let mut compressed = vec![0u8; compress_bound(src_size) as usize];
    
    // Compress with HC level 1 (minimum)
    let compressed_size = unsafe {
        compress_hc(
            original.as_ptr(),
            compressed.as_mut_ptr(),
            src_size,
            compressed.len() as i32,
            LZ4HC_CLEVEL_MIN,
        )
    };
    
    assert!(compressed_size > 0, "HC level 1 compression failed");
    
    let mut decompressed = vec![0u8; original.len()];
    let result = lz4_decompress_safe(
        &compressed[..compressed_size as usize],
        &mut decompressed,
    );
    
    assert!(result.is_ok(), "Decompression failed: {:?}", result);
    assert_eq!(decompressed, original, "Roundtrip failed for HC level 1");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: compress_HC maximum level (12) roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_maximum_level_roundtrip() {
    // 4 KB repetitive input
    let original = vec![0x42u8; 4096];
    let src_size = original.len() as i32;
    
    let mut compressed = vec![0u8; compress_bound(src_size) as usize];
    
    // Compress with HC level 12 (maximum)
    let compressed_size = unsafe {
        compress_hc(
            original.as_ptr(),
            compressed.as_mut_ptr(),
            src_size,
            compressed.len() as i32,
            LZ4HC_CLEVEL_MAX,
        )
    };
    
    assert!(compressed_size > 0, "HC level 12 compression failed");
    // Very repetitive data should compress extremely well
    assert!(
        compressed_size < 100,
        "Expected very high compression for uniform data, got {} bytes",
        compressed_size
    );
    
    let mut decompressed = vec![0u8; original.len()];
    let result = lz4_decompress_safe(
        &compressed[..compressed_size as usize],
        &mut decompressed,
    );
    
    assert!(result.is_ok(), "Decompression failed: {:?}", result);
    assert_eq!(decompressed, original, "Roundtrip failed for HC level 12");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: HC produces smaller output than fast for compressible data
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_vs_fast_compression_ratio() {
    // Highly repetitive 4 KB input
    let original = b"AAAABBBBCCCCDDDD".repeat(256); // 4096 bytes
    let src_size = original.len() as i32;
    
    let bound = compress_bound(src_size) as usize;
    let mut compressed_fast = vec![0u8; bound];
    let mut compressed_hc = vec![0u8; bound];
    
    // Compress with fast (default)
    let fast_size = lz4_compress_default(&original, &mut compressed_fast)
        .expect("Fast compression failed");
    
    // Compress with HC level 9
    let hc_size = unsafe {
        compress_hc(
            original.as_ptr(),
            compressed_hc.as_mut_ptr(),
            src_size,
            compressed_hc.len() as i32,
            LZ4HC_CLEVEL_DEFAULT,
        )
    };
    
    assert!(hc_size > 0, "HC compression failed");
    
    // HC should produce smaller or equal output for repetitive data
    assert!(
        hc_size as usize <= fast_size,
        "HC output ({} bytes) should be <= fast output ({} bytes)",
        hc_size,
        fast_size
    );
    
    // Verify both decompress correctly
    let mut decompressed = vec![0u8; original.len()];
    
    let result_hc = lz4_decompress_safe(
        &compressed_hc[..hc_size as usize],
        &mut decompressed,
    );
    assert!(result_hc.is_ok(), "HC decompression failed");
    assert_eq!(decompressed, original, "HC roundtrip mismatch");
    
    let result_fast = lz4_decompress_safe(
        &compressed_fast[..fast_size],
        &mut decompressed,
    );
    assert!(result_fast.is_ok(), "Fast decompression failed");
    assert_eq!(decompressed, original, "Fast roundtrip mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: HC streaming: single chunk
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_streaming_single_chunk() {
    // 2 KB data
    let original = b"Streaming HC test data! ".repeat(87); // ~2088 bytes
    let src_size = original.len() as i32;
    
    // Create HC streaming context
    let mut stream = Lz4StreamHc::create().expect("Failed to create HC stream");
    
    let mut compressed = vec![0u8; compress_bound(src_size) as usize];
    
    // Compress single chunk
    let compressed_size = unsafe {
        compress_hc_continue(
            &mut stream,
            original.as_ptr(),
            compressed.as_mut_ptr(),
            src_size,
            compressed.len() as i32,
        )
    };
    
    assert!(compressed_size > 0, "HC streaming compression failed");
    
    // Decompress
    let mut decompressed = vec![0u8; original.len()];
    let result = lz4_decompress_safe(
        &compressed[..compressed_size as usize],
        &mut decompressed,
    );
    
    assert!(result.is_ok(), "Decompression failed: {:?}", result);
    assert_eq!(decompressed, original, "Streaming single-chunk roundtrip failed");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: HC streaming: multi-chunk
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: HC streaming: multi-chunk
//
// This test validates that HC streaming can compress multiple chunks while
// maintaining independent block boundaries that decompress correctly.
// We reset between chunks to ensure each block is self-contained.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_streaming_multi_chunk() {
    // Create 3 chunks of 2 KB each (6 KB total)
    let chunk1 = b"First chunk data repeated! ".repeat(76); // ~2052 bytes
    let chunk2 = b"Second chunk data repeated! ".repeat(74); // ~2072 bytes
    let chunk3 = b"Third chunk data repeated! ".repeat(79); // ~2054 bytes
    
    let mut stream = Lz4StreamHc::create().expect("Failed to create HC stream");
    
    let chunks = [chunk1.as_slice(), chunk2.as_slice(), chunk3.as_slice()];
    let mut all_compressed = Vec::new();
    let mut all_decompressed = Vec::new();
    
    // Compress each chunk through the streaming context
    // For this test, we want independent blocks, so we'll actually just
    // use the stream object without maintaining context between chunks
    for (i, chunk) in chunks.iter().enumerate() {
        // For truly independent blocks, reset stream before each chunk
        if i > 0 {
            reset_stream_hc(&mut stream, LZ4HC_CLEVEL_DEFAULT);
        }
        
        let src_size = chunk.len() as i32;
        let mut compressed = vec![0u8; compress_bound(src_size) as usize];
        
        let compressed_size = unsafe {
            compress_hc_continue(
                &mut stream,
                chunk.as_ptr(),
                compressed.as_mut_ptr(),
                src_size,
                compressed.len() as i32,
            )
        };
        
        assert!(
            compressed_size > 0,
            "HC streaming compression failed for chunk {}",
            i
        );
        
        compressed.truncate(compressed_size as usize);
        all_compressed.push(compressed);
    }
    
    // Decompress each chunk independently
    for (i, (original, compressed)) in chunks.iter().zip(all_compressed.iter()).enumerate() {
        let mut decompressed_chunk = vec![0u8; original.len()];
        let result = lz4_decompress_safe(compressed, &mut decompressed_chunk);
        
        assert!(
            result.is_ok(),
            "Decompression failed for chunk {}: {:?}",
            i,
            result
        );
        
        let decompressed_size = result.unwrap();
        assert_eq!(
            decompressed_size,
            original.len(),
            "Decompressed size mismatch for chunk {}",
            i
        );
        
        assert_eq!(
            decompressed_chunk, *original,
            "Chunk {} roundtrip failed",
            i
        );
        
        all_decompressed.extend_from_slice(&decompressed_chunk);
    }
    
    // Verify total size
    let total_original_size: usize = chunks.iter().map(|c| c.len()).sum();
    assert_eq!(
        all_decompressed.len(),
        total_original_size,
        "Total decompressed size mismatch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7: HC streaming: reset preserves correctness
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_hc_streaming_reset() {
    let message1 = b"First independent message! ".repeat(40); // ~1080 bytes
    let message2 = b"Second independent message! ".repeat(38); // ~1064 bytes
    
    let mut stream = Lz4StreamHc::create().expect("Failed to create HC stream");
    
    // Compress first message
    let src_size1 = message1.len() as i32;
    let mut compressed1 = vec![0u8; compress_bound(src_size1) as usize];
    
    let compressed_size1 = unsafe {
        compress_hc_continue(
            &mut stream,
            message1.as_ptr(),
            compressed1.as_mut_ptr(),
            src_size1,
            compressed1.len() as i32,
        )
    };
    
    assert!(compressed_size1 > 0, "First message compression failed");
    
    // Reset the stream (level 9)
    reset_stream_hc(&mut stream, LZ4HC_CLEVEL_DEFAULT);
    
    // Compress second message
    let src_size2 = message2.len() as i32;
    let mut compressed2 = vec![0u8; compress_bound(src_size2) as usize];
    
    let compressed_size2 = unsafe {
        compress_hc_continue(
            &mut stream,
            message2.as_ptr(),
            compressed2.as_mut_ptr(),
            src_size2,
            compressed2.len() as i32,
        )
    };
    
    assert!(compressed_size2 > 0, "Second message compression failed");
    
    // Decompress both messages independently
    let mut decompressed1 = vec![0u8; message1.len()];
    let result1 = lz4_decompress_safe(
        &compressed1[..compressed_size1 as usize],
        &mut decompressed1,
    );
    
    assert!(result1.is_ok(), "First message decompression failed: {:?}", result1);
    assert_eq!(decompressed1, message1, "First message roundtrip failed");
    
    let mut decompressed2 = vec![0u8; message2.len()];
    let result2 = lz4_decompress_safe(
        &compressed2[..compressed_size2 as usize],
        &mut decompressed2,
    );
    
    assert!(result2.is_ok(), "Second message decompression failed: {:?}", result2);
    assert_eq!(decompressed2, message2, "Second message roundtrip failed");
}
