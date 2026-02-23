// Integration tests for task-030: cli/help.rs — Help Text Functions (Chunk 2)
//
// Verifies parity with lz4cli.c lines 104–260:
//   - print_usage       → usage() — brief usage to stderr
//   - print_usage_advanced → usage_advanced() — welcome banner + advanced options
//   - print_long_help   → usage_longhelp() — full long-form help
//   - error_out         → errorOut() — prints to stderr then exits 1
//   - print_bad_usage   → badusage() — prints "Incorrect parameters" then exits 1
//   - wait_enter        → waitEnter() — prompt and read a line
//
// Note: error_out and print_bad_usage call std::process::exit(1) and cannot be
// called directly in integration tests. They are tested via subprocess spawning
// using test helper functions. wait_enter blocks on stdin and is tested similarly.

use lz4::cli::constants::set_lz4c_legacy_commands;
use lz4::cli::help::{print_long_help, print_usage, print_usage_advanced};

// ─────────────────────────────────────────────────────────────────────────────
// Subprocess helper tests — called only when env var is set by parent test.
// The parent test spawns the test binary filtered to this specific test name.
// ─────────────────────────────────────────────────────────────────────────────

/// Subprocess helper: calls error_out when LZ4_TEST_ERROR_OUT env var is set.
/// This test is a no-op when run normally; it exits(1) only in the child process.
#[test]
fn subprocess_helper_error_out() {
    if std::env::var("LZ4_TEST_ERROR_OUT").is_ok() {
        lz4::cli::help::error_out("test error");
    }
}

/// Subprocess helper: calls print_bad_usage when LZ4_TEST_BAD_USAGE env var is set.
#[test]
fn subprocess_helper_bad_usage() {
    if std::env::var("LZ4_TEST_BAD_USAGE").is_ok() {
        lz4::cli::help::print_bad_usage("lz4");
    }
}

/// Subprocess helper: calls wait_enter with null stdin when LZ4_TEST_WAIT_ENTER is set.
#[test]
fn subprocess_helper_wait_enter() {
    if std::env::var("LZ4_TEST_WAIT_ENTER").is_ok() {
        lz4::cli::help::wait_enter();
        std::process::exit(0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// print_usage  (mirrors C usage() lz4cli.c lines 124–141)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn print_usage_does_not_panic_with_typical_name() {
    // Mirrors the basic usage() call with the default program name "lz4"
    print_usage("lz4");
}

#[test]
fn print_usage_does_not_panic_with_empty_name() {
    // Edge case: empty program name should not panic
    print_usage("");
}

#[test]
fn print_usage_does_not_panic_with_path() {
    // Edge case: full path as argv[0]
    print_usage("/usr/local/bin/lz4");
}

#[test]
fn print_usage_does_not_panic_with_lz4cat_and_unlz4() {
    // lz4cat and unlz4 also call print_usage — mirror lz4cli.c exeName variants
    print_usage("lz4cat");
    print_usage("unlz4");
}

#[test]
fn print_usage_does_not_panic_with_unicode_name() {
    // Edge case: unicode in program name should not panic
    print_usage("lz4-ü");
}

// ─────────────────────────────────────────────────────────────────────────────
// print_usage_advanced  (mirrors C usage_advanced() lz4cli.c lines 143–185)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn print_usage_advanced_does_not_panic_with_typical_name() {
    // Mirrors usage_advanced("lz4") — the common case in main()
    print_usage_advanced("lz4");
}

#[test]
fn print_usage_advanced_does_not_panic_with_empty_name() {
    print_usage_advanced("");
}

#[test]
fn print_usage_advanced_with_lz4c_legacy_enabled() {
    // When invoked as lz4c, lz4c_legacy_commands() returns true and the
    // legacy arguments section is printed — mirrors lines 177–183 of lz4cli.c
    set_lz4c_legacy_commands(true);
    print_usage_advanced("lz4c");
    set_lz4c_legacy_commands(false);
}

#[test]
fn print_usage_advanced_with_lz4c_legacy_disabled() {
    // Non-legacy path: legacy section is suppressed — mirrors default state
    set_lz4c_legacy_commands(false);
    print_usage_advanced("lz4");
}

// ─────────────────────────────────────────────────────────────────────────────
// print_long_help  (mirrors C usage_longhelp() lz4cli.c lines 187–246)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn print_long_help_does_not_panic_with_typical_name() {
    // Mirrors usage_longhelp("lz4")
    print_long_help("lz4");
}

#[test]
fn print_long_help_does_not_panic_with_empty_name() {
    print_long_help("");
}

#[test]
fn print_long_help_with_lz4c_legacy_enabled() {
    // When lz4c_legacy_commands() is true, extra legacy warning block is printed
    // — mirrors lines 234–244 of lz4cli.c
    set_lz4c_legacy_commands(true);
    print_long_help("lz4c");
    set_lz4c_legacy_commands(false);
}

#[test]
fn print_long_help_with_lz4c_legacy_disabled() {
    set_lz4c_legacy_commands(false);
    print_long_help("lz4");
}

#[test]
fn print_long_help_does_not_panic_with_path_name() {
    print_long_help("/usr/bin/lz4");
}

// ─────────────────────────────────────────────────────────────────────────────
// error_out  (mirrors C errorOut() lz4cli.c lines 104–107)
// Calls process::exit(1) — tested via subprocess.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn error_out_exits_with_code_1() {
    // Spawn a child process that runs subprocess_helper_error_out with the env var set.
    // Mirrors `errorOut` which always calls exit(1).
    let exe = std::env::current_exe().expect("could not find test executable");
    let output = std::process::Command::new(&exe)
        .args([
            "help::subprocess_helper_error_out",
            "--exact",
            "--nocapture",
        ])
        .env("LZ4_TEST_ERROR_OUT", "1")
        .output()
        .expect("failed to spawn subprocess");
    assert_eq!(
        output.status.code(),
        Some(1),
        "error_out must exit with code 1"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// print_bad_usage  (mirrors C badusage() lz4cli.c lines 248–253)
// Calls process::exit(1) — tested via subprocess.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn print_bad_usage_exits_with_code_1() {
    // Spawn a child process that runs subprocess_helper_bad_usage with env var set.
    // Mirrors `badusage` which always calls exit(1).
    let exe = std::env::current_exe().expect("could not find test executable");
    let output = std::process::Command::new(&exe)
        .args([
            "help::subprocess_helper_bad_usage",
            "--exact",
            "--nocapture",
        ])
        .env("LZ4_TEST_BAD_USAGE", "1")
        .output()
        .expect("failed to spawn subprocess");
    assert_eq!(
        output.status.code(),
        Some(1),
        "print_bad_usage must exit with code 1"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// wait_enter  (mirrors C waitEnter() lz4cli.c lines 256–260)
// Reads a line from stdin — tested with closed stdin (EOF) via subprocess.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn wait_enter_returns_gracefully_on_eof_stdin() {
    // Spawn a child process with stdin closed; wait_enter should complete
    // gracefully on EOF, then exit(0). Mirrors waitEnter's EOF handling.
    let exe = std::env::current_exe().expect("could not find test executable");
    let output = std::process::Command::new(&exe)
        .args([
            "help::subprocess_helper_wait_enter",
            "--exact",
            "--nocapture",
        ])
        .env("LZ4_TEST_WAIT_ENTER", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed to spawn subprocess");
    assert_eq!(
        output.status.code(),
        Some(0),
        "wait_enter must return gracefully on EOF stdin"
    );
}
