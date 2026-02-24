// Tests for frame::compress_frame_to_vec / decompress_frame_to_vec
//
// These exercise the one-shot helpers in src/frame/mod.rs to cover:
//   - Normal round-trip for various data sizes
//   - Empty input
//   - Error path (invalid compressed data)
//   - Stall/no-progress loop exit in decompress_frame_to_vec

use lz4::frame::{compress_frame_to_vec, decompress_frame_to_vec};

#[test]
fn compress_decompress_roundtrip_small() {
    let data = b"hello world, this is a frame roundtrip test!";
    let compressed = compress_frame_to_vec(data);
    assert!(!compressed.is_empty());
    let decompressed = decompress_frame_to_vec(&compressed).unwrap();
    assert_eq!(&decompressed, data);
}

#[test]
fn compress_decompress_roundtrip_empty() {
    let data: &[u8] = &[];
    let compressed = compress_frame_to_vec(data);
    assert!(!compressed.is_empty());
    let decompressed = decompress_frame_to_vec(&compressed).unwrap();
    assert_eq!(decompressed.len(), 0);
}

#[test]
fn compress_decompress_roundtrip_large() {
    let data: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    let compressed = compress_frame_to_vec(&data);
    assert!(!compressed.is_empty());
    assert!(compressed.len() < data.len()); // should compress
    let decompressed = decompress_frame_to_vec(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_frame_to_vec_invalid_data_returns_error() {
    let garbage = b"this is not a valid LZ4 frame";
    let result = decompress_frame_to_vec(garbage);
    assert!(result.is_err());
}

#[test]
fn decompress_frame_to_vec_truncated_frame() {
    let data = b"some data to compress into a frame for truncation test";
    let compressed = compress_frame_to_vec(data);
    // Truncate to just the header (first 7 bytes)
    let truncated = &compressed[..7.min(compressed.len())];
    let result = decompress_frame_to_vec(truncated);
    // Should either error or return partial/empty result
    // The stall-detection logic should kick in when no progress is made
    match result {
        Ok(v) => assert!(v.len() < data.len()),
        Err(_) => {} // error is also acceptable
    }
}

#[test]
fn decompress_frame_to_vec_empty_input() {
    // Empty input should trigger the `pos >= compressed.len()` early exit
    let result = decompress_frame_to_vec(&[]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 0);
}

#[test]
fn compress_frame_to_vec_incompressible_data() {
    // Random-like data: each byte unique enough to prevent compression
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let data: Vec<u8> = (0..512)
        .map(|i| {
            let mut h = DefaultHasher::new();
            i.hash(&mut h);
            (h.finish() & 0xFF) as u8
        })
        .collect();
    let compressed = compress_frame_to_vec(&data);
    assert!(!compressed.is_empty());
    let decompressed = decompress_frame_to_vec(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn compress_decompress_roundtrip_all_zeros() {
    // All-zeros should compress very well
    let data = vec![0u8; 65536];
    let compressed = compress_frame_to_vec(&data);
    assert!(compressed.len() < 1000, "all zeros must compress well");
    let decompressed = decompress_frame_to_vec(&compressed).unwrap();
    assert_eq!(decompressed, data);
}
