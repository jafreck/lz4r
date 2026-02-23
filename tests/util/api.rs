// Integration tests for task-009: util module assembly (`src/util/mod.rs`).
//
// Verifies that `util::` re-exports are reachable and behave correctly, and
// that the helpers defined directly in mod.rs (`same_string`, `sleep_secs`,
// `sleep_millis`) match the semantics of their C originals:
//
//   UTIL_sameString  → util::same_string
//   UTIL_sleep       → util::sleep_secs
//   UTIL_sleepMilli  → util::sleep_millis
//
// Re-exported symbols (already unit-tested in their submodules) are
// smoke-tested here to confirm the flat `util::` namespace works.

use lz4::util;

// ─────────────────────────────────────────────────────────────────────────────
// same_string — UTIL_sameString
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn same_string_equal_returns_true() {
    // C: UTIL_sameString("abc", "abc") == 1
    assert!(util::same_string("abc", "abc"));
}

#[test]
fn same_string_unequal_returns_false() {
    // C: UTIL_sameString("abc", "xyz") == 0
    assert!(!util::same_string("abc", "xyz"));
}

#[test]
fn same_string_both_empty_returns_true() {
    // C: UTIL_sameString("", "") == 1
    assert!(util::same_string("", ""));
}

#[test]
fn same_string_one_empty_returns_false() {
    // C: UTIL_sameString("a", "") == 0  and  UTIL_sameString("", "b") == 0
    assert!(!util::same_string("a", ""));
    assert!(!util::same_string("", "b"));
}

#[test]
fn same_string_case_sensitive() {
    // C strcmp is case-sensitive; Rust == on &str likewise.
    assert!(!util::same_string("Hello", "hello"));
    assert!(!util::same_string("HELLO", "hello"));
}

#[test]
fn same_string_unicode() {
    // Both sides equal — multibyte content.
    assert!(util::same_string("héllo", "héllo"));
    assert!(!util::same_string("héllo", "hello"));
}

#[test]
fn same_string_same_prefix_different_suffix() {
    // Ensure prefix matching does not cause false positive.
    assert!(!util::same_string("abc", "abcd"));
    assert!(!util::same_string("abcd", "abc"));
}

// ─────────────────────────────────────────────────────────────────────────────
// sleep_secs / sleep_millis — UTIL_sleep / UTIL_sleepMilli
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn sleep_millis_zero_does_not_block() {
    // Sleeping 0 ms must return promptly without panicking.
    let start = std::time::Instant::now();
    util::sleep_millis(0);
    let elapsed = start.elapsed();
    // Allow generous 500 ms upper bound to account for slow CI machines.
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "sleep_millis(0) took unexpectedly long: {elapsed:?}"
    );
}

#[test]
fn sleep_secs_zero_does_not_block() {
    // Sleeping 0 s must return promptly without panicking.
    let start = std::time::Instant::now();
    util::sleep_secs(0);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "sleep_secs(0) took unexpectedly long: {elapsed:?}"
    );
}

#[test]
fn sleep_millis_nonzero_waits_at_least_that_long() {
    // Sleep 50 ms and verify at least that much wall time passes.
    let start = std::time::Instant::now();
    util::sleep_millis(50);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(40), // 20 % margin
        "sleep_millis(50) returned too early: {elapsed:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-export smoke tests — util:: namespace is flat (mirrors C's flat namespace)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reexport_count_cores_accessible() {
    // util::count_cores must be callable and ≥ 1 (UTIL_countCores fallback is 1).
    assert!(util::count_cores() >= 1);
}

#[test]
fn reexport_is_reg_file_accessible() {
    use std::path::Path;
    // Non-existent path must return false — mirrors UTIL_isRegFile returning 0.
    assert!(!util::is_reg_file(Path::new("/nonexistent/__lz4_util_mod_test_reg__")));
}

#[test]
fn reexport_is_directory_accessible() {
    use std::path::Path;
    // Non-existent path must return false — mirrors UTIL_isDirectory returning 0.
    assert!(!util::is_directory(Path::new("/nonexistent/__lz4_util_mod_test_dir__")));
}

#[test]
fn reexport_get_file_size_accessible() {
    use std::path::Path;
    // Non-existent path returns 0 — mirrors UTIL_getFileSize returning 0 on error.
    assert_eq!(util::get_file_size(Path::new("/nonexistent/__lz4_util_mod_size__")), 0);
}

#[test]
fn reexport_get_open_file_size_accessible() {
    // Open a real temp file and confirm the re-export is reachable.
    let file = tempfile::tempfile().unwrap();
    // Empty file → 0 bytes.
    assert_eq!(util::get_open_file_size(&file), 0);
}

#[test]
fn reexport_get_total_file_size_accessible() {
    // Empty slice → 0 total, confirming re-export works.
    assert_eq!(util::get_total_file_size(&[]), 0);
}

#[test]
fn reexport_create_file_list_accessible() {
    // Empty inputs → empty list.
    let list = util::create_file_list(&[]).unwrap();
    assert!(list.is_empty());
}

#[cfg(unix)]
#[test]
fn reexport_is_reg_fd_accessible() {
    // stdin (fd 0) is a pipe/terminal in test harness, not a regular file.
    assert!(!util::is_reg_fd(0));
}
