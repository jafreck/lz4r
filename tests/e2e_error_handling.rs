//! E2E Test Suite 06: Error Handling & Edge Cases
//!
//! Tests that verify the LZ4 Rust implementation handles errors gracefully,
//! returning proper error types without panicking or causing undefined behavior.
//!
//! Coverage:
//! - Block API error conditions (output too small, corrupt data, invalid parameters)
//! - Frame API error conditions (invalid magic, truncated frames, checksum failures)
//! - Edge cases (empty buffers, zero acceleration, max input size validation)
//! - Partial decompression edge cases

use lz4::{
    lz4_compress_default as compress_default,
    compress_fast,
    lz4_decompress_safe as decompress_safe,
    decompress_safe_partial,
    lz4f_compress_frame,
    lz4f_decompress,
    DecompressError,
    Lz4Error,
    LZ4_MAX_INPUT_SIZE,
};
use lz4::frame::decompress::Lz4FDCtx;
use lz4::frame::types::{Lz4FError, LZ4F_VERSION};

// ═════════════════════════════════════════════════════════════════════════════
// Test 1: decompress_safe with destination too small
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_decompress_dst_too_small() {
    // Compress some data first
    let src = b"Hello, this is a test message for LZ4 compression!";
    let mut compressed = vec![0u8; 1024];
    
    let comp_size = compress_default(src, &mut compressed)
        .expect("compression should succeed");
    
    // Try to decompress into a buffer that's too small
    let mut dst = vec![0u8; 10]; // Much smaller than original (51 bytes)
    
    let result = decompress_safe(&compressed[..comp_size], &mut dst);
    
    // Should return an error, not panic
    assert!(result.is_err(), "decompress_safe should return Err when dst is too small");
    
    match result {
        Err(DecompressError::MalformedInput) => {
            // Expected: LZ4 decompression detects insufficient output space
        }
        other => panic!("Expected Err(MalformedInput), got {:?}", other),
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 2: decompress_safe on corrupt/garbage data
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_decompress_corrupt_data() {
    // Random garbage bytes that are not valid LZ4 compressed data
    let garbage = b"\xDE\xAD\xBE\xEF\xCA\xFE\xBA\xBE\x00\x11\x22\x33\x44\x55\x66\x77";
    let mut dst = vec![0u8; 1024];
    
    let result = decompress_safe(garbage, &mut dst);
    
    // Should return an error, not panic
    assert!(result.is_err(), "decompress_safe should return Err on corrupt data");
    
    match result {
        Err(DecompressError::MalformedInput) => {
            // Expected: corrupt data detected
        }
        other => panic!("Expected Err(MalformedInput), got {:?}", other),
    }
}

#[test]
fn test_decompress_empty_input() {
    let empty: &[u8] = &[];
    let mut dst = vec![0u8; 100];
    
    let result = decompress_safe(empty, &mut dst);
    
    // Empty input should be treated as malformed
    assert!(result.is_err(), "decompress_safe should return Err on empty input");
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 3: compress_default with empty destination
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_compress_dst_empty() {
    let src = b"Some data to compress";
    let mut dst: Vec<u8> = vec![]; // Empty destination
    
    let result = compress_default(src, &mut dst);
    
    // Should return an error, not panic
    assert!(result.is_err(), "compress_default should return Err when dst is empty");
    
    match result {
        Err(Lz4Error::OutputTooSmall) => {
            // Expected error type
        }
        other => panic!("Expected Err(OutputTooSmall), got {:?}", other),
    }
}

#[test]
fn test_compress_dst_too_small() {
    let src = b"This is a longer message that needs more space when compressed with metadata";
    let mut dst = vec![0u8; 5]; // Way too small
    
    let result = compress_default(src, &mut dst);
    
    // Should return an error, not panic
    assert!(result.is_err(), "compress_default should return Err when dst is too small");
    
    match result {
        Err(Lz4Error::OutputTooSmall) => {
            // Expected error type
        }
        other => panic!("Expected Err(OutputTooSmall), got {:?}", other),
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 4: compress_fast with acceleration=0 is handled gracefully
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_compress_fast_zero_acceleration() {
    let src = b"Test data for compression";
    let mut dst = vec![0u8; 1024];
    
    // In the C implementation, acceleration < 1 is clamped to 1
    // The Rust port should handle this gracefully without panicking
    let result = compress_fast(src, &mut dst, 0);
    
    // Should either succeed (if clamped to 1) or return a reasonable error
    match result {
        Ok(_size) => {
            // Acceleration was clamped to 1, compression succeeded
        }
        Err(Lz4Error::OutputTooSmall) | Err(Lz4Error::InputTooLarge) => {
            // Also acceptable error types
        }
    }
    
    // Key assertion: should not panic
}

#[test]
fn test_compress_fast_negative_acceleration() {
    let src = b"Test data";
    let mut dst = vec![0u8; 1024];
    
    // Negative acceleration should also be handled gracefully
    let result = compress_fast(src, &mut dst, -5);
    
    // Should not panic; either succeeds (clamped to 1) or returns error
    match result {
        Ok(_) | Err(_) => {
            // Both outcomes are acceptable as long as no panic
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 5: Frame decompress with truncated frame
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_frame_decompress_truncated() {
    // Create a valid frame first
    let src = b"Frame test data";
    let mut frame_buf = vec![0u8; 1024];
    
    let frame_size = lz4f_compress_frame(&mut frame_buf, src, None)
        .expect("frame compression should succeed");
    
    // Truncate the frame (take only first half)
    let truncated = &frame_buf[..frame_size / 2];
    
    // Try to decompress the truncated frame
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 1024];
    
    let result = lz4f_decompress(
        &mut dctx,
        Some(&mut dst),
        truncated,
        None,
    );
    
    // Should either succeed with hint for more data, or error
    match result {
        Ok((_dst_written, _src_consumed, _hint)) => {
            // If it succeeds, it might be waiting for more data (hint > 0)
            // or detected the truncation will cause issues on next call
            // This is acceptable behavior
        }
        Err(err) => {
            // Expected: some error indicating incomplete/malformed frame
            assert_ne!(err, Lz4FError::OkNoError);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 6: Frame decompress with invalid magic number
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_frame_decompress_invalid_magic() {
    // Invalid magic number (should be 0x184D2204 for LZ4 frame)
    let invalid_frame = b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\x00\x00\x00\x00";
    
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 1024];
    
    let result = lz4f_decompress(
        &mut dctx,
        Some(&mut dst),
        invalid_frame,
        None,
    );
    
    // Should return an error for invalid magic
    assert!(result.is_err(), "lz4f_decompress should return Err for invalid magic");
    
    match result {
        Err(Lz4FError::FrameTypeUnknown) => {
            // Expected error for unknown frame type
        }
        Err(other_err) => {
            // Other errors related to frame parsing are also acceptable
            assert_ne!(other_err, Lz4FError::OkNoError);
        }
        Ok(_) => {
            panic!("Expected error for invalid magic number");
        }
    }
}

#[test]
fn test_frame_decompress_all_zeros() {
    let zeros = [0u8; 32];
    
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 1024];
    
    let result = lz4f_decompress(
        &mut dctx,
        Some(&mut dst),
        &zeros,
        None,
    );
    
    // All zeros is not a valid frame magic, should error
    assert!(result.is_err(), "lz4f_decompress should return Err for all-zero data");
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 7: LZ4_MAX_INPUT_SIZE constant validation
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_max_input_size_constant() {
    // Verify the constant has the correct value: 0x7E000000 = 2,113,929,216
    assert_eq!(LZ4_MAX_INPUT_SIZE, 0x7E00_0000);
    assert_eq!(LZ4_MAX_INPUT_SIZE, 2_113_929_216);
}

#[test]
fn test_compress_input_too_large() {
    // We can't actually allocate 2GB+ of memory in a test, but we can
    // verify the API would reject it by checking compress_bound
    use lz4::compress_bound;
    
    // Input size exceeding LZ4_MAX_INPUT_SIZE
    let oversized = (LZ4_MAX_INPUT_SIZE as i32) + 1;
    
    // compress_bound should return 0 for oversized input
    let bound = compress_bound(oversized);
    assert_eq!(bound, 0, "compress_bound should return 0 for input > LZ4_MAX_INPUT_SIZE");
    
    // Also test exactly at the limit
    let at_limit = LZ4_MAX_INPUT_SIZE as i32;
    let bound_at_limit = compress_bound(at_limit);
    assert!(bound_at_limit > 0, "compress_bound should accept input at exactly LZ4_MAX_INPUT_SIZE");
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 8: decompress_safe_partial edge cases
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_decompress_partial_target_exceeds_dst() {
    // Compress some data
    let src = b"Partial decompression test data";
    let mut compressed = vec![0u8; 1024];
    
    let comp_size = compress_default(src, &mut compressed)
        .expect("compression should succeed");
    
    // Create a small destination buffer
    let mut dst = vec![0u8; 10];
    
    // Request target_output_size > dst.len()
    let target = 20; // More than dst capacity
    
    let result = decompress_safe_partial(
        &compressed[..comp_size],
        &mut dst,
        target,
    );
    
    // Should either:
    // 1. Return Err (safer, detects the issue)
    // 2. Return Ok with size <= dst.len() (safe partial decompression)
    match result {
        Ok(size) => {
            assert!(size <= dst.len(), "returned size should not exceed dst.len()");
        }
        Err(DecompressError::MalformedInput) => {
            // Also acceptable: detected the constraint violation
        }
    }
}

#[test]
fn test_decompress_partial_zero_target() {
    let src = b"Test";
    let mut compressed = vec![0u8; 1024];
    
    let comp_size = compress_default(src, &mut compressed)
        .expect("compression should succeed");
    
    let mut dst = vec![0u8; 100];
    
    // Request zero bytes output
    let result = decompress_safe_partial(&compressed[..comp_size], &mut dst, 0);
    
    // Should handle gracefully - either return Ok(0) or an error
    match result {
        Ok(size) => {
            assert_eq!(size, 0, "zero target should yield zero output");
        }
        Err(_) => {
            // Also acceptable
        }
    }
}

#[test]
fn test_decompress_partial_target_larger_than_original() {
    let src = b"Short";
    let mut compressed = vec![0u8; 1024];
    
    let comp_size = compress_default(src, &mut compressed)
        .expect("compression should succeed");
    
    let mut dst = vec![0u8; 1024];
    
    // Request more bytes than the original data
    let target = 1000;
    
    let result = decompress_safe_partial(&compressed[..comp_size], &mut dst, target);
    
    // Should decompress all available data and not exceed it
    match result {
        Ok(size) => {
            assert!(size <= src.len(), "output should not exceed original size");
            assert_eq!(size, src.len(), "should decompress all available data");
        }
        Err(_) => {
            // Errors are also acceptable if the API doesn't support oversized targets
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 9: Additional edge cases
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_compress_empty_input() {
    let empty: &[u8] = &[];
    let mut dst = vec![0u8; 100];
    
    let result = compress_default(empty, &mut dst);
    
    // Compressing empty data might succeed with minimal output or fail
    // Key: should not panic
    match result {
        Ok(_size) => {
            // Some implementations allow empty compression
        }
        Err(_) => {
            // Also acceptable
        }
    }
}

#[test]
fn test_roundtrip_single_byte() {
    let src = b"X";
    let mut compressed = vec![0u8; 100];
    
    let comp_size = compress_default(src, &mut compressed)
        .expect("compressing single byte should succeed");
    
    let mut decompressed = vec![0u8; 100];
    let decomp_size = decompress_safe(&compressed[..comp_size], &mut decompressed)
        .expect("decompressing single byte should succeed");
    
    assert_eq!(decomp_size, 1);
    assert_eq!(&decompressed[..decomp_size], src);
}

#[test]
fn test_compress_large_repeated_data() {
    // LZ4 should handle highly compressible data efficiently
    let src = vec![b'A'; 10_000]; // 10KB of 'A's
    let mut compressed = vec![0u8; 20_000];
    
    let result = compress_default(&src, &mut compressed);
    
    match result {
        Ok(comp_size) => {
            // Should compress very well (repeated data)
            assert!(comp_size < src.len(), "repeated data should compress well");
            
            // Verify roundtrip
            let mut decompressed = vec![0u8; 10_000];
            let decomp_size = decompress_safe(&compressed[..comp_size], &mut decompressed)
                .expect("decompression should succeed");
            
            assert_eq!(decomp_size, src.len());
            assert_eq!(&decompressed[..decomp_size], &src[..]);
        }
        Err(_) => {
            panic!("compressing 10KB of repeated data should succeed");
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 10: Frame error recovery
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn test_frame_decompress_partial_header() {
    // Provide only part of the frame header (less than minimum 7 bytes)
    let partial_header = b"\x04\x22\x4D\x18"; // First 4 bytes of LZ4 magic
    
    let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
    let mut dst = vec![0u8; 1024];
    
    let result = lz4f_decompress(
        &mut dctx,
        Some(&mut dst),
        partial_header,
        None,
    );
    
    // Should either succeed with hint for more data, or error
    match result {
        Ok((_dst_written, _src_consumed, hint)) => {
            // Hint should indicate more data is needed
            assert!(hint > 0, "should request more data for incomplete header");
        }
        Err(Lz4FError::FrameHeaderIncomplete) => {
            // Expected error for incomplete header
        }
        Err(_) => {
            // Other errors are also acceptable
        }
    }
}
