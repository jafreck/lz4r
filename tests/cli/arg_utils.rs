// Integration tests for task-031: cli/arg_utils.rs — Argument-Parsing Utilities
//
// Verifies parity with lz4cli.c lines 262–341:
//   - `lastNameFromPath`  → `last_name_from_path`
//   - `exeNameMatch`      → `exe_name_match`
//   - `readU32FromChar`   → `read_u32_from_str`
//   - `longCommandWArg`   → `long_command_w_arg`

use lz4::cli::arg_utils::{
    exe_name_match, last_name_from_path, long_command_w_arg, read_u32_from_str,
};

// ─────────────────────────────────────────────────────────────────────────────
// last_name_from_path  (lz4cli.c: lastNameFromPath)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn last_name_from_path_unix_single_component() {
    // "/a/b/c" → "c"
    assert_eq!(last_name_from_path("/a/b/c"), "c");
}

#[test]
fn last_name_from_path_unix_trailing_component_only() {
    // "file.txt" has no separator → full string returned
    assert_eq!(last_name_from_path("file.txt"), "file.txt");
}

#[test]
fn last_name_from_path_unix_root_slash() {
    // "/file" → "file"
    assert_eq!(last_name_from_path("/file"), "file");
}

#[test]
fn last_name_from_path_windows_backslash() {
    // "a\\b\\c" → "c"
    assert_eq!(last_name_from_path("a\\b\\c"), "c");
}

#[test]
fn last_name_from_path_windows_single_backslash() {
    // "a\\b" → "b"
    assert_eq!(last_name_from_path("a\\b"), "b");
}

#[test]
fn last_name_from_path_mixed_separators() {
    // "a/b\\c" — forward slash then backslash → "c"
    assert_eq!(last_name_from_path("a/b\\c"), "c");
}

#[test]
fn last_name_from_path_empty_string() {
    // Empty path → empty string (no separator to split on)
    assert_eq!(last_name_from_path(""), "");
}

#[test]
fn last_name_from_path_trailing_slash() {
    // "a/b/" → "" (nothing after the last slash)
    assert_eq!(last_name_from_path("a/b/"), "");
}

#[test]
fn last_name_from_path_just_slash() {
    // "/" → ""
    assert_eq!(last_name_from_path("/"), "");
}

#[test]
fn last_name_from_path_backslash_after_slash() {
    // "/path/to/dir\\file.exe" — backslash wins as the innermost separator
    assert_eq!(last_name_from_path("/path/to/dir\\file.exe"), "file.exe");
}

// ─────────────────────────────────────────────────────────────────────────────
// exe_name_match  (lz4cli.c: exeNameMatch)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn exe_name_match_exact_name() {
    // "lz4" matches name "lz4" (rest is empty → true)
    assert!(exe_name_match("lz4", "lz4"));
}

#[test]
fn exe_name_match_name_with_dot_extension() {
    // "lz4.exe" matches name "lz4" (rest starts with '.' → true)
    assert!(exe_name_match("lz4.exe", "lz4"));
}

#[test]
fn exe_name_match_name_with_multiple_dots() {
    // "lz4.1.0" — rest is ".1.0" which starts with '.' → true
    assert!(exe_name_match("lz4.1.0", "lz4"));
}

#[test]
fn exe_name_match_different_name() {
    // "lz4cat" does not match "lz4" (rest is "cat", not empty or '.')
    assert!(!exe_name_match("lz4cat", "lz4"));
}

#[test]
fn exe_name_match_prefix_mismatch() {
    // "unlz4" does not match "lz4" (no "lz4" prefix at position 0)
    assert!(!exe_name_match("unlz4", "lz4"));
}

#[test]
fn exe_name_match_empty_exe_path() {
    // "" does not match "lz4"
    assert!(!exe_name_match("", "lz4"));
}

#[test]
fn exe_name_match_empty_name() {
    // Any string matches empty name (rest is full string; must be empty or start with '.')
    // "" matches "" (rest is empty)
    assert!(exe_name_match("", ""));
}

#[test]
fn exe_name_match_empty_name_non_empty_path() {
    // "lz4" with empty name: rest = "lz4", not empty and doesn't start with '.' → false
    assert!(!exe_name_match("lz4", ""));
}

#[test]
fn exe_name_match_lz4cat_name() {
    assert!(exe_name_match("lz4cat", "lz4cat"));
    assert!(exe_name_match("lz4cat.exe", "lz4cat"));
    assert!(!exe_name_match("lz4catx", "lz4cat"));
}

#[test]
fn exe_name_match_unlz4_name() {
    assert!(exe_name_match("unlz4", "unlz4"));
    assert!(exe_name_match("unlz4.exe", "unlz4"));
    assert!(!exe_name_match("unlz4x", "unlz4"));
}

// ─────────────────────────────────────────────────────────────────────────────
// read_u32_from_str  (lz4cli.c: readU32FromChar)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn read_u32_plain_integer() {
    assert_eq!(read_u32_from_str("42"), Some((42, "")));
}

#[test]
fn read_u32_zero() {
    assert_eq!(read_u32_from_str("0"), Some((0, "")));
}

#[test]
fn read_u32_large_value() {
    assert_eq!(read_u32_from_str("1000000"), Some((1_000_000, "")));
}

#[test]
fn read_u32_empty_string_returns_none() {
    // No leading digits → None
    assert_eq!(read_u32_from_str(""), None);
}

#[test]
fn read_u32_no_leading_digits_returns_none() {
    // Suffix letter without digits → None
    assert_eq!(read_u32_from_str("K"), None);
    assert_eq!(read_u32_from_str("M"), None);
    assert_eq!(read_u32_from_str("G"), None);
}

#[test]
fn read_u32_k_suffix_shifts_10_bits() {
    // 1K = 1 << 10 = 1024
    assert_eq!(read_u32_from_str("1K"), Some((1024, "")));
}

#[test]
fn read_u32_64k() {
    assert_eq!(read_u32_from_str("64K"), Some((65_536, "")));
}

#[test]
fn read_u32_kb_suffix() {
    // "KB" — same as "K"
    assert_eq!(read_u32_from_str("64KB"), Some((65_536, "")));
}

#[test]
fn read_u32_kib_suffix() {
    // "KiB" — same as "K"
    assert_eq!(read_u32_from_str("64KiB"), Some((65_536, "")));
}

#[test]
fn read_u32_m_suffix_shifts_20_bits() {
    // 1M = 1 << 20 = 1_048_576
    assert_eq!(read_u32_from_str("1M"), Some((1_048_576, "")));
}

#[test]
fn read_u32_mb_suffix() {
    assert_eq!(read_u32_from_str("1MB"), Some((1_048_576, "")));
}

#[test]
fn read_u32_mib_suffix() {
    assert_eq!(read_u32_from_str("1MiB"), Some((1_048_576, "")));
}

#[test]
fn read_u32_g_suffix_shifts_30_bits() {
    // 1G = 1 << 30 = 1_073_741_824
    assert_eq!(read_u32_from_str("1G"), Some((1_073_741_824, "")));
}

#[test]
fn read_u32_gb_suffix() {
    assert_eq!(read_u32_from_str("1GB"), Some((1_073_741_824, "")));
}

#[test]
fn read_u32_gib_suffix() {
    assert_eq!(read_u32_from_str("1GiB"), Some((1_073_741_824, "")));
}

#[test]
fn read_u32_digits_with_trailing_alpha_no_suffix() {
    // Unknown trailing character after digits → digits parsed, no shift,
    // remainder contains the trailing char(s)
    // e.g. "5x" → Some((5, "x"))
    assert_eq!(read_u32_from_str("5x"), Some((5, "x")));
}

#[test]
fn read_u32_multiple_digits_k() {
    // 128K = 128 * 1024 = 131_072
    assert_eq!(read_u32_from_str("128K"), Some((131_072, "")));
}

#[test]
fn read_u32_4m() {
    // 4M = 4 * 1_048_576 = 4_194_304
    assert_eq!(read_u32_from_str("4M"), Some((4_194_304, "")));
}

// ─────────────────────────────────────────────────────────────────────────────
// long_command_w_arg  (lz4cli.c: longCommandWArg)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn long_command_w_arg_matching_prefix() {
    // "--block-size=64K" with prefix "--block-size=" → Some("64K")
    assert_eq!(
        long_command_w_arg("--block-size=64K", "--block-size="),
        Some("64K")
    );
}

#[test]
fn long_command_w_arg_no_match() {
    // Non-matching prefix → None
    assert_eq!(long_command_w_arg("--level=5", "--block-size="), None);
}

#[test]
fn long_command_w_arg_exact_match_returns_empty() {
    // arg equals prefix exactly → Some("")
    assert_eq!(long_command_w_arg("--fast", "--fast"), Some(""));
}

#[test]
fn long_command_w_arg_empty_prefix() {
    // Empty prefix matches everything → returns full arg
    assert_eq!(long_command_w_arg("--foo", ""), Some("--foo"));
}

#[test]
fn long_command_w_arg_both_empty() {
    assert_eq!(long_command_w_arg("", ""), Some(""));
}

#[test]
fn long_command_w_arg_arg_shorter_than_prefix() {
    // arg is shorter than prefix → None
    assert_eq!(long_command_w_arg("--b", "--block-size="), None);
}

#[test]
fn long_command_w_arg_prefix_longer_than_arg_no_match() {
    assert_eq!(long_command_w_arg("x", "xx"), None);
}
