//! E2E Test Suite 10: Legacy Format
//!
//! Validates `compress_filename_legacy`, `compress_multiple_filenames_legacy`,
//! and magic-number auto-detection in `decompress_filename`.
//! Legacy format uses magic number `0x184C2102`.

use lz4::io::prefs::{set_notification_level, Prefs};
use lz4::io::{
    compress_filename, compress_filename_legacy, compress_multiple_filenames_legacy,
    decompress_filename, LEGACY_MAGICNUMBER,
};
use std::fs;
use tempfile::TempDir;

fn silent_prefs() -> Prefs {
    set_notification_level(0);
    Prefs::default()
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. test_legacy_roundtrip
//    Compress 32 KB repetitive data in legacy format, decompress, compare.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_legacy_roundtrip() {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("input.bin");
    let lz4_path = dir.path().join("input.bin.lz4");
    let out_path = dir.path().join("output.bin");

    // 32 KB of repetitive data (easily compressible)
    let original: Vec<u8> = b"abcdefghijklmnopqrstuvwxyz"
        .iter()
        .cycle()
        .take(32 * 1024)
        .cloned()
        .collect();
    fs::write(&src_path, &original).unwrap();

    let prefs = silent_prefs();

    // Compress with legacy format
    let c_result = compress_filename_legacy(
        src_path.to_str().unwrap(),
        lz4_path.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename_legacy should succeed");

    assert!(lz4_path.exists(), "compressed output file must exist");
    assert_eq!(c_result.bytes_read, original.len() as u64);

    // Decompress
    decompress_filename(
        lz4_path.to_str().unwrap(),
        out_path.to_str().unwrap(),
        &prefs,
    )
    .expect("decompress_filename should succeed on legacy-compressed file");

    let recovered = fs::read(&out_path).unwrap();
    assert_eq!(
        recovered, original,
        "decompressed content must match original"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. test_legacy_magic_number
//    First 4 bytes of the compressed output must be the legacy magic number
//    in little-endian: [0x02, 0x21, 0x4C, 0x18].
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_legacy_magic_number() {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("input.bin");
    let lz4_path = dir.path().join("input.bin.lz4");

    fs::write(&src_path, b"hello world from legacy lz4!").unwrap();

    let prefs = silent_prefs();
    compress_filename_legacy(
        src_path.to_str().unwrap(),
        lz4_path.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename_legacy should succeed");

    let compressed = fs::read(&lz4_path).unwrap();
    assert!(compressed.len() >= 4, "output must have at least 4 bytes");

    // LEGACY_MAGICNUMBER = 0x184C2102 → LE bytes [0x02, 0x21, 0x4C, 0x18]
    let magic = u32::from_le_bytes([compressed[0], compressed[1], compressed[2], compressed[3]]);
    assert_eq!(
        magic, LEGACY_MAGICNUMBER,
        "first 4 bytes must be the legacy magic number 0x{:08X}",
        LEGACY_MAGICNUMBER
    );
    assert_eq!(
        &compressed[..4],
        &[0x02, 0x21, 0x4C, 0x18],
        "magic bytes must be in little-endian order"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. test_legacy_auto_detected_by_decompressor
//    The standard decompress_filename must auto-detect the legacy magic and
//    succeed without any explicit legacy-mode flag.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_legacy_auto_detected_by_decompressor() {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("data.bin");
    let lz4_path = dir.path().join("data.bin.lz4");
    let out_path = dir.path().join("data_recovered.bin");

    let original = b"auto-detection test data for legacy lz4 format";
    fs::write(&src_path, original).unwrap();

    let prefs = silent_prefs();

    // Compress using legacy-specific function
    compress_filename_legacy(
        src_path.to_str().unwrap(),
        lz4_path.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename_legacy should succeed");

    // Verify magic number is legacy
    let header = fs::read(&lz4_path).unwrap();
    let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    assert_eq!(magic, LEGACY_MAGICNUMBER);

    // Use the standard (dispatch) decompressor — it must auto-detect legacy format
    let result = decompress_filename(
        lz4_path.to_str().unwrap(),
        out_path.to_str().unwrap(),
        &prefs,
    );
    assert!(
        result.is_ok(),
        "decompress_filename must auto-detect legacy magic and succeed: {:?}",
        result
    );

    let recovered = fs::read(&out_path).unwrap();
    assert_eq!(recovered.as_slice(), original as &[u8]);
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. test_legacy_compress_multiple
//    Compress two files; both output files must exist and decompress correctly.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_legacy_compress_multiple() {
    let dir = TempDir::new().unwrap();

    let src1 = dir.path().join("file1.txt");
    let src2 = dir.path().join("file2.txt");
    let out1 = dir.path().join("out1.bin");
    let out2 = dir.path().join("out2.bin");

    let content1 = b"content of file one, unique data AAAA";
    let content2 = b"content of file two, different BBBB";
    fs::write(&src1, content1).unwrap();
    fs::write(&src2, content2).unwrap();

    let prefs = silent_prefs();
    let srcs = [src1.to_str().unwrap(), src2.to_str().unwrap()];

    compress_multiple_filenames_legacy(&srcs, ".lz4", 1, &prefs)
        .expect("compress_multiple_filenames_legacy should succeed");

    // Both compressed outputs must exist
    let lz4_1 = dir.path().join("file1.txt.lz4");
    let lz4_2 = dir.path().join("file2.txt.lz4");
    assert!(lz4_1.exists(), "file1.txt.lz4 must exist");
    assert!(lz4_2.exists(), "file2.txt.lz4 must exist");

    // Verify both files have correct legacy magic
    for lz4_path in [&lz4_1, &lz4_2] {
        let bytes = fs::read(lz4_path).unwrap();
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(magic, LEGACY_MAGICNUMBER);
    }

    // Both must decompress correctly via the standard decompressor
    decompress_filename(lz4_1.to_str().unwrap(), out1.to_str().unwrap(), &prefs)
        .expect("decompress file1 should succeed");
    decompress_filename(lz4_2.to_str().unwrap(), out2.to_str().unwrap(), &prefs)
        .expect("decompress file2 should succeed");

    assert_eq!(fs::read(&out1).unwrap().as_slice(), content1 as &[u8]);
    assert_eq!(fs::read(&out2).unwrap().as_slice(), content2 as &[u8]);
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. test_legacy_vs_frame_format_roundtrip
//    8 KB data compressed in both legacy and frame formats must both
//    decompress back to the original content.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_legacy_vs_frame_format_roundtrip() {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("data.bin");

    // 8 KB of mixed data
    let original: Vec<u8> = (0u8..=255).cycle().take(8 * 1024).collect();
    fs::write(&src_path, &original).unwrap();

    let prefs = silent_prefs();

    // --- Legacy format ---
    let legacy_lz4 = dir.path().join("data_legacy.lz4");
    let legacy_out = dir.path().join("data_legacy_out.bin");

    compress_filename_legacy(
        src_path.to_str().unwrap(),
        legacy_lz4.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("legacy compress should succeed");

    decompress_filename(
        legacy_lz4.to_str().unwrap(),
        legacy_out.to_str().unwrap(),
        &prefs,
    )
    .expect("legacy decompress should succeed");

    assert_eq!(
        fs::read(&legacy_out).unwrap(),
        original,
        "legacy format roundtrip must reproduce original"
    );

    // --- Frame format ---
    let frame_lz4 = dir.path().join("data_frame.lz4");
    let frame_out = dir.path().join("data_frame_out.bin");

    compress_filename(
        src_path.to_str().unwrap(),
        frame_lz4.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("frame compress should succeed");

    decompress_filename(
        frame_lz4.to_str().unwrap(),
        frame_out.to_str().unwrap(),
        &prefs,
    )
    .expect("frame decompress should succeed");

    assert_eq!(
        fs::read(&frame_out).unwrap(),
        original,
        "frame format roundtrip must reproduce original"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. test_legacy_empty_file
//    Compressing an empty file must complete without panic or error and the
//    output file must exist.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_legacy_empty_file() {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("empty.bin");
    let lz4_path = dir.path().join("empty.bin.lz4");

    fs::write(&src_path, b"").unwrap(); // 0-byte input

    let prefs = silent_prefs();

    let result = compress_filename_legacy(
        src_path.to_str().unwrap(),
        lz4_path.to_str().unwrap(),
        1,
        &prefs,
    );

    assert!(
        result.is_ok(),
        "compressing an empty file must not fail: {:?}",
        result
    );
    assert!(
        lz4_path.exists(),
        "output file must exist even for empty input"
    );

    // The output must contain at least the 4-byte magic header
    let compressed = fs::read(&lz4_path).unwrap();
    assert!(
        compressed.len() >= 4,
        "output must contain at least the magic number header"
    );
    let magic = u32::from_le_bytes([compressed[0], compressed[1], compressed[2], compressed[3]]);
    assert_eq!(
        magic, LEGACY_MAGICNUMBER,
        "magic must be present even for empty file"
    );
}
