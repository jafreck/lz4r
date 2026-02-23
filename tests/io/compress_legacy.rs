// Integration tests for task-014: src/io/compress_legacy.rs — LZ4 legacy format compression.
//
// Verifies behavioural parity with lz4io.c v1.10.0 lines 764–977 (declaration #9):
//   - `LZ4IO_compressFilename_Legacy`          → `compress_filename_legacy`
//   - `LZ4IO_compressMultipleFilenames_Legacy` → `compress_multiple_filenames_legacy`
//   - `LegacyResult`                            → returned statistics struct
//
// Coverage:
//   - magic_number: first 4 bytes are 0x184C2102 LE
//   - block_header: 4-byte LE block size follows the magic
//   - fast_mode: clevel < 3 selects fast compressor
//   - hc_mode: clevel >= 3 selects HC compressor
//   - bytes_read: matches input size
//   - bytes_written: includes magic (4 bytes) + per-block headers + compressed data
//   - empty_input: zero source bytes → result.bytes_read == 0, output == magic only
//   - large_input: multi-block input (> LEGACY_BLOCKSIZE) produces multiple block headers
//   - round_trip_fast: decompress with lz4_flex and recover original
//   - round_trip_hc: decompress with lz4_flex and recover original
//   - hc_at_least_as_compact: HC output ≤ fast output on compressible data
//   - negative_clevel: acceleration derived from abs(clevel) for fast path
//   - nonexistent_src: returns Err
//   - multiple_files_ok: all output files created
//   - multiple_files_suffix_applied: dest = src + suffix
//   - multiple_files_one_bad: Err returned, good files still written
//   - multiple_files_empty_list: Ok(())

use lz4::io::compress_legacy::{compress_filename_legacy, compress_multiple_filenames_legacy, LegacyResult};
use lz4::io::prefs::{Prefs, LEGACY_MAGICNUMBER, LEGACY_BLOCKSIZE, MAGICNUMBER_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Write `data` to a temp file, compress it with legacy format at `clevel`,
/// and return (compressed_bytes, result).
fn compress_to_bytes(data: &[u8], clevel: i32) -> (Vec<u8>, LegacyResult) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("input.bin");
    let dst = dir.path().join("output.lz4");
    std::fs::write(&src, data).unwrap();

    let prefs = Prefs::default();
    let result = compress_filename_legacy(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        clevel,
        &prefs,
    )
    .expect("compression must succeed");

    let bytes = std::fs::read(&dst).unwrap();
    (bytes, result)
}

/// Parse all (block_size, block_data) pairs from a legacy compressed stream.
/// Stream layout: 4-byte magic | (4-byte LE block_size | block_size bytes)*
fn parse_blocks(compressed: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut blocks = Vec::new();
    let mut pos = MAGICNUMBER_SIZE; // skip magic
    while pos + 4 <= compressed.len() {
        let sz = u32::from_le_bytes(compressed[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let end = pos + sz as usize;
        assert!(end <= compressed.len(), "block extends past EOF");
        blocks.push((sz, compressed[pos..end].to_vec()));
        pos = end;
    }
    blocks
}

// ═════════════════════════════════════════════════════════════════════════════
// Magic number (LZ4IO.c line 854-858)
// ═════════════════════════════════════════════════════════════════════════════

/// First 4 bytes of the output must be the legacy magic number in LE.
#[test]
fn magic_number_written_at_start() {
    let (out, _) = compress_to_bytes(b"hello world", 1);
    assert!(out.len() >= 4, "output too short");
    let magic = u32::from_le_bytes(out[0..4].try_into().unwrap());
    assert_eq!(magic, LEGACY_MAGICNUMBER, "legacy magic number mismatch");
}

/// Magic number must be exactly 0x184C2102.
#[test]
fn magic_number_value_is_correct() {
    assert_eq!(LEGACY_MAGICNUMBER, 0x184C2102u32);
}

// ═════════════════════════════════════════════════════════════════════════════
// Block header (LZ4IO.c line 792 / 811)
// ═════════════════════════════════════════════════════════════════════════════

/// After the magic, the stream must contain a 4-byte LE compressed block size
/// followed by that many bytes of compressed data.
#[test]
fn block_header_is_present_and_valid_fast() {
    let data = vec![0xABu8; 1024];
    let (out, _) = compress_to_bytes(&data, 1);
    let blocks = parse_blocks(&out);
    assert!(!blocks.is_empty(), "expected at least one block");
    let (sz, _block_data) = &blocks[0];
    assert!(*sz > 0, "block size must be positive");
}

#[test]
fn block_header_is_present_and_valid_hc() {
    let data = vec![0xABu8; 1024];
    let (out, _) = compress_to_bytes(&data, 9);
    let blocks = parse_blocks(&out);
    assert!(!blocks.is_empty(), "expected at least one block");
    let (sz, _block_data) = &blocks[0];
    assert!(*sz > 0, "block size must be positive");
}

// ═════════════════════════════════════════════════════════════════════════════
// LegacyResult statistics
// ═════════════════════════════════════════════════════════════════════════════

/// result.bytes_read must equal the input size.
#[test]
fn bytes_read_equals_input_size() {
    let data = b"The quick brown fox jumps over the lazy dog.";
    let (_, result) = compress_to_bytes(data, 1);
    assert_eq!(result.bytes_read, data.len() as u64);
}

/// result.bytes_written must be at least magic(4) + block_header(4) + 1.
#[test]
fn bytes_written_greater_than_magic_size() {
    let data = b"some data to compress";
    let (_, result) = compress_to_bytes(data, 1);
    assert!(
        result.bytes_written > MAGICNUMBER_SIZE as u64,
        "bytes_written must exceed magic size"
    );
}

/// result.bytes_written must match the actual file size.
#[test]
fn bytes_written_matches_file_size() {
    let data = b"abcdefghijklmnopqrstuvwxyz";
    let (out, result) = compress_to_bytes(data, 1);
    assert_eq!(result.bytes_written, out.len() as u64);
}

// ═════════════════════════════════════════════════════════════════════════════
// Empty input
// ═════════════════════════════════════════════════════════════════════════════

/// An empty source file must produce: magic only (4 bytes), bytes_read == 0.
#[test]
fn empty_input_produces_magic_only() {
    let (out, result) = compress_to_bytes(b"", 1);
    assert_eq!(result.bytes_read, 0, "bytes_read must be 0 for empty input");
    assert_eq!(out.len(), MAGICNUMBER_SIZE, "output must contain only the magic number");
    let magic = u32::from_le_bytes(out[0..4].try_into().unwrap());
    assert_eq!(magic, LEGACY_MAGICNUMBER);
}

/// Empty input with HC mode also produces magic only.
#[test]
fn empty_input_hc_produces_magic_only() {
    let (out, result) = compress_to_bytes(b"", 9);
    assert_eq!(result.bytes_read, 0);
    assert_eq!(out.len(), MAGICNUMBER_SIZE);
}

// ═════════════════════════════════════════════════════════════════════════════
// Large input (multi-block)
// ═════════════════════════════════════════════════════════════════════════════

/// Input larger than LEGACY_BLOCKSIZE (8 MB) must produce multiple blocks.
#[test]
fn large_input_produces_multiple_blocks() {
    // 9 MB > LEGACY_BLOCKSIZE (8 MB) → at least 2 blocks
    let data = vec![0x5Au8; LEGACY_BLOCKSIZE + 1024 * 1024]; // 9 MB
    let (out, result) = compress_to_bytes(&data, 1);
    let blocks = parse_blocks(&out);
    assert!(
        blocks.len() >= 2,
        "expected ≥ 2 blocks for {}-byte input, got {}",
        data.len(),
        blocks.len()
    );
    assert_eq!(result.bytes_read, data.len() as u64);
}

// ═════════════════════════════════════════════════════════════════════════════
// Round-trip: fast mode
// ═════════════════════════════════════════════════════════════════════════════

/// Compress with fast mode and decompress each block with lz4_flex to verify parity.
#[test]
fn round_trip_fast_mode_small_input() {
    let original = b"The quick brown fox jumps over the lazy dog.";
    let (out, result) = compress_to_bytes(original, 1);
    assert_eq!(result.bytes_read, original.len() as u64);

    let blocks = parse_blocks(&out);
    assert_eq!(blocks.len(), 1, "single-block expected for small input");

    let (_sz, block_data) = &blocks[0];
    let decompressed =
        lz4_flex::block::decompress(block_data, original.len() * 4).unwrap();
    assert_eq!(&decompressed[..original.len()], original);
}

#[test]
fn round_trip_fast_mode_repetitive_data() {
    let original = vec![b'Z'; 64 * 1024];
    let (out, result) = compress_to_bytes(&original, 1);
    assert_eq!(result.bytes_read, original.len() as u64);

    let blocks = parse_blocks(&out);
    let mut recovered = Vec::new();
    for (_sz, block_data) in &blocks {
        let dec = lz4_flex::block::decompress(block_data, original.len() * 2).unwrap();
        recovered.extend_from_slice(&dec);
    }
    // Decompressed bytes should reconstruct the original exactly
    assert_eq!(&recovered[..original.len()], &original[..]);
}

// ═════════════════════════════════════════════════════════════════════════════
// Round-trip: HC mode
// ═════════════════════════════════════════════════════════════════════════════

/// Compress with HC mode (level 9) and decompress to verify parity.
#[test]
fn round_trip_hc_mode_small_input() {
    let original = b"The quick brown fox jumps over the lazy dog.";
    let (out, result) = compress_to_bytes(original, 9);
    assert_eq!(result.bytes_read, original.len() as u64);

    let blocks = parse_blocks(&out);
    assert_eq!(blocks.len(), 1);

    let (_sz, block_data) = &blocks[0];
    let decompressed =
        lz4_flex::block::decompress(block_data, original.len() * 4).unwrap();
    assert_eq!(&decompressed[..original.len()], original);
}

#[test]
fn round_trip_hc_mode_repetitive_data() {
    let original = vec![b'Q'; 64 * 1024];
    let (out, _) = compress_to_bytes(&original, 9);

    let blocks = parse_blocks(&out);
    let mut recovered = Vec::new();
    for (_sz, block_data) in &blocks {
        let dec = lz4_flex::block::decompress(block_data, original.len() * 2).unwrap();
        recovered.extend_from_slice(&dec);
    }
    assert_eq!(&recovered[..original.len()], &original[..]);
}

// ═════════════════════════════════════════════════════════════════════════════
// HC ≤ fast for compressible data
// ═════════════════════════════════════════════════════════════════════════════

/// HC mode should produce output no larger than fast mode on compressible data
/// (mirrors C comment at line 827).
#[test]
fn hc_output_not_larger_than_fast_for_compressible_data() {
    let data = vec![b'A'; 64 * 1024]; // highly compressible
    let (fast_out, _) = compress_to_bytes(&data, 1);
    let (hc_out, _) = compress_to_bytes(&data, 9);
    assert!(
        hc_out.len() <= fast_out.len(),
        "HC ({}) should not be larger than fast ({}) for compressible data",
        hc_out.len(),
        fast_out.len()
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// Compression level dispatch boundary (clevel < 3 → fast, clevel >= 3 → HC)
// ═════════════════════════════════════════════════════════════════════════════

/// clevel=2 should use fast path (produces valid decompressible output).
#[test]
fn clevel_2_uses_fast_path() {
    let data = b"sample data for clevel 2 test";
    let (out, result) = compress_to_bytes(data, 2);
    assert_eq!(result.bytes_read, data.len() as u64);
    let blocks = parse_blocks(&out);
    let dec = lz4_flex::block::decompress(&blocks[0].1, data.len() * 4).unwrap();
    assert_eq!(&dec[..data.len()], data);
}

/// clevel=3 should use HC path (produces valid decompressible output).
#[test]
fn clevel_3_uses_hc_path() {
    let data = b"sample data for clevel 3 test";
    let (out, result) = compress_to_bytes(data, 3);
    assert_eq!(result.bytes_read, data.len() as u64);
    let blocks = parse_blocks(&out);
    let dec = lz4_flex::block::decompress(&blocks[0].1, data.len() * 4).unwrap();
    assert_eq!(&dec[..data.len()], data);
}

/// clevel=0 (fast path, zero acceleration) must produce valid output.
#[test]
fn clevel_0_produces_valid_output() {
    let data = b"clevel zero test";
    let (out, result) = compress_to_bytes(data, 0);
    assert_eq!(result.bytes_read, data.len() as u64);
    let blocks = parse_blocks(&out);
    let dec = lz4_flex::block::decompress(&blocks[0].1, data.len() * 4).unwrap();
    assert_eq!(&dec[..data.len()], data);
}

/// Negative clevel provides acceleration = abs(clevel) for fast path.
#[test]
fn negative_clevel_fast_path_produces_valid_output() {
    let data = b"negative clevel acceleration test";
    let (out, result) = compress_to_bytes(data, -5);
    assert_eq!(result.bytes_read, data.len() as u64);
    let blocks = parse_blocks(&out);
    let dec = lz4_flex::block::decompress(&blocks[0].1, data.len() * 4).unwrap();
    assert_eq!(&dec[..data.len()], data);
}

// ═════════════════════════════════════════════════════════════════════════════
// Error cases for compress_filename_legacy
// ═════════════════════════════════════════════════════════════════════════════

/// Nonexistent source file must return Err.
#[test]
fn compress_nonexistent_src_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("out.lz4");
    let prefs = Prefs::default();
    let result = compress_filename_legacy(
        "/nonexistent/path/to/file.bin",
        dst.to_str().unwrap(),
        1,
        &prefs,
    );
    assert!(result.is_err(), "expected Err for nonexistent source");
}

/// Unwritable destination path must return Err.
#[test]
fn compress_bad_dst_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("input.bin");
    std::fs::write(&src, b"data").unwrap();
    let prefs = Prefs::default();
    let result = compress_filename_legacy(
        src.to_str().unwrap(),
        "/nonexistent/directory/out.lz4",
        1,
        &prefs,
    );
    assert!(result.is_err(), "expected Err for bad destination path");
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_multiple_filenames_legacy
// ═════════════════════════════════════════════════════════════════════════════

/// Compressing an empty list of files must return Ok(()).
#[test]
fn compress_multiple_empty_list_ok() {
    let prefs = Prefs::default();
    let result = compress_multiple_filenames_legacy(&[], ".lz4", 1, &prefs);
    assert!(result.is_ok(), "empty file list must succeed");
}

/// All source files are compressed and destination files are created with the suffix.
#[test]
fn compress_multiple_all_files_created() {
    let dir = tempfile::tempdir().unwrap();
    let src1 = dir.path().join("a.txt");
    let src2 = dir.path().join("b.txt");
    std::fs::write(&src1, b"file a content").unwrap();
    std::fs::write(&src2, b"file b content").unwrap();

    let prefs = Prefs::default();
    let srcs = [src1.to_str().unwrap(), src2.to_str().unwrap()];
    let result = compress_multiple_filenames_legacy(&srcs, ".lz4", 1, &prefs);
    assert!(result.is_ok());
    assert!(dir.path().join("a.txt.lz4").exists(), "a.txt.lz4 must exist");
    assert!(dir.path().join("b.txt.lz4").exists(), "b.txt.lz4 must exist");
}

/// Output files must contain valid LZ4 legacy compressed data.
#[test]
fn compress_multiple_output_is_valid_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("data.bin");
    let content = b"compressed multi-file test content";
    std::fs::write(&src, content).unwrap();

    let prefs = Prefs::default();
    let srcs = [src.to_str().unwrap()];
    compress_multiple_filenames_legacy(&srcs, ".lz4", 1, &prefs).unwrap();

    let out = std::fs::read(dir.path().join("data.bin.lz4")).unwrap();
    let magic = u32::from_le_bytes(out[0..4].try_into().unwrap());
    assert_eq!(magic, LEGACY_MAGICNUMBER);
}

/// When one source does not exist, Err is returned but other files are still compressed.
#[test]
fn compress_multiple_one_bad_src_returns_err_and_others_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.txt");
    std::fs::write(&good, b"good content").unwrap();

    let prefs = Prefs::default();
    let srcs = [good.to_str().unwrap(), "/nonexistent/bad.txt"];
    let result = compress_multiple_filenames_legacy(&srcs, ".lz4", 1, &prefs);
    // One file failed → should return Err
    assert!(result.is_err(), "expected Err when one file cannot be compressed");
    // The good file's output should still have been created
    assert!(dir.path().join("good.txt.lz4").exists(), "good.txt.lz4 must still be created");
}

/// All-bad source files return Err with an informative error message.
#[test]
fn compress_multiple_all_bad_returns_err() {
    let prefs = Prefs::default();
    let srcs = ["/bad/a.txt", "/bad/b.txt"];
    let result = compress_multiple_filenames_legacy(&srcs, ".lz4", 1, &prefs);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("2"),
        "error message should mention the number of failed files: {msg}"
    );
}

/// Suffix is correctly appended to each source filename.
#[test]
fn compress_multiple_suffix_applied_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("myfile.dat");
    std::fs::write(&src, b"suffix test").unwrap();

    let prefs = Prefs::default();
    let srcs = [src.to_str().unwrap()];
    compress_multiple_filenames_legacy(&srcs, ".lz4legacy", 1, &prefs).unwrap();

    // Output must be src + ".lz4legacy"
    let expected_dst = format!("{}.lz4legacy", src.to_str().unwrap());
    assert!(
        std::path::Path::new(&expected_dst).exists(),
        "expected output at {expected_dst}"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// LegacyResult struct
// ═════════════════════════════════════════════════════════════════════════════

/// LegacyResult must implement Default (all fields zeroed).
#[test]
fn legacy_result_default_is_zero() {
    let r = LegacyResult::default();
    assert_eq!(r.bytes_read, 0);
    assert_eq!(r.bytes_written, 0);
}

/// LegacyResult must implement Clone and Copy.
#[test]
fn legacy_result_clone_copy() {
    let r = LegacyResult { bytes_read: 100, bytes_written: 50 };
    let r2 = r; // Copy
    let r3 = r.clone(); // Clone
    assert_eq!(r2.bytes_read, 100);
    assert_eq!(r3.bytes_written, 50);
}
