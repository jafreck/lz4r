//! E2E Test Suite 07: File API
//!
//! Validates the file I/O wrapper API (`file.rs`) that provides streaming
//! compression/decompression using `std::io::Read` and `std::io::Write` traits.
//!
//! Tests use in-memory `std::io::Cursor<Vec<u8>>` for deterministic I/O without
//! requiring actual filesystem access.

use lz4::file::{lz4_read_frame, lz4_write_frame, Lz4ReadFile, Lz4WriteFile};
use std::io::{Cursor, Read, Write};

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: Write then read back via in-memory buffer roundtrip
// ─────────────────────────────────────────────────────────────────────────────

/// Write 4KB data via Lz4WriteFile, then read it back via Lz4ReadFile.
/// Verifies basic roundtrip correctness of the streaming file API.
#[test]
fn test_write_read_4kb_roundtrip() {
    let original: Vec<u8> = b"LZ4 file test data! "
        .iter()
        .cycle()
        .take(4096)
        .cloned()
        .collect();

    // Compress: write to in-memory buffer
    let mut compressed = Vec::new();
    let mut writer = Lz4WriteFile::open(Cursor::new(&mut compressed), None)
        .expect("failed to open writer");
    writer
        .write_all(&original)
        .expect("failed to write data");
    writer.finish().expect("failed to finish write");

    // Decompress: read from in-memory buffer
    let mut reader = Lz4ReadFile::open(Cursor::new(&compressed))
        .expect("failed to open reader");
    let mut recovered = Vec::new();
    reader
        .read_to_end(&mut recovered)
        .expect("failed to read data");

    assert_eq!(recovered, original, "roundtrip mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: Write empty data
// ─────────────────────────────────────────────────────────────────────────────

/// Write zero bytes via Lz4WriteFile, then read back.
/// Ensures that empty input produces a valid empty frame that decompresses to empty.
#[test]
fn test_write_read_empty_data() {
    let original: &[u8] = b"";

    // Compress: write empty slice
    let mut compressed = Vec::new();
    let mut writer = Lz4WriteFile::open(Cursor::new(&mut compressed), None)
        .expect("failed to open writer for empty data");
    writer
        .write_all(original)
        .expect("failed to write empty data");
    writer
        .finish()
        .expect("failed to finish write for empty data");

    // Decompress: read back
    let mut reader = Lz4ReadFile::open(Cursor::new(&compressed))
        .expect("failed to open reader for empty data");
    let mut recovered = Vec::new();
    reader
        .read_to_end(&mut recovered)
        .expect("failed to read empty data");

    assert_eq!(
        recovered.as_slice(),
        original,
        "empty data roundtrip mismatch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: Write multiple chunks
// ─────────────────────────────────────────────────────────────────────────────

/// Write 100 KB of data split into 10 separate write calls (10 KB each).
/// Verifies that the streaming API correctly handles multiple writes.
#[test]
fn test_write_multiple_chunks_read_back() {
    const CHUNK_SIZE: usize = 10 * 1024;
    const NUM_CHUNKS: usize = 10;
    let original: Vec<u8> = (0u8..=255)
        .cycle()
        .take(CHUNK_SIZE * NUM_CHUNKS)
        .collect();

    // Compress: write in 10 KB chunks
    let mut compressed = Vec::new();
    let mut writer = Lz4WriteFile::open(Cursor::new(&mut compressed), None)
        .expect("failed to open writer for multi-chunk");

    for chunk in original.chunks(CHUNK_SIZE) {
        writer
            .write_all(chunk)
            .expect("failed to write chunk");
    }
    writer
        .finish()
        .expect("failed to finish multi-chunk write");

    // Decompress: read all back at once
    let mut reader = Lz4ReadFile::open(Cursor::new(&compressed))
        .expect("failed to open reader for multi-chunk");
    let mut recovered = Vec::new();
    reader
        .read_to_end(&mut recovered)
        .expect("failed to read multi-chunk data");

    assert_eq!(recovered, original, "multi-chunk roundtrip mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: Read from pre-compressed frame data
// ─────────────────────────────────────────────────────────────────────────────

/// Compress data using the one-shot `lz4_write_frame` function, then read it
/// back using `Lz4ReadFile` to verify compatibility between the convenience API
/// and the streaming API.
#[test]
fn test_read_from_precompressed_frame() {
    let original = b"Pre-compressed test data using lz4_write_frame!".repeat(50);

    // Use convenience function to produce a complete frame
    let compressed = lz4_write_frame(&original, Vec::new())
        .expect("failed to compress frame");

    // Read back using streaming reader
    let mut reader = Lz4ReadFile::open(Cursor::new(&compressed))
        .expect("failed to open reader for pre-compressed frame");
    let mut recovered = Vec::new();
    reader
        .read_to_end(&mut recovered)
        .expect("failed to read pre-compressed frame");

    assert_eq!(recovered, original, "pre-compressed frame mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: Read with small buffer (multi-read loop)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress 50 KB of data by reading in 4 KB chunks repeatedly.
/// Verifies that the reader handles small output buffers gracefully and
/// accumulates data correctly across multiple read calls.
#[test]
fn test_read_with_small_buffer_loop() {
    const ORIGINAL_SIZE: usize = 50 * 1024;
    const READ_BUFFER_SIZE: usize = 4 * 1024;

    let original: Vec<u8> = b"Repeated data for multi-read test. "
        .iter()
        .cycle()
        .take(ORIGINAL_SIZE)
        .cloned()
        .collect();

    // Compress
    let compressed = lz4_write_frame(&original, Vec::new())
        .expect("failed to compress for multi-read test");

    // Decompress with 4 KB reads in a loop
    let mut reader = Lz4ReadFile::open(Cursor::new(&compressed))
        .expect("failed to open reader for multi-read test");
    let mut recovered = Vec::new();
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        let n = reader
            .read(&mut buffer)
            .expect("read failed during multi-read loop");
        if n == 0 {
            break; // EOF
        }
        recovered.extend_from_slice(&buffer[..n]);
    }

    assert_eq!(recovered, original, "multi-read loop mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: Write to failing writer propagates error
// ─────────────────────────────────────────────────────────────────────────────

/// A mock writer that fails after the first (header) write.
struct FailingWriter {
    first_write: bool,
}

impl FailingWriter {
    fn new(_fail_after: usize) -> Self {
        Self { first_write: true }
    }
}

impl Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Allow the first write (frame header) to succeed
        if self.first_write {
            self.first_write = false;
            return Ok(buf.len());
        }
        
        // Fail all subsequent writes
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "intentional write failure",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Attempt to write data through Lz4WriteFile to a writer that fails after limited bytes.
/// Verifies that I/O errors are correctly propagate (not panicked).
#[test]
fn test_write_to_failing_writer_propagates_error() {
    // Create large incompressible data to ensure compressed output > 0
    let mut original = Vec::with_capacity(100_000);
    for i in 0..100_000 {
        original.push((i % 256) as u8);
    }

    // Create a writer that allows header but fails on data write
    let failing_writer = FailingWriter::new(50);
    let mut lz4_writer = Lz4WriteFile::open(failing_writer, None)
        .expect("failed to open writer with failing backend");

    // Attempt to write large data — should hit the failure during compressed data write
    let result = lz4_writer.write_all(&original);

    // Expect an error (not panic)
    assert!(
        result.is_err(),
        "expected write_all to propagate error from failing writer"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional tests: convenience functions lz4_write_frame / lz4_read_frame
// ─────────────────────────────────────────────────────────────────────────────

/// Test the one-shot convenience functions for basic roundtrip.
#[test]
fn test_convenience_functions_roundtrip() {
    let original = b"One-shot convenience API test.".repeat(200);

    // Compress
    let compressed = lz4_write_frame(&original, Vec::new())
        .expect("lz4_write_frame failed");

    // Decompress
    let mut recovered = Vec::new();
    lz4_read_frame(Cursor::new(&compressed), &mut recovered)
        .expect("lz4_read_frame failed");

    assert_eq!(recovered, original, "convenience functions roundtrip mismatch");
}
