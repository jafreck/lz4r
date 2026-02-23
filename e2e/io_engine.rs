//! E2E Test Suite 08: IO Engine
//!
//! Validates the `lz4::io` module's file-level compress and decompress
//! functions using real temp files.  Corresponds to `LZ4IO_compressFilename`
//! / `LZ4IO_decompressFilename` in the original C lz4 codebase.

use lz4::io::prefs::{set_notification_level, Prefs};
use lz4::io::{
    compress_filename, compress_multiple_filenames, decompress_filename, LEGACY_MAGICNUMBER,
    LZ4IO_MAGICNUMBER,
};
use std::fs;
use tempfile::TempDir;

// Silence progress output in all tests.
fn silent_prefs() -> Prefs {
    set_notification_level(0);
    Prefs::default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: Compress / decompress round-trip (64 KB ASCII data)
// Validates: LZ4IO_compressFilename + LZ4IO_decompressFilename parity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_decompress_roundtrip() {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("input.txt");
    let lz4_path = dir.path().join("input.txt.lz4");
    let out_path = dir.path().join("output.txt");

    // 64 KB of ASCII data.
    let original: Vec<u8> = b"abcdefghijklmnopqrstuvwxyz0123456789"
        .iter()
        .cycle()
        .take(64 * 1024)
        .cloned()
        .collect();
    fs::write(&src_path, &original).unwrap();

    let prefs = silent_prefs();

    // Compress.
    let c_stats = compress_filename(
        src_path.to_str().unwrap(),
        lz4_path.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename should succeed");

    assert!(lz4_path.exists(), ".lz4 output file must exist");
    let compressed_size = fs::metadata(&lz4_path).unwrap().len();
    assert!(
        compressed_size < original.len() as u64,
        "compressed size ({compressed_size}) should be smaller than original ({})",
        original.len()
    );
    assert_eq!(c_stats.bytes_in, original.len() as u64);

    // Decompress.
    let d_stats = decompress_filename(
        lz4_path.to_str().unwrap(),
        out_path.to_str().unwrap(),
        &prefs,
    )
    .expect("decompress_filename should succeed");

    let recovered = fs::read(&out_path).unwrap();
    assert_eq!(recovered, original, "roundtrip content must match");
    assert_eq!(
        d_stats.decompressed_bytes,
        original.len() as u64,
        "decompressed_bytes stat must match original length"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: Default prefs, small ASCII text, level 1 round-trip
// Validates: Prefs::default() produces a working configuration
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_default_prefs() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("hello.txt");
    let lz4 = dir.path().join("hello.txt.lz4");
    let out = dir.path().join("hello_out.txt");

    let original = b"Hello, world! This is a small test string for LZ4 compression.";
    fs::write(&src, original).unwrap();

    let prefs = silent_prefs();

    compress_filename(src.to_str().unwrap(), lz4.to_str().unwrap(), 1, &prefs)
        .expect("compress should succeed with default prefs");

    decompress_filename(lz4.to_str().unwrap(), out.to_str().unwrap(), &prefs)
        .expect("decompress should succeed with default prefs");

    assert_eq!(fs::read(&out).unwrap(), original as &[u8]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: High compression level (12) round-trip with repetitive data
// Validates: level 12 produces valid output that decompresses correctly
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_high_level() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("repetitive.bin");
    let lz4 = dir.path().join("repetitive.bin.lz4");
    let out = dir.path().join("repetitive_out.bin");

    // 32 KB of highly repetitive data — ideal for compression.
    let original: Vec<u8> = std::iter::repeat(b'A').take(32 * 1024).collect();
    fs::write(&src, &original).unwrap();

    let prefs = silent_prefs();

    compress_filename(src.to_str().unwrap(), lz4.to_str().unwrap(), 12, &prefs)
        .expect("compress at level 12 should succeed");

    decompress_filename(lz4.to_str().unwrap(), out.to_str().unwrap(), &prefs)
        .expect("decompress of level-12 output should succeed");

    assert_eq!(
        fs::read(&out).unwrap(),
        original,
        "level-12 roundtrip must be lossless"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: Decompress a file with invalid magic number → Err
// Validates: non-LZ4 input is rejected cleanly (no panic)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decompress_invalid_magic_returns_error() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("not_lz4.lz4");
    let out = dir.path().join("not_lz4_out.bin");

    // 32 bytes of data whose first 4 bytes are not a valid LZ4 magic number.
    fs::write(&bad, b"THIS_IS_NOT_VALID_LZ4_DATA_XXXXX").unwrap();

    let prefs = silent_prefs();

    let result = decompress_filename(bad.to_str().unwrap(), out.to_str().unwrap(), &prefs);
    assert!(
        result.is_err(),
        "decompress of invalid magic should return Err, got Ok"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: Decompress a truncated LZ4 frame → Err
// Validates: partial/corrupt streams are rejected without panicking
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decompress_truncated_frame_returns_error() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("good.txt");
    let lz4 = dir.path().join("good.txt.lz4");
    let truncated = dir.path().join("truncated.lz4");
    let out = dir.path().join("truncated_out.txt");

    let original: Vec<u8> = b"Some data to compress and then truncate."
        .iter()
        .cycle()
        .take(4096)
        .cloned()
        .collect();
    fs::write(&src, &original).unwrap();

    let prefs = silent_prefs();
    compress_filename(src.to_str().unwrap(), lz4.to_str().unwrap(), 1, &prefs)
        .expect("compress should succeed");

    // Truncate compressed data to 50% of its size.
    let compressed = fs::read(&lz4).unwrap();
    let half = compressed.len() / 2;
    fs::write(&truncated, &compressed[..half]).unwrap();

    let result = decompress_filename(truncated.to_str().unwrap(), out.to_str().unwrap(), &prefs);
    assert!(
        result.is_err(),
        "decompress of truncated frame should return Err, got Ok"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: compress_multiple_filenames — 3 files, each decompresses correctly
// Validates: LZ4IO_compressMultipleFilenames parity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_multiple_files() {
    let dir = TempDir::new().unwrap();
    let prefs = silent_prefs();

    let files: Vec<(&[u8], &str)> = vec![
        (b"content of file one", "file1.txt"),
        (
            b"content of file two -- slightly longer text here",
            "file2.txt",
        ),
        (
            b"content of file three --- even more text for variety",
            "file3.txt",
        ),
    ];

    let mut paths: Vec<String> = Vec::new();
    for (data, name) in &files {
        let p = dir.path().join(name);
        fs::write(&p, data).unwrap();
        paths.push(p.to_str().unwrap().to_owned());
    }

    let src_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

    let missed = compress_multiple_filenames(&src_refs, ".lz4", 1, &prefs)
        .expect("compress_multiple_filenames should return Ok");

    assert_eq!(missed, 0, "no files should be missed");

    // Verify each .lz4 exists and decompresses to the original content.
    for (data, name) in &files {
        let lz4_path = dir.path().join(format!("{}.lz4", name));
        assert!(lz4_path.exists(), "{}.lz4 must exist", name);

        let out_path = dir.path().join(format!("{}.out", name));
        decompress_filename(
            lz4_path.to_str().unwrap(),
            out_path.to_str().unwrap(),
            &prefs,
        )
        .unwrap_or_else(|e| panic!("decompress of {} failed: {}", name, e));

        assert_eq!(
            fs::read(&out_path).unwrap(),
            *data,
            "roundtrip content must match for {}",
            name
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7: First 4 bytes of compressed output equal LZ4IO_MAGICNUMBER
// Validates: frame-format magic number written at offset 0
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_output_magic_number() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("magic_test.txt");
    let lz4 = dir.path().join("magic_test.txt.lz4");

    fs::write(&src, b"some data to trigger magic number check").unwrap();

    let prefs = silent_prefs();
    compress_filename(src.to_str().unwrap(), lz4.to_str().unwrap(), 1, &prefs)
        .expect("compress should succeed");

    let bytes = fs::read(&lz4).unwrap();
    assert!(
        bytes.len() >= 4,
        "compressed output must have at least 4 bytes"
    );

    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    assert_eq!(
        magic, LZ4IO_MAGICNUMBER,
        "first 4 bytes must equal LZ4IO_MAGICNUMBER (0x{:08X}), got 0x{:08X}",
        LZ4IO_MAGICNUMBER, magic
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 8: LZ4IO_MAGICNUMBER and LEGACY_MAGICNUMBER constant values
// Validates: constants match C lz4io.h definitions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_magic_number_constants() {
    assert_eq!(LZ4IO_MAGICNUMBER, 0x184D2204_u32);
    assert_eq!(LEGACY_MAGICNUMBER, 0x184C2102_u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 9: compress_multiple_filenames with a missing source file
// Validates: missing file counts as a missed file, doesn't abort the rest
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compress_multiple_files_one_missing() {
    let dir = TempDir::new().unwrap();
    let prefs = silent_prefs();

    let good = dir.path().join("good.txt");
    fs::write(&good, b"good file content").unwrap();

    let missing = dir.path().join("does_not_exist.txt");

    let src_refs = [good.to_str().unwrap(), missing.to_str().unwrap()];
    let missed = compress_multiple_filenames(&src_refs, ".lz4", 1, &prefs)
        .expect("compress_multiple_filenames should return Ok even with missing files");

    assert_eq!(missed, 1, "one missing file should be counted as missed");

    // The good file's .lz4 should still exist.
    let good_lz4 = dir.path().join("good.txt.lz4");
    assert!(
        good_lz4.exists(),
        "good.txt.lz4 must exist despite the missing peer"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 10: set_notification_level silences output (level 0)
// Validates: set_notification_level is callable and takes effect
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_set_notification_level_zero() {
    use lz4::io::prefs::DISPLAY_LEVEL;
    use std::sync::atomic::Ordering;

    let returned = set_notification_level(0);
    assert_eq!(returned, 0);
    assert_eq!(DISPLAY_LEVEL.load(Ordering::Relaxed), 0);
}
