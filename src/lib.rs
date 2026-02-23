//! `lz4r` — a pure-Rust implementation of the LZ4 compression algorithms and
//! command-line programs (equivalent to the `programs/` tree of LZ4 v1.10.0).
//!
//! # Crate layout
//!
//! | Module       | Contents |
//! |--------------|----------|
//! | `block`      | Low-level block compression and decompression (LZ4 spec §§ 1–3). |
//! | `frame`      | LZ4 Frame format (magic number, header, content checksum). |
//! | `hc`         | High-compression (`lz4hc`) encoder variants. |
//! | `io`         | File-level I/O: compress / decompress single and multiple files. |
//! | `file`       | Streaming `Read`/`Write` wrappers over the Frame API. |
//! | `cli`        | Command-line argument parsing and dispatch. |
//! | `bench`      | Throughput benchmarking infrastructure. |
//! | `xxhash`     | XXH32 content-checksum wrapper. |
//! | `lorem`      | Deterministic lorem ipsum generator (benchmark corpus). |
//! | `timefn`     | Monotonic high-resolution timer. |
//! | `threadpool` | Fixed-size work-stealing thread pool. |
//! | `config`     | Compile-time configuration constants. |
//! | `util`       | File enumeration and sizing utilities. |

pub mod lorem;
pub mod timefn;
pub mod config;

#[cfg(feature = "c-abi")]
pub mod abi;
pub mod util;
pub mod threadpool;
pub mod io;
pub mod bench;
pub mod block;
pub mod hc;
pub mod frame;
pub mod xxhash;
pub mod file;
pub mod cli;

// ── Version constants (mirrors lz4.h lines 131–143) ──────────────────────────
pub const LZ4_VERSION_MAJOR: u32 = 1;
pub const LZ4_VERSION_MINOR: u32 = 10;
pub const LZ4_VERSION_RELEASE: u32 = 0;
pub const LZ4_VERSION_NUMBER: u32 =
    LZ4_VERSION_MAJOR * 100 * 100 + LZ4_VERSION_MINOR * 100 + LZ4_VERSION_RELEASE;
pub const LZ4_VERSION_STRING: &str = "1.10.0";
/// Git commit hash string injected at build time via the `LZ4_GIT_COMMIT`
/// environment variable.  Empty when the variable is not set.
pub const LZ4_GIT_COMMIT_STRING: &str = "";

/// Returns the runtime version number (equivalent to LZ4_versionNumber()).
pub fn version_number() -> u32 {
    LZ4_VERSION_NUMBER
}

/// Returns the runtime version string (equivalent to LZ4_versionString()).
pub fn version_string() -> &'static str {
    LZ4_VERSION_STRING
}

// ── Distance / inplace constants ──────────────────────────────────────────────
pub const LZ4_DISTANCE_MAX: usize = 65_535;
pub const COMPRESS_INPLACE_MARGIN: usize = LZ4_DISTANCE_MAX + 32;

/// Returns the size in bytes of the internal stream state (LZ4_sizeofState()).
pub fn size_of_state() -> i32 {
    core::mem::size_of::<block::types::StreamStateInternal>() as i32
}

/// Margin required for in-place decompression (lz4.h line 670).
pub fn decompress_inplace_margin(compressed_size: usize) -> usize {
    (compressed_size >> 8) + 32
}

/// Minimum buffer size for in-place decompression (lz4.h line 672).
pub fn decompress_inplace_buffer_size(decompressed_size: usize) -> usize {
    decompressed_size + decompress_inplace_margin(decompressed_size)
}

/// Minimum buffer size for in-place compression (lz4.h line 678).
pub fn compress_inplace_buffer_size(max_compressed_size: usize) -> usize {
    max_compressed_size + COMPRESS_INPLACE_MARGIN
}

// ── Top-level re-exports ──────────────────────────────────────────────────────
pub use block::compress::compress_default as lz4_compress_default;
pub use block::decompress_api::decompress_safe as lz4_decompress_safe;

// Block API — one-shot compression (needed by e2e tests)
pub use block::{
    compress_bound, compress_dest_size, compress_fast,
    decompress_safe_partial, decompress_safe_using_dict,
    LZ4_ACCELERATION_DEFAULT, LZ4_ACCELERATION_MAX, LZ4_MAX_INPUT_SIZE,
    Lz4Error,
};

// Error types
pub use block::decompress_core::DecompressError;

// Frame API convenience re-exports
pub use frame::{lz4f_compress_frame, lz4f_decompress};
