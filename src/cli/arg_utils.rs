//! Argument-parsing utilities for the `lz4r` CLI.
//!
//! This module provides small, focused helpers that sit between the raw
//! argument strings produced by the OS and the higher-level argument types
//! in [`crate::cli::args`].  Each function is `#[inline]`-friendly and has
//! no allocations, operating entirely on string slices.

/// Returns the last path component of `path`.
///
/// Both `/` (Unix) and `\` (Windows) path separators are recognised, so the
/// function handles paths with mixed separators correctly.
pub fn last_name_from_path(path: &str) -> &str {
    let after_slash = match path.rfind('/') {
        Some(pos) => &path[pos + 1..],
        None => path,
    };
    match after_slash.rfind('\\') {
        Some(pos) => &after_slash[pos + 1..],
        None => after_slash,
    }
}

/// Returns `true` if the basename of `exe_path` matches `name`.
///
/// A match requires `exe_path` to start with `name` and the immediately
/// following character to be either absent (exact match) or `'.'` (allowing
/// for platform suffixes such as `.exe`).
pub fn exe_name_match(exe_path: &str, name: &str) -> bool {
    if let Some(rest) = exe_path.strip_prefix(name) {
        rest.is_empty() || rest.starts_with('.')
    } else {
        false
    }
}

/// Parses an unsigned 32-bit integer from the start of `s`, optionally
/// followed by a binary size suffix.  Returns `None` if `s` contains no
/// leading digits, or `Some((value, remainder))` where `remainder` is the
/// unconsumed tail of `s`.
///
/// Recognised suffixes (case-sensitive):
///   `K` / `KB` / `KiB`  → multiply by 1 024
///   `M` / `MB` / `MiB`  → multiply by 1 048 576
///   `G` / `GB` / `GiB`  → multiply by 1 073 741 824
///
/// Arithmetic wraps silently on overflow, consistent with [`u32::wrapping_mul`].
pub fn read_u32_from_str(s: &str) -> Option<(u32, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;

    // Require at least one digit.
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }

    let mut result: u32 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        result = result
            .wrapping_mul(10)
            .wrapping_add((bytes[i] - b'0') as u32);
        i += 1;
    }

    if i < bytes.len() {
        match bytes[i] {
            b'K' => {
                result <<= 10;
                i += 1;
                if i < bytes.len() && bytes[i] == b'i' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'B' {
                    i += 1;
                }
            }
            b'M' => {
                result <<= 20;
                i += 1;
                if i < bytes.len() && bytes[i] == b'i' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'B' {
                    i += 1;
                }
            }
            b'G' => {
                result <<= 30;
                i += 1;
                if i < bytes.len() && bytes[i] == b'i' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'B' {
                    i += 1;
                }
            }
            _ => {}
        }
    }

    Some((result, &s[i..]))
}

/// Returns the value portion of a long CLI option if `arg` begins with
/// `prefix`, or `None` otherwise.
///
/// For example, `long_command_w_arg("--block-size=64K", "--block-size=")`
/// returns `Some("64K")`.  The returned slice borrows directly from `arg`.
pub fn long_command_w_arg<'a>(arg: &'a str, prefix: &str) -> Option<&'a str> {
    arg.strip_prefix(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- last_name_from_path ---

    #[test]
    fn test_last_name_from_path_unix() {
        assert_eq!(last_name_from_path("/a/b/c"), "c");
    }

    #[test]
    fn test_last_name_from_path_windows() {
        assert_eq!(last_name_from_path("a\\b"), "b");
    }

    #[test]
    fn test_last_name_from_path_no_separator() {
        assert_eq!(last_name_from_path("file.txt"), "file.txt");
    }

    #[test]
    fn test_last_name_from_path_mixed() {
        assert_eq!(last_name_from_path("a/b\\c"), "c");
    }

    // --- exe_name_match ---

    #[test]
    fn test_exe_name_match_exact() {
        assert!(exe_name_match("lz4", "lz4"));
    }

    #[test]
    fn test_exe_name_match_with_extension() {
        assert!(exe_name_match("lz4.exe", "lz4"));
    }

    #[test]
    fn test_exe_name_match_no_match() {
        assert!(!exe_name_match("lz4cat", "lz4"));
    }

    #[test]
    fn test_exe_name_match_prefix_only() {
        // "lz4catx" starts with "lz4cat" but the remainder "x" is neither
        // empty nor a '.'-prefixed extension, so it must not match.
        assert!(!exe_name_match("lz4catx", "lz4cat"));
    }

    // --- read_u32_from_str ---

    #[test]
    fn test_read_u32_plain() {
        assert_eq!(read_u32_from_str("42"), Some((42, "")));
    }

    #[test]
    fn test_read_u32_k_suffix() {
        assert_eq!(read_u32_from_str("64K"), Some((65536, "")));
    }

    #[test]
    fn test_read_u32_kb_suffix() {
        assert_eq!(read_u32_from_str("64KB"), Some((65536, "")));
    }

    #[test]
    fn test_read_u32_kib_suffix() {
        assert_eq!(read_u32_from_str("64KiB"), Some((65536, "")));
    }

    #[test]
    fn test_read_u32_m_suffix() {
        assert_eq!(read_u32_from_str("1M"), Some((1048576, "")));
    }

    #[test]
    fn test_read_u32_mb_suffix() {
        assert_eq!(read_u32_from_str("1MB"), Some((1048576, "")));
    }

    #[test]
    fn test_read_u32_mib_suffix() {
        assert_eq!(read_u32_from_str("1MiB"), Some((1048576, "")));
    }

    #[test]
    fn test_read_u32_g_suffix() {
        assert_eq!(read_u32_from_str("1G"), Some((1073741824, "")));
    }

    #[test]
    fn test_read_u32_gb_suffix() {
        assert_eq!(read_u32_from_str("1GB"), Some((1073741824, "")));
    }

    #[test]
    fn test_read_u32_gib_suffix() {
        assert_eq!(read_u32_from_str("1GiB"), Some((1073741824, "")));
    }

    #[test]
    fn test_read_u32_empty() {
        assert_eq!(read_u32_from_str(""), None);
    }

    #[test]
    fn test_read_u32_no_digits() {
        assert_eq!(read_u32_from_str("K"), None);
    }

    #[test]
    fn test_read_u32_trailing_garbage() {
        // "12Mfoo": digits "12", suffix 'M' → 12 × 2²⁰ = 12 582 912; remainder "foo"
        let (val, rest) = read_u32_from_str("12Mfoo").unwrap();
        assert_eq!(val, 12582912);
        assert_eq!(rest, "foo");
    }

    #[test]
    fn test_read_u32_plain_with_remainder() {
        // Plain number followed by non-digit, non-suffix chars
        let (val, rest) = read_u32_from_str("42xyz").unwrap();
        assert_eq!(val, 42);
        assert_eq!(rest, "xyz");
    }

    // --- long_command_w_arg ---

    #[test]
    fn test_long_command_w_arg_match() {
        assert_eq!(
            long_command_w_arg("--block-size=64K", "--block-size="),
            Some("64K")
        );
    }

    #[test]
    fn test_long_command_w_arg_no_match() {
        assert_eq!(long_command_w_arg("--level=5", "--block-size="), None);
    }

    #[test]
    fn test_long_command_w_arg_exact() {
        assert_eq!(long_command_w_arg("--fast", "--fast"), Some(""));
    }
}
