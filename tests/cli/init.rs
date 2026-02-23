// Integration tests for task-033: cli/init.rs — main Initialization and Alias Detection (Chunk 5)
//
// Verifies parity with lz4cli.c lines 388–441:
//   - `CliInit` struct holds initial state mirroring C locals + `LZ4IO_defaultPreferences`
//   - `detect_alias(argv0)` replicates the three alias checks at lines 427–439:
//       lz4cat  → decompress + stdout + multiple_inputs + display_level=1
//       unlz4   → decompress
//       lz4c    → lz4c_legacy flag

use lz4::cli::constants::{set_display_level, set_lz4c_legacy_commands};
use lz4::cli::init::detect_alias;
use lz4::cli::op_mode::{OpMode, LZ4_CLEVEL_DEFAULT, LZ4_NBWORKERS_DEFAULT};
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

/// Subprocess helper: called only when `LZ4_TEST_CLEVEL_WIRING` is set by the
/// parent test. Calls `detect_alias` in a fresh process whose environment
/// already has `LZ4_CLEVEL` set, so there is no race with other test threads.
#[test]
fn subprocess_helper_clevel_wiring() {
    if let Ok(expected) = std::env::var("LZ4_TEST_CLEVEL_WIRING") {
        let expected: i32 = expected
            .parse()
            .expect("LZ4_TEST_CLEVEL_WIRING must be numeric");
        reset_globals();
        let init = detect_alias("lz4");
        assert_eq!(
            init.c_level, expected,
            "detect_alias must propagate LZ4_CLEVEL into c_level"
        );
    }
}

#[test]
fn detect_alias_c_level_defaults_to_lz4_clevel_default() {
    // detect_alias wires c_level from init_c_level(); verify the default value
    // is propagated. Exhaustive parsing of LZ4_CLEVEL is covered by
    // init_c_level_from tests in op_mode — no env mutation needed here.
    reset_globals();
    let init = detect_alias("lz4");
    // When LZ4_CLEVEL is not overridden, c_level must equal LZ4_CLEVEL_DEFAULT (1).
    assert_eq!(init.c_level, LZ4_CLEVEL_DEFAULT);
}

#[test]
fn detect_alias_c_level_reads_env_var() {
    // Verifies the integration: detect_alias actually calls init_c_level() and
    // wires the env-var result into c_level (not just hardcoding the default).
    // Uses a subprocess so LZ4_CLEVEL=9 is isolated to a single-threaded child
    // process with no risk of racing against other concurrent test threads.
    let exe = std::env::current_exe().expect("could not find test executable");
    let output = std::process::Command::new(&exe)
        .args([
            "init::subprocess_helper_clevel_wiring",
            "--exact",
            "--nocapture",
        ])
        .env("LZ4_CLEVEL", "9")
        .env("LZ4_TEST_CLEVEL_WIRING", "9")
        .output()
        .expect("failed to spawn subprocess");
    assert_eq!(
        output.status.code(),
        Some(0),
        "detect_alias must read LZ4_CLEVEL=9 into c_level"
    );
}

#[test]
fn detect_alias_nb_workers_defaults_to_lz4_nbworkers_default() {
    // detect_alias wires nb_workers from init_nb_workers(); verify the default value
    // is propagated. Exhaustive parsing of LZ4_NBWORKERS is covered by
    // init_nb_workers_from tests in op_mode — no env mutation needed here.
    reset_globals();
    let init = detect_alias("lz4");
    // When LZ4_NBWORKERS is not overridden, nb_workers must equal LZ4_NBWORKERS_DEFAULT (0).
    assert_eq!(init.nb_workers, LZ4_NBWORKERS_DEFAULT);
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
