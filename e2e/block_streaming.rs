//! E2E Test Suite 02: Block Streaming API
//!
//! Tests the streaming block compression/decompression API (Lz4Stream / Lz4StreamDecode).
//! Validates that the Rust port correctly handles:
//! - Single-chunk and multi-chunk streaming compression
//! - Stream reset between messages
//! - Streaming decompression contexts
//! - Dictionary usage with streaming
//! - Ring buffer size calculation

use lz4::block::decompress_api::{
    decoder_ring_buffer_size, decompress_safe, decompress_safe_continue, set_stream_decode,
    Lz4StreamDecode,
};
use lz4::block::stream::Lz4Stream;

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: Single-chunk streaming compress + decompress
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_single_chunk_streaming() {
    // 1 KB of repetitive data
    let src: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
        .iter()
        .copied()
        .cycle()
        .take(1024)
        .collect();

    let mut stream = Lz4Stream::new();
    let mut compressed = vec![0u8; lz4::compress_bound(src.len() as i32) as usize];

    // Compress with streaming API
    let compressed_size =
        stream.compress_fast_continue(&src, &mut compressed, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(
        compressed_size > 0,
        "Single-chunk streaming compression failed"
    );
    compressed.truncate(compressed_size as usize);

    // Decompress with standard one-shot API
    let mut decompressed = vec![0u8; src.len()];
    let decompressed_size =
        decompress_safe(&compressed, &mut decompressed).expect("Single-chunk decompression failed");
    assert_eq!(decompressed_size, src.len());
    assert_eq!(&decompressed[..], &src[..], "Data mismatch after roundtrip");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: Multi-chunk streaming compress + decompress
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_multi_chunk_streaming() {
    // 4 KB input split into 1 KB chunks
    let full_input: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    let chunks: Vec<&[u8]> = full_input.chunks(1024).collect();

    let mut stream = Lz4Stream::new();
    let mut compressed_blocks = Vec::new();

    // Compress each chunk with streaming API
    for chunk in &chunks {
        let mut compressed = vec![0u8; lz4::compress_bound(chunk.len() as i32) as usize];
        let compressed_size =
            stream.compress_fast_continue(chunk, &mut compressed, lz4::LZ4_ACCELERATION_DEFAULT);
        assert!(
            compressed_size > 0,
            "Multi-chunk compression failed for chunk"
        );
        compressed.truncate(compressed_size as usize);
        compressed_blocks.push(compressed);
    }

    // For streaming compression, we need streaming decompression
    // Create a contiguous output buffer and use streaming decode context
    let mut decode_ctx = Lz4StreamDecode::new();
    unsafe {
        set_stream_decode(&mut decode_ctx, &[]);
    }

    let mut output_buffer = vec![0u8; 8192]; // Extra space for safety
    let mut write_pos = 0;

    for (i, block) in compressed_blocks.iter().enumerate() {
        let chunk_size = 1024;
        let size = unsafe {
            decompress_safe_continue(
                &mut decode_ctx,
                block.as_ptr(),
                output_buffer[write_pos..].as_mut_ptr(),
                block.len(),
                chunk_size,
            )
        }
        .unwrap_or_else(|_| panic!("Streaming decompression failed for block {}", i));

        write_pos += size;
    }

    assert_eq!(
        write_pos,
        full_input.len(),
        "Total decompressed size mismatch"
    );
    assert_eq!(
        &output_buffer[..write_pos],
        &full_input[..],
        "Multi-chunk roundtrip data mismatch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: Stream reset between messages
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_stream_reset_between_messages() {
    let message1 = b"First message: some data here";
    let message2 = b"Second message: completely different data";

    let mut stream = Lz4Stream::new();

    // Compress first message
    let mut compressed1 = vec![0u8; lz4::compress_bound(message1.len() as i32) as usize];
    let size1 =
        stream.compress_fast_continue(message1, &mut compressed1, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(size1 > 0);
    compressed1.truncate(size1 as usize);

    // Reset stream
    stream.reset();

    // Compress second message
    let mut compressed2 = vec![0u8; lz4::compress_bound(message2.len() as i32) as usize];
    let size2 =
        stream.compress_fast_continue(message2, &mut compressed2, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(size2 > 0);
    compressed2.truncate(size2 as usize);

    // Decompress each independently
    let mut decompressed1 = vec![0u8; message1.len()];
    decompress_safe(&compressed1, &mut decompressed1).expect("Failed to decompress message1");
    assert_eq!(&decompressed1[..], &message1[..], "First message mismatch");

    let mut decompressed2 = vec![0u8; message2.len()];
    decompress_safe(&compressed2, &mut decompressed2).expect("Failed to decompress message2");
    assert_eq!(&decompressed2[..], &message2[..], "Second message mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: Lz4StreamDecode: set_stream_decode with no dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_set_stream_decode_no_dict() {
    let src = b"Test data for streaming decompression without dictionary";

    // Compress with default API
    let mut compressed = vec![0u8; lz4::compress_bound(src.len() as i32) as usize];
    let compressed_size =
        lz4::lz4_compress_default(src, &mut compressed).expect("Compression failed");
    compressed.truncate(compressed_size);

    // Create streaming decode context with no dictionary
    let mut decode_ctx = Lz4StreamDecode::new();
    unsafe {
        set_stream_decode(&mut decode_ctx, &[]);
    }

    // Decompress using streaming API
    let mut decompressed = vec![0u8; src.len()];
    let size = unsafe {
        decompress_safe_continue(
            &mut decode_ctx,
            compressed.as_ptr(),
            decompressed.as_mut_ptr(),
            compressed.len(),
            decompressed.len(),
        )
    }
    .expect("Streaming decompression failed");

    assert_eq!(size, src.len());
    assert_eq!(
        &decompressed[..],
        &src[..],
        "Streaming decode with no dict: data mismatch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: Full streaming compress → streaming decompress roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_full_streaming_roundtrip() {
    // 8 KB text split into 2 KB chunks
    let text: Vec<u8> = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. "
        .iter()
        .copied()
        .cycle()
        .take(8192)
        .collect();
    let chunks: Vec<&[u8]> = text.chunks(2048).collect();

    // Compress each chunk with streaming context
    let mut compress_stream = Lz4Stream::new();
    let mut compressed_blocks = Vec::new();

    for chunk in &chunks {
        let mut compressed = vec![0u8; lz4::compress_bound(chunk.len() as i32) as usize];
        let size = compress_stream.compress_fast_continue(
            chunk,
            &mut compressed,
            lz4::LZ4_ACCELERATION_DEFAULT,
        );
        assert!(size > 0, "Compression failed for chunk");
        compressed.truncate(size as usize);
        compressed_blocks.push(compressed);
    }

    // Decompress each chunk with streaming context
    let mut decode_ctx = Lz4StreamDecode::new();
    unsafe {
        set_stream_decode(&mut decode_ctx, &[]);
    }

    // We need a contiguous buffer for streaming decompression to work correctly
    let mut output_buffer = vec![0u8; 8192 * 2]; // Extra space for safety
    let mut write_pos = 0;

    for (i, block) in compressed_blocks.iter().enumerate() {
        let chunk_size = 2048;
        let size = unsafe {
            decompress_safe_continue(
                &mut decode_ctx,
                block.as_ptr(),
                output_buffer[write_pos..].as_mut_ptr(),
                block.len(),
                chunk_size,
            )
        }
        .unwrap_or_else(|_| panic!("Streaming decompression failed for block {}", i));

        assert!(
            size <= chunk_size,
            "Decompressed size {} exceeds chunk size {}",
            size,
            chunk_size
        );
        write_pos += size;
    }

    assert_eq!(write_pos, text.len(), "Total decompressed size mismatch");
    assert_eq!(
        &output_buffer[..write_pos],
        &text[..],
        "Full streaming roundtrip data mismatch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: decoder_ring_buffer_size returns positive value
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decoder_ring_buffer_size() {
    let max_block_size = 65536;
    let ring_buffer_size = decoder_ring_buffer_size(max_block_size)
        .expect("decoder_ring_buffer_size returned None for valid input");

    assert!(
        ring_buffer_size > max_block_size,
        "Ring buffer size {} must be larger than max block size {}",
        ring_buffer_size,
        max_block_size
    );

    // According to LZ4 spec, ring buffer size should be at least 65536 + 14 + block_size
    let expected_min = 65536 + 14 + max_block_size;
    assert!(
        ring_buffer_size >= expected_min,
        "Ring buffer size {} is less than expected minimum {}",
        ring_buffer_size,
        expected_min
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7: decompress_safe_using_dict roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decompress_safe_using_dict() {
    // Dictionary for compression
    let dictionary = b"Common prefix data that appears in many messages. The quick brown fox jumps over the lazy dog.";

    let src =
        b"Common prefix data that appears in many messages. This is the actual message content.";

    // Create a streaming context and load the dictionary
    let mut stream = Lz4Stream::new();
    let dict_loaded = stream.load_dict(dictionary);
    assert!(dict_loaded > 0, "Dictionary loading failed");

    // Compress with dictionary
    let mut compressed = vec![0u8; lz4::compress_bound(src.len() as i32) as usize];
    let compressed_size =
        stream.compress_fast_continue(src, &mut compressed, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(compressed_size > 0, "Compression with dict failed");
    compressed.truncate(compressed_size as usize);

    // Decompress with dictionary using the unsafe API
    let mut decompressed = vec![0u8; src.len()];
    let size = unsafe {
        lz4::decompress_safe_using_dict(
            compressed.as_ptr(),
            decompressed.as_mut_ptr(),
            compressed.len(),
            decompressed.len(),
            dictionary.as_ptr(),
            dictionary.len(),
        )
    }
    .expect("Decompression with dict failed");

    assert_eq!(size, src.len());
    assert_eq!(
        &decompressed[..],
        &src[..],
        "Dictionary roundtrip data mismatch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional edge case: Empty input streaming
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_streaming_empty_input() {
    let empty: &[u8] = &[];
    let mut stream = Lz4Stream::new();
    let mut compressed = vec![0u8; lz4::compress_bound(1) as usize];

    let size = stream.compress_fast_continue(empty, &mut compressed, lz4::LZ4_ACCELERATION_DEFAULT);

    // LZ4 should handle empty input gracefully
    // Either return 0 or produce a minimal valid block
    assert!(size >= 0, "Empty input should not produce negative size");
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional test: Stream reset_fast
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_stream_reset_fast() {
    let data = b"Test data for reset_fast validation";

    let mut stream = Lz4Stream::new();

    // First compression
    let mut compressed1 = vec![0u8; lz4::compress_bound(data.len() as i32) as usize];
    let size1 =
        stream.compress_fast_continue(data, &mut compressed1, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(size1 > 0);
    compressed1.truncate(size1 as usize);

    // Fast reset
    stream.reset_fast();

    // Second compression after fast reset
    let mut compressed2 = vec![0u8; lz4::compress_bound(data.len() as i32) as usize];
    let size2 =
        stream.compress_fast_continue(data, &mut compressed2, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(size2 > 0);
    compressed2.truncate(size2 as usize);

    // Both should decompress correctly
    let mut decompressed1 = vec![0u8; data.len()];
    decompress_safe(&compressed1, &mut decompressed1).expect("Decompression 1 failed");
    assert_eq!(&decompressed1[..], &data[..]);

    let mut decompressed2 = vec![0u8; data.len()];
    decompress_safe(&compressed2, &mut decompressed2).expect("Decompression 2 failed");
    assert_eq!(&decompressed2[..], &data[..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional test: save_dict functionality
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_save_dict() {
    let data1 = b"First block of data with some repetitive content. ";
    let data2 = b"Second block that may reference the first block content. ";

    let mut stream = Lz4Stream::new();

    // Compress first block
    let mut compressed1 = vec![0u8; lz4::compress_bound(data1.len() as i32) as usize];
    let size1 =
        stream.compress_fast_continue(data1, &mut compressed1, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(size1 > 0);
    compressed1.truncate(size1 as usize);

    // Save dictionary
    let mut saved_dict = vec![0u8; 64 * 1024];
    let dict_size = stream.save_dict(&mut saved_dict);
    assert!(dict_size >= 0, "save_dict failed");
    saved_dict.truncate(dict_size as usize);

    // Compress second block (should still work after save_dict)
    let mut compressed2 = vec![0u8; lz4::compress_bound(data2.len() as i32) as usize];
    let size2 =
        stream.compress_fast_continue(data2, &mut compressed2, lz4::LZ4_ACCELERATION_DEFAULT);
    assert!(size2 > 0);
    compressed2.truncate(size2 as usize);

    // For streaming compression, use streaming decompression with the saved dict
    let mut decode_ctx = Lz4StreamDecode::new();

    // Set up decoder with no initial dict
    unsafe {
        set_stream_decode(&mut decode_ctx, &[]);
    }

    // Create a contiguous buffer for both decompressed blocks
    let mut output_buffer = vec![0u8; 4096];
    let mut write_pos = 0;

    // Decompress first block
    let size = unsafe {
        decompress_safe_continue(
            &mut decode_ctx,
            compressed1.as_ptr(),
            output_buffer[write_pos..].as_mut_ptr(),
            compressed1.len(),
            data1.len(),
        )
    }
    .expect("Decompression 1 failed");
    assert_eq!(&output_buffer[write_pos..write_pos + size], &data1[..]);
    write_pos += size;

    // Decompress second block (continues from first)
    let size = unsafe {
        decompress_safe_continue(
            &mut decode_ctx,
            compressed2.as_ptr(),
            output_buffer[write_pos..].as_mut_ptr(),
            compressed2.len(),
            data2.len(),
        )
    }
    .expect("Decompression 2 failed");
    assert_eq!(&output_buffer[write_pos..write_pos + size], &data2[..]);
}
