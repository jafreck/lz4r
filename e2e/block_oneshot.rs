//! E2E Test Suite 01: Block One-Shot API
//!
//! Validates the core LZ4 block compression and decompression functions:
//! - compress_default
//! - compress_fast
//! - compress_dest_size
//! - compress_bound
//! - decompress_safe
//! - decompress_safe_partial
//!
//! These tests verify that the Rust port produces correct results matching
//! the LZ4 specification.

extern crate lz4;

use lz4::{
    compress_bound, compress_dest_size, compress_fast, decompress_safe_partial,
    lz4_compress_default as compress_default, lz4_decompress_safe, LZ4_ACCELERATION_DEFAULT,
    LZ4_ACCELERATION_MAX, LZ4_MAX_INPUT_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: compress_default roundtrip — typical data
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_default_roundtrip_typical_data() {
    // Highly compressible data with repetitions
    let original = b"The quick brown fox jumps over the lazy dog. ".repeat(20);

    let bound = compress_bound(original.len() as i32) as usize;
    let mut compressed = vec![0u8; bound];

    let compressed_size =
        compress_default(&original, &mut compressed).expect("compression should succeed");

    // Compressed should be smaller than original for repetitive data
    assert!(
        compressed_size < original.len(),
        "compressed size {} should be less than original {}",
        compressed_size,
        original.len()
    );

    // Decompress
    let mut decompressed = vec![0u8; original.len()];
    let decompressed_size = lz4_decompress_safe(&compressed[..compressed_size], &mut decompressed)
        .expect("decompression should succeed");

    assert_eq!(decompressed_size, original.len());
    assert_eq!(&decompressed[..decompressed_size], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: compress_default roundtrip — incompressible data
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_default_roundtrip_incompressible_data() {
    // Nearly incompressible: ascending byte values
    let original: Vec<u8> = (0..255).cycle().take(1000).collect();

    let bound = compress_bound(original.len() as i32) as usize;
    let mut compressed = vec![0u8; bound];

    let compressed_size =
        compress_default(&original, &mut compressed).expect("compression should succeed");

    // Incompressible data may expand slightly
    assert!(compressed_size <= bound);

    // Decompress
    let mut decompressed = vec![0u8; original.len()];
    let decompressed_size = lz4_decompress_safe(&compressed[..compressed_size], &mut decompressed)
        .expect("decompression should succeed");

    assert_eq!(decompressed_size, original.len());
    assert_eq!(&decompressed[..decompressed_size], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: compress_bound returns adequate buffer size
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_bound_returns_adequate_size() {
    let input_sizes = [0, 1, 10, 100, 1000, 10000, 100000];

    for &size in &input_sizes {
        let input = vec![0x42u8; size];
        let bound = compress_bound(size as i32);

        if size == 0 {
            // Empty input case
            assert!(bound >= 0);
            continue;
        }

        assert!(
            bound > 0,
            "compress_bound should return positive value for size {}",
            size
        );

        let mut dst = vec![0u8; bound as usize];
        let result = compress_default(&input, &mut dst);

        assert!(
            result.is_ok(),
            "compression with compress_bound buffer should succeed for size {}",
            size
        );
        let compressed_size = result.unwrap();
        assert!(
            compressed_size <= bound as usize,
            "compressed size {} should not exceed bound {} for input size {}",
            compressed_size,
            bound,
            size
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: compress_fast with acceleration=1 roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_fast_acceleration_1_roundtrip() {
    let original = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(10);

    let bound = compress_bound(original.len() as i32) as usize;
    let mut compressed = vec![0u8; bound];

    let compressed_size = compress_fast(&original, &mut compressed, 1)
        .expect("compression with acceleration=1 should succeed");

    assert!(compressed_size < original.len());

    // Decompress
    let mut decompressed = vec![0u8; original.len()];
    let decompressed_size = lz4_decompress_safe(&compressed[..compressed_size], &mut decompressed)
        .expect("decompression should succeed");

    assert_eq!(decompressed_size, original.len());
    assert_eq!(&decompressed[..decompressed_size], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: compress_fast with max acceleration roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_fast_max_acceleration_roundtrip() {
    // Highly repetitive data
    let original = vec![b'A'; 5000];

    let bound = compress_bound(original.len() as i32) as usize;
    let mut compressed = vec![0u8; bound];

    let compressed_size = compress_fast(&original, &mut compressed, LZ4_ACCELERATION_MAX)
        .expect("compression with max acceleration should succeed");

    // Should compress very well
    assert!(
        compressed_size < original.len() / 10,
        "highly repetitive data should compress to < 10% of original"
    );

    // Decompress
    let mut decompressed = vec![0u8; original.len()];
    let decompressed_size = lz4_decompress_safe(&compressed[..compressed_size], &mut decompressed)
        .expect("decompression should succeed");

    assert_eq!(decompressed_size, original.len());
    assert_eq!(&decompressed[..decompressed_size], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: compress_dest_size fills destination exactly
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_dest_size_fills_destination_exactly() {
    // Create less compressible data - random-ish pattern
    let original: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

    let target_dst_size = 64;
    let mut dst = vec![0u8; target_dst_size];

    let result = compress_dest_size(&original, &mut dst);
    assert!(result.is_ok(), "compress_dest_size should succeed");

    let (src_consumed, compressed_size) = result.unwrap();

    // Should have filled dst
    assert!(compressed_size <= target_dst_size);
    assert!(src_consumed > 0, "should have consumed some source bytes");

    // For incompressible data into tiny buffer, we should not consume all input
    if src_consumed == original.len() {
        // If all input was consumed, it must have fit in the output buffer
        assert!(
            compressed_size <= target_dst_size,
            "if all input consumed, compressed size must fit in target"
        );
    }

    // Verify decompression of whatever was compressed
    if src_consumed > 0 && compressed_size > 0 {
        let mut decompressed = vec![0u8; src_consumed + 100]; // extra room
        let decompressed_size = lz4_decompress_safe(&dst[..compressed_size], &mut decompressed)
            .expect("decompression of partial block should succeed");

        assert_eq!(decompressed_size, src_consumed);
        assert_eq!(
            &decompressed[..decompressed_size],
            &original[..src_consumed]
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7: empty input compresses and decompresses
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_empty_input_roundtrip() {
    let original: &[u8] = &[];

    let bound = compress_bound(0);
    assert!(bound >= 0);

    let mut compressed = vec![0u8; (bound + 10) as usize];

    let result = compress_default(original, &mut compressed);
    assert!(result.is_ok(), "compressing empty input should succeed");

    let compressed_size = result.unwrap();

    // Empty input should produce minimal output (just end marker)
    assert!(
        compressed_size <= 16,
        "empty input should produce small output"
    );

    // Decompress
    let mut decompressed = vec![0u8; 100];
    let decompressed_size = lz4_decompress_safe(&compressed[..compressed_size], &mut decompressed)
        .expect("decompressing empty block should succeed");

    assert_eq!(
        decompressed_size, 0,
        "decompressed empty input should yield 0 bytes"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 8: decompress_safe_partial stops early
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decompress_safe_partial_stops_early() {
    // Create data known to decompress to >=100 bytes
    let original = b"0123456789".repeat(15); // 150 bytes

    let bound = compress_bound(original.len() as i32) as usize;
    let mut compressed = vec![0u8; bound];

    let compressed_size =
        compress_default(&original, &mut compressed).expect("compression should succeed");

    // Request only 50 bytes via decompress_safe_partial
    let target_output_size = 50;
    let mut decompressed = vec![0u8; 100]; // buffer larger than target

    let decompressed_size = decompress_safe_partial(
        &compressed[..compressed_size],
        &mut decompressed,
        target_output_size,
    )
    .expect("partial decompression should succeed");

    // Should stop at target_output_size (or slightly before if at boundary)
    assert!(
        decompressed_size <= target_output_size,
        "partial decompress should not exceed target_output_size"
    );
    assert!(decompressed_size > 0, "should decompress some data");

    // Verify the decompressed portion matches the start of original
    assert_eq!(
        &decompressed[..decompressed_size],
        &original[..decompressed_size],
        "partial decompressed data should match original prefix"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Bonus: Verify constants
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lz4_constants_match_spec() {
    // LZ4_MAX_INPUT_SIZE should be 0x7E000000 (2,113,929,216)
    assert_eq!(LZ4_MAX_INPUT_SIZE, 0x7E00_0000);

    // LZ4_ACCELERATION_DEFAULT should be 1
    assert_eq!(LZ4_ACCELERATION_DEFAULT, 1);

    // LZ4_ACCELERATION_MAX should be 65537
    assert_eq!(LZ4_ACCELERATION_MAX, 65537);
}
