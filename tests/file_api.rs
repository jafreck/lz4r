// Unit tests for task-020: src/file.rs — LZ4 file-level streaming I/O
//
// Verifies behavioural parity with lz4file.c / lz4file.h v1.10.0:
//   - Lz4ReadFile::open       → LZ4F_readOpen (lines 73–138)
//   - Lz4ReadFile::read       → LZ4F_read     (lines 140–181)
//   - Lz4WriteFile::open      → LZ4F_writeOpen (lines 217–279)
//   - Lz4WriteFile::write     → LZ4F_write    (lines 281–315)
//   - Lz4WriteFile::finish    → LZ4F_writeClose (lines 317–341)
//   - lz4_write_frame         → LZ4_writeFile convenience
//   - lz4_read_frame          → LZ4_readFile  convenience
//   - Sticky errored flag (C: errCode), Drop finalisation

use lz4::file::{lz4_read_frame, lz4_write_frame, Lz4ReadFile, Lz4WriteFile};
use lz4::frame::types::{BlockSizeId, ContentChecksum, FrameInfo, Preferences};
use std::io::{Cursor, Read, Write};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `src` into a valid LZ4 frame using the convenience helper.
fn compress_to_frame(src: &[u8]) -> Vec<u8> {
    lz4_write_frame(src, Vec::new()).expect("lz4_write_frame failed")
}

/// Decompress a full LZ4 frame using the convenience helper.
fn decompress_frame(frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    lz4_read_frame(Cursor::new(frame), &mut out).expect("lz4_read_frame failed");
    out
}

/// Build cycling bytes 0..=255 repeated as needed.
fn cycling_bytes(len: usize) -> Vec<u8> {
    (0u8..=255).cycle().take(len).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4_write_frame / lz4_read_frame — convenience round-trips
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn convenience_round_trip_small() {
    // Basic round-trip: verify the convenience wrappers compress + decompress correctly.
    let original = b"Hello, LZ4 world! This is a test.";
    let compressed = compress_to_frame(original);
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

#[test]
fn convenience_round_trip_empty() {
    // Empty payload: lz4_write_frame should emit a valid frame (header + end-mark).
    // lz4_read_frame should recover an empty byte slice.
    let original: &[u8] = b"";
    let compressed = compress_to_frame(original);
    assert!(
        !compressed.is_empty(),
        "compressed frame must not be empty even for empty input"
    );
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered.as_slice(), original);
}

#[test]
fn convenience_round_trip_single_byte() {
    // Single-byte payload: smallest meaningful input.
    let original = b"x";
    let compressed = compress_to_frame(original);
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered.as_slice(), original.as_ref());
}

#[test]
fn convenience_round_trip_multi_block() {
    // Input > 64 KiB forces multiple LZ4 blocks within the frame (LZ4F default max64KB).
    let original: Vec<u8> = cycling_bytes(200 * 1024);
    let compressed = compress_to_frame(&original);
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

#[test]
fn convenience_round_trip_large_repetitive() {
    // Repetitive data compresses well — verify decompressed length equality.
    let original: Vec<u8> = b"AAAA".iter().cycle().take(512 * 1024).cloned().collect();
    let compressed = compress_to_frame(&original);
    // Compressed size should be significantly smaller than original for repetitive data.
    assert!(compressed.len() < original.len() / 2);
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

#[test]
fn convenience_round_trip_incompressible() {
    // Incompressible (random-ish) data: compressed may be slightly larger than input.
    let original: Vec<u8> = cycling_bytes(32 * 1024);
    let compressed = compress_to_frame(&original);
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4_write_frame — returns inner writer on success
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn write_frame_returns_inner_writer() {
    // lz4_write_frame should return the inner writer (the Vec) on success.
    let data = b"test data";
    let result: Vec<u8> = lz4_write_frame(data, Vec::new()).expect("should succeed");
    assert!(
        !result.is_empty(),
        "returned Vec must contain the LZ4 frame bytes"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4WriteFile — streaming Write impl
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn write_file_open_default_prefs() {
    // Lz4WriteFile::open with None preferences should succeed and write the frame header.
    let mut buf = Vec::new();
    {
        let writer = Lz4WriteFile::open(&mut buf, None).expect("open should succeed");
        // finish() explicitly to ensure end-mark is written.
        writer.finish().expect("finish should succeed");
    }
    // The resulting frame should be a valid LZ4 frame (starts with magic 0x184D2204).
    assert!(
        buf.len() >= 7,
        "frame must contain at least header + end-mark"
    );
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    assert_eq!(magic, 0x184D2204, "frame must start with LZ4 magic number");
}

#[test]
fn write_file_open_with_preferences() {
    // Lz4WriteFile::open with explicit preferences (Max256Kb block size).
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max256Kb,
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), Some(&prefs)).expect("open");
        lz4w.write_all(b"hello preferences").unwrap();
        lz4w.finish().expect("finish")
    };
    // Should decompress cleanly.
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, b"hello preferences");
}

#[test]
fn write_file_chunked_writes() {
    // The C LZ4F_write loops over maxWriteSize-sized chunks.
    // Verify that writing in small pieces produces the same output as one large write.
    let original: Vec<u8> = cycling_bytes(8 * 1024);

    // Write in 256-byte chunks.
    let chunked = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), None).expect("open");
        for chunk in original.chunks(256) {
            lz4w.write_all(chunk).unwrap();
        }
        lz4w.finish().expect("finish")
    };

    // Write all at once.
    let one_shot = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), None).expect("open");
        lz4w.write_all(&original).unwrap();
        lz4w.finish().expect("finish")
    };

    // Both should decompress to the original.
    assert_eq!(decompress_frame(&chunked), original);
    assert_eq!(decompress_frame(&one_shot), original);
}

#[test]
fn write_file_write_returns_buf_len() {
    // The Write::write impl must return Ok(buf.len()) on success (all bytes consumed),
    // mirroring the C LZ4F_write return convention.
    let mut lz4w = Lz4WriteFile::open(Vec::new(), None).expect("open");
    let data = b"some data here";
    let written = lz4w.write(data).expect("write");
    assert_eq!(written, data.len());
    lz4w.finish().expect("finish");
}

#[test]
fn write_file_empty_write() {
    // Writing zero bytes should be a no-op (loop doesn't execute).
    let mut lz4w = Lz4WriteFile::open(Vec::new(), None).expect("open");
    let written = lz4w.write(b"").expect("write empty");
    assert_eq!(written, 0);
    lz4w.finish().expect("finish");
}

#[test]
fn write_file_multiple_finish_not_called_drop_finalizes() {
    // When finish() is NOT called, Drop should still write the end-mark.
    // The resulting frame should be decompressible.
    let mut buf = Vec::new();
    {
        let mut lz4w = Lz4WriteFile::open(&mut buf, None).expect("open");
        lz4w.write_all(b"dropped without finish").unwrap();
        // Drop is called here — it must write end-mark.
    }
    let recovered = decompress_frame(&buf);
    assert_eq!(recovered, b"dropped without finish");
}

#[test]
fn write_file_finish_takes_inner_writer() {
    // finish() should return the inner writer with the complete frame.
    let data = b"finish returns inner";
    let inner: Vec<u8> = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), None).expect("open");
        lz4w.write_all(data).unwrap();
        lz4w.finish().expect("finish")
    };
    let recovered = decompress_frame(&inner);
    assert_eq!(recovered, data);
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4ReadFile — streaming Read impl
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn read_file_open_valid_frame() {
    // Lz4ReadFile::open should succeed on a valid LZ4 frame.
    let compressed = compress_to_frame(b"valid frame");
    let _reader = Lz4ReadFile::open(Cursor::new(compressed)).expect("open should succeed");
}

#[test]
fn read_file_open_empty_input_fails() {
    // Lz4ReadFile::open on an empty reader should return an error
    // (the C LZ4F_readOpen fails if fread returns 0 bytes — no header to parse).
    let result = Lz4ReadFile::open(Cursor::new(b""));
    assert!(result.is_err(), "open on empty input must fail");
}

#[test]
fn read_file_open_corrupt_magic_fails() {
    // A stream with an invalid magic number should be rejected during open.
    let corrupt = vec![
        0xDEu8, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];
    let result = Lz4ReadFile::open(Cursor::new(corrupt));
    assert!(result.is_err(), "open on corrupt magic must fail");
}

#[test]
fn read_file_read_full_small() {
    // Read all decompressed bytes in a single large buffer.
    let original = b"small read test";
    let compressed = compress_to_frame(original);
    let mut lz4r = Lz4ReadFile::open(Cursor::new(compressed)).expect("open");
    let mut out = vec![0u8; 1024];
    let n = lz4r.read(&mut out).unwrap();
    assert_eq!(&out[..n], original);
}

#[test]
fn read_file_read_chunks_reassemble_correctly() {
    // Read decompressed data in small pieces and verify the concatenation equals original.
    let original: Vec<u8> = cycling_bytes(16 * 1024);
    let compressed = compress_to_frame(&original);
    let mut lz4r = Lz4ReadFile::open(Cursor::new(compressed)).expect("open");

    let mut recovered = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = lz4r.read(&mut tmp).unwrap();
        if n == 0 {
            break;
        }
        recovered.extend_from_slice(&tmp[..n]);
    }
    assert_eq!(recovered, original);
}

#[test]
fn read_file_returns_zero_at_eof() {
    // After the frame is fully decompressed, subsequent reads must return 0.
    let original = b"eof test";
    let compressed = compress_to_frame(original);
    let mut lz4r = Lz4ReadFile::open(Cursor::new(compressed)).expect("open");

    // Drain the stream.
    let mut out = vec![0u8; 4096];
    loop {
        let n = lz4r.read(&mut out).unwrap();
        if n == 0 {
            break;
        }
    }
    // Another read should return 0 (EOF).
    let n = lz4r.read(&mut out).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn read_file_multi_block_frame() {
    // Data exceeding 64 KiB (default block size) creates multiple blocks;
    // verify all blocks are decompressed correctly.
    let original: Vec<u8> = cycling_bytes(150 * 1024);
    let compressed = compress_to_frame(&original);
    let mut lz4r = Lz4ReadFile::open(Cursor::new(compressed)).expect("open");
    let mut recovered = Vec::new();
    let mut tmp = [0u8; 65536];
    loop {
        let n = lz4r.read(&mut tmp).unwrap();
        if n == 0 {
            break;
        }
        recovered.extend_from_slice(&tmp[..n]);
    }
    assert_eq!(recovered, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming Write then Read — full parity scenarios
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn streaming_write_then_streaming_read() {
    // Validates the core parity scenario: Lz4WriteFile + Lz4ReadFile behave
    // like LZ4F_writeOpen/write/writeClose + LZ4F_readOpen/read/readClose.
    let original: Vec<u8> = b"streaming parity test"
        .iter()
        .cycle()
        .take(32 * 1024)
        .cloned()
        .collect();

    // Write in 1 KiB pieces (exercises the chunking loop in LZ4F_write).
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), None).expect("open writer");
        for chunk in original.chunks(1024) {
            lz4w.write_all(chunk).unwrap();
        }
        lz4w.finish().expect("finish")
    };

    // Read back in 3 KiB pieces (exercises multiple refill passes in LZ4F_read).
    let mut lz4r = Lz4ReadFile::open(Cursor::new(&compressed)).expect("open reader");
    let mut recovered = Vec::new();
    let mut tmp = [0u8; 3 * 1024];
    loop {
        let n = lz4r.read(&mut tmp).unwrap();
        if n == 0 {
            break;
        }
        recovered.extend_from_slice(&tmp[..n]);
    }

    assert_eq!(recovered, original);
}

#[test]
fn round_trip_with_content_checksum() {
    // Content checksum (FLG.C_Size bit) is validated during decompression.
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let original = b"checksum round trip";
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), Some(&prefs)).expect("open");
        lz4w.write_all(original).unwrap();
        lz4w.finish().expect("finish")
    };
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered.as_slice(), original.as_ref());
}

#[test]
fn round_trip_max1mb_block_size() {
    // Verify the block_size_from_id mapping for Max1Mb (1 MiB block size).
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max1Mb,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let original: Vec<u8> = cycling_bytes(2 * 1024 * 1024);
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), Some(&prefs)).expect("open");
        lz4w.write_all(&original).unwrap();
        lz4w.finish().expect("finish")
    };
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

#[test]
fn round_trip_max256kb_block_size() {
    // Verify the block_size_from_id mapping for Max256Kb.
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max256Kb,
            ..FrameInfo::default()
        },
        ..Preferences::default()
    };
    let original: Vec<u8> = cycling_bytes(500 * 1024);
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), Some(&prefs)).expect("open");
        lz4w.write_all(&original).unwrap();
        lz4w.finish().expect("finish")
    };
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// Compressed output structure sanity checks
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compressed_frame_starts_with_lz4_magic() {
    // Every LZ4 frame must begin with magic number 0x184D2204 (little-endian).
    let compressed = compress_to_frame(b"magic check");
    assert!(compressed.len() >= 4);
    let magic = u32::from_le_bytes(compressed[0..4].try_into().unwrap());
    assert_eq!(magic, 0x184D2204u32);
}

#[test]
fn compressed_output_smaller_than_repetitive_input() {
    // LZ4 compression of repetitive data must produce output smaller than input
    // for inputs above the minimum useful size.
    let original: Vec<u8> = vec![b'A'; 64 * 1024];
    let compressed = compress_to_frame(&original);
    assert!(
        compressed.len() < original.len(),
        "compressed ({} bytes) should be < original ({} bytes) for repetitive data",
        compressed.len(),
        original.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: Additional coverage tests for file.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Writer that errors after N bytes to test the sticky errored flag path.
struct BrokenWriter {
    inner: Vec<u8>,
    remaining: usize,
}

impl BrokenWriter {
    fn new(fail_after: usize) -> Self {
        BrokenWriter {
            inner: Vec::new(),
            remaining: fail_after,
        }
    }
}

impl Write for BrokenWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "intentional write error",
            ));
        }
        let n = buf.len().min(self.remaining);
        self.inner.extend_from_slice(&buf[..n]);
        self.remaining -= n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Write error (sticky errored flag) prevents double-finalization in Drop.
#[test]
fn write_file_write_error_sets_errored_flag() {
    // The writer will fail after accepting the header, so the first
    // write_all of compressed data should trigger the errored flag.
    let broken = BrokenWriter::new(50); // Fail after 50 bytes (header fits, data fails)
    let mut lz4w = Lz4WriteFile::open(broken, None).expect("open");
    // Write enough data to trigger a compressed block write
    let result = lz4w.write_all(&[0xAA; 100_000]);
    // The write should fail
    assert!(result.is_err(), "Write should fail on broken writer");
    // Drop should NOT panic — errored flag prevents finalization attempt
    drop(lz4w);
}

/// Using BlockSizeId::Default exercises the Default match arm in block_size_from_id.
#[test]
fn write_file_with_block_size_default() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Default,
            ..Default::default()
        },
        ..Default::default()
    };
    let original = vec![0xCCu8; 200_000];
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), Some(&prefs)).expect("open");
        lz4w.write_all(&original).unwrap();
        lz4w.finish().expect("finish")
    };
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}

/// Large data with Max4Mb block size exercises finish() writing end mark.
#[test]
fn write_file_large_data_finish() {
    let prefs = Preferences {
        frame_info: FrameInfo {
            block_size_id: BlockSizeId::Max4Mb,
            ..Default::default()
        },
        ..Default::default()
    };
    let original: Vec<u8> = (0..500_000).map(|i| (i % 251) as u8).collect();
    let compressed = {
        let mut lz4w = Lz4WriteFile::open(Vec::new(), Some(&prefs)).expect("open");
        lz4w.write_all(&original).unwrap();
        lz4w.finish().expect("finish")
    };
    let recovered = decompress_frame(&compressed);
    assert_eq!(recovered, original);
}
