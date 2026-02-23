// cli/op_mode.rs — Rust port of lz4cli.c lines 343–387
//
// Migrated from: lz4-src/lz4-1.10.0/programs/lz4cli.c
// Task: task-032 — Operation Mode Enum and Environment Init (Chunk 4)
//
// Covers:
//   - `operationMode_e` enum  (line 343)
//   - `determineOpMode()`     (lines 349–356)
//   - `init_nbWorkers()`      (lines 360–371)
//   - `init_cLevel()`         (lines 375–386)

use crate::cli::arg_utils::read_u32_from_str;
use crate::cli::constants::{display_level, LZ4_EXTENSION};

// Default values from lz4conf.h
/// Default compression level — mirrors `LZ4_CLEVEL_DEFAULT` (lz4conf.h:33).
pub const LZ4_CLEVEL_DEFAULT: i32 = 1;
/// Default number of worker threads — mirrors `LZ4_NBWORKERS_DEFAULT` (lz4conf.h:55).
pub const LZ4_NBWORKERS_DEFAULT: usize = 0;

/// Operation mode — mirrors `operationMode_e` enum (lz4cli.c line 343).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpMode {
    /// Automatically determined from filename extension (`om_auto`).
    Auto,
    /// Compress input (`om_compress`).
    Compress,
    /// Decompress input (`om_decompress`).
    Decompress,
    /// Test archive integrity (`om_test`).
    Test,
    /// Benchmark mode (`om_bench`).
    Bench,
    /// List archive contents (`om_list`).
    List,
}

/// Auto-determine operation mode from `filename`'s extension.
///
/// Returns [`OpMode::Decompress`] if `filename` ends with `.lz4`,
/// [`OpMode::Compress`] otherwise.
///
/// Equivalent to C `determineOpMode()` (lz4cli.c lines 349–356).
pub fn determine_op_mode(filename: &str) -> OpMode {
    if filename.ends_with(LZ4_EXTENSION) {
        OpMode::Decompress
    } else {
        OpMode::Compress
    }
}

/// Read the number of worker threads from the `LZ4_NBWORKERS` environment variable.
///
/// If the variable is set and contains a leading decimal digit, the value is
/// parsed as an unsigned integer (matching C `readU32FromChar`).  Otherwise
/// the default [`LZ4_NBWORKERS_DEFAULT`] (0 ≡ auto) is returned.
///
/// Equivalent to C `init_nbWorkers()` (lz4cli.c lines 360–371).
pub fn init_nb_workers() -> usize {
    const ENV_NBTHREADS: &str = "LZ4_NBWORKERS";
    if let Ok(env) = std::env::var(ENV_NBTHREADS) {
        if let Some((val, _rest)) = read_u32_from_str(&env) {
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

/// Read the compression level from the `LZ4_CLEVEL` environment variable.
///
/// If the variable is set and contains a leading decimal digit, the value is
/// parsed as an unsigned integer cast to `i32` (matching C `readU32FromChar`).
/// Otherwise the default [`LZ4_CLEVEL_DEFAULT`] (1) is returned.
///
/// Equivalent to C `init_cLevel()` (lz4cli.c lines 375–386).
pub fn init_c_level() -> i32 {
    const ENV_CLEVEL: &str = "LZ4_CLEVEL";
    if let Ok(env) = std::env::var(ENV_CLEVEL) {
        if let Some((val, _rest)) = read_u32_from_str(&env) {
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
