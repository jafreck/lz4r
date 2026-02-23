//! File status queries and metadata mutation.
//!
//! Functions in this module inspect filesystem entries and, on supported
//! platforms, update file attributes:
//!
//! - [`is_reg_file`]   — true if a path refers to a regular file
//! - [`is_directory`]  — true if a path refers to a directory
//! - [`is_reg_fd`]     — true if a raw file descriptor refers to a regular
//!                       file (available on POSIX and Windows targets)
//! - [`set_file_stat`] — apply modification time, ownership (POSIX), and
//!                       permission bits to a regular file
//!
//! Ownership and permission operations use the [`filetime`] and [`nix`] crates
//! on POSIX targets and `libc` on Windows.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use filetime::FileTime;

#[cfg(unix)]
use std::os::unix::io::RawFd;

#[cfg(windows)]
use libc;

/// Apply modification time, ownership, and permission bits to a regular file.
///
/// Returns `Err` if `path` is not a regular file. Attribute operations are
/// applied in order; the first failure is returned immediately.
///
/// # Parameters
/// * `mtime` — desired last-modification time
/// * `uid`   — desired owner UID (POSIX only; ignored on other targets)
/// * `gid`   — desired owner GID (POSIX only; ignored on other targets)
/// * `mode`  — permission bits; only the lower 12 bits are applied
///             (`mode & 0o7777`), i.e. rwxrwxrwx plus the setuid/setgid/sticky
///             bits. On Windows only the read-only bit is honoured.
pub fn set_file_stat(
    path: &Path,
    mtime: SystemTime,
    uid: u32,
    gid: u32,
    mode: u32,
) -> io::Result<()> {
    if !is_reg_file(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "set_file_stat: not a regular file",
        ));
    }

    // Set modification time; access time is updated to now (current wall-clock).
    // filetime handles the platform-specific syscall (utimensat on POSIX, SetFileTime on Windows).
    let atime = FileTime::from_system_time(SystemTime::now());
    let ft_mtime = FileTime::from_system_time(mtime);
    filetime::set_file_times(path, atime, ft_mtime)?;

    // Copy ownership — POSIX only (chown is absent on Windows).
    #[cfg(unix)]
    {
        use nix::unistd::{chown, Gid, Uid};
        chown(path, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid))).map_err(io::Error::from)?;
    }

    // Suppress "unused variable" warnings on non-Unix targets.
    #[cfg(not(unix))]
    let _ = (uid, gid);

    // Apply the lower 12 permission bits (rwxrwxrwx + setuid/setgid/sticky).
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

/// Returns `true` if the raw file descriptor `fd` refers to a regular file.
///
/// Uses `fstat(2)` to query the file type. Returns `false` if the `fstat`
/// call fails (e.g. the descriptor is invalid) or the entry is not a regular
/// file (pipes, sockets, terminals, etc.).
#[cfg(unix)]
pub fn is_reg_fd(fd: RawFd) -> bool {
    use nix::sys::stat::{fstat, SFlag};
    match fstat(fd) {
        Ok(stat) => {
            (stat.st_mode as u32) & (SFlag::S_IFMT.bits() as u32) == SFlag::S_IFREG.bits() as u32
        }
        Err(_) => false,
    }
}

/// Returns `true` if the file descriptor `fd` refers to a regular file.
///
/// Descriptors 0, 1, and 2 (stdin/stdout/stderr) always return `false`: the
/// Windows CRT opens them in text mode, making them unsuitable for binary I/O.
/// For all other descriptors, `fstat` is used to check the file type.
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
/// Returns `false` for directories, symlinks, special files, and paths that
/// do not exist. Symlinks are not followed — the link itself is examined.
pub fn is_reg_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// Returns `true` if `path` refers to a directory.
///
/// Returns `false` for regular files, symlinks, special files, and paths that
/// do not exist.
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
        assert!(!is_reg_file(Path::new(
            "/nonexistent/__lz4_test_path__.txt"
        )));
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
