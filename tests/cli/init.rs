// Integration tests for task-033: cli/init.rs — main Initialization and Alias Detection (Chunk 5)
//
// Verifies parity with lz4cli.c lines 388–441:
//   - `CliInit` struct holds initial state mirroring C locals + `LZ4IO_defaultPreferences`
//   - `detect_alias(argv0)` replicates the three alias checks at lines 427–439:
//       lz4cat  → decompress + stdout + multiple_inputs + display_level=1
//       unlz4   → decompress
//       lz4c    → lz4c_legacy flag

use lz4::cli::init::detect_alias;
use lz4::cli::op_mode::OpMode;
use lz4::cli::constants::{set_display_level, set_lz4c_legacy_commands};
use lz4::io::file_io::STDOUT_MARK;

// Helper: reset global state so tests don't interfere with each other.
fn reset_globals() {
    set_display_level(2);
    set_lz4c_legacy_commands(false);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4cat alias  (lz4cli.c lines 428–437)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4cat_sets_decompress_mode() {
    // lz4cat → mode = om_decompress
    reset_globals();
    let init = detect_alias("lz4cat");
    assert_eq!(init.op_mode, OpMode::Decompress);
}

#[test]
fn lz4cat_sets_multiple_inputs() {
    // lz4cat → multiple_inputs = 1
    reset_globals();
    let init = detect_alias("lz4cat");
    assert!(init.multiple_inputs);
}

#[test]
fn lz4cat_sets_force_stdout() {
    // lz4cat → forceStdout = 1
    reset_globals();
    let init = detect_alias("lz4cat");
    assert!(init.force_stdout);
}

#[test]
fn lz4cat_sets_output_filename_to_stdout_mark() {
    // lz4cat → output_filename = stdoutmark
    reset_globals();
    let init = detect_alias("lz4cat");
    assert_eq!(init.output_filename.as_deref(), Some(STDOUT_MARK));
}

#[test]
fn lz4cat_sets_display_level_override_to_1() {
    // lz4cat → displayLevel = 1 (returned as override so caller can apply it)
    reset_globals();
    let init = detect_alias("lz4cat");
    assert_eq!(init.display_level_override, Some(1));
}

#[test]
fn lz4cat_sets_overwrite_and_pass_through() {
    // lz4cat → overwrite=1, passThrough=1, removeSrc=0
    reset_globals();
    let init = detect_alias("lz4cat");
    assert!(init.prefs.overwrite);
    assert!(init.prefs.pass_through);
    assert!(!init.prefs.remove_src_file);
}

#[test]
fn lz4cat_does_not_set_lz4c_legacy() {
    // lz4cat does not set legacy flag
    reset_globals();
    let init = detect_alias("lz4cat");
    assert!(!init.lz4c_legacy);
}

#[test]
fn lz4cat_with_path_prefix() {
    // argv[0] may include a path — last_name_from_path must strip it (lz4cli.c line 412)
    reset_globals();
    let init = detect_alias("/usr/bin/lz4cat");
    assert_eq!(init.op_mode, OpMode::Decompress);
    assert!(init.multiple_inputs);
}

#[test]
fn lz4cat_with_exe_extension() {
    // On Windows argv[0] may have ".exe"; exeNameMatch must still match
    reset_globals();
    let init = detect_alias("lz4cat.exe");
    assert_eq!(init.op_mode, OpMode::Decompress);
    assert!(init.multiple_inputs);
}

// ─────────────────────────────────────────────────────────────────────────────
// unlz4 alias  (lz4cli.c line 438)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn unlz4_sets_decompress_mode() {
    // unlz4 → mode = om_decompress
    reset_globals();
    let init = detect_alias("unlz4");
    assert_eq!(init.op_mode, OpMode::Decompress);
}

#[test]
fn unlz4_does_not_set_multiple_inputs() {
    // unlz4 does NOT set multiple_inputs (only lz4cat does)
    reset_globals();
    let init = detect_alias("unlz4");
    assert!(!init.multiple_inputs);
}

#[test]
fn unlz4_does_not_set_force_stdout() {
    // unlz4 does NOT set forceStdout (only lz4cat does)
    reset_globals();
    let init = detect_alias("unlz4");
    assert!(!init.force_stdout);
}

#[test]
fn unlz4_output_filename_is_none() {
    // unlz4 does NOT set output_filename
    reset_globals();
    let init = detect_alias("unlz4");
    assert!(init.output_filename.is_none());
}

#[test]
fn unlz4_display_level_override_is_none() {
    // unlz4 does NOT set display level override
    reset_globals();
    let init = detect_alias("unlz4");
    assert!(init.display_level_override.is_none());
}

#[test]
fn unlz4_does_not_set_lz4c_legacy() {
    reset_globals();
    let init = detect_alias("unlz4");
    assert!(!init.lz4c_legacy);
}

#[test]
fn unlz4_with_exe_extension() {
    // exeNameMatch handles ".exe" suffix
    reset_globals();
    let init = detect_alias("unlz4.exe");
    assert_eq!(init.op_mode, OpMode::Decompress);
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4c (legacy) alias  (lz4cli.c line 439)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4c_sets_lz4c_legacy_field() {
    // lz4c → g_lz4c_legacy_commands = 1; reflected as lz4c_legacy field
    reset_globals();
    let init = detect_alias("lz4c");
    assert!(init.lz4c_legacy);
}

#[test]
fn lz4c_op_mode_is_auto() {
    // lz4c sets legacy flag but does NOT change the operation mode
    reset_globals();
    let init = detect_alias("lz4c");
    assert_eq!(init.op_mode, OpMode::Auto);
}

#[test]
fn lz4c_does_not_set_multiple_inputs() {
    reset_globals();
    let init = detect_alias("lz4c");
    assert!(!init.multiple_inputs);
}

#[test]
fn lz4c_does_not_set_force_stdout() {
    reset_globals();
    let init = detect_alias("lz4c");
    assert!(!init.force_stdout);
}

// ─────────────────────────────────────────────────────────────────────────────
// plain lz4 (no alias)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4_returns_auto_mode() {
    // Plain "lz4" binary → op_mode = Auto (no alias match)
    reset_globals();
    let init = detect_alias("lz4");
    assert_eq!(init.op_mode, OpMode::Auto);
}

#[test]
fn lz4_returns_no_legacy_flag() {
    reset_globals();
    let init = detect_alias("lz4");
    assert!(!init.lz4c_legacy);
}

#[test]
fn lz4_returns_no_multiple_inputs() {
    reset_globals();
    let init = detect_alias("lz4");
    assert!(!init.multiple_inputs);
}

#[test]
fn lz4_returns_no_force_stdout() {
    reset_globals();
    let init = detect_alias("lz4");
    assert!(!init.force_stdout);
}

#[test]
fn lz4_output_filename_is_none() {
    reset_globals();
    let init = detect_alias("lz4");
    assert!(init.output_filename.is_none());
}

#[test]
fn lz4_display_level_override_is_none() {
    reset_globals();
    let init = detect_alias("lz4");
    assert!(init.display_level_override.is_none());
}

#[test]
fn lz4_overwrite_is_false() {
    // C main() calls LZ4IO_setOverwrite(prefs, 0) for the normal (non-lz4cat) path.
    reset_globals();
    let init = detect_alias("lz4");
    assert!(!init.prefs.overwrite);
}

// ─────────────────────────────────────────────────────────────────────────────
// unrecognised binary name
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn unknown_binary_returns_auto_mode() {
    reset_globals();
    let init = detect_alias("my-lz4-wrapper");
    assert_eq!(init.op_mode, OpMode::Auto);
}

#[test]
fn unknown_binary_no_flags_set() {
    reset_globals();
    let init = detect_alias("my-lz4-wrapper");
    assert!(!init.lz4c_legacy);
    assert!(!init.multiple_inputs);
    assert!(!init.force_stdout);
    assert!(init.output_filename.is_none());
    assert!(init.display_level_override.is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// CliInit struct — c_level and nb_workers initialised from environment
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn detect_alias_includes_c_level_from_env() {
    // c_level comes from LZ4_CLEVEL env var (mirrors init_cLevel in lz4cli.c)
    std::env::remove_var("LZ4_CLEVEL");
    reset_globals();
    let init = detect_alias("lz4");
    // Default is 1 (LZ4_CLEVEL_DEFAULT from lz4conf.h)
    assert_eq!(init.c_level, 1);
}

#[test]
fn detect_alias_c_level_reads_env_var() {
    std::env::set_var("LZ4_CLEVEL", "9");
    reset_globals();
    let init = detect_alias("lz4");
    std::env::remove_var("LZ4_CLEVEL");
    assert_eq!(init.c_level, 9);
}

#[test]
fn detect_alias_includes_nb_workers_from_env() {
    // nb_workers comes from LZ4_NBWORKERS env var (mirrors init_nbWorkers in lz4cli.c)
    std::env::remove_var("LZ4_NBWORKERS");
    reset_globals();
    let init = detect_alias("lz4");
    // Default is 0 (LZ4_NBWORKERS_DEFAULT)
    assert_eq!(init.nb_workers, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// CliInit struct — Clone derives (used when passing around init state)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cli_init_clone() {
    reset_globals();
    let init = detect_alias("lz4");
    let cloned = init.clone();
    assert_eq!(init.op_mode, cloned.op_mode);
    assert_eq!(init.lz4c_legacy, cloned.lz4c_legacy);
    assert_eq!(init.multiple_inputs, cloned.multiple_inputs);
    assert_eq!(init.force_stdout, cloned.force_stdout);
    assert_eq!(init.output_filename, cloned.output_filename);
    assert_eq!(init.display_level_override, cloned.display_level_override);
    assert_eq!(init.c_level, cloned.c_level);
    assert_eq!(init.nb_workers, cloned.nb_workers);
}
