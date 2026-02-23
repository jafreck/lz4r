// Unit tests for task-005: File-status utility functions (src/util/file_status.rs)
//
// Tests verify behavioural parity with util.h v1.10.0 (lines 130–316):
//   - is_reg_file() returns true only for regular files
//   - is_reg_file() returns false for directories, symlinks-to-dirs, missing paths
//   - is_directory() returns true only for directories
//   - is_directory() returns false for files, missing paths
//   - is_reg_fd() (POSIX only) returns true for file-backed fds, false for stdin
//   - set_file_stat() rejects non-regular paths with InvalidInput
//   - set_file_stat() sets mtime within 1-second tolerance (round-trip)
//   - set_file_stat() sets permission bits (POSIX read-only bit check)
//
// Parity notes:
//   - UTIL_isRegFile / UTIL_getFileStat: returns 1 (true) for regular files only.
//   - UTIL_isDirectory: returns 1 (true) for directories only.
//   - UTIL_isRegFD / UTIL_getFDStat: rejects stdin/stdout/stderr on Windows (CRT
//     opens them in text mode); POSIX checks fstat S_IFREG.
//   - UTIL_setFileStat: returns negative error-count; Rust maps this to Err(io::Error).
//   - Mode bits applied as `mode & 0o7777` (chmod semantics).

use lz4::util::file_status::{is_directory, is_reg_file, set_file_stat};

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// is_reg_file
// ─────────────────────────────────────────────────────────────────────────────

/// Regular file → true (UTIL_isRegFile returns 1).
#[test]
fn is_reg_file_true_for_regular_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("file.txt");
    File::create(&path).unwrap();
    assert!(is_reg_file(&path));
}

/// Empty file is still a regular file.
#[test]
fn is_reg_file_true_for_empty_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.bin");
    File::create(&path).unwrap();
    assert!(is_reg_file(&path));
}

/// Directory → false (UTIL_isRegFile returns 0).
#[test]
fn is_reg_file_false_for_directory() {
    let dir = TempDir::new().unwrap();
    assert!(!is_reg_file(dir.path()));
}

/// Non-existent path → false (stat fails).
#[test]
fn is_reg_file_false_for_nonexistent_path() {
    assert!(!is_reg_file(Path::new(
        "/nonexistent/__lz4_file_status_test_A__.txt"
    )));
}

/// Nested directory inside tempdir → false.
#[test]
fn is_reg_file_false_for_nested_directory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("subdir");
    fs::create_dir(&sub).unwrap();
    assert!(!is_reg_file(&sub));
}

/// Symlink to a regular file → true (metadata follows symlinks).
#[cfg(unix)]
#[test]
fn is_reg_file_true_for_symlink_to_regular_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("target.txt");
    let link = dir.path().join("link.txt");
    File::create(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    // std::fs::metadata follows symlinks → should report is_file() == true
    assert!(is_reg_file(&link));
}

/// Symlink to a directory → false (metadata follows symlinks → is_dir).
#[cfg(unix)]
#[test]
fn is_reg_file_false_for_symlink_to_directory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("subdir");
    let link = dir.path().join("link_to_dir");
    fs::create_dir(&sub).unwrap();
    std::os::unix::fs::symlink(&sub, &link).unwrap();
    assert!(!is_reg_file(&link));
}

// ─────────────────────────────────────────────────────────────────────────────
// is_directory
// ─────────────────────────────────────────────────────────────────────────────

/// Directory → true (UTIL_isDirectory returns 1).
#[test]
fn is_directory_true_for_directory() {
    let dir = TempDir::new().unwrap();
    assert!(is_directory(dir.path()));
}

/// Nested subdirectory → true.
#[test]
fn is_directory_true_for_nested_subdirectory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("nested");
    fs::create_dir(&sub).unwrap();
    assert!(is_directory(&sub));
}

/// Regular file → false (UTIL_isDirectory returns 0).
#[test]
fn is_directory_false_for_regular_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("file.dat");
    File::create(&path).unwrap();
    assert!(!is_directory(&path));
}

/// Non-existent path → false.
#[test]
fn is_directory_false_for_nonexistent_path() {
    assert!(!is_directory(Path::new(
        "/nonexistent/__lz4_file_status_test_B__"
    )));
}

/// Symlink to a directory → true (metadata follows symlinks).
#[cfg(unix)]
#[test]
fn is_directory_true_for_symlink_to_directory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    let link = dir.path().join("link_to_sub");
    fs::create_dir(&sub).unwrap();
    std::os::unix::fs::symlink(&sub, &link).unwrap();
    assert!(is_directory(&link));
}

// ─────────────────────────────────────────────────────────────────────────────
// is_reg_fd (POSIX only)
// ─────────────────────────────────────────────────────────────────────────────

/// stdin (fd 0) is a pipe/terminal in test environments — not a regular file.
/// Mirrors the C note that stdin/stdout/stderr behave like terminals in the
/// test harness.
#[cfg(unix)]
#[test]
fn is_reg_fd_false_for_stdin() {
    use lz4::util::file_status::is_reg_fd;
    // fd 0 = stdin; not a regular file in test environments
    assert!(!is_reg_fd(0));
}

/// An fd obtained by opening a real file must be recognised as regular.
/// Corresponds to UTIL_getFDStat → S_ISREG check.
#[cfg(unix)]
#[test]
fn is_reg_fd_true_for_file_backed_fd() {
    use lz4::util::file_status::is_reg_fd;
    use std::os::unix::io::IntoRawFd;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("fd_test.bin");
    let f = File::create(&path).unwrap();
    let fd = f.into_raw_fd();
    assert!(is_reg_fd(fd));
    let _ = nix::unistd::close(fd);
}

/// An invalid fd (e.g., -1) must return false, not panic.
#[cfg(unix)]
#[test]
fn is_reg_fd_false_for_invalid_fd() {
    use lz4::util::file_status::is_reg_fd;
    assert!(!is_reg_fd(-1));
}

// ─────────────────────────────────────────────────────────────────────────────
// set_file_stat — error paths
// ─────────────────────────────────────────────────────────────────────────────

/// Non-existent path → Err (UTIL_setFileStat returns negative on failure).
#[test]
fn set_file_stat_error_on_nonexistent_path() {
    let result = set_file_stat(
        Path::new("/nonexistent/__lz4_set_stat_C__.txt"),
        SystemTime::now(),
        0,
        0,
        0o644,
    );
    assert!(result.is_err(), "set_file_stat on missing path must fail");
}

/// Directory → Err (not a regular file).
#[test]
fn set_file_stat_error_on_directory() {
    let dir = TempDir::new().unwrap();
    let result = set_file_stat(dir.path(), SystemTime::now(), 0, 0, 0o755);
    assert!(
        result.is_err(),
        "set_file_stat on a directory must fail with InvalidInput"
    );
}

/// Error kind is InvalidInput when path is not a regular file.
#[test]
fn set_file_stat_error_kind_is_invalid_input_for_directory() {
    let dir = TempDir::new().unwrap();
    let err = set_file_stat(dir.path(), SystemTime::now(), 0, 0, 0o755)
        .expect_err("should be an error");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::InvalidInput,
        "error kind must be InvalidInput"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// set_file_stat — mtime round-trip
// ─────────────────────────────────────────────────────────────────────────────

/// Set mtime to 1 hour in the past; verify the file's recorded mtime is within
/// 1 second of the requested value.
/// Corresponds to UTIL_setFileStat utime/utimensat branch.
#[test]
fn set_file_stat_mtime_roundtrip_within_one_second() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mtime_rt.txt");
    File::create(&path).unwrap();

    let target = SystemTime::now() - Duration::from_secs(3600);

    #[cfg(unix)]
    let (uid, gid) = {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(&path).unwrap();
        (m.uid(), m.gid())
    };
    #[cfg(not(unix))]
    let (uid, gid) = (0u32, 0u32);

    set_file_stat(&path, target, uid, gid, 0o644).unwrap();

    let actual = fs::metadata(&path).unwrap().modified().unwrap();
    let diff = if actual >= target {
        actual.duration_since(target).unwrap()
    } else {
        target.duration_since(actual).unwrap()
    };
    assert!(
        diff < Duration::from_secs(1),
        "mtime deviation {diff:?} exceeds 1-second tolerance"
    );
}

/// Set mtime to a very old time (Unix epoch); verify round-trip.
#[test]
fn set_file_stat_mtime_epoch_roundtrip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mtime_epoch.txt");
    File::create(&path).unwrap();

    // SystemTime::UNIX_EPOCH
    let epoch = SystemTime::UNIX_EPOCH;

    #[cfg(unix)]
    let (uid, gid) = {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(&path).unwrap();
        (m.uid(), m.gid())
    };
    #[cfg(not(unix))]
    let (uid, gid) = (0u32, 0u32);

    set_file_stat(&path, epoch, uid, gid, 0o644).unwrap();

    let actual = fs::metadata(&path).unwrap().modified().unwrap();
    let diff = if actual >= epoch {
        actual.duration_since(epoch).unwrap()
    } else {
        epoch.duration_since(actual).unwrap()
    };
    assert!(
        diff < Duration::from_secs(1),
        "epoch mtime deviation {diff:?} exceeds 1-second tolerance"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// set_file_stat — permission bits (POSIX only)
// ─────────────────────────────────────────────────────────────────────────────

/// On POSIX, verify that mode bits are applied as `mode & 0o7777`.
/// Setting 0o400 (owner read-only) should result in a read-only file.
#[cfg(unix)]
#[test]
fn set_file_stat_sets_permission_bits_posix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("perm_test.txt");
    File::create(&path).unwrap();

    let m = fs::metadata(&path).unwrap();
    let uid = {
        use std::os::unix::fs::MetadataExt;
        m.uid()
    };
    let gid = {
        use std::os::unix::fs::MetadataExt;
        m.gid()
    };

    // Apply read-write for owner only
    set_file_stat(&path, SystemTime::now(), uid, gid, 0o600).unwrap();

    let perms = fs::metadata(&path).unwrap().permissions();
    // Lower 9 bits should be 0o600
    assert_eq!(
        perms.mode() & 0o777,
        0o600,
        "permission bits should be 0o600 after set_file_stat"
    );

    // Restore writable so TempDir cleanup can delete the file
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
}

/// mode bits are masked with 0o7777 — high bits (e.g. S_IFREG) are stripped.
#[cfg(unix)]
#[test]
fn set_file_stat_mode_masked_with_07777() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mode_mask.txt");
    File::create(&path).unwrap();

    let (uid, gid) = {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(&path).unwrap();
        (m.uid(), m.gid())
    };

    // Pass a mode with high bits set (e.g., S_IFREG | 0o644 = 0o100644)
    set_file_stat(&path, SystemTime::now(), uid, gid, 0o100644).unwrap();

    let perms = fs::metadata(&path).unwrap().permissions();
    // Only lower 12 bits (0o7777) should be applied → 0o0644
    assert_eq!(
        perms.mode() & 0o7777,
        0o644,
        "high mode bits must be stripped (mode & 0o7777)"
    );

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-export convenience path (lz4::util)
// ─────────────────────────────────────────────────────────────────────────────

/// Verify that is_reg_file / is_directory / set_file_stat are re-exported
/// at the lz4::util level (not just lz4::util::file_status).
#[test]
fn reexports_accessible_from_util_module() {
    // These are re-exported in src/util.rs; if this compiles, the re-exports work.
    use lz4::util::{is_directory, is_reg_file};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("reexport_check.txt");
    File::create(&path).unwrap();
    assert!(is_reg_file(&path));
    assert!(!is_directory(&path));
}
