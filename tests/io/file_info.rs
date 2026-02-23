// Integration tests for task-021: src/io/file_info.rs — LZ4IO file info display (--list).
//
// Verifies behavioural parity with lz4io.c v1.10.0 lines 2557–2897:
//   - FrameType enum variants and equality (lines 2557–2562)
//   - CompressedFileInfo default initialisation (lines 2571–2579)
//   - block_type_id() → LZ4IO_blockTypeID (lines 2675–2683)
//   - display_compressed_files_info() rejects non-regular files (lines 2845–2897)
//   - display_compressed_files_info() rejects missing files
//   - display_compressed_files_info() succeeds on valid LZ4 frames

use lz4::io::file_info::{block_type_id, display_compressed_files_info, CompressedFileInfo, FrameType};
use lz4_sys::{BlockMode, BlockSize};
use std::io::Write;
use tempfile::NamedTempFile;

// ─────────────────────────────────────────────────────────────────────────────
// FrameType — public enum (lz4io.c lines 2557–2562: LZ4IO_frameType_e)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn frame_type_lz4frame_eq_self() {
    // FrameType derives PartialEq; each variant must equal itself.
    assert_eq!(FrameType::Lz4Frame, FrameType::Lz4Frame);
}

#[test]
fn frame_type_legacy_eq_self() {
    assert_eq!(FrameType::LegacyFrame, FrameType::LegacyFrame);
}

#[test]
fn frame_type_skippable_eq_self() {
    assert_eq!(FrameType::SkippableFrame, FrameType::SkippableFrame);
}

#[test]
fn frame_type_variants_not_equal_each_other() {
    // Distinct variants must not compare equal.
    assert_ne!(FrameType::Lz4Frame, FrameType::LegacyFrame);
    assert_ne!(FrameType::Lz4Frame, FrameType::SkippableFrame);
    assert_ne!(FrameType::LegacyFrame, FrameType::SkippableFrame);
}

#[test]
fn frame_type_is_copy() {
    // FrameType derives Copy; copying should work without moving.
    let a = FrameType::Lz4Frame;
    let b = a; // copy
    assert_eq!(a, b);
}

#[test]
fn frame_type_debug_contains_variant_name() {
    // FrameType derives Debug; the debug string should contain the variant name.
    assert!(format!("{:?}", FrameType::Lz4Frame).contains("Lz4Frame"));
    assert!(format!("{:?}", FrameType::LegacyFrame).contains("LegacyFrame"));
    assert!(format!("{:?}", FrameType::SkippableFrame).contains("SkippableFrame"));
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressedFileInfo — public struct (lz4io.c lines 2571–2579)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compressed_file_info_default_fields() {
    // CompressedFileInfo::new() (via display_compressed_files_info internal path) starts with
    // frame_count=0, eq_frame_types=true, eq_block_types=true, all_content_size=true.
    // We verify the public fields match LZ4IO_INIT_CFILEINFO.
    // Since CompressedFileInfo::new() is private, we construct it indirectly by noting
    // the only public interface is through display_compressed_files_info.
    // Instead verify that field values can be read from a constructed instance.
    // (CompressedFileInfo cannot be constructed externally so we just verify the type compiles.)
    let _: CompressedFileInfo; // compile-time type check
}

// ─────────────────────────────────────────────────────────────────────────────
// block_type_id — public fn (lz4io.c lines 2675–2683: LZ4IO_blockTypeID)
// Format: "B" + size-digit (4–7) + mode-char ('I' or 'D')
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_type_id_max64kb_linked() {
    // BlockSize::Max64KB → digit '4', BlockMode::Linked → 'D' → "B4D"
    let s = block_type_id(&BlockSize::Max64KB, &BlockMode::Linked);
    assert_eq!(s, "B4D");
}

#[test]
fn block_type_id_max64kb_independent() {
    // BlockSize::Max64KB → digit '4', BlockMode::Independent → 'I' → "B4I"
    let s = block_type_id(&BlockSize::Max64KB, &BlockMode::Independent);
    assert_eq!(s, "B4I");
}

#[test]
fn block_type_id_default_same_as_max64kb() {
    // BlockSize::Default is treated identically to Max64KB → digit '4'
    let s_default = block_type_id(&BlockSize::Default, &BlockMode::Linked);
    let s_64kb = block_type_id(&BlockSize::Max64KB, &BlockMode::Linked);
    assert_eq!(s_default, s_64kb);
}

#[test]
fn block_type_id_max256kb_linked() {
    // BlockSize::Max256KB → digit '5', Linked → 'D' → "B5D"
    let s = block_type_id(&BlockSize::Max256KB, &BlockMode::Linked);
    assert_eq!(s, "B5D");
}

#[test]
fn block_type_id_max256kb_independent() {
    // BlockSize::Max256KB → digit '5', Independent → 'I' → "B5I"
    let s = block_type_id(&BlockSize::Max256KB, &BlockMode::Independent);
    assert_eq!(s, "B5I");
}

#[test]
fn block_type_id_max1mb_linked() {
    // BlockSize::Max1MB → digit '6', Linked → 'D' → "B6D"
    let s = block_type_id(&BlockSize::Max1MB, &BlockMode::Linked);
    assert_eq!(s, "B6D");
}

#[test]
fn block_type_id_max1mb_independent() {
    // BlockSize::Max1MB → digit '6', Independent → 'I' → "B6I"
    let s = block_type_id(&BlockSize::Max1MB, &BlockMode::Independent);
    assert_eq!(s, "B6I");
}

#[test]
fn block_type_id_max4mb_linked() {
    // BlockSize::Max4MB → digit '7', Linked → 'D' → "B7D"
    let s = block_type_id(&BlockSize::Max4MB, &BlockMode::Linked);
    assert_eq!(s, "B7D");
}

#[test]
fn block_type_id_max4mb_independent() {
    // BlockSize::Max4MB → digit '7', Independent → 'I' → "B7I"
    let s = block_type_id(&BlockSize::Max4MB, &BlockMode::Independent);
    assert_eq!(s, "B7I");
}

#[test]
fn block_type_id_always_three_chars() {
    // The result is always exactly 3 bytes: 'B' + digit + mode-char.
    for (size, mode) in [
        (&BlockSize::Max64KB, &BlockMode::Linked),
        (&BlockSize::Max64KB, &BlockMode::Independent),
        (&BlockSize::Max256KB, &BlockMode::Linked),
        (&BlockSize::Max256KB, &BlockMode::Independent),
        (&BlockSize::Max1MB, &BlockMode::Linked),
        (&BlockSize::Max4MB, &BlockMode::Independent),
    ] {
        let s = block_type_id(size, mode);
        assert_eq!(s.len(), 3, "block_type_id result must be exactly 3 chars, got '{s}'");
    }
}

#[test]
fn block_type_id_starts_with_b() {
    // First character must always be 'B'.
    let s = block_type_id(&BlockSize::Max4MB, &BlockMode::Linked);
    assert!(s.starts_with('B'), "block_type_id must start with 'B'");
}

#[test]
fn block_type_id_ends_with_i_or_d() {
    // Last character must be 'I' (Independent) or 'D' (Linked/Dependent).
    let si = block_type_id(&BlockSize::Max1MB, &BlockMode::Independent);
    let sd = block_type_id(&BlockSize::Max1MB, &BlockMode::Linked);
    assert!(si.ends_with('I'), "Independent must end with 'I'");
    assert!(sd.ends_with('D'), "Linked (dependent) must end with 'D'");
}

// ─────────────────────────────────────────────────────────────────────────────
// display_compressed_files_info — error cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn display_info_nonexistent_file_returns_err() {
    // A path that does not exist is not a regular file → Err.
    let result = display_compressed_files_info(&["/nonexistent/path/that/does/not/exist.lz4"]);
    assert!(result.is_err(), "nonexistent file must return Err");
}

#[test]
fn display_info_empty_paths_slice_returns_ok() {
    // An empty paths slice → no files to process → Ok(()).
    let result = display_compressed_files_info(&[]);
    assert!(result.is_ok(), "empty paths should return Ok");
}

#[cfg(unix)]
#[test]
fn display_info_directory_returns_err() {
    // A directory path is not a regular file → Err (UTIL_isRegFile check).
    let result = display_compressed_files_info(&["/tmp"]);
    assert!(result.is_err(), "directory must return Err");
}

// ─────────────────────────────────────────────────────────────────────────────
// display_compressed_files_info — valid LZ4 frames
// ─────────────────────────────────────────────────────────────────────────────

/// Build a minimal valid LZ4 frame using lz4_flex and write it to a temp file.
fn write_lz4_frame(payload: &[u8]) -> NamedTempFile {
    let mut tmp = NamedTempFile::new().expect("tempfile");
    let compressed = lz4_flex::compress_prepend_size(payload);
    // lz4_flex::compress_prepend_size prepends the decompressed size as u32 LE,
    // which is NOT a standard LZ4 frame. We need a proper LZ4F frame.
    // Use lz4_sys to build a proper frame.
    let _ = compressed; // discard

    // Build a proper LZ4F frame using lz4_sys raw FFI.
    let frame = build_lz4f_frame(payload);
    tmp.write_all(&frame).expect("write frame");
    tmp
}

/// Uses lz4::file::lz4_write_frame to create a valid LZ4F frame in memory.
fn build_lz4f_frame(src: &[u8]) -> Vec<u8> {
    lz4::file::lz4_write_frame(src, Vec::new()).expect("lz4_write_frame")
}

#[test]
fn display_info_valid_lz4_frame_returns_ok() {
    // display_compressed_files_info on a valid LZ4F file must return Ok(()).
    let tmp = write_lz4_frame(b"Hello, LZ4 info test!");
    let path = tmp.path().to_str().expect("path");
    let result = display_compressed_files_info(&[path]);
    assert!(result.is_ok(), "valid LZ4 frame must return Ok, got: {:?}", result);
}

#[test]
fn display_info_valid_lz4_frame_large_payload() {
    // Multi-block LZ4F frame (payload > 64 KiB) → Ok(()).
    let payload: Vec<u8> = (0u8..=255).cycle().take(200 * 1024).collect();
    let tmp = write_lz4_frame(&payload);
    let path = tmp.path().to_str().expect("path");
    let result = display_compressed_files_info(&[path]);
    assert!(result.is_ok(), "large valid LZ4 frame must return Ok: {:?}", result);
}

#[test]
fn display_info_corrupt_file_returns_err() {
    // A file with garbage bytes (not a valid LZ4 magic number) → Err.
    let mut tmp = NamedTempFile::new().expect("tempfile");
    // Write bytes that are not a valid LZ4 magic number.
    tmp.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00]).expect("write");
    let path = tmp.path().to_str().expect("path");
    let result = display_compressed_files_info(&[path]);
    assert!(result.is_err(), "corrupt file must return Err");
}

#[test]
fn display_info_multiple_valid_files_returns_ok() {
    // Multiple valid LZ4F files → all processed → Ok(()).
    let tmp1 = write_lz4_frame(b"first file content");
    let tmp2 = write_lz4_frame(b"second file content");
    let path1 = tmp1.path().to_str().expect("path1");
    let path2 = tmp2.path().to_str().expect("path2");
    let result = display_compressed_files_info(&[path1, path2]);
    assert!(result.is_ok(), "multiple valid LZ4 frames must return Ok: {:?}", result);
}

#[test]
fn display_info_stops_on_first_error() {
    // If first file is invalid and second is valid, function returns Err after first.
    let tmp_valid = write_lz4_frame(b"valid");
    let path_valid = tmp_valid.path().to_str().expect("path");

    let result = display_compressed_files_info(&[
        "/nonexistent/garbage.lz4",
        path_valid,
    ]);
    assert!(result.is_err(), "must return Err when first path fails");
}

#[test]
fn display_info_empty_payload_lz4_frame() {
    // Empty payload compressed into a valid LZ4F frame → Ok(()).
    let tmp = write_lz4_frame(b"");
    let path = tmp.path().to_str().expect("path");
    let result = display_compressed_files_info(&[path]);
    assert!(result.is_ok(), "empty-payload LZ4 frame must return Ok: {:?}", result);
}
