//! Cross-cutting utility functions used by the CLI and I/O layers.
//!
//! Submodules:
//! - [`cores`]       — CPU core counting via [`std::thread::available_parallelism`]
//! - [`file_status`] — file-type queries (`is_reg_file`, `is_directory`, `is_reg_fd`)
//!                     and metadata mutation (`set_file_stat`)
//! - [`file_size`]   — file size queries (`get_file_size`, `get_open_file_size`,
//!                     `get_total_file_size`)
//! - [`file_list`]   — recursive directory expansion into a flat `Vec<PathBuf>`
//!
//! The most commonly needed symbols are re-exported at the `util` module level.

pub mod cores;
pub mod file_list;
pub mod file_size;
pub mod file_status;

// ── Re-exports at `util::` level ─────────────────────────────────────────────
// Commonly used symbols are re-exported here so callers can write
// `util::count_cores()` instead of `util::cores::count_cores()`.

pub use cores::count_cores;

pub use file_status::{is_directory, is_reg_file, set_file_stat};

#[cfg(unix)]
pub use file_status::is_reg_fd;

pub use file_size::{get_file_size, get_open_file_size, get_total_file_size};

pub use file_list::create_file_list;

// ── String helpers ────────────────────────────────────────────────────────────

/// Returns `true` if both string slices are equal.
///
/// Equivalent to `a == b`; provided as a named function to give call-sites a
/// self-documenting label when comparing filenames or format identifiers.
pub fn same_string(a: &str, b: &str) -> bool {
    a == b
}

// ── Sleep helpers ─────────────────────────────────────────────────────────────
// Correspond to the `UTIL_sleep` / `UTIL_sleepMilli` macros in util.h.

/// Blocks the current thread for `secs` seconds.
pub fn sleep_secs(secs: u64) {
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

/// Blocks the current thread for `millis` milliseconds.
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
