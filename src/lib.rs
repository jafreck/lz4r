// lz4-programs — Rust port of lz4-1.10.0/programs

pub mod lorem;
pub mod timefn;
pub mod config;
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
/// Git commit string injected at build time (mirrors bench.c lines 61–64).
/// Defaults to `""` when `LZ4_GIT_COMMIT` is not set, matching the C default.
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
