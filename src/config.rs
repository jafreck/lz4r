//! Compile-time configuration constants for the `lz4r` programs layer.
//!
//! These constants govern defaults for compression level, block size,
//! multithreading, and related tunables.  Most can be overridden at runtime
//! via CLI flags or environment variables; see the individual constants for
//! details.
//!
//! Platform-specific concerns handled elsewhere:
//! - 64-bit file offsets: native to Rust; no `_FILE_OFFSET_BITS` dance needed.
//! - Binary-mode I/O: `std::fs::File` has no text/binary distinction on Unix,
//!   and on Windows Rust opens files in binary mode by default.
//! - Sparse-file support: detected by `build.rs` via `#[cfg(has_sparse_files)]`.
//! - Terminal detection: `std::io::IsTerminal` (Rust 1.70+) at each call site.

/// Default compression level applied when no `-#` flag is given.
///
/// The value `1` selects the fast (non-HC) compressor at its baseline
/// acceleration.  Mirrors `LZ4_CLEVEL_DEFAULT` in `lz4conf.h`.
pub const CLEVEL_DEFAULT: i32 = 1;

/// Whether multithreaded compression is available in this build.
///
/// `true` on Windows (where I/O Completion Ports are available) and whenever
/// the `multithread` Cargo feature is enabled; `false` otherwise.  Mirrors
/// `LZ4IO_MULTITHREAD` in `lz4conf.h`.
pub const MULTITHREAD: bool = cfg!(target_os = "windows") || cfg!(feature = "multithread");

/// Default number of compression worker threads when `-T0` (auto) is requested.
///
/// Can be overridden at runtime with the `LZ4_NBWORKERS` environment variable
/// or the `-T#` flag.  Mirrors `LZ4_NBWORKERS_DEFAULT` in `lz4conf.h`.
pub const NB_WORKERS_DEFAULT: usize = 4;

/// Hard upper bound on the number of compression worker threads.
///
/// Requests exceeding this value are silently clamped.  Mirrors
/// `LZ4_NBWORKERS_MAX` in `lz4conf.h`.
pub const NB_WORKERS_MAX: usize = 200;

/// Default block size ID (`7` = 4 MiB blocks).
///
/// Controls the maximum uncompressed block size used by the Frame API.
/// Can be overridden at runtime with the `-B#` flag.  Mirrors
/// `LZ4_BLOCKSIZEID_DEFAULT` in `lz4conf.h`.
pub const BLOCKSIZEID_DEFAULT: u32 = 7;
