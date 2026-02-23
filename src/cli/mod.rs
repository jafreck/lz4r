//! Command-line interface for the `lz4` binary.
//!
//! This module organises the full CLI pipeline:
//!
//! | Submodule     | Responsibility |
//! |---------------|---------------|
//! | [`constants`] | Program identity strings, size multipliers, and shared atomics (`DISPLAY_LEVEL`, `LZ4C_LEGACY_COMMANDS`). |
//! | [`help`]      | Usage/help text printers and `error_out` / `bad_usage` exit helpers. |
//! | [`arg_utils`] | Low-level argument parsing utilities: path basename, executable-name matching, integer parsing. |
//! | [`op_mode`]   | `OperationMode` enum, default compression level/worker-count constants, and environment-based initialisation helpers. |
//! | [`init`]      | `CliInit` — initial state built from the binary name (alias detection for `lz4cat`, `unlz4`, `lz4c`). |
//! | [`args`]      | `ParsedArgs` — full argument-parsing loop that consumes `argv` and produces the final set of runtime options. |
//!
//! Typical call sequence: `CliInit::detect_alias` → `ParsedArgs::parse` → dispatch to the I/O layer.

pub mod constants;
pub mod help;
pub mod arg_utils;
pub mod op_mode;
pub mod init;
pub mod args;
