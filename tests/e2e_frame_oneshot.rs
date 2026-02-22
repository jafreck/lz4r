//! E2E Test Suite 04: Frame One-Shot API
//!
//! Validates the LZ4 Frame format one-shot compress/decompress path:
//! - lz4f_compress_frame
//! - lz4f_decompress (with context)
//! - Frame magic number verification
//! - Roundtrip correctness for various data sizes
//! - Content checksum support
//! - Error handling for small buffers
//!
//! These tests verify that the Rust port correctly implements the LZ4 Frame
//! format specification (one-shot API).

extern crate lz4;

use lz4::frame::{
    header::lz4f_compress_frame_bound,
    lz4f_compress_frame,
    lz4f_decompress,
    Lz4FDCtx,
    Preferences,
    ContentChecksum,
};

/// LZ4 frame magic number as defined by the specification.
const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4D, 0x18];

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: lz4f_compress_frame roundtrip — default preferences
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_frame_roundtrip_default_prefs() {
    // 4 KB ASCII text
    let original = b"The LZ4 frame format provides framing, checksums, and metadata. ".repeat(64);
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, &original, None)
        .expect("frame compression should succeed");
    
    assert!(compressed_size > 0, "compressed size should be positive");
    assert!(compressed_size <= frame_bound, "compressed size should not exceed bound");
    
    // Decompress using frame decompression context
    let mut dctx = Lz4FDCtx::new(100);
    let mut decompressed = vec![0u8; original.len() + 1024]; // extra room
    let mut total_decompressed = 0;
    let mut src_pos = 0;
    
    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..compressed_size],
            None,
        ).expect("frame decompression should succeed");
        
        src_pos += src_read;
        total_decompressed += dst_written;
        
        if src_pos >= compressed_size {
            break;
        }
    }
    
    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: lz4f_compress_frame output starts with LZ4 magic bytes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_frame_magic_bytes() {
    let original = b"Testing magic number";
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, original, None)
        .expect("frame compression should succeed");
    
    assert!(compressed_size >= 4, "compressed frame must be at least 4 bytes for magic");
    
    // Verify magic number [0x04, 0x22, 0x4D, 0x18]
    assert_eq!(
        &compressed[..4],
        &LZ4_FRAME_MAGIC,
        "frame must start with LZ4 magic number"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: lz4f_compress_frame roundtrip — small input (<64 bytes)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_frame_roundtrip_small_input() {
    let original = b"Short ASCII string for testing";
    assert!(original.len() < 64, "test input should be < 64 bytes");
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, original, None)
        .expect("frame compression should succeed");
    
    // Decompress
    let mut dctx = Lz4FDCtx::new(100);
    let mut decompressed = vec![0u8; original.len() + 256];
    let mut total_decompressed = 0;
    let mut src_pos = 0;
    
    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..compressed_size],
            None,
        ).expect("frame decompression should succeed");
        
        src_pos += src_read;
        total_decompressed += dst_written;
        
        if src_pos >= compressed_size {
            break;
        }
    }
    
    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: lz4f_compress_frame roundtrip — large input (>1 MB)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_frame_roundtrip_large_input() {
    // 1.5 MB of repeated pattern
    let pattern = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let original: Vec<u8> = pattern.iter()
        .cycle()
        .take(1_500_000)
        .copied()
        .collect();
    assert!(original.len() > 1_000_000, "test input should be > 1 MB");
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, &original, None)
        .expect("frame compression should succeed");
    
    // Decompress
    let mut dctx = Lz4FDCtx::new(100);
    let mut decompressed = vec![0u8; original.len() + 4096];
    let mut total_decompressed = 0;
    let mut src_pos = 0;
    
    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..compressed_size],
            None,
        ).expect("frame decompression should succeed");
        
        src_pos += src_read;
        total_decompressed += dst_written;
        
        if src_pos >= compressed_size {
            break;
        }
    }
    
    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: lz4f_compress_frame with content checksum enabled
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_frame_with_content_checksum() {
    let original = b"Data with content checksum verification. ".repeat(50);
    
    let mut prefs = Preferences::default();
    prefs.frame_info.content_checksum_flag = ContentChecksum::Enabled;
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), Some(&prefs));
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, &original, Some(&prefs))
        .expect("frame compression with checksum should succeed");
    
    // Decompress — checksum is verified internally
    let mut dctx = Lz4FDCtx::new(100);
    let mut decompressed = vec![0u8; original.len() + 1024];
    let mut total_decompressed = 0;
    let mut src_pos = 0;
    
    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..compressed_size],
            None,
        ).expect("frame decompression should succeed and verify checksum");
        
        src_pos += src_read;
        total_decompressed += dst_written;
        
        if src_pos >= compressed_size {
            break;
        }
    }
    
    assert_eq!(total_decompressed, original.len());
    assert_eq!(&decompressed[..total_decompressed], &original[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: lz4f_compress_frame empty input
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_frame_empty_input() {
    let original: &[u8] = &[];
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, original, None)
        .expect("frame compression of empty input should succeed");
    
    // Even empty input produces a frame with header
    assert!(compressed_size > 0, "empty frame should still have header");
    
    // Decompress
    let mut dctx = Lz4FDCtx::new(100);
    let mut decompressed = vec![0u8; 256];
    let mut total_decompressed = 0;
    let mut src_pos = 0;
    
    loop {
        let (src_read, dst_written, _hint) = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed[total_decompressed..]),
            &compressed[src_pos..compressed_size],
            None,
        ).expect("frame decompression should succeed");
        
        src_pos += src_read;
        total_decompressed += dst_written;
        
        if src_pos >= compressed_size {
            break;
        }
    }
    
    assert_eq!(total_decompressed, 0, "decompressed empty frame should be empty");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7: Frame decompress with small dst buffer does not panic
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decompress_with_small_buffer_no_panic() {
    // Compress 1 KB of data
    let original = b"X".repeat(1024);
    
    let frame_bound = lz4f_compress_frame_bound(original.len(), None);
    let mut compressed = vec![0u8; frame_bound];
    
    let compressed_size = lz4f_compress_frame(&mut compressed, &original, None)
        .expect("frame compression should succeed");
    
    // Decompress with a very small buffer (100 bytes)
    let mut dctx = Lz4FDCtx::new(100);
    let mut decompressed = vec![0u8; 100];
    let mut all_decompressed = Vec::new();
    let mut src_pos = 0;
    
    // Loop through decompression, accumulating output
    loop {
        let result = lz4f_decompress(
            &mut dctx,
            Some(&mut decompressed),
            &compressed[src_pos..compressed_size],
            None,
        );
        
        match result {
            Ok((src_read, dst_written, _hint)) => {
                src_pos += src_read;
                all_decompressed.extend_from_slice(&decompressed[..dst_written]);
                
                if src_pos >= compressed_size && dst_written == 0 {
                    break;
                }
            }
            Err(_e) => {
                // Small buffer may cause errors or partial reads
                // The key is: should not panic
                // If error occurs, ensure we made progress or can stop gracefully
                break;
            }
        }
        
        if src_pos >= compressed_size {
            break;
        }
    }
    
    // The test passes if we didn't panic. 
    // We may not get all data with such a small buffer, but no panic is required.
    // In most implementations, the state machine should handle this gracefully.
}
