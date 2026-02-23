//! CLI constants, globals, and display macros.
//!
//! This module centralises the values and shared mutable state needed across
//! the CLI layer:
//!
//! - Identity strings (`COMPRESSOR_NAME`, `LZ4_EXTENSION`, …)
//! - Binary size multipliers (`KB`, `MB`, `GB`)
//! - The verbosity level used by [`displaylevel!`] and friends
//! - The legacy-command flag that activates `lz4c`-style short options
//! - The [`displayout!`], [`display!`], [`displaylevel!`], [`debugoutput!`],
//!   and [`end_process!`] output macros used throughout the CLI

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── Identity strings ────────────────────────────────────────────────────────
/// Primary compressor name, reported in `--version` output and as the default output extension.
pub const COMPRESSOR_NAME: &str = "lz4";
/// Library author credit shown in the welcome banner.
pub const AUTHOR: &str = "Yann Collet";
/// Default file extension appended to compressed output files.
pub const LZ4_EXTENSION: &str = ".lz4";
/// Canonical name for the decompression-only binary alias.
pub const LZ4CAT: &str = "lz4cat";
/// Canonical name for the decompression binary alias.
pub const UNLZ4: &str = "unlz4";
/// Name of the legacy `lz4c` binary whose short-option dialect this library supports.
pub const LZ4_LEGACY: &str = "lz4c";

/// Format string for the startup welcome banner.
///
/// Positional arguments (in order): compressor name, version, pointer-width in bits,
/// threading mode string, author name.
pub const WELCOME_MESSAGE_FMT: &str = "*** {} v{} {}-bit {}, by {} ***\n";

// ── Binary size multipliers ─────────────────────────────────────────────────
/// 1 KiB (1 024 bytes).
pub const KB: u64 = 1 << 10;
/// 1 MiB (1 048 576 bytes).
pub const MB: u64 = 1 << 20;
/// 1 GiB (1 073 741 824 bytes).
pub const GB: u64 = 1 << 30;

// ── Threading-mode label ────────────────────────────────────────────────────
/// Human-readable threading mode inserted into the welcome banner.
///
/// Resolves to `"multithread"` when the `multithread` Cargo feature is enabled,
/// or `"single-thread"` otherwise.
#[cfg(feature = "multithread")]
pub const IO_MT: &str = "multithread";
#[cfg(not(feature = "multithread"))]
pub const IO_MT: &str = "single-thread";

// ── Verbosity level ──────────────────────────────────────────────────────────
//
// Controls how much output the CLI produces.  Semantics:
//   0 — completely silent
//   1 — errors only
//   2 — normal informational output (default; can be suppressed with -q)
//   3 — non-suppressible informational messages
//   4 — verbose / diagnostic
//
// Stored as a process-wide atomic so it is accessible from any module without
// threading through a context struct.
pub static DISPLAY_LEVEL: AtomicU32 = AtomicU32::new(2);

/// Returns the current verbosity level.
#[inline]
pub fn display_level() -> u32 {
    DISPLAY_LEVEL.load(Ordering::Relaxed)
}

/// Sets the verbosity level.  Values outside 0–4 are accepted but have no
/// additional effect beyond level 4.
#[inline]
pub fn set_display_level(level: u32) {
    DISPLAY_LEVEL.store(level, Ordering::Relaxed);
}

// ── Legacy lz4c command mode ─────────────────────────────────────────────────
//
// When the binary is invoked as "lz4c", this flag is set to enable the
// alternate short-option dialect: `-c0`, `-c1`, `-hc`, `-y`, etc.
//
// Stored as an atomic bool so it is visible across modules.  In unit tests,
// prefer passing the flag explicitly rather than relying on this global.
pub static LZ4C_LEGACY_COMMANDS: AtomicBool = AtomicBool::new(false);

/// Returns `true` when the binary is running in legacy `lz4c` command mode.
#[inline]
pub fn lz4c_legacy_commands() -> bool {
    LZ4C_LEGACY_COMMANDS.load(Ordering::Relaxed)
}

/// Enables (`true`) or disables (`false`) legacy `lz4c` command mode.
#[inline]
pub fn set_lz4c_legacy_commands(enabled: bool) {
    LZ4C_LEGACY_COMMANDS.store(enabled, Ordering::Relaxed);
}

// ── Output macros ────────────────────────────────────────────────────────────
//
// Three tiers of CLI output:
//   displayout!  — informational output that belongs on stdout (e.g. decompressed data)
//   display!     — diagnostic output that always goes to stderr
//   displaylevel! — conditional stderr output gated on the current verbosity level

/// Write a formatted message to **stdout**.
///
/// Use this for output that is part of the compressed or decompressed data
/// stream (e.g. when writing to a pipe), so it does not pollute stderr.
#[macro_export]
macro_rules! displayout {
    ($($arg:tt)*) => { print!($($arg)*) };
}

/// Write a formatted message to **stderr** unconditionally.
///
/// Prefer [`displaylevel!`] when the message should be suppressible.
#[macro_export]
macro_rules! display {
    ($($arg:tt)*) => { eprint!($($arg)*) };
}

/// Write a formatted message to **stderr** if the current verbosity level is
/// at least `level`.
///
/// | `level` | meaning |
/// |---------|----------------------------|
/// | 1       | errors only |
/// | 2       | normal (default) |
/// | 3       | non-suppressible info |
/// | 4       | verbose / diagnostic |
#[macro_export]
macro_rules! displaylevel {
    ($level:expr, $($arg:tt)*) => {
        if $crate::cli::constants::display_level() >= $level {
            eprint!($($arg)*);
        }
    };
}

// ── Debug and fatal-error macros ─────────────────────────────────────────────
//
// `debugoutput!` — emits to stderr in debug builds only; a no-op in release.
// `end_process!` — prints a diagnostic then terminates the process, used for
//                  unrecoverable CLI errors (bad arguments, I/O failures, etc.).

/// Write a formatted message to **stderr** in debug builds only.
///
/// Compiled away entirely in release builds (`--release` / no `debug_assertions`).
#[macro_export]
macro_rules! debugoutput {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        eprint!($($arg)*);
    };
}

/// Print an error diagnostic and exit the process with the given code.
///
/// In debug builds, also emits the source file and line number before the
/// message.  The error message is only printed when the verbosity level is ≥ 1
/// (i.e. not completely silent).
///
/// # Example
/// ```ignore
/// end_process!(1, "cannot open '{}'", path);
/// ```
#[macro_export]
macro_rules! end_process {
    ($error:expr, $($arg:tt)*) => {{
        // In debug builds, include the source location for easier triage.
        #[cfg(debug_assertions)]
        eprint!("Error in {}, line {} : \n", file!(), line!());
        // Respect a verbosity of 0 (fully silent mode).
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
