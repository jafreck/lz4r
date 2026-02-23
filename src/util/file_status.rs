//! File status utility functions.
//!
//! Migrated from `util.h` lines 130–316 (Sections 7, 8, 13, 14):
//! - Stat macros (`UTIL_TYPE_stat`, `UTIL_stat`, `UTIL_fstat`, `UTIL_STAT_MODE_ISREG`)
//! - `fileno` macro (`UTIL_fileno`)
//! - `stat_t` typedef
//! - `UTIL_setFileStat`, `UTIL_getFDStat`, `UTIL_getFileStat`,
//!   `UTIL_isRegFD`, `UTIL_isRegFile`, `UTIL_isDirectory`
//!
//! All platform-specific preprocessor branches are replaced by Rust's
//! `std::fs::Metadata`, the `filetime` crate, and `nix` for POSIX-only
//! chown / permission operations.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use filetime::FileTime;

#[cfg(unix)]
use std::os::unix::io::RawFd;

#[cfg(windows)]
use libc;

/// Sets modification time, ownership (POSIX), and file-permission bits on a
/// regular file.
///
/// Returns `Err` if `path` is not a regular file.  On success every
/// platform-specific attribute is applied; errors from individual operations
/// accumulate and are returned as the first encountered `io::Error`.
///
/// Corresponds to `UTIL_setFileStat` (util.h lines 228–258).
///
/// * `mtime` — desired last-modification time
/// * `uid`   — desired owner UID (POSIX only; silently ignored on Windows)
/// * `gid`   — desired owner GID (POSIX only; silently ignored on Windows)
/// * `mode`  — desired permission bits; lower 12 bits are applied
///             (`mode & 0o7777`), matching the C `chmod` call
pub fn set_file_stat(
    path: &Path,
    mtime: SystemTime,
    uid: u32,
    gid: u32,
    mode: u32,
) -> io::Result<()> {
    // Mirrors: if (!UTIL_isRegFile(filename)) return -1;
    if !is_reg_file(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "set_file_stat: not a regular file",
        ));
    }

    // Set modification time and access time — replaces utime() / utimensat() branches.
    // The C code sets atime = time(NULL) (current wall-clock) and mtime = statbuf->st_mtime.
    let atime = FileTime::from_system_time(SystemTime::now());
    let ft_mtime = FileTime::from_system_time(mtime);
    filetime::set_file_times(path, atime, ft_mtime)?;

    // Copy ownership — POSIX only (chown is absent on Windows).
    #[cfg(unix)]
    {
        use nix::unistd::{chown, Gid, Uid};
        chown(path, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
            .map_err(io::Error::from)?;
    }

    // Suppress "unused variable" warnings on non-Unix targets.
    #[cfg(not(unix))]
    let _ = (uid, gid);

    // Copy file permissions — mode & 07777, matching `chmod` in the C source.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))?;
    }
    #[cfg(windows)]
    {
        // Windows does not support full POSIX mode bits; honour read-only bit only.
        let readonly = (mode & 0o200) == 0;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_readonly(readonly);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(not(any(unix, windows)))]
    let _ = mode;

    Ok(())
}

/// Returns `true` if the file descriptor `fd` refers to a regular file.
///
/// On Windows the C runtime always opens descriptors 0, 1 and 2 in text mode,
/// so they cannot be used for binary I/O; this function therefore returns
/// `false` for those values.  The POSIX path has no such restriction.
///
/// Corresponds to `UTIL_isRegFD` / `UTIL_getFDStat` (util.h lines 261–295).
#[cfg(unix)]
pub fn is_reg_fd(fd: RawFd) -> bool {
    use nix::sys::stat::{fstat, SFlag};
    match fstat(fd) {
        Ok(stat) => (stat.st_mode as u32) & (SFlag::S_IFMT.bits() as u32) == SFlag::S_IFREG.bits() as u32,
        Err(_) => false,
    }
}

/// Returns `true` if the file descriptor `fd` refers to a regular file.
///
/// Windows CRT always opens fds 0, 1, 2 in text mode — not usable for binary
/// I/O — so this function returns `false` for those values, matching the C
/// source's `if(fd < 3) return 0` guard.  For other fds `_fstat64` is used.
///
/// Corresponds to `UTIL_isRegFD` / `UTIL_getFDStat` (util.h lines 261–295).
#[cfg(windows)]
pub fn is_reg_fd(fd: i32) -> bool {
    if fd < 3 {
        return false;
    }
    unsafe {
        let mut stat_buf = std::mem::zeroed::<libc::stat>();
        if libc::fstat(fd, &mut stat_buf) != 0 {
            return false;
        }
        (stat_buf.st_mode as u32) & (libc::S_IFMT as u32) == (libc::S_IFREG as u32)
    }
}

/// Returns `true` if `path` refers to a regular file.
///
/// Returns `false` for directories, symlinks to directories, special files,
/// and paths that do not exist.
///
/// Corresponds to `UTIL_isRegFile` / `UTIL_getFileStat` (util.h lines 274–301).
pub fn is_reg_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// Returns `true` if `path` refers to a directory.
///
/// Returns `false` for regular files, special files, and paths that do not
/// exist.
///
/// Corresponds to `UTIL_isDirectory` (util.h lines 303–316).
pub fn is_directory(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::Duration;
    use tempfile::TempDir;

    // ── is_reg_file ──────────────────────────────────────────────────────────

    #[test]
    fn is_reg_file_returns_true_for_regular_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        File::create(&path).unwrap();
        assert!(is_reg_file(&path));
    }

    #[test]
    fn is_reg_file_returns_false_for_directory() {
        let dir = TempDir::new().unwrap();
        assert!(!is_reg_file(dir.path()));
    }

    #[test]
    fn is_reg_file_returns_false_for_nonexistent_path() {
        assert!(!is_reg_file(Path::new("/nonexistent/__lz4_test_path__.txt")));
    }

    // ── is_directory ─────────────────────────────────────────────────────────

    #[test]
    fn is_directory_returns_true_for_directory() {
        let dir = TempDir::new().unwrap();
        assert!(is_directory(dir.path()));
    }

    #[test]
    fn is_directory_returns_false_for_regular_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        File::create(&path).unwrap();
        assert!(!is_directory(&path));
    }

    #[test]
    fn is_directory_returns_false_for_nonexistent_path() {
        assert!(!is_directory(Path::new("/nonexistent/__lz4_test_dir__")));
    }

    // ── is_reg_fd ────────────────────────────────────────────────────────────

    /// stdin (fd 0) is a terminal / pipe in test environments, not a regular file.
    #[cfg(unix)]
    #[test]
    fn is_reg_fd_stdin_is_not_regular_file() {
        assert!(!is_reg_fd(0));
    }

    /// An fd backed by a real file on disk must be recognised as regular.
    #[cfg(unix)]
    #[test]
    fn is_reg_fd_returns_true_for_file_fd() {
        use std::os::unix::io::IntoRawFd;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fd_test.bin");
        let f = File::create(&path).unwrap();
        let fd = f.into_raw_fd();
        assert!(is_reg_fd(fd));
        // Close the fd explicitly so TempDir cleanup succeeds.
        let _ = nix::unistd::close(fd);
    }

    // ── set_file_stat ────────────────────────────────────────────────────────

    /// set_file_stat on a non-existent file must return an error.
    #[test]
    fn set_file_stat_errors_on_nonexistent_file() {
        let result = set_file_stat(
            Path::new("/nonexistent/__lz4_set_stat__.txt"),
            SystemTime::now(),
            0,
            0,
            0o644,
        );
        assert!(result.is_err());
    }

    /// set_file_stat on a directory must return an error (not a regular file).
    #[test]
    fn set_file_stat_errors_on_directory() {
        let dir = TempDir::new().unwrap();
        let result = set_file_stat(dir.path(), SystemTime::now(), 0, 0, 0o755);
        assert!(result.is_err());
    }

    /// Round-trip: set mtime → read mtime → difference must be < 1 second.
    #[test]
    fn set_file_stat_mtime_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mtime_test.txt");
        File::create(&path).unwrap();

        let target_mtime = SystemTime::now() - Duration::from_secs(3600);

        // Fetch the current uid/gid so chown is a no-op (we own the file).
        #[cfg(unix)]
        let (uid, gid) = {
            use std::os::unix::fs::MetadataExt;
            let m = fs::metadata(&path).unwrap();
            (m.uid(), m.gid())
        };
        #[cfg(not(unix))]
        let (uid, gid) = (0u32, 0u32);

        set_file_stat(&path, target_mtime, uid, gid, 0o644).unwrap();

        let actual_mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let diff = if actual_mtime >= target_mtime {
            actual_mtime.duration_since(target_mtime).unwrap()
        } else {
            target_mtime.duration_since(actual_mtime).unwrap()
        };
        assert!(
            diff < Duration::from_secs(1),
            "mtime deviation {diff:?} exceeds 1-second tolerance"
        );
    }
}
