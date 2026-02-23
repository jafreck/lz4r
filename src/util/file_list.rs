//! File list construction with recursive directory expansion.
//!
//! Replaces the three platform-specific `UTIL_prepareFileList` implementations
//! (Win32, POSIX, stub) and `UTIL_createFileList`/`UTIL_freeFileList` from
//! `util.h` sections 16–19 (lines 357–560).
//!
//! Instead of a flat heap buffer with a pointer table, this module returns a
//! `Vec<PathBuf>` — ownership is handled automatically.
//!
//! **Symlink handling**: Unlike the C POSIX implementation (which uses `stat()`
//! and therefore follows symlinks), this implementation uses `walkdir` with its
//! default `follow_links(false)` setting. Symlink entries are not treated as
//! regular files and symlinks to directories are not recursed into. This is an
//! intentional divergence that avoids infinite loops from symlink cycles.

use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Expand a mixed list of files and directories into a flat list of regular files.
///
/// Behavioural notes relative to `UTIL_createFileList`:
/// - Regular files in `inputs` are passed through unchanged.
/// - Directories are walked recursively; only regular files (`ft.is_file()`)
///   are included. Symlinks to directories are **not** followed and symlinks
///   to regular files are **not** included — walkdir's default
///   `follow_links(false)` is used, so symlink entries have a symlink
///   `file_type()` rather than the target's type.
/// - If a directory cannot be opened or a `readdir` call fails, an
///   `io::Error` is returned (the C code printed to stderr and returned 0;
///   here we surface the error to the caller instead).
///
/// Unlike `UTIL_createFileList`, which returns `NULL` when the input list
/// expands to zero files, this function returns an empty `Vec` — callers
/// should check `is_empty()` if needed.
pub fn create_file_list(inputs: &[&Path]) -> io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for input in inputs {
        if input.is_dir() {
            // Walk the directory tree without following symlinks. Symlinks to
            // directories are not recursed into; symlink entries are not
            // is_file() so they are excluded from the result.
            for entry in WalkDir::new(input) {
                let entry = entry.map_err(|e| {
                    e.io_error()
                        .map(|io| io::Error::new(io.kind(), io.to_string()))
                        .unwrap_or_else(|| io::Error::new(io::ErrorKind::Other, e.to_string()))
                })?;
                if entry.file_type().is_file() {
                    result.push(entry.into_path());
                }
            }
        } else {
            // Non-directory inputs are passed through as-is, just like the C
            // code copies the path string directly into the buffer.
            result.push(input.to_path_buf());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        fs::write(root.join("sub/b.txt"), b"b").unwrap();
        dir
    }

    #[test]
    fn expands_directory_recursively() {
        let dir = make_tree();
        let root = dir.path();
        let inputs = vec![root];
        let list = create_file_list(&inputs).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn passes_regular_file_through() {
        let dir = make_tree();
        let file = dir.path().join("a.txt");
        let inputs = vec![file.as_path()];
        let list = create_file_list(&inputs).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], file);
    }

    #[test]
    fn empty_inputs_returns_empty_list() {
        let list = create_file_list(&[]).unwrap();
        assert!(list.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_regular_file_in_direct_input_passes_through() {
        use std::os::unix::fs::symlink;
        let dir = make_tree();
        let root = dir.path();
        let target = root.join("a.txt");
        let link = root.join("link_to_a.txt");
        symlink(&target, &link).unwrap();

        // When the symlink itself is given as a direct (non-directory) input,
        // it is passed through as-is (no is_dir() check resolves the link).
        let inputs = vec![link.as_path()];
        let list = create_file_list(&inputs).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], link);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_file_inside_directory_is_excluded() {
        use std::os::unix::fs::symlink;
        let dir = make_tree();
        let root = dir.path();
        let target = root.join("a.txt");
        let link = root.join("sub/link_to_a.txt");
        symlink(&target, &link).unwrap();

        // Walk the directory; symlink entry has is_symlink() type (not is_file()),
        // so it is excluded. Only a.txt and sub/b.txt appear.
        let inputs = vec![root];
        let list = create_file_list(&inputs).unwrap();
        // a.txt, sub/b.txt — the symlink is not counted
        assert_eq!(list.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_directory_is_not_recursed_into() {
        use std::os::unix::fs::symlink;
        let dir = make_tree();
        let root = dir.path();
        // Create a separate directory with a file in it.
        let other = TempDir::new().unwrap();
        fs::write(other.path().join("c.txt"), b"c").unwrap();
        // Create a symlink inside root that points to the other directory.
        let link = root.join("link_to_other");
        symlink(other.path(), &link).unwrap();

        // Walk the root; symlink-to-directory is NOT recursed into
        // (follow_links=false default), so c.txt is not exposed.
        let inputs = vec![root];
        let list = create_file_list(&inputs).unwrap();
        // a.txt, sub/b.txt — link_to_other is not followed
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn mixed_inputs() {
        let dir = make_tree();
        let root = dir.path();
        let file = root.join("a.txt");
        // Pass one regular file and the directory containing that same file.
        let inputs = vec![file.as_path(), root];
        let list = create_file_list(&inputs).unwrap();
        // The file appears once from the direct pass-through and both files
        // appear from the directory walk: total 3 entries.
        assert_eq!(list.len(), 3);
    }
}
