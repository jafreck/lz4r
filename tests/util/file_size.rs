// Unit tests for task-006: File-size utility functions (src/util/file_size.rs)
//
// Tests verify behavioural parity with util.h v1.10.0 (lines 319–354):
//   - get_file_size() returns the byte count for a regular file
//   - get_file_size() returns 0 for a non-existent path (C: stat fails → 0)
//   - get_file_size() returns 0 for a directory (C: !S_ISREG → 0)
//   - get_file_size() returns 0 for a symlink-to-directory (follows symlink)
//   - get_open_file_size() returns the byte count for an open File handle
//   - get_open_file_size() reflects content written before the call
//   - get_total_file_size() returns 0 for an empty slice
//   - get_total_file_size() sums sizes of multiple regular files
//   - get_total_file_size() treats non-existent / non-regular paths as 0
//   - Re-exports from lz4::util are accessible
//
// Parity notes:
//   UTIL_getFileSize / UTIL_getOpenFileSize return UTIL_FILESIZE_UNKNOWN
//   (cast to 0 here) when stat fails or the file is not a regular file.
//   UTIL_getTotalFileSize accumulates per-file sizes, contributing 0 for any
//   entry that would return UTIL_FILESIZE_UNKNOWN.

use lz4::util::file_size::{get_file_size, get_open_file_size, get_total_file_size};

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// get_file_size
// ─────────────────────────────────────────────────────────────────────────────

/// Regular file → correct byte count (UTIL_getFileSize happy path).
#[test]
fn get_file_size_returns_correct_size_for_regular_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("data.bin");
    let data = b"hello lz4";
    fs::write(&path, data).unwrap();
    assert_eq!(get_file_size(&path), data.len() as u64);
}

/// Empty file → 0 bytes (valid regular file, just empty).
#[test]
fn get_file_size_returns_zero_for_empty_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.bin");
    File::create(&path).unwrap();
    assert_eq!(get_file_size(&path), 0);
}

/// Matches std::fs::metadata().len() exactly.
#[test]
fn get_file_size_matches_metadata_len() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta_check.bin");
    fs::write(&path, b"abcdefghij").unwrap();
    let expected = fs::metadata(&path).unwrap().len();
    assert_eq!(get_file_size(&path), expected);
}

/// Non-existent path → 0 (stat fails → UTIL_FILESIZE_UNKNOWN → 0).
#[test]
fn get_file_size_returns_zero_for_nonexistent_path() {
    let path = Path::new("/nonexistent/__lz4_fs_task006_A__.bin");
    assert_eq!(get_file_size(path), 0);
}

/// Directory → 0 (!S_ISREG guard in C source).
#[test]
fn get_file_size_returns_zero_for_directory() {
    let dir = TempDir::new().unwrap();
    assert_eq!(get_file_size(dir.path()), 0);
}

/// Nested subdirectory → 0.
#[test]
fn get_file_size_returns_zero_for_nested_subdirectory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    assert_eq!(get_file_size(&sub), 0);
}

/// Symlink to regular file → correct size (metadata follows symlinks).
#[cfg(unix)]
#[test]
fn get_file_size_follows_symlink_to_regular_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("target.bin");
    let link = dir.path().join("link.bin");
    fs::write(&target, b"symlink content").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(get_file_size(&link), b"symlink content".len() as u64);
}

/// Symlink to directory → 0 (metadata follows symlink → is_dir → 0).
#[cfg(unix)]
#[test]
fn get_file_size_returns_zero_for_symlink_to_directory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    let link = dir.path().join("link_to_dir");
    fs::create_dir(&sub).unwrap();
    std::os::unix::fs::symlink(&sub, &link).unwrap();
    assert_eq!(get_file_size(&link), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// get_open_file_size
// ─────────────────────────────────────────────────────────────────────────────

/// Open regular file → correct size (UTIL_getOpenFileSize happy path).
#[test]
fn get_open_file_size_returns_correct_size() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("open_test.bin");
    let data = b"open file size test";
    fs::write(&path, data).unwrap();
    let file = File::open(&path).unwrap();
    assert_eq!(get_open_file_size(&file), data.len() as u64);
}

/// Content written before the call is reflected in the reported size.
#[test]
fn get_open_file_size_reflects_written_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("written.bin");
    let mut file = File::create(&path).unwrap();
    let data = b"some content";
    file.write_all(data).unwrap();
    file.flush().unwrap();
    assert_eq!(get_open_file_size(&file), data.len() as u64);
}

/// Larger payload returns the correct size.
#[test]
fn get_open_file_size_large_payload() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("large.bin");
    let data = vec![0xABu8; 65_536];
    fs::write(&path, &data).unwrap();
    let file = File::open(&path).unwrap();
    assert_eq!(get_open_file_size(&file), 65_536);
}

// ─────────────────────────────────────────────────────────────────────────────
// get_total_file_size
// ─────────────────────────────────────────────────────────────────────────────

/// Empty slice → 0 (no files to accumulate).
#[test]
fn get_total_file_size_empty_slice_returns_zero() {
    assert_eq!(get_total_file_size(&[]), 0);
}

/// Single file → same as get_file_size for that file.
#[test]
fn get_total_file_size_single_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("single.bin");
    fs::write(&path, b"abc").unwrap();
    assert_eq!(get_total_file_size(&[path.as_path()]), 3);
}

/// Multiple files → sum of individual sizes.
#[test]
fn get_total_file_size_sums_multiple_files() {
    let dir = TempDir::new().unwrap();
    let p1 = dir.path().join("a.bin");
    let p2 = dir.path().join("b.bin");
    let p3 = dir.path().join("c.bin");
    fs::write(&p1, b"aaa").unwrap(); // 3 bytes
    fs::write(&p2, b"bbbbb").unwrap(); // 5 bytes
    fs::write(&p3, b"cccccccc").unwrap(); // 8 bytes
    let total = get_total_file_size(&[p1.as_path(), p2.as_path(), p3.as_path()]);
    assert_eq!(total, 3 + 5 + 8);
}

/// Non-existent paths contribute 0 to the total.
#[test]
fn get_total_file_size_skips_nonexistent_paths() {
    let dir = TempDir::new().unwrap();
    let p1 = dir.path().join("real.bin");
    fs::write(&p1, b"12345").unwrap();
    let missing = Path::new("/nonexistent/__lz4_fs_task006_B__.bin");
    let total = get_total_file_size(&[p1.as_path(), missing]);
    assert_eq!(total, 5);
}

/// Directory entries in the slice contribute 0.
#[test]
fn get_total_file_size_skips_directories() {
    let dir = TempDir::new().unwrap();
    let p1 = dir.path().join("file.bin");
    fs::write(&p1, b"xyz").unwrap();
    // dir.path() itself is a directory → 0
    let total = get_total_file_size(&[p1.as_path(), dir.path()]);
    assert_eq!(total, 3);
}

/// All-nonexistent slice → 0.
#[test]
fn get_total_file_size_all_nonexistent_returns_zero() {
    let p1 = Path::new("/nonexistent/__lz4_fs_task006_C__.bin");
    let p2 = Path::new("/nonexistent/__lz4_fs_task006_D__.bin");
    assert_eq!(get_total_file_size(&[p1, p2]), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-export convenience path (lz4::util)
// ─────────────────────────────────────────────────────────────────────────────

/// Verify that get_file_size / get_open_file_size / get_total_file_size are
/// re-exported at the lz4::util level (src/util.rs pub use).
#[test]
fn reexports_accessible_from_util_module() {
    use lz4::util::{get_file_size, get_open_file_size, get_total_file_size};

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("reexport_check.bin");
    fs::write(&path, b"reexport").unwrap();

    let file = File::open(&path).unwrap();
    assert_eq!(get_file_size(&path), 8);
    assert_eq!(get_open_file_size(&file), 8);
    assert_eq!(get_total_file_size(&[path.as_path()]), 8);
}
