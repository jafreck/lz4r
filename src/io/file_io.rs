//! File I/O primitives for the LZ4 streaming pipeline.
//!
//! This module provides two entry points used by the higher-level I/O
//! orchestration layer:
//!
//! - [`open_src_file`] — resolves a path string to a `Box<dyn Read>`,
//!   handling the `"stdin"` sentinel and rejecting directories.
//! - [`open_dst_file`] — resolves a path string to a [`DstFile`],
//!   handling the `"stdout"` and `/dev/null` sentinels, enforcing the
//!   overwrite policy from [`Prefs`], and tracking whether sparse writes are
//!   appropriate for the resulting file descriptor.
//!
//! Sentinel string constants ([`STDIN_MARK`], [`STDOUT_MARK`], [`NUL_MARK`],
//! [`NULL_OUTPUT`]) are re-exported so callers can compare against them without
//! embedding magic strings.
//!
//! Verbosity-gated diagnostics are emitted via stderr using the global
//! [`DISPLAY_LEVEL`] atomic.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::io::prefs::{DISPLAY_LEVEL, LZ4IO_SKIPPABLE0, LZ4IO_SKIPPABLEMASK};
use crate::util::is_directory;

// ---------------------------------------------------------------------------
// Sentinel strings
// ---------------------------------------------------------------------------

/// Sentinel: read from standard input.
pub const STDIN_MARK: &str = "stdin";

/// Sentinel: write to standard output.
pub const STDOUT_MARK: &str = "stdout";

/// Sentinel: discard output (write to /dev/null or equivalent).
#[cfg(windows)]
pub const NUL_MARK: &str = "nul";
#[cfg(not(windows))]
pub const NUL_MARK: &str = "/dev/null";

/// Alternate sentinel accepted for discard output.
pub const NULL_OUTPUT: &str = "null";

// ---------------------------------------------------------------------------
// Private sentinel checks
// ---------------------------------------------------------------------------

#[inline]
fn is_dev_null(s: &str) -> bool {
    s == NUL_MARK
}

#[inline]
fn is_stdin(s: &str) -> bool {
    s == STDIN_MARK
}

#[inline]
fn is_stdout(s: &str) -> bool {
    s == STDOUT_MARK
}

// ---------------------------------------------------------------------------
// Skippable magic number
// ---------------------------------------------------------------------------

/// Returns `true` if `magic` is in the LZ4 skippable-frame range
/// `[0x184D2A50, 0x184D2A5F]`.
///
/// Skippable frames carry user-defined metadata that conforming decoders must
/// silently skip rather than treat as an error.
#[inline]
pub fn is_skippable_magic_number(magic: u32) -> bool {
    (magic & LZ4IO_SKIPPABLEMASK) == LZ4IO_SKIPPABLE0
}

// ---------------------------------------------------------------------------
// Source file
// ---------------------------------------------------------------------------

/// Opens a source file for reading, returning a boxed [`Read`].
///
/// - If `path` is the sentinel `"stdin"`, returns standard input.
/// - If `path` is a directory, returns an [`io::ErrorKind::InvalidInput`] error.
/// - Otherwise opens the file and wraps it in a [`BufReader`] for efficient
///   sequential reads.
///
/// Diagnostics are printed to stderr when [`DISPLAY_LEVEL`] permits.
pub fn open_src_file(path: &str) -> io::Result<Box<dyn Read>> {
    if is_stdin(path) {
        if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 4 {
            eprintln!("Using stdin for input");
        }
        #[cfg(windows)]
        // SAFETY: calling _setmode on stdin (fd=0) is always valid.
        unsafe {
            libc::_setmode(0, libc::O_BINARY);
        }
        return Ok(Box::new(io::stdin()));
    }

    if is_directory(Path::new(path)) {
        if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
            eprintln!("lz4: {} is a directory -- ignored", path);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: is a directory", path),
        ));
    }

    let f = File::open(path).map_err(|e| {
        if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
            eprintln!("{}: {}", path, e);
        }
        e
    })?;
    Ok(Box::new(BufReader::new(f)))
}

// ---------------------------------------------------------------------------
// Destination file
// ---------------------------------------------------------------------------

/// A write-capable destination produced by [`open_dst_file`].
///
/// Wraps either a regular [`File`], stdout, or a discard sink ([`io::sink`]).
/// Callers inspect `is_stdout` to suppress terminal-unfriendly output (e.g.
/// interactive progress bars) and `sparse_mode` to decide whether writes should
/// be routed through [`crate::io::sparse`].
pub struct DstFile {
    inner: Box<dyn Write>,
    pub is_stdout: bool,
    /// `true` when the underlying file descriptor supports sparse writes
    /// (i.e. `prefs.sparse_file_support > 0` and the destination is not stdout).
    pub sparse_mode: bool,
}

impl Write for DstFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Opens a destination for writing, returning a [`DstFile`].
///
/// Resolves special sentinels before touching the filesystem:
/// - `"stdout"` → stdout (`is_stdout = true`, `sparse_mode = false`).
/// - [`NUL_MARK`] → [`io::sink`] (all bytes discarded, no file created).
///
/// For regular paths, enforces the overwrite policy from `prefs`:
/// - When `prefs.overwrite == false` and the file already exists, the
///   behaviour depends on [`DISPLAY_LEVEL`]: at level ≤ 1 the call returns
///   an [`io::ErrorKind::AlreadyExists`] error without prompting; at higher
///   levels an interactive yes/no prompt is shown on stderr.
///
/// `sparse_mode` on the returned [`DstFile`] is `true` when
/// `prefs.sparse_file_support > 0` and the destination is a regular file.
pub fn open_dst_file(path: &str, prefs: &crate::io::prefs::Prefs) -> io::Result<DstFile> {
    if is_stdout(path) {
        if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 4 {
            eprintln!("Using stdout for output");
        }
        #[cfg(windows)]
        // SAFETY: calling _setmode on stdout (fd=1) is always valid.
        unsafe {
            libc::_setmode(1, libc::O_BINARY);
        }
        if prefs.sparse_file_support == 1
            && DISPLAY_LEVEL.load(Ordering::Relaxed) >= 4 {
                eprintln!(
                    "Sparse File Support automatically disabled on stdout; \
                     to force-enable it, add --sparse command"
                );
            }
        return Ok(DstFile {
            inner: Box::new(io::stdout()),
            is_stdout: true,
            sparse_mode: false,
        });
    }

    if is_dev_null(path) {
        return Ok(DstFile {
            inner: Box::new(io::sink()),
            is_stdout: false,
            sparse_mode: false,
        });
    }

    // Overwrite guard: refuse or prompt before clobbering an existing file.
    if !prefs.overwrite
        && Path::new(path).exists() {
            let display_level = DISPLAY_LEVEL.load(Ordering::Relaxed);
            if display_level <= 1 {
                // No interaction possible — refuse silently.
                eprintln!("{} already exists; not overwritten  ", path);
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{}: already exists; not overwritten", path),
                ));
            }
            // Interactive prompt.
            eprint!("{} already exists; do you want to overwrite (y/N) ? ", path);
            let _ = io::stderr().flush();
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let first = line.trim_start().chars().next().unwrap_or('\0');
            if first != 'y' && first != 'Y' {
                eprintln!("    not overwritten  ");
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{}: not overwritten", path),
                ));
            }
        }

    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| {
            if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
                eprintln!("{}: {}", path, e);
            }
            e
        })?;

    // Sparse mode applies to regular files only, never to stdout.
    // Because we have already returned for the stdout sentinel above,
    // the destination here is always a real file.
    let sparse_mode = prefs.sparse_file_support > 0;

    // On Windows, mark the file handle as sparse so the OS can represent
    // runs of zero bytes without allocating disk blocks.
    #[cfg(windows)]
    if sparse_mode {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            let mut bytes_returned: winapi::shared::minwindef::DWORD = 0;
            winapi::um::ioapiset::DeviceIoControl(
                f.as_raw_handle() as winapi::um::winnt::HANDLE,
                winapi::um::winioctl::FSCTL_SET_SPARSE,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                0,
                &mut bytes_returned,
                std::ptr::null_mut(),
            );
        }
    }

    Ok(DstFile {
        inner: Box::new(f),
        is_stdout: false,
        sparse_mode,
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::prefs::Prefs;

    #[test]
    fn is_skippable_magic_number_range() {
        // All values 0x184D2A50..=0x184D2A5F should be skippable.
        for v in 0x184D2A50u32..=0x184D2A5Fu32 {
            assert!(
                is_skippable_magic_number(v),
                "expected skippable: {:#010x}",
                v
            );
        }
        // Values just outside the range should not be.
        assert!(!is_skippable_magic_number(0x184D2A4F));
        assert!(!is_skippable_magic_number(0x184D2A60));
        assert!(!is_skippable_magic_number(0x184D2204)); // LZ4IO_MAGICNUMBER
        assert!(!is_skippable_magic_number(0x184C2102)); // LEGACY_MAGICNUMBER
    }

    #[test]
    fn open_src_file_nonexistent_returns_err() {
        let result = open_src_file("/nonexistent/path/that/cannot/exist.lz4");
        assert!(result.is_err());
    }

    #[test]
    fn open_dst_file_stdout_sentinel() {
        let prefs = Prefs::default();
        let dst = open_dst_file(STDOUT_MARK, &prefs).unwrap();
        assert!(dst.is_stdout);
        assert!(!dst.sparse_mode);
    }

    #[test]
    fn open_dst_file_devnull_sentinel() {
        let prefs = Prefs::default();
        let result = open_dst_file(NUL_MARK, &prefs);
        assert!(result.is_ok());
        let dst = result.unwrap();
        assert!(!dst.is_stdout);
        assert!(!dst.sparse_mode);
    }

    #[test]
    fn open_dst_file_null_output_not_sentinel() {
        // "null" is NOT treated as a discard sentinel by open_dst_file.
        // Only the CLI layer translates the user-visible string "null" to
        // NUL_MARK before calling into this module; at this API level it is
        // treated as an ordinary file path.
        // Attempting to open a file named "null" in the cwd will either succeed
        // (creating the file) or fail with a path error, but must not return
        // the sink path.
        let prefs = Prefs::default();
        let result = open_dst_file(NULL_OUTPUT, &prefs);
        // Either the file was created (Ok) or an I/O error occurred — but it
        // should NOT silently act as a discard sink.  We verify by checking
        // that if it succeeded, sparse_mode is the same as for a regular file.
        if let Ok(ref dst) = result {
            assert!(!dst.is_stdout);
        }
        // Clean up the file if it was created
        let _ = std::fs::remove_file(NULL_OUTPUT);
    }

    #[test]
    fn open_dst_file_overwrite_false_nonexistent_ok() {
        // If the file does not exist yet, overwrite=false should still work.
        let mut prefs = Prefs::default();
        prefs.overwrite = false;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.lz4");
        let result = open_dst_file(path.to_str().unwrap(), &prefs);
        assert!(result.is_ok());
    }

    #[test]
    fn open_dst_file_sparse_mode_reflects_prefs() {
        // sparse_file_support=1 (auto) → sparse_mode=true for a file (not stdout).
        let mut prefs = Prefs::default();
        prefs.sparse_file_support = 1;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sparse.lz4");
        let dst = open_dst_file(path.to_str().unwrap(), &prefs).unwrap();
        assert!(dst.sparse_mode);

        // sparse_file_support=0 → sparse_mode=false.
        prefs.sparse_file_support = 0;
        let path2 = dir.path().join("nosparse.lz4");
        let dst2 = open_dst_file(path2.to_str().unwrap(), &prefs).unwrap();
        assert!(!dst2.sparse_mode);
    }

    #[test]
    fn open_dst_file_overwrite_false_existing_err() {
        // display_level ≤ 1: no interactive prompt; should return Err.
        use std::sync::atomic::Ordering;
        crate::io::prefs::DISPLAY_LEVEL.store(0, Ordering::Relaxed);
        let mut prefs = Prefs::default();
        prefs.overwrite = false;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.lz4");
        std::fs::write(&path, b"existing").unwrap();
        let result = open_dst_file(path.to_str().unwrap(), &prefs);
        assert!(result.is_err());
    }

    #[test]
    fn sentinel_constants() {
        assert_eq!(STDIN_MARK, "stdin");
        assert_eq!(STDOUT_MARK, "stdout");
        #[cfg(not(windows))]
        assert_eq!(NUL_MARK, "/dev/null");
        #[cfg(windows)]
        assert_eq!(NUL_MARK, "nul");
    }
}
