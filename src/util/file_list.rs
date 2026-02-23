//! File list construction with recursive directory expansion.
//!
//! Given a mixed list of file and directory paths, [`create_file_list`] returns
//! a flat `Vec<PathBuf>` containing only regular files. Directories are walked
//! recursively using the [`walkdir`] crate.
//!
//! **Symlink handling**: Symlinks are never followed during directory traversal.
//! `walkdir` runs with its default `follow_links(false)` setting, so symlink
//! entries report a symlink `file_type()` rather than the target's type and are
//! excluded from the result. This prevents infinite loops from cyclic symlinks.
//! A symlink passed directly as a non-directory input is forwarded as-is.

use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Expand a mixed list of file and directory paths into a flat list of regular files.
///
/// - Paths that are already regular files are forwarded unchanged.
/// - Directories are walked recursively; only entries whose
///   `file_type().is_file()` returns `true` are included. Symlinks are excluded
///   regardless of target type — `walkdir` uses `follow_links(false)`.
/// - If any directory entry cannot be read, the walk is aborted and an
///   `io::Error` is returned. Callers that prefer best-effort enumeration
///   should handle or filter errors before calling this function.
///
/// Returns an empty `Vec` when `inputs` is empty or contains no regular files.
/// Callers should check `result.is_empty()` if a non-empty list is required.
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
                        .unwrap_or_else(|| io::Error::other(e.to_string()))
                })?;
                if entry.file_type().is_file() {
                    result.push(entry.into_path());
                }
            }
        } else {
            // Non-directory inputs are forwarded unchanged; no existence or
            // type check is performed on them.
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
