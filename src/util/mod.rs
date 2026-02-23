//! Utility module — Rust port of `util.h` + `util.c` from lz4-1.10.0/programs.
//!
//! All platform-specific C preprocessor branches in `util.h` are replaced by
//! Rust standard-library types and the `filetime`/`nix`/`walkdir`/`num_cpus`
//! crates. Functions that were `UTIL_STATIC` (static inline in C) become
//! regular `pub fn` here.
//!
//! Submodules correspond to logical sections of the original header:
//! - [`cores`]       — CPU core counting (`UTIL_countCores`, `util.c`)
//! - [`file_status`] — stat, isRegFile, isDirectory, setFileStat
//! - [`file_size`]   — getFileSize, getOpenFileSize, getTotalFileSize
//! - [`file_list`]   — createFileList / prepareFileList (directory traversal)

pub mod cores;
pub mod file_status;
pub mod file_size;
pub mod file_list;

// ── Re-exports at `util::` level ─────────────────────────────────────────────
// Mirrors the flat C namespace where all UTIL_* symbols are in scope after
// `#include "util.h"`.

pub use cores::count_cores;

pub use file_status::{is_directory, is_reg_file, set_file_stat};

#[cfg(unix)]
pub use file_status::is_reg_fd;

pub use file_size::{get_file_size, get_open_file_size, get_total_file_size};

pub use file_list::create_file_list;

// ── String helpers ────────────────────────────────────────────────────────────

/// Returns `true` if both string slices are equal.
///
/// Migrated from `UTIL_sameString` (util.h lines 203–209).
/// The C version accepts `const char*` and handles NULL gracefully (returns 0
/// if either is NULL); here `&str` slices are always valid, so the NULL guard
/// is not needed. The comparison is a simple `==`.
pub fn same_string(a: &str, b: &str) -> bool {
    a == b
}

// ── Sleep helpers ─────────────────────────────────────────────────────────────
// Correspond to the `UTIL_sleep` / `UTIL_sleepMilli` macros in util.h.

/// Sleep for `secs` seconds.
///
/// Corresponds to the `UTIL_sleep(s)` macro in util.h.
pub fn sleep_secs(secs: u64) {
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

/// Sleep for `millis` milliseconds.
///
/// Corresponds to the `UTIL_sleepMilli(milli)` macro in util.h.
pub fn sleep_millis(millis: u64) {
    std::thread::sleep(std::time::Duration::from_millis(millis));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_string_equal_strings() {
        assert!(same_string("hello", "hello"));
    }

    #[test]
    fn same_string_unequal_strings() {
        assert!(!same_string("hello", "world"));
    }

    #[test]
    fn same_string_empty_strings() {
        assert!(same_string("", ""));
    }

    #[test]
    fn same_string_one_empty() {
        assert!(!same_string("a", ""));
        assert!(!same_string("", "b"));
    }

    #[test]
    fn count_cores_at_least_one() {
        assert!(count_cores() >= 1);
    }
}
