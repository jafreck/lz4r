//! File size utility functions.
//!
//! Migrated from `util.h` lines 319–354 (Section 15):
//! - `UTIL_getOpenFileSize`  → [`get_open_file_size`]
//! - `UTIL_getFileSize`      → [`get_file_size`]
//! - `UTIL_getTotalFileSize` → [`get_total_file_size`]
//!
//! All platform-specific stat/fileno/fstat branches are replaced by
//! `std::fs::File::metadata()` and `std::fs::metadata()`, which return
//! `std::fs::Metadata` on every supported platform.

use std::fs::{self, File};
use std::path::Path;

/// Returns the size in bytes of the open file `file`.
///
/// Returns `0` if the underlying file descriptor does not refer to a regular
/// file (e.g. stdin/stdout, a pipe, or a directory), mirroring the C source's
/// `if (r || !UTIL_STAT_MODE_ISREG(statbuf.st_mode)) return 0` guard.
///
/// Corresponds to `UTIL_getOpenFileSize` (util.h lines 319–332).
pub fn get_open_file_size(file: &File) -> u64 {
    file.metadata()
        .ok()
        .filter(|m| m.file_type().is_file())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Returns the size in bytes of the regular file at `path`.
///
/// Returns `0` if the path does not exist, is not a regular file, or
/// cannot be stat-ted — matching the C source's
/// `if (r || !UTIL_STAT_MODE_ISREG(statbuf.st_mode)) return 0` guard.
///
/// Corresponds to `UTIL_getFileSize` (util.h lines 335–344).
pub fn get_file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .filter(|m| m.file_type().is_file())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Returns the total size in bytes of all regular files in `paths`.
///
/// Each element is processed by [`get_file_size`]; non-regular files and
/// paths that cannot be stat-ted contribute `0`, matching the C source's
/// accumulation loop.
///
/// Corresponds to `UTIL_getTotalFileSize` (util.h lines 347–354).
pub fn get_total_file_size(paths: &[&Path]) -> u64 {
    paths.iter().map(|p| get_file_size(p)).sum()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    // ── get_file_size ─────────────────────────────────────────────────────────

    #[test]
    fn get_file_size_matches_metadata_len() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bin");
        let data = b"hello lz4";
        fs::write(&path, data).unwrap();
        let expected = fs::metadata(&path).unwrap().len();
        assert_eq!(get_file_size(&path), expected);
        assert_eq!(get_file_size(&path), data.len() as u64);
    }

    #[test]
    fn get_file_size_returns_zero_for_nonexistent_path() {
        let path = Path::new("/nonexistent/__lz4_file_size_test__.bin");
        assert_eq!(get_file_size(path), 0);
    }

    #[test]
    fn get_file_size_returns_zero_for_directory() {
        let dir = TempDir::new().unwrap();
        assert_eq!(get_file_size(dir.path()), 0);
    }

    // ── get_open_file_size ────────────────────────────────────────────────────

    #[test]
    fn get_open_file_size_matches_file_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("open_test.bin");
        let data = b"open file size test";
        fs::write(&path, data).unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(get_open_file_size(&file), data.len() as u64);
    }

    #[test]
    fn get_open_file_size_of_written_file_reflects_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("written.bin");
        let mut file = File::create(&path).unwrap();
        let data = b"some content";
        file.write_all(data).unwrap();
        file.flush().unwrap();
        assert_eq!(get_open_file_size(&file), data.len() as u64);
    }

    // ── get_total_file_size ───────────────────────────────────────────────────

    #[test]
    fn get_total_file_size_empty_slice_returns_zero() {
        assert_eq!(get_total_file_size(&[]), 0);
    }

    #[test]
    fn get_total_file_size_sums_all_files() {
        let dir = TempDir::new().unwrap();
        let p1 = dir.path().join("a.bin");
        let p2 = dir.path().join("b.bin");
        fs::write(&p1, b"aaa").unwrap();
        fs::write(&p2, b"bbbbb").unwrap();
        let total = get_total_file_size(&[p1.as_path(), p2.as_path()]);
        assert_eq!(total, 3 + 5);
    }

    #[test]
    fn get_total_file_size_skips_nonexistent() {
        let dir = TempDir::new().unwrap();
        let p1 = dir.path().join("real.bin");
        fs::write(&p1, b"12345").unwrap();
        let missing = Path::new("/nonexistent/__lz4_missing__.bin");
        let total = get_total_file_size(&[p1.as_path(), missing]);
        assert_eq!(total, 5);
    }
}
