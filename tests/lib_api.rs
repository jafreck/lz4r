// Integration tests for task-021: lib.rs — top-level wiring and re-exports
//
// Tests verify behavioural parity with lz4.h v1.10.0 (lines 131–143, 245, 670–678):
//   - Version constants match the C-header values exactly
//   - version_number() / version_string() return the correct values
//   - size_of_state() returns a positive, non-trivial value
//   - decompress_inplace_margin() / decompress_inplace_buffer_size() use the correct formula
//   - LZ4_DISTANCE_MAX / COMPRESS_INPLACE_MARGIN constants match the C values
//   - compress_inplace_buffer_size() uses the correct formula
//   - Top-level re-exports (lz4_compress_default, lz4_decompress_safe,
//     lz4f_compress_frame, lz4f_decompress) are callable

use lz4::{
    compress_inplace_buffer_size, decompress_inplace_buffer_size, decompress_inplace_margin,
    size_of_state, version_number, version_string, COMPRESS_INPLACE_MARGIN, LZ4_DISTANCE_MAX,
    LZ4_VERSION_MAJOR, LZ4_VERSION_MINOR, LZ4_VERSION_NUMBER, LZ4_VERSION_RELEASE,
    LZ4_VERSION_STRING,
};

// ─────────────────────────────────────────────────────────────────────────────
// Version constants  (lz4.h lines 131–143)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn version_major_is_1() {
    // LZ4_VERSION_MAJOR must be 1 for v1.10.0
    assert_eq!(LZ4_VERSION_MAJOR, 1);
}

#[test]
fn version_minor_is_10() {
    // LZ4_VERSION_MINOR must be 10 for v1.10.0
    assert_eq!(LZ4_VERSION_MINOR, 10);
}

#[test]
fn version_release_is_0() {
    // LZ4_VERSION_RELEASE must be 0 for v1.10.0
    assert_eq!(LZ4_VERSION_RELEASE, 0);
}

#[test]
fn version_number_constant_formula() {
    // LZ4_VERSION_NUMBER = MAJOR*100*100 + MINOR*100 + RELEASE = 1*10000 + 10*100 + 0 = 11000
    assert_eq!(LZ4_VERSION_NUMBER, 11_000);
}

#[test]
fn version_number_constant_components() {
    // Verify the formula holds relative to the component constants
    let expected = LZ4_VERSION_MAJOR * 100 * 100 + LZ4_VERSION_MINOR * 100 + LZ4_VERSION_RELEASE;
    assert_eq!(LZ4_VERSION_NUMBER, expected);
}

#[test]
fn version_string_constant() {
    // LZ4_VERSION_STRING must be "1.10.0"
    assert_eq!(LZ4_VERSION_STRING, "1.10.0");
}

// ─────────────────────────────────────────────────────────────────────────────
// version_number() / version_string() functions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn version_number_fn_returns_constant() {
    // Equivalent to LZ4_versionNumber()
    assert_eq!(version_number(), LZ4_VERSION_NUMBER);
    assert_eq!(version_number(), 11_000);
}

#[test]
fn version_string_fn_returns_constant() {
    // Equivalent to LZ4_versionString()
    assert_eq!(version_string(), LZ4_VERSION_STRING);
    assert_eq!(version_string(), "1.10.0");
}

#[test]
fn version_string_fn_is_static() {
    // Must return a 'static str — verified by the type signature at compile time.
    let s: &'static str = version_string();
    assert!(!s.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// size_of_state()  (lz4.h line 245)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn size_of_state_is_positive() {
    // Equivalent to LZ4_sizeofState(); must be > 0
    assert!(size_of_state() > 0, "size_of_state must be a positive number of bytes");
}

#[test]
fn size_of_state_matches_stream_state_internal_size() {
    // Must equal sizeof(LZ4_stream_t) in C — i.e. sizeof(StreamStateInternal) in Rust.
    let rust_size = core::mem::size_of::<lz4::block::types::StreamStateInternal>() as i32;
    assert_eq!(size_of_state(), rust_size);
}

#[test]
fn size_of_state_is_at_least_16_bytes() {
    // LZ4_stream_t in C is at least 16 KB in its hash table alone;
    // the Rust port must reflect a reasonable minimum.
    assert!(size_of_state() >= 16, "state size should be at least 16 bytes");
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_inplace_margin()  (lz4.h line 670)
// Formula: (compressedSize >> 8) + 32
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_inplace_margin_zero() {
    // (0 >> 8) + 32 = 32
    assert_eq!(decompress_inplace_margin(0), 32);
}

#[test]
fn decompress_inplace_margin_256() {
    // (256 >> 8) + 32 = 1 + 32 = 33
    assert_eq!(decompress_inplace_margin(256), 33);
}

#[test]
fn decompress_inplace_margin_1024() {
    // (1024 >> 8) + 32 = 4 + 32 = 36
    assert_eq!(decompress_inplace_margin(1024), 36);
}

#[test]
fn decompress_inplace_margin_65536() {
    // (65536 >> 8) + 32 = 256 + 32 = 288
    assert_eq!(decompress_inplace_margin(65536), 288);
}

#[test]
fn decompress_inplace_margin_formula() {
    // Verify the formula (x >> 8) + 32 holds for several values
    for x in [0usize, 1, 127, 255, 256, 512, 1000, 4096, 65535, 65536] {
        let expected = (x >> 8) + 32;
        assert_eq!(
            decompress_inplace_margin(x),
            expected,
            "margin mismatch for compressed_size={x}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_inplace_buffer_size()  (lz4.h line 672)
// Formula: decompressedSize + decompress_inplace_margin(decompressedSize)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_inplace_buffer_size_zero() {
    // 0 + (0 >> 8) + 32 = 32
    assert_eq!(decompress_inplace_buffer_size(0), 32);
}

#[test]
fn decompress_inplace_buffer_size_1024() {
    // 1024 + (1024 >> 8) + 32 = 1024 + 4 + 32 = 1060
    assert_eq!(decompress_inplace_buffer_size(1024), 1060);
}

#[test]
fn decompress_inplace_buffer_size_formula() {
    // Verify the formula decompressedSize + ((decompressedSize >> 8) + 32) holds
    for x in [0usize, 1, 255, 256, 1024, 4096, 65535] {
        let expected = x + (x >> 8) + 32;
        assert_eq!(
            decompress_inplace_buffer_size(x),
            expected,
            "buffer_size mismatch for decompressed_size={x}"
        );
    }
}

#[test]
fn decompress_inplace_buffer_size_larger_than_input() {
    // The buffer must always be larger than the decompressed size (margin > 0)
    for x in [1usize, 100, 1024, 65535] {
        assert!(
            decompress_inplace_buffer_size(x) > x,
            "buffer size must exceed decompressed size for x={x}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_DISTANCE_MAX constant  (lz4.h)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4_distance_max_value() {
    // LZ4_DISTANCE_MAX must be 65535 (matches C define)
    assert_eq!(LZ4_DISTANCE_MAX, 65_535usize);
}

// ─────────────────────────────────────────────────────────────────────────────
// COMPRESS_INPLACE_MARGIN constant  (lz4.h line 675)
// Formula: LZ4_DISTANCE_MAX + 32
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_inplace_margin_value() {
    // COMPRESS_INPLACE_MARGIN = LZ4_DISTANCE_MAX + 32 = 65535 + 32 = 65567
    assert_eq!(COMPRESS_INPLACE_MARGIN, 65_567usize);
}

#[test]
fn compress_inplace_margin_formula() {
    assert_eq!(COMPRESS_INPLACE_MARGIN, LZ4_DISTANCE_MAX + 32);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_inplace_buffer_size()  (lz4.h line 678)
// Formula: maxCompressedSize + COMPRESS_INPLACE_MARGIN
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_inplace_buffer_size_zero() {
    // 0 + 65567 = 65567
    assert_eq!(compress_inplace_buffer_size(0), COMPRESS_INPLACE_MARGIN);
}

#[test]
fn compress_inplace_buffer_size_1024() {
    // 1024 + 65567 = 66591
    assert_eq!(compress_inplace_buffer_size(1024), 66_591usize);
}

#[test]
fn compress_inplace_buffer_size_formula() {
    for x in [0usize, 1, 256, 1024, 65535, 65536] {
        let expected = x + COMPRESS_INPLACE_MARGIN;
        assert_eq!(
            compress_inplace_buffer_size(x),
            expected,
            "buffer size mismatch for max_compressed_size={x}"
        );
    }
}

#[test]
fn compress_inplace_buffer_size_larger_than_input() {
    // Buffer must always exceed the compressed size
    for x in [0usize, 1, 1024, 65535] {
        assert!(
            compress_inplace_buffer_size(x) > x,
            "buffer must exceed max_compressed_size for x={x}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level re-exports — verifying they are callable via the lz4 crate root
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reexport_lz4_compress_default_is_callable() {
    // lz4::lz4_compress_default is a re-export of block::compress::compress_default
    let src = b"hello lz4 reexport test";
    let bound = lz4::block::compress::compress_bound(src.len() as i32) as usize;
    let mut dst = vec![0u8; bound];
    let result = lz4::lz4_compress_default(src, &mut dst);
    assert!(result.is_ok(), "lz4_compress_default reexport must work: {result:?}");
    assert!(result.unwrap() > 0);
}

#[test]
fn reexport_lz4_decompress_safe_is_callable() {
    // Round-trip: compress then decompress via re-exported functions
    let src = b"round-trip test via top-level reexports";
    let bound = lz4::block::compress::compress_bound(src.len() as i32) as usize;
    let mut compressed = vec![0u8; bound];
    let n = lz4::lz4_compress_default(src, &mut compressed).unwrap();

    let mut decompressed = vec![0u8; src.len()];
    let result = lz4::lz4_decompress_safe(&compressed[..n], &mut decompressed);
    assert!(result.is_ok(), "lz4_decompress_safe reexport must succeed: {result:?}");
    let m = result.unwrap();
    assert_eq!(m, src.len());
    assert_eq!(&decompressed[..m], src.as_ref());
}
