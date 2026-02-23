// cli/constants.rs — Rust port of lz4cli.c lines 1–102 (declarations #1, #2, #3, #4, #21)
//
// Migrated from: lz4-src/lz4-1.10.0/programs/lz4cli.c
// Task: task-029 — Constants, Globals, and Display Infrastructure (Chunk 1)

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── String / identity constants (lz4cli.c lines 65–71) ────────────────────────
pub const COMPRESSOR_NAME: &str = "lz4";
pub const AUTHOR: &str = "Yann Collet";
pub const LZ4_EXTENSION: &str = ".lz4";
pub const LZ4CAT: &str = "lz4cat";
pub const UNLZ4: &str = "unlz4";
pub const LZ4_LEGACY: &str = "lz4c";

/// Welcome message format — matches WELCOME_MESSAGE macro in lz4cli.c line 67.
/// Caller substitutes: compressor name, version string, pointer-width bits, threading mode, author.
pub const WELCOME_MESSAGE_FMT: &str = "*** {} v{} {}-bit {}, by {} ***\n";

// ── Size multiplier constants (lz4cli.c lines 74–76) ──────────────────────────
/// 1 KiB  — mirrors `#define KB *(1U<<10)`
pub const KB: u64 = 1 << 10;
/// 1 MiB  — mirrors `#define MB *(1U<<20)`
pub const MB: u64 = 1 << 20;
/// 1 GiB  — mirrors `#define GB *(1U<<30)`
pub const GB: u64 = 1 << 30;

// ── Threading-mode label (lz4cli.c lines 60–63) ───────────────────────────────
/// Corresponds to the `IO_MT` macro: "multithread" when the `multithread` feature is enabled,
/// "single-thread" otherwise.
#[cfg(feature = "multithread")]
pub const IO_MT: &str = "multithread";
#[cfg(not(feature = "multithread"))]
pub const IO_MT: &str = "single-thread";

// ── Display level global (lz4cli.c line 85) ───────────────────────────────────
//
// In the C source, `static unsigned displayLevel = 2` is a file-scoped global
// used by the DISPLAYLEVEL macro throughout lz4cli.c.
//
// In Rust, this is a crate-level atomic so it can be shared across modules.
// When `crate::io::prefs` is implemented (task-011), `DISPLAY_LEVEL` there
// will be the authoritative source and this will become a thin alias.
//
// 0 = no output; 1 = errors only; 2 = normal (downgradable); 3 = non-downgradable; 4 = verbose
pub static DISPLAY_LEVEL: AtomicU32 = AtomicU32::new(2);

/// Returns the current display level.
#[inline]
pub fn display_level() -> u32 {
    DISPLAY_LEVEL.load(Ordering::Relaxed)
}

/// Sets the display level.
#[inline]
pub fn set_display_level(level: u32) {
    DISPLAY_LEVEL.store(level, Ordering::Relaxed);
}

// ── Legacy-command global (lz4cli.c line 72) ──────────────────────────────────
//
// `static int g_lz4c_legacy_commands = 0;` — set to 1 when the binary is invoked
// as "lz4c", enabling alternate short-option spellings (`-c0`, `-c1`, `-hc`, `-y`).
//
// Modelled as an atomic bool; callers may also pass this as a function argument
// to avoid global state in unit tests.
pub static LZ4C_LEGACY_COMMANDS: AtomicBool = AtomicBool::new(false);

/// Returns `true` when legacy lz4c command mode is active.
#[inline]
pub fn lz4c_legacy_commands() -> bool {
    LZ4C_LEGACY_COMMANDS.load(Ordering::Relaxed)
}

/// Enables or disables legacy lz4c command mode.
#[inline]
pub fn set_lz4c_legacy_commands(enabled: bool) {
    LZ4C_LEGACY_COMMANDS.store(enabled, Ordering::Relaxed);
}

// ── Display helpers (lz4cli.c lines 82–85) ────────────────────────────────────
//
// The C macros DISPLAYOUT, DISPLAY, and DISPLAYLEVEL are replaced by these
// helper macros / inline functions:
//
//   DISPLAYOUT(...)      → print!(...) / use `displayout!` macro
//   DISPLAY(...)         → eprint!(...) / use `display!` macro
//   DISPLAYLEVEL(l, ...) → if display_level() >= l { eprint!(...) }

/// Print to stdout — equivalent to C `DISPLAYOUT(...)`.
#[macro_export]
macro_rules! displayout {
    ($($arg:tt)*) => { print!($($arg)*) };
}

/// Print to stderr — equivalent to C `DISPLAY(...)`.
#[macro_export]
macro_rules! display {
    ($($arg:tt)*) => { eprint!($($arg)*) };
}

/// Conditionally print to stderr at or above `level` — equivalent to C `DISPLAYLEVEL(l, ...)`.
#[macro_export]
macro_rules! displaylevel {
    ($level:expr, $($arg:tt)*) => {
        if $crate::cli::constants::display_level() >= $level {
            eprint!($($arg)*);
        }
    };
}

// ── Error / debug macros (lz4cli.c lines 91–102) ─────────────────────────────
//
// `DEBUGOUTPUT` — prints to stderr only when DEBUG is non-zero.
// In Rust this is a no-op in release builds and active in debug builds via `cfg(debug_assertions)`.
//
// `END_PROCESS(error, ...)` — prints location info, an error message, then exits.
// In Rust this becomes a function + the `end_process!` macro below.

/// Print debug output — equivalent to C `DEBUGOUTPUT(...)`.
/// Only active in debug builds (mirrors `#ifndef DEBUG / #define DEBUG 0`).
#[macro_export]
macro_rules! debugoutput {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        eprint!($($arg)*);
    };
}

/// Terminate the process with an error code after printing a diagnostic.
/// Equivalent to the C `END_PROCESS(error, ...)` macro.
///
/// Usage: `end_process!(exit_code, "message {}", arg)`
#[macro_export]
macro_rules! end_process {
    ($error:expr, $($arg:tt)*) => {{
        // Mirror DEBUGOUTPUT("Error in %s, line %i : \n", __FILE__, __LINE__)
        #[cfg(debug_assertions)]
        eprint!("Error in {}, line {} : \n", file!(), line!());
        // Mirror DISPLAYLEVEL(1, "Error %i : ", error)
        if $crate::cli::constants::display_level() >= 1 {
            eprint!("Error {} : ", $error);
            eprint!($($arg)*);
            eprint!("\n");
        }
        std::process::exit($error as i32);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_constant() {
        assert_eq!(LZ4_EXTENSION, ".lz4");
    }

    #[test]
    fn compressor_name_constant() {
        assert_eq!(COMPRESSOR_NAME, "lz4");
    }

    #[test]
    fn size_constants() {
        assert_eq!(KB, 1024);
        assert_eq!(MB, 1024 * 1024);
        assert_eq!(GB, 1024 * 1024 * 1024);
    }

    #[test]
    fn display_level_default() {
        // Default is 2 (normal, downgradable).
        // Note: other tests may mutate this; reset after checking.
        let prev = display_level();
        // Confirm the accessor works
        assert!(display_level() <= 4);
        // Confirm setter round-trips
        set_display_level(3);
        assert_eq!(display_level(), 3);
        set_display_level(prev);
    }

    #[test]
    fn legacy_commands_default_false() {
        // Reset to known state first — parallel tests in other modules may have mutated the global.
        set_lz4c_legacy_commands(false);
        assert!(!lz4c_legacy_commands());
        set_lz4c_legacy_commands(true);
        assert!(lz4c_legacy_commands());
        set_lz4c_legacy_commands(false);
    }
}
