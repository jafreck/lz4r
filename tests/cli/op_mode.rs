// Integration tests for task-032: cli/op_mode.rs — Operation Mode Enum and Environment Init
//
// Verifies parity with lz4cli.c lines 343–387:
//   - `operationMode_e`  → `OpMode` enum (6 variants)
//   - `determineOpMode` → `determine_op_mode`
//   - `init_nbWorkers`  → `init_nb_workers`
//   - `init_cLevel`     → `init_c_level`

use lz4::cli::op_mode::{
    determine_op_mode, init_c_level, init_nb_workers, OpMode, LZ4_CLEVEL_DEFAULT,
    LZ4_NBWORKERS_DEFAULT,
};

// ─────────────────────────────────────────────────────────────────────────────
// OpMode enum  (lz4cli.c: operationMode_e line 343)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn op_mode_variants_are_distinct() {
    // All six variants must exist and be mutually distinct.
    assert_ne!(OpMode::Auto, OpMode::Compress);
    assert_ne!(OpMode::Compress, OpMode::Decompress);
    assert_ne!(OpMode::Decompress, OpMode::Test);
    assert_ne!(OpMode::Test, OpMode::Bench);
    assert_ne!(OpMode::Bench, OpMode::List);
}

#[test]
fn op_mode_copy_clone() {
    // OpMode must implement Copy and Clone (used across call boundaries in lz4cli.c).
    let a = OpMode::Compress;
    let b = a; // Copy
    let c = a.clone(); // Clone
    assert_eq!(a, b);
    assert_eq!(a, c);
}

#[test]
fn op_mode_debug_format() {
    // Debug must be implemented (used in error messages).
    let s = format!("{:?}", OpMode::Decompress);
    assert!(!s.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// determine_op_mode  (lz4cli.c: determineOpMode lines 349–356)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn determine_op_mode_lz4_extension_returns_decompress() {
    // Files ending with ".lz4" must be decompressed — mirrors lz4cli.c line 352.
    assert_eq!(determine_op_mode("archive.lz4"), OpMode::Decompress);
}

#[test]
fn determine_op_mode_other_extension_returns_compress() {
    // Files not ending with ".lz4" must be compressed — mirrors lz4cli.c line 354.
    assert_eq!(determine_op_mode("file.txt"), OpMode::Compress);
}

#[test]
fn determine_op_mode_no_extension_returns_compress() {
    // No extension → compress.
    assert_eq!(determine_op_mode("README"), OpMode::Compress);
}

#[test]
fn determine_op_mode_dotlz4_only() {
    // A filename that is exactly ".lz4" still ends with ".lz4" → decompress.
    assert_eq!(determine_op_mode(".lz4"), OpMode::Decompress);
}

#[test]
fn determine_op_mode_lz4_extension_case_sensitive() {
    // The C strcmp is case-sensitive on Linux; ".LZ4" must NOT match.
    assert_eq!(determine_op_mode("archive.LZ4"), OpMode::Compress);
}

#[test]
fn determine_op_mode_lz4_in_middle_does_not_match() {
    // ".lz4" occurring in the middle of the name must not trigger decompress.
    assert_eq!(determine_op_mode("file.lz4.bak"), OpMode::Compress);
}

#[test]
fn determine_op_mode_empty_string_returns_compress() {
    // Empty filename: does not end with ".lz4" → compress.
    assert_eq!(determine_op_mode(""), OpMode::Compress);
}

#[test]
fn determine_op_mode_tar_lz4_decompresses() {
    // Common double-extension archive.
    assert_eq!(determine_op_mode("backup.tar.lz4"), OpMode::Decompress);
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_CLEVEL_DEFAULT / LZ4_NBWORKERS_DEFAULT constants
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn clevel_default_is_1() {
    // lz4conf.h:33 defines LZ4_CLEVEL_DEFAULT as 1.
    assert_eq!(LZ4_CLEVEL_DEFAULT, 1_i32);
}

#[test]
fn nbworkers_default_is_0() {
    // lz4conf.h:55 defines LZ4_NBWORKERS_DEFAULT as 0 (auto).
    assert_eq!(LZ4_NBWORKERS_DEFAULT, 0_usize);
}

// ─────────────────────────────────────────────────────────────────────────────
// init_nb_workers  (lz4cli.c: init_nbWorkers lines 360–371)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn init_nb_workers_numeric_env_var_is_parsed() {
    // When LZ4_NBWORKERS contains a numeric string, parse it.
    // Mirrors init_nbWorkers() which calls readU32FromChar on the env value.
    std::env::set_var("LZ4_NBWORKERS", "4");
    let result = init_nb_workers();
    std::env::remove_var("LZ4_NBWORKERS");
    assert_eq!(result, 4);
}

#[test]
fn init_nb_workers_zero_env_var() {
    std::env::set_var("LZ4_NBWORKERS", "0");
    let result = init_nb_workers();
    std::env::remove_var("LZ4_NBWORKERS");
    assert_eq!(result, 0);
}

#[test]
fn init_nb_workers_unset_returns_default() {
    // Unset variable → default value.
    std::env::remove_var("LZ4_NBWORKERS");
    assert_eq!(init_nb_workers(), LZ4_NBWORKERS_DEFAULT);
}

#[test]
fn init_nb_workers_non_numeric_returns_default() {
    // Non-numeric string (no leading digit) → default.
    // Mirrors the C path where readU32FromChar returns 0 on non-digit.
    std::env::set_var("LZ4_NBWORKERS", "auto");
    let result = init_nb_workers();
    std::env::remove_var("LZ4_NBWORKERS");
    assert_eq!(result, LZ4_NBWORKERS_DEFAULT);
}

#[test]
fn init_nb_workers_empty_env_var_returns_default() {
    // Empty string has no leading digit → default.
    std::env::set_var("LZ4_NBWORKERS", "");
    let result = init_nb_workers();
    std::env::remove_var("LZ4_NBWORKERS");
    assert_eq!(result, LZ4_NBWORKERS_DEFAULT);
}

// ─────────────────────────────────────────────────────────────────────────────
// init_c_level  (lz4cli.c: init_cLevel lines 375–386)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn init_c_level_numeric_env_var_is_parsed() {
    // When LZ4_CLEVEL contains a numeric string, parse and cast to i32.
    std::env::set_var("LZ4_CLEVEL", "9");
    let result = init_c_level();
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(result, 9_i32);
}

#[test]
fn init_c_level_level_12() {
    // High compression level.
    std::env::set_var("LZ4_CLEVEL", "12");
    let result = init_c_level();
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(result, 12_i32);
}

#[test]
fn init_c_level_level_1() {
    // Minimum non-zero level.
    std::env::set_var("LZ4_CLEVEL", "1");
    let result = init_c_level();
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(result, 1_i32);
}

#[test]
fn init_c_level_unset_returns_default() {
    // Unset variable → LZ4_CLEVEL_DEFAULT (1).
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(init_c_level(), LZ4_CLEVEL_DEFAULT);
}

#[test]
fn init_c_level_non_numeric_returns_default() {
    // Non-numeric string → default.
    std::env::set_var("LZ4_CLEVEL", "fast");
    let result = init_c_level();
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(result, LZ4_CLEVEL_DEFAULT);
}

#[test]
fn init_c_level_empty_env_var_returns_default() {
    // Empty string → default.
    std::env::set_var("LZ4_CLEVEL", "");
    let result = init_c_level();
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(result, LZ4_CLEVEL_DEFAULT);
}
