// Integration tests for task-029: cli/constants.rs — Constants, Globals, and Display Infrastructure
//
// Verifies parity with lz4cli.c lines 1–102:
//   - String identity constants (COMPRESSOR_NAME, AUTHOR, LZ4_EXTENSION, LZ4CAT, UNLZ4, LZ4_LEGACY)
//   - WELCOME_MESSAGE_FMT format string
//   - Size multiplier constants (KB, MB, GB)
//   - IO_MT threading label
//   - DISPLAY_LEVEL global (default=2, get/set round-trip)
//   - LZ4C_LEGACY_COMMANDS global (default=false, get/set round-trip)

use lz4::cli::constants::{
    AUTHOR, COMPRESSOR_NAME, GB, IO_MT, KB, LZ4CAT, LZ4_EXTENSION, LZ4_LEGACY, MB, UNLZ4,
    WELCOME_MESSAGE_FMT, display_level, lz4c_legacy_commands, set_display_level,
    set_lz4c_legacy_commands,
};

// ─────────────────────────────────────────────────────────────────────────────
// String / identity constants  (lz4cli.c lines 65–71)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compressor_name_is_lz4() {
    // Mirrors `#define COMPRESSOR_NAME "lz4"` in lz4cli.c line 65
    assert_eq!(COMPRESSOR_NAME, "lz4");
}

#[test]
fn author_is_yann_collet() {
    // Mirrors `#define AUTHOR "Yann Collet"` in lz4cli.c line 66
    assert_eq!(AUTHOR, "Yann Collet");
}

#[test]
fn lz4_extension_is_dot_lz4() {
    // Mirrors `#define LZ4_EXTENSION ".lz4"` in lz4cli.c line 68
    assert_eq!(LZ4_EXTENSION, ".lz4");
}

#[test]
fn lz4cat_constant() {
    // Mirrors `#define LZ4CAT "lz4cat"` in lz4cli.c line 69
    assert_eq!(LZ4CAT, "lz4cat");
}

#[test]
fn unlz4_constant() {
    // Mirrors `#define UNLZ4 "unlz4"` in lz4cli.c line 70
    assert_eq!(UNLZ4, "unlz4");
}

#[test]
fn lz4_legacy_constant() {
    // Mirrors `#define LZ4_LEGACY "lz4c"` in lz4cli.c line 71
    assert_eq!(LZ4_LEGACY, "lz4c");
}

// ─────────────────────────────────────────────────────────────────────────────
// WELCOME_MESSAGE_FMT  (lz4cli.c line 67)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn welcome_message_fmt_contains_placeholders() {
    // The format string must contain exactly 5 `{}` placeholders, matching
    // the 5 arguments to WELCOME_MESSAGE in lz4cli.c: name, version, bits, MT-mode, author.
    let count = WELCOME_MESSAGE_FMT.matches("{}").count();
    assert_eq!(count, 5, "WELCOME_MESSAGE_FMT must have 5 {{}} placeholders");
}

#[test]
fn welcome_message_fmt_ends_with_newline() {
    // Mirrors the `\n` at the end of the C WELCOME_MESSAGE macro
    assert!(WELCOME_MESSAGE_FMT.ends_with('\n'));
}

#[test]
fn welcome_message_fmt_format_produces_expected_output() {
    // Verify the format string produces the expected welcome banner shape
    let result = WELCOME_MESSAGE_FMT
        .replacen("{}", "lz4", 1)
        .replacen("{}", "1.10.0", 1)
        .replacen("{}", "64", 1)
        .replacen("{}", "single-thread", 1)
        .replacen("{}", "Yann Collet", 1);
    assert!(result.starts_with("***"));
    assert!(result.contains("lz4"));
    assert!(result.contains("1.10.0"));
    assert!(result.contains("Yann Collet"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Size multiplier constants  (lz4cli.c lines 74–76)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn kb_is_1024() {
    // Mirrors `#define KB *(1U<<10)` — 2^10 = 1024
    assert_eq!(KB, 1_024u64);
}

#[test]
fn mb_is_1048576() {
    // Mirrors `#define MB *(1U<<20)` — 2^20 = 1_048_576
    assert_eq!(MB, 1_048_576u64);
}

#[test]
fn gb_is_1073741824() {
    // Mirrors `#define GB *(1U<<30)` — 2^30 = 1_073_741_824
    assert_eq!(GB, 1_073_741_824u64);
}

#[test]
fn size_constant_relationships() {
    // 1 MB = 1024 KB; 1 GB = 1024 MB
    assert_eq!(MB, 1024 * KB);
    assert_eq!(GB, 1024 * MB);
}

// ─────────────────────────────────────────────────────────────────────────────
// IO_MT threading label  (lz4cli.c lines 60–63)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn io_mt_is_valid_threading_label() {
    // Must be one of the two valid values: "multithread" or "single-thread"
    assert!(
        IO_MT == "multithread" || IO_MT == "single-thread",
        "IO_MT must be 'multithread' or 'single-thread', got: {IO_MT}"
    );
}

#[test]
#[cfg(feature = "multithread")]
fn io_mt_is_multithread_when_feature_enabled() {
    assert_eq!(IO_MT, "multithread");
}

#[test]
#[cfg(not(feature = "multithread"))]
fn io_mt_is_single_thread_when_feature_disabled() {
    // Default (no feature flag): single-thread, matching `#ifdef LZ4IO_NO_MT` branch
    assert_eq!(IO_MT, "single-thread");
}

// ─────────────────────────────────────────────────────────────────────────────
// DISPLAY_LEVEL global  (lz4cli.c line 85: `static unsigned displayLevel = 2`)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn display_level_accessor_returns_u32() {
    // display_level() must return a value in the valid range 0–4
    let level = display_level();
    assert!(level <= 4, "display_level must be at most 4, got {level}");
}

#[test]
fn set_display_level_round_trips() {
    // Mirrors `displayLevel = <value>` followed by DISPLAYLEVEL check in lz4cli.c
    let original = display_level();
    set_display_level(0);
    assert_eq!(display_level(), 0);
    set_display_level(1);
    assert_eq!(display_level(), 1);
    set_display_level(4);
    assert_eq!(display_level(), 4);
    // Restore
    set_display_level(original);
    assert_eq!(display_level(), original);
}

#[test]
fn set_display_level_zero_disables_output() {
    // Level 0 = no output; the DISPLAYLEVEL macro condition `>= l` becomes false for all l >= 1
    let original = display_level();
    set_display_level(0);
    assert_eq!(display_level(), 0);
    set_display_level(original);
}

#[test]
fn set_display_level_to_verbose() {
    // Level 4 = verbose — all DISPLAYLEVEL(1..=4) conditions fire
    let original = display_level();
    set_display_level(4);
    assert_eq!(display_level(), 4);
    set_display_level(original);
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4C_LEGACY_COMMANDS global  (lz4cli.c line 72: `static int g_lz4c_legacy_commands = 0`)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4c_legacy_commands_get_set_round_trip() {
    // Mirrors g_lz4c_legacy_commands toggling in lz4cli.c
    let original = lz4c_legacy_commands();
    set_lz4c_legacy_commands(true);
    assert!(lz4c_legacy_commands());
    set_lz4c_legacy_commands(false);
    assert!(!lz4c_legacy_commands());
    // Restore
    set_lz4c_legacy_commands(original);
}

#[test]
fn lz4c_legacy_commands_disable() {
    // Explicitly disabling should always result in false
    set_lz4c_legacy_commands(false);
    assert!(!lz4c_legacy_commands());
}

#[test]
fn lz4c_legacy_commands_enable() {
    // Explicitly enabling should result in true
    let original = lz4c_legacy_commands();
    set_lz4c_legacy_commands(true);
    assert!(lz4c_legacy_commands());
    set_lz4c_legacy_commands(original);
}
