//! Operation mode selection and startup defaults for the CLI.
//!
//! This module provides:
//! - [`OpMode`] — an enum describing what the CLI should do (compress, decompress, bench, …).
//! - [`determine_op_mode`] — infers the intended mode from a filename's extension.
//! - [`init_nb_workers`] / [`init_c_level`] — read per-process defaults from environment variables.
//! - [`LZ4_CLEVEL_DEFAULT`] / [`LZ4_NBWORKERS_DEFAULT`] — fallback constants used when no
//!   environment override is present.

use crate::cli::arg_utils::read_u32_from_str;
use crate::cli::constants::{display_level, LZ4_EXTENSION};

/// Default compression level (1 — fast, lossless). Used when `LZ4_CLEVEL` is unset or invalid.
pub const LZ4_CLEVEL_DEFAULT: i32 = 1;
/// Default worker-thread count. `0` means "auto" — the runtime picks a suitable value.
pub const LZ4_NBWORKERS_DEFAULT: usize = 0;

/// What the CLI should do with its inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpMode {
    /// Mode inferred from the filename extension: decompress if `.lz4`, compress otherwise.
    Auto,
    /// Compress input to LZ4 format.
    Compress,
    /// Decompress LZ4-encoded input.
    Decompress,
    /// Verify archive integrity without writing output.
    Test,
    /// Run internal compression benchmarks.
    Bench,
    /// Print metadata about LZ4 archives.
    List,
}

/// Infer the operation mode from `filename`'s extension.
///
/// Returns [`OpMode::Decompress`] if `filename` ends with `.lz4`,
/// [`OpMode::Compress`] otherwise.
pub fn determine_op_mode(filename: &str) -> OpMode {
    if filename.ends_with(LZ4_EXTENSION) {
        OpMode::Decompress
    } else {
        OpMode::Compress
    }
}

/// Read the number of worker threads from the `LZ4_NBWORKERS` environment variable.
///
/// If the variable is set and starts with a decimal digit, it is parsed as an
/// unsigned integer. Otherwise [`LZ4_NBWORKERS_DEFAULT`] (`0` — auto) is returned.
pub fn init_nb_workers() -> usize {
    init_nb_workers_from(std::env::var("LZ4_NBWORKERS").ok().as_deref())
}

/// Testable core of [`init_nb_workers`]: parse an optional `LZ4_NBWORKERS` value.
///
/// Pass `Some(s)` with the raw string, or `None` to simulate the variable being
/// unset. Separating env-var I/O from parsing keeps the conversion logic
/// unit-testable without touching the process environment.
pub fn init_nb_workers_from(env_val: Option<&str>) -> usize {
    const ENV_NBTHREADS: &str = "LZ4_NBWORKERS";
    if let Some(env) = env_val {
        if let Some((val, _rest)) = read_u32_from_str(env) {
            return val as usize;
        }
        // Non-numeric value — warn and fall through to default.
        if display_level() >= 2 {
            eprintln!(
                "Ignore environment variable setting {}={}: not a valid unsigned value ",
                ENV_NBTHREADS, env
            );
        }
    }
    LZ4_NBWORKERS_DEFAULT
}

/// Read the default compression level from the `LZ4_CLEVEL` environment variable.
///
/// If the variable is set and starts with a decimal digit, it is parsed as an
/// unsigned integer and widened to `i32`. Otherwise [`LZ4_CLEVEL_DEFAULT`] (1)
/// is returned.
pub fn init_c_level() -> i32 {
    init_c_level_from(std::env::var("LZ4_CLEVEL").ok().as_deref())
}

/// Testable core of [`init_c_level`]: parse an optional `LZ4_CLEVEL` value.
///
/// Pass `Some(s)` with the raw string, or `None` to simulate the variable being
/// unset. Separating env-var I/O from parsing keeps the conversion logic
/// unit-testable without touching the process environment.
pub fn init_c_level_from(env_val: Option<&str>) -> i32 {
    const ENV_CLEVEL: &str = "LZ4_CLEVEL";
    if let Some(env) = env_val {
        if let Some((val, _rest)) = read_u32_from_str(env) {
            return val as i32;
        }
        // Non-numeric value — warn and fall through to default.
        if display_level() >= 2 {
            eprintln!(
                "Ignore environment variable setting {}={}: not a valid unsigned value ",
                ENV_CLEVEL, env
            );
        }
    }
    LZ4_CLEVEL_DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── OpMode enum ──────────────────────────────────────────────────────────

    #[test]
    fn op_mode_has_six_variants() {
        // Ensure all six variants exist and are distinct.
        let variants = [
            OpMode::Auto,
            OpMode::Compress,
            OpMode::Decompress,
            OpMode::Test,
            OpMode::Bench,
            OpMode::List,
        ];
        assert_eq!(variants.len(), 6);
    }

    // ── determine_op_mode ───────────────────────────────────────────────────

    #[test]
    fn determine_op_mode_lz4_extension_decompresses() {
        assert_eq!(determine_op_mode("foo.lz4"), OpMode::Decompress);
    }

    #[test]
    fn determine_op_mode_other_extension_compresses() {
        assert_eq!(determine_op_mode("foo.txt"), OpMode::Compress);
    }

    #[test]
    fn determine_op_mode_no_extension_compresses() {
        assert_eq!(determine_op_mode("archive"), OpMode::Compress);
    }

    #[test]
    fn determine_op_mode_dotlz4_only_decompresses() {
        // A filename that is exactly ".lz4" should decompress.
        assert_eq!(determine_op_mode(".lz4"), OpMode::Decompress);
    }

    // ── init_nb_workers ─────────────────────────────────────────────────────

    #[test]
    fn init_nb_workers_env_var_numeric() {
        // Safety: tests run in the same process; set and restore the variable.
        std::env::set_var("LZ4_NBWORKERS", "2");
        let result = init_nb_workers();
        std::env::remove_var("LZ4_NBWORKERS");
        assert_eq!(result, 2);
    }

    #[test]
    fn init_nb_workers_env_var_unset_returns_default() {
        std::env::remove_var("LZ4_NBWORKERS");
        assert_eq!(init_nb_workers(), LZ4_NBWORKERS_DEFAULT);
    }

    #[test]
    fn init_nb_workers_env_var_nonnumeric_returns_default() {
        std::env::set_var("LZ4_NBWORKERS", "auto");
        let result = init_nb_workers();
        std::env::remove_var("LZ4_NBWORKERS");
        assert_eq!(result, LZ4_NBWORKERS_DEFAULT);
    }

    // ── init_c_level ────────────────────────────────────────────────────────

    #[test]
    fn init_c_level_env_var_numeric() {
        std::env::set_var("LZ4_CLEVEL", "9");
        let result = init_c_level();
        std::env::remove_var("LZ4_CLEVEL");
        assert_eq!(result, 9);
    }

    #[test]
    fn init_c_level_env_var_unset_returns_default() {
        std::env::remove_var("LZ4_CLEVEL");
        assert_eq!(init_c_level(), LZ4_CLEVEL_DEFAULT);
    }

    #[test]
    fn init_c_level_env_var_nonnumeric_returns_default() {
        std::env::set_var("LZ4_CLEVEL", "fast");
        let result = init_c_level();
        std::env::remove_var("LZ4_CLEVEL");
        assert_eq!(result, LZ4_CLEVEL_DEFAULT);
    }
}
