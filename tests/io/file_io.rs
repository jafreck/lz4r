// Integration tests for task-012: src/io/file_io.rs — File open/close utilities.
//
// Verifies behavioural parity with lz4io.c v1.10.0 lines 347–455 and lz4io.h:
//   - Sentinel constants (STDIN_MARK, STDOUT_MARK, NUL_MARK, NULL_OUTPUT)
//   - `LZ4IO_isSkippableMagicNumber` → `is_skippable_magic_number`
//   - `LZ4IO_openSrcFile`            → `open_src_file`
//   - `LZ4IO_openDstFile`            → `open_dst_file`
//   - `DstFile` — Write impl and `is_stdout` flag
//
// Coverage:
//   - sentinel_constants: values match C header sentinels
//   - is_skippable_magic_number: full valid range [0x184D2A50..0x184D2A5F], neighbours
//   - open_src_file: nonexistent path → Err; directory → Err(InvalidInput);
//                    real file → Ok (can read bytes)
//   - open_dst_file: stdout sentinel → is_stdout=true; nul/null sentinels → ok+not-stdout;
//                    overwrite=true creates file; overwrite=false+nonexistent → ok;
//                    overwrite=false+existing+low-display-level → Err(AlreadyExists);
//                    write-through trait works; existing file is truncated on overwrite
//   - DstFile::write: bytes written reach the file on disk

use lz4::io::file_io::{
    is_skippable_magic_number, open_dst_file, open_src_file, NULL_OUTPUT, NUL_MARK, STDIN_MARK,
    STDOUT_MARK,
};
use lz4::io::prefs::{Prefs, DISPLAY_LEVEL};
use std::io::{Read, Write};
use std::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Set display level for the duration of a test, reset to 0 after.
/// Returns the previous value so callers can restore if needed.
fn set_display(level: i32) {
    DISPLAY_LEVEL.store(level, Ordering::Relaxed);
}

/// Ensure display level is 0 after each test that changes it.
fn reset_display() {
    DISPLAY_LEVEL.store(0, Ordering::Relaxed);
}

// ═════════════════════════════════════════════════════════════════════════════
// Sentinel constants
// ═════════════════════════════════════════════════════════════════════════════

/// Sentinel strings must match the values defined in lz4io.h lines 42–49.
#[test]
fn sentinel_stdin_mark_value() {
    assert_eq!(STDIN_MARK, "stdin");
}

#[test]
fn sentinel_stdout_mark_value() {
    assert_eq!(STDOUT_MARK, "stdout");
}

#[test]
#[cfg(not(windows))]
fn sentinel_nul_mark_value_unix() {
    // On Unix: /dev/null (lz4io.h line 43)
    assert_eq!(NUL_MARK, "/dev/null");
}

#[test]
#[cfg(windows)]
fn sentinel_nul_mark_value_windows() {
    // On Windows: nul (lz4io.h alternate)
    assert_eq!(NUL_MARK, "nul");
}

#[test]
fn sentinel_null_output_value() {
    // Alternate discard sentinel accepted by open_dst_file.
    assert_eq!(NULL_OUTPUT, "null");
}

// ═════════════════════════════════════════════════════════════════════════════
// is_skippable_magic_number (LZ4IO_isSkippableMagicNumber)
// ═════════════════════════════════════════════════════════════════════════════

/// All values in [0x184D2A50, 0x184D2A5F] are skippable (16 values).
#[test]
fn is_skippable_magic_number_full_valid_range() {
    for v in 0x184D2A50u32..=0x184D2A5Fu32 {
        assert!(
            is_skippable_magic_number(v),
            "expected skippable: {:#010x}",
            v
        );
    }
}

/// The value immediately below the range is not skippable.
#[test]
fn is_skippable_magic_number_below_range() {
    assert!(!is_skippable_magic_number(0x184D2A4F));
}

/// The value immediately above the range is not skippable.
#[test]
fn is_skippable_magic_number_above_range() {
    assert!(!is_skippable_magic_number(0x184D2A60));
}

/// Zero is not a skippable magic number.
#[test]
fn is_skippable_magic_number_zero() {
    assert!(!is_skippable_magic_number(0));
}

/// u32::MAX is not a skippable magic number.
#[test]
fn is_skippable_magic_number_max() {
    assert!(!is_skippable_magic_number(u32::MAX));
}

/// LZ4IO_MAGICNUMBER (0x184D2204) is not skippable.
#[test]
fn is_skippable_magic_number_lz4_magic() {
    assert!(!is_skippable_magic_number(0x184D2204));
}

/// LEGACY_MAGICNUMBER (0x184C2102) is not skippable.
#[test]
fn is_skippable_magic_number_legacy_magic() {
    assert!(!is_skippable_magic_number(0x184C2102));
}

/// Boundary: 0x184D2A50 (first) and 0x184D2A5F (last) are both skippable.
#[test]
fn is_skippable_magic_number_boundary_values() {
    assert!(is_skippable_magic_number(0x184D2A50));
    assert!(is_skippable_magic_number(0x184D2A5F));
}

// ═════════════════════════════════════════════════════════════════════════════
// open_src_file (LZ4IO_openSrcFile)
// ═════════════════════════════════════════════════════════════════════════════

/// A path that does not exist returns an error.
#[test]
fn open_src_file_nonexistent_returns_err() {
    set_display(0);
    let result = open_src_file("/nonexistent/path/that/cannot/exist.lz4");
    assert!(result.is_err(), "expected Err for nonexistent path");
    reset_display();
}

/// A path pointing to a directory returns Err(InvalidInput) — directories are ignored.
#[test]
fn open_src_file_directory_returns_invalid_input() {
    set_display(0);
    let dir = tempfile::tempdir().unwrap();
    let result = open_src_file(dir.path().to_str().unwrap());
    assert!(result.is_err(), "expected Err for directory path");
    assert_eq!(
        result.err().unwrap().kind(),
        std::io::ErrorKind::InvalidInput,
        "directory must return InvalidInput"
    );
    reset_display();
}

/// A real file can be opened and its bytes read back.
#[test]
fn open_src_file_real_file_readable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.bin");
    std::fs::write(&path, b"hello source").unwrap();

    let mut src = open_src_file(path.to_str().unwrap()).expect("should open a real file");
    let mut buf = Vec::new();
    src.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"hello source");
}

/// An empty file can be opened; reading returns zero bytes.
#[test]
fn open_src_file_empty_file_returns_empty_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.bin");
    std::fs::write(&path, b"").unwrap();

    let mut src = open_src_file(path.to_str().unwrap()).expect("should open empty file");
    let mut buf = Vec::new();
    src.read_to_end(&mut buf).unwrap();
    assert!(buf.is_empty());
}

/// A large file can be opened and read completely.
#[test]
fn open_src_file_large_file_readable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.bin");
    let data: Vec<u8> = (0u8..=255).cycle().take(64 * 1024).collect();
    std::fs::write(&path, &data).unwrap();

    let mut src = open_src_file(path.to_str().unwrap()).expect("should open large file");
    let mut buf = Vec::new();
    src.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, data);
}

// ═════════════════════════════════════════════════════════════════════════════
// open_dst_file (LZ4IO_openDstFile)
// ═════════════════════════════════════════════════════════════════════════════

// ── Stdout sentinel ───────────────────────────────────────────────────────────

/// The "stdout" sentinel returns a DstFile with is_stdout=true.
#[test]
fn open_dst_file_stdout_sentinel_sets_is_stdout() {
    let prefs = Prefs::default();
    let dst = open_dst_file(STDOUT_MARK, &prefs).expect("stdout sentinel must succeed");
    assert!(dst.is_stdout, "is_stdout must be true for stdout sentinel");
}

/// Stdout DstFile has is_stdout=true regardless of sparse_file_support.
#[test]
fn open_dst_file_stdout_with_sparse_support() {
    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 1;
    let dst = open_dst_file(STDOUT_MARK, &prefs).expect("stdout sentinel must succeed");
    assert!(dst.is_stdout);
}

// ── Discard (devnull) sentinels ───────────────────────────────────────────────

/// The NUL_MARK sentinel (/dev/null or nul) returns a sink DstFile with is_stdout=false.
#[test]
fn open_dst_file_nul_mark_returns_sink() {
    let prefs = Prefs::default();
    let dst = open_dst_file(NUL_MARK, &prefs).expect("nul mark must succeed");
    assert!(!dst.is_stdout, "nul mark must not set is_stdout");
}

// ── Real file creation ────────────────────────────────────────────────────────

/// With overwrite=true, a new file is created successfully.
#[test]
fn open_dst_file_new_file_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new_output.lz4");
    let prefs = Prefs::default(); // overwrite=true by default
    let result = open_dst_file(path.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "new file creation must succeed");
    assert!(!result.unwrap().is_stdout);
}

/// Data written to a DstFile is persisted to disk.
#[test]
fn open_dst_file_writes_data_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("output.bin");
    let prefs = Prefs::default();
    {
        let mut dst = open_dst_file(path.to_str().unwrap(), &prefs).expect("should create file");
        dst.write_all(b"written data").unwrap();
        dst.flush().unwrap();
    }
    let contents = std::fs::read(&path).unwrap();
    assert_eq!(contents, b"written data");
}

/// With overwrite=true, an existing file is truncated and overwritten.
#[test]
fn open_dst_file_overwrite_true_truncates_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.bin");
    // Write a longer file first.
    std::fs::write(&path, b"original long content here").unwrap();

    let prefs = Prefs::default(); // overwrite=true
    {
        let mut dst = open_dst_file(path.to_str().unwrap(), &prefs)
            .expect("overwrite=true must open existing file");
        dst.write_all(b"new").unwrap();
        dst.flush().unwrap();
    }
    let contents = std::fs::read(&path).unwrap();
    // File must contain only the new content (old content truncated).
    assert_eq!(contents, b"new");
}

// ── overwrite=false ───────────────────────────────────────────────────────────

/// overwrite=false + non-existent file → success (C: file does not exist → create it).
#[test]
fn open_dst_file_overwrite_false_nonexistent_ok() {
    set_display(0);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("output_new.lz4");
    let mut prefs = Prefs::default();
    prefs.overwrite = false;
    let result = open_dst_file(path.to_str().unwrap(), &prefs);
    assert!(
        result.is_ok(),
        "non-existent file with overwrite=false must succeed"
    );
    reset_display();
}

/// overwrite=false + existing file + display_level ≤ 1 → Err(AlreadyExists).
/// (C: display_level ≤ 1 means no TTY interaction → refuse silently.)
#[test]
fn open_dst_file_overwrite_false_existing_low_display_returns_err() {
    set_display(0);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.lz4");
    std::fs::write(&path, b"existing content").unwrap();

    let mut prefs = Prefs::default();
    prefs.overwrite = false;
    let result = open_dst_file(path.to_str().unwrap(), &prefs);
    assert!(
        result.is_err(),
        "should refuse overwrite with display_level=0"
    );
    assert_eq!(
        result.err().unwrap().kind(),
        std::io::ErrorKind::AlreadyExists,
        "error kind must be AlreadyExists"
    );
    reset_display();
}

/// overwrite=false + existing file at display_level=1 → Err(AlreadyExists).
#[test]
fn open_dst_file_overwrite_false_existing_display_level_1_returns_err() {
    set_display(1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing2.lz4");
    std::fs::write(&path, b"existing content").unwrap();

    let mut prefs = Prefs::default();
    prefs.overwrite = false;
    let result = open_dst_file(path.to_str().unwrap(), &prefs);
    assert!(
        result.is_err(),
        "should refuse overwrite with display_level=1"
    );
    assert_eq!(
        result.err().unwrap().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    reset_display();
}

// ═════════════════════════════════════════════════════════════════════════════
// DstFile — Write trait
// ═════════════════════════════════════════════════════════════════════════════

/// DstFile::write returns Ok(buf.len()) on success.
#[test]
fn dst_file_write_returns_buf_len() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("write_test.bin");
    let prefs = Prefs::default();
    let mut dst = open_dst_file(path.to_str().unwrap(), &prefs).unwrap();
    let data = b"some bytes";
    let written = dst.write(data).expect("write must succeed");
    assert_eq!(written, data.len());
}

/// DstFile::flush succeeds.
#[test]
fn dst_file_flush_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flush_test.bin");
    let prefs = Prefs::default();
    let mut dst = open_dst_file(path.to_str().unwrap(), &prefs).unwrap();
    dst.write_all(b"flush me").unwrap();
    dst.flush().expect("flush must succeed");
}

/// Multiple write calls to DstFile accumulate correctly.
#[test]
fn dst_file_multiple_writes_accumulate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi_write.bin");
    let prefs = Prefs::default();
    {
        let mut dst = open_dst_file(path.to_str().unwrap(), &prefs).unwrap();
        dst.write_all(b"part1-").unwrap();
        dst.write_all(b"part2-").unwrap();
        dst.write_all(b"part3").unwrap();
        dst.flush().unwrap();
    }
    let contents = std::fs::read(&path).unwrap();
    assert_eq!(contents, b"part1-part2-part3");
}

/// Writing zero bytes to DstFile is a no-op (returns 0, no error).
#[test]
fn dst_file_write_empty_slice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty_write.bin");
    let prefs = Prefs::default();
    let mut dst = open_dst_file(path.to_str().unwrap(), &prefs).unwrap();
    let written = dst.write(b"").expect("empty write must succeed");
    assert_eq!(written, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: directory rejection, sparse_file_support warnings, display_level paths
// ─────────────────────────────────────────────────────────────────────────────

/// open_src_file with a directory path returns Err(InvalidInput).
#[test]
fn open_src_file_directory_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = open_src_file(dir.path().to_str().unwrap());
    assert!(result.is_err(), "opening a directory must fail");
}

/// open_dst_file with sparse_file_support at display_level 0
/// exercises the dst_file path without interfering with overwrite tests.
#[test]
fn open_dst_file_sparse_support() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse_dst.bin");
    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 1;
    let result = open_dst_file(path.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "open_dst_file with sparse must succeed");
}
