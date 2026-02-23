// Unit tests for task-022: src/io.rs — lz4io public API surface assembly
//
// Verifies that all symbols declared in lz4io.h are accessible through the
// `lz4::io` module and have the correct values, matching lz4io.h / lz4io.c
// (lz4-1.10.0/programs).
//
// Public API under test:
//   `lz4::io::Prefs`                       — LZ4IO_prefs_t
//   `lz4::io::CompressedFileInfo`          — LZ4IO_cFileInfo_t
//   `lz4::io::STDIN_MARK`                  — "stdin" sentinel
//   `lz4::io::STDOUT_MARK`                 — "stdout" sentinel
//   `lz4::io::NULL_OUTPUT`                 — "null" sentinel
//   `lz4::io::NUL_MARK`                    — "/dev/null" / "nul" sentinel
//   `lz4::io::LZ4IO_MAGICNUMBER`           — 0x184D2204
//   `lz4::io::LEGACY_MAGICNUMBER`          — 0x184C2102
//   `lz4::io::LZ4IO_SKIPPABLE0`            — 0x184D2A50
//   `lz4::io::LZ4IO_SKIPPABLEMASK`         — 0xFFFFFFF0
//   `lz4::io::set_notification_level`      — LZ4IO_setNotificationLevel
//   `lz4::io::default_nb_workers`          — LZ4IO_defaultNbWorkers
//   `lz4::io::compress_filename`           — LZ4IO_compressFilename
//   `lz4::io::compress_multiple_filenames` — LZ4IO_compressMultipleFilenames
//   `lz4::io::compress_filename_legacy`    — LZ4IO_compressFilename_Legacy
//   `lz4::io::compress_multiple_filenames_legacy`
//   `lz4::io::decompress_filename`         — LZ4IO_decompressFilename
//   `lz4::io::decompress_multiple_filenames`
//   `lz4::io::display_compressed_files_info`

use lz4::io::Prefs;
use lz4::io::{compress_filename, decompress_filename};
use lz4::io::{compress_filename_legacy, compress_multiple_filenames_legacy};
use lz4::io::{compress_multiple_filenames, decompress_multiple_filenames};
use lz4::io::{default_nb_workers, set_notification_level};
use lz4::io::{
    CompressedFileInfo, LEGACY_MAGICNUMBER, LZ4IO_MAGICNUMBER, LZ4IO_SKIPPABLE0,
    LZ4IO_SKIPPABLEMASK, NULL_OUTPUT, NUL_MARK, STDIN_MARK, STDOUT_MARK,
};

use std::fs;
use std::io::Write;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn make_frame_stream(data: &[u8]) -> Vec<u8> {
    lz4::frame::compress_frame_to_vec(data)
}

// ─────────────────────────────────────────────────────────────────────────────
// Magic number constants (lz4io.h lines 43–57)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn magic_number_lz4_frame_value() {
    // LZ4IO_MAGICNUMBER == 0x184D2204 (lz4io.c line 79 / lz4frame.h).
    assert_eq!(LZ4IO_MAGICNUMBER, 0x184D_2204u32);
}

#[test]
fn magic_number_legacy_value() {
    // LEGACY_MAGICNUMBER == 0x184C2102 (lz4io.c line 80).
    assert_eq!(LEGACY_MAGICNUMBER, 0x184C_2102u32);
}

#[test]
fn magic_number_skippable0_value() {
    // LZ4IO_SKIPPABLE0 == 0x184D2A50 (lz4io.c line 81).
    assert_eq!(LZ4IO_SKIPPABLE0, 0x184D_2A50u32);
}

#[test]
fn magic_number_skippable_mask_value() {
    // LZ4IO_SKIPPABLEMASK == 0xFFFFFFF0 (lz4io.c line 82).
    // A frame with (magic & mask) == SKIPPABLE0 must be skipped.
    assert_eq!(LZ4IO_SKIPPABLEMASK, 0xFFFF_FFF0u32);
}

#[test]
fn skippable_mask_correctly_classifies_skippable_frames() {
    // Skippable frame IDs 0x184D2A50 – 0x184D2A5F are all skippable.
    for id in 0u32..=0x0F {
        let magic = LZ4IO_SKIPPABLE0 | id;
        assert_eq!(
            magic & LZ4IO_SKIPPABLEMASK,
            LZ4IO_SKIPPABLE0,
            "frame 0x{:08X} should be classified as skippable",
            magic
        );
    }
}

#[test]
fn frame_magic_not_classified_as_skippable() {
    // The main LZ4 frame magic is NOT a skippable frame.
    assert_ne!(LZ4IO_MAGICNUMBER & LZ4IO_SKIPPABLEMASK, LZ4IO_SKIPPABLE0);
}

// ─────────────────────────────────────────────────────────────────────────────
// I/O sentinel constants (lz4io.h lines 42–49)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stdin_mark_is_stdin() {
    // stdinmark == "stdin" (lz4io.h line 42).
    assert_eq!(STDIN_MARK, "stdin");
}

#[test]
fn stdout_mark_is_stdout() {
    // stdoutmark == "stdout" (lz4io.h line 43).
    assert_eq!(STDOUT_MARK, "stdout");
}

#[test]
fn null_output_is_null() {
    // NULL_OUTPUT == "null" (lz4io.h line 44).
    assert_eq!(NULL_OUTPUT, "null");
}

#[test]
fn nul_mark_is_platform_devnull() {
    // nulmark == "/dev/null" on POSIX, "nul" on Windows (lz4io.h line 45–47).
    #[cfg(not(windows))]
    assert_eq!(NUL_MARK, "/dev/null");
    #[cfg(windows)]
    assert_eq!(NUL_MARK, "nul");
}

// ─────────────────────────────────────────────────────────────────────────────
// set_notification_level — global setter (LZ4IO_setNotificationLevel)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_notification_level_returns_stored_value() {
    // C: int LZ4IO_setNotificationLevel(int level) — returns the level stored.
    assert_eq!(set_notification_level(2), 2);
    assert_eq!(set_notification_level(0), 0);
}

#[test]
fn set_notification_level_zero_is_silent() {
    // Level 0 == silent (C default g_displayLevel = 0).
    let ret = set_notification_level(0);
    assert_eq!(ret, 0);
}

#[test]
fn set_notification_level_idempotent() {
    // Setting the same level twice should succeed and return the level.
    set_notification_level(1);
    assert_eq!(set_notification_level(1), 1);
    set_notification_level(0); // restore
}

// ─────────────────────────────────────────────────────────────────────────────
// default_nb_workers — worker count (LZ4IO_defaultNbWorkers)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn default_nb_workers_returns_positive() {
    // LZ4IO_defaultNbWorkers() must return ≥ 1.
    let n = default_nb_workers();
    assert!(
        n >= 1,
        "default_nb_workers must return at least 1, got {}",
        n
    );
}

#[test]
fn default_nb_workers_single_thread_without_feature() {
    // Without the `multithread` feature, always returns 1.
    #[cfg(not(feature = "multithread"))]
    assert_eq!(default_nb_workers(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Prefs — accessible via lz4::io::Prefs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prefs_default_accessible_via_io_module() {
    // LZ4IO_defaultPreferences() — Prefs is re-exported from lz4::io.
    let p = Prefs::default();
    // Verify key defaults match lz4io.c LZ4IO_defaultPreferences().
    assert!(p.overwrite, "overwrite defaults to true");
    assert!(!p.pass_through, "pass_through defaults to false");
    assert!(!p.test_mode, "test_mode defaults to false");
    assert!(p.stream_checksum, "stream_checksum defaults to true");
    assert_eq!(
        p.sparse_file_support, 1,
        "sparse_file_support defaults to 1 (auto)"
    );
    assert!(!p.remove_src_file, "remove_src_file defaults to false");
    assert!(p.nb_workers >= 1, "nb_workers must be at least 1");
}

#[test]
fn prefs_new_equals_default() {
    // Prefs::new() must equal Prefs::default() field-by-field.
    let a = Prefs::new();
    let b = Prefs::default();
    assert_eq!(a.overwrite, b.overwrite);
    assert_eq!(a.pass_through, b.pass_through);
    assert_eq!(a.test_mode, b.test_mode);
    assert_eq!(a.block_size_id, b.block_size_id);
    assert_eq!(a.block_checksum, b.block_checksum);
    assert_eq!(a.stream_checksum, b.stream_checksum);
    assert_eq!(a.sparse_file_support, b.sparse_file_support);
    assert_eq!(a.remove_src_file, b.remove_src_file);
}

#[test]
fn prefs_clone_works() {
    // Prefs::clone() must produce an identical copy (Prefs implements Clone).
    let original = Prefs::default();
    let cloned = original.clone();
    assert_eq!(original.overwrite, cloned.overwrite);
    assert_eq!(original.nb_workers, cloned.nb_workers);
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressedFileInfo — accessible via lz4::io::CompressedFileInfo
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compressed_file_info_type_is_accessible() {
    // CompressedFileInfo (LZ4IO_cFileInfo_t) is re-exported via lz4::io.
    // Verify the type is usable (compile-time check).
    let _: fn() -> Option<CompressedFileInfo> = || None;
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_filename / decompress_filename — integration round-trip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_and_decompress_filename_round_trip() {
    // End-to-end: compress via lz4::io::compress_filename then decompress
    // via lz4::io::decompress_filename.  Both are re-exported from sub-modules.
    let original = b"Hello from lz4::io public API!";
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("input.txt");
    let compressed = dir.path().join("input.txt.lz4");
    let output = dir.path().join("output.txt");

    fs::write(&src, original).unwrap();

    let prefs = Prefs::default();
    compress_filename(
        src.to_str().unwrap(),
        compressed.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename should succeed");

    let stats = decompress_filename(
        compressed.to_str().unwrap(),
        output.to_str().unwrap(),
        &prefs,
    )
    .expect("decompress_filename should succeed");

    assert_eq!(fs::read(&output).unwrap().as_slice(), original.as_ref());
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

#[test]
fn compress_filename_missing_src_returns_error() {
    // compress_filename on a non-existent source returns an error.
    let dir = tempfile::tempdir().unwrap();
    let prefs = Prefs::default();
    let result = compress_filename(
        "/nonexistent/path/input.txt",
        dir.path().join("output.lz4").to_str().unwrap(),
        1,
        &prefs,
    );
    assert!(result.is_err(), "missing src must return error");
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_multiple_filenames / decompress_multiple_filenames — round-trip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_and_decompress_multiple_filenames_round_trip() {
    // compress_multiple_filenames + decompress_multiple_filenames via lz4::io.
    let suffix = ".lz4";
    let dir = tempfile::tempdir().unwrap();
    let originals: &[&[u8]] = &[b"alpha content", b"beta content"];
    let srcs: Vec<_> = originals
        .iter()
        .enumerate()
        .map(|(i, data)| {
            let path = dir.path().join(format!("file{}.txt", i));
            fs::write(&path, data).unwrap();
            path
        })
        .collect();
    let src_strs: Vec<&str> = srcs.iter().map(|p| p.to_str().unwrap()).collect();

    let prefs = Prefs::default();
    compress_multiple_filenames(&src_strs, suffix, 1, &prefs)
        .expect("compress_multiple_filenames should succeed");

    // Verify compressed files exist.
    for src in &srcs {
        let compressed = src.with_extension("txt.lz4");
        assert!(compressed.exists(), "expected {:?}", compressed);
    }

    // Decompress: strip the .lz4 suffix to produce new outputs.
    let compressed_srcs: Vec<_> = srcs.iter().map(|p| p.with_extension("txt.lz4")).collect();
    let comp_strs: Vec<&str> = compressed_srcs
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect();

    // Remove original files so decompression produces fresh copies.
    for src in &srcs {
        fs::remove_file(src).unwrap();
    }

    decompress_multiple_filenames(&comp_strs, suffix, &prefs)
        .expect("decompress_multiple_filenames should succeed");

    for (src, original) in srcs.iter().zip(originals.iter()) {
        assert_eq!(fs::read(src).unwrap().as_slice(), *original);
    }
}

#[test]
fn compress_multiple_filenames_empty_list_succeeds() {
    // An empty input list should succeed without error.
    let prefs = Prefs::default();
    compress_multiple_filenames(&[], ".lz4", 1, &prefs).expect("empty list must succeed");
}

#[test]
fn decompress_multiple_filenames_empty_list_succeeds() {
    // An empty input list should succeed without error.
    let prefs = Prefs::default();
    decompress_multiple_filenames(&[], ".lz4", &prefs).expect("empty list must succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_filename_legacy / compress_multiple_filenames_legacy
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_filename_legacy_round_trip() {
    // LZ4IO_compressFilename_Legacy produces a stream that the dispatch
    // decompresses correctly (magic 0x184C2102 → legacy path).
    let original = b"Legacy API round-trip via lz4::io";
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy_input.txt");
    let compressed = dir.path().join("legacy_input.lz4");
    let output = dir.path().join("legacy_output.txt");

    fs::write(&src, original).unwrap();

    let prefs = Prefs::default();
    compress_filename_legacy(
        src.to_str().unwrap(),
        compressed.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename_legacy should succeed");

    // Decompress using the unified dispatch (detects legacy magic).
    let stats = decompress_filename(
        compressed.to_str().unwrap(),
        output.to_str().unwrap(),
        &prefs,
    )
    .expect("decompress_filename on legacy stream should succeed");

    assert_eq!(fs::read(&output).unwrap().as_slice(), original.as_ref());
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

#[test]
fn compress_filename_legacy_produces_legacy_magic() {
    // The output stream must begin with LEGACY_MAGICNUMBER (0x184C2102 LE).
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("check_magic.txt");
    let compressed = dir.path().join("check_magic.lz4");
    fs::write(&src, b"magic check data").unwrap();

    let prefs = Prefs::default();
    compress_filename_legacy(
        src.to_str().unwrap(),
        compressed.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename_legacy should succeed");

    let bytes = fs::read(&compressed).unwrap();
    assert!(bytes.len() >= 4, "output too short");
    let magic = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    assert_eq!(magic, LEGACY_MAGICNUMBER, "expected legacy magic number");
}

#[test]
fn compress_multiple_filenames_legacy_empty_list_succeeds() {
    let prefs = Prefs::default();
    compress_multiple_filenames_legacy(&[], ".lz4", 1, &prefs).expect("empty list must succeed");
}

#[test]
fn compress_multiple_filenames_legacy_round_trip() {
    // compress_multiple_filenames_legacy for a single file; decompress and verify.
    let suffix = ".lz4";
    let original = b"Multiple legacy round trip";
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("multi_legacy.txt");
    let compressed = dir.path().join("multi_legacy.txt.lz4");
    let output = dir.path().join("multi_legacy.txt");
    fs::write(&src, original).unwrap();

    let src_str = src.to_str().unwrap();
    let prefs = Prefs::default();
    compress_multiple_filenames_legacy(&[src_str], suffix, 1, &prefs)
        .expect("compress_multiple_filenames_legacy should succeed");
    assert!(compressed.exists());

    // Remove original so decompression writes a fresh copy.
    fs::remove_file(&src).unwrap();
    let comp_str = compressed.to_str().unwrap();
    decompress_multiple_filenames(&[comp_str], suffix, &prefs)
        .expect("decompress_multiple_filenames of legacy stream should succeed");

    assert_eq!(fs::read(&output).unwrap().as_slice(), original.as_ref());
}

// ─────────────────────────────────────────────────────────────────────────────
// display_compressed_files_info — accessible (smoke test)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn display_compressed_files_info_valid_frame_file() {
    // LZ4IO_displayCompressedFilesInfo: on a valid lz4 file, must succeed (return 0 / Ok).
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("info_test.lz4");
    fs::write(&src, make_frame_stream(b"info test content")).unwrap();

    let result = lz4::io::display_compressed_files_info(&[src.to_str().unwrap()]);
    assert!(
        result.is_ok(),
        "display_compressed_files_info on valid file must succeed"
    );
}

#[test]
fn display_compressed_files_info_missing_file_returns_error() {
    // A missing file should cause the function to return an error.
    let result = lz4::io::display_compressed_files_info(&["/nonexistent/file.lz4"]);
    assert!(result.is_err(), "missing file must return error");
}

#[test]
fn display_compressed_files_info_empty_list_succeeds() {
    // An empty file list should succeed without printing anything.
    let result = lz4::io::display_compressed_files_info(&[]);
    assert!(result.is_ok(), "empty list must succeed");
}
