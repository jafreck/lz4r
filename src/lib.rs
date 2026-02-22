// LZ4 v1.10.0 — Rust port

pub mod block;
pub mod file;
pub mod frame;
pub mod hc;
pub mod xxhash;

// ─────────────────────────────────────────────────────────────────────────────
// Top-level convenience re-exports for the most common API entry points.
// These mirror the primary symbols from lz4.h and lz4frame.h.
// ─────────────────────────────────────────────────────────────────────────────

/// One-shot block compression.  Equivalent to `LZ4_compress_default`.
pub use block::compress::compress_default as lz4_compress_default;
/// One-shot block decompression.  Equivalent to `LZ4_decompress_safe`.
pub use block::decompress_api::decompress_safe as lz4_decompress_safe;
/// One-shot frame compression.  Equivalent to `LZ4F_compressFrame`.
pub use frame::compress::lz4f_compress_frame;
/// Streaming frame decompression (one call per chunk).  Equivalent to `LZ4F_decompress`.
pub use frame::decompress::lz4f_decompress;

// ─────────────────────────────────────────────────────────────────────────────
// Block API re-exports  (lz4.h lines 191–560)
// ─────────────────────────────────────────────────────────────────────────────

/// Error type for block compression operations.
pub use block::compress::Lz4Error;
/// Error type for block decompression operations.
pub use block::decompress_api::BlockDecompressError as DecompressError;

/// Maximum input size for a single LZ4 block. Equivalent to `LZ4_MAX_INPUT_SIZE`.
pub use block::compress::LZ4_MAX_INPUT_SIZE;

/// Default acceleration factor. Equivalent to `LZ4_ACCELERATION_DEFAULT`.
pub use block::compress::LZ4_ACCELERATION_DEFAULT;

/// Maximum acceleration factor. Equivalent to `LZ4_ACCELERATION_MAX`.
pub use block::compress::LZ4_ACCELERATION_MAX;

/// Returns the maximum compressed output size for a given input size.
/// Equivalent to `LZ4_compressBound` / `LZ4_COMPRESSBOUND`.
pub use block::compress::compress_bound;

/// Compress `src` into `dst` with a tunable acceleration factor.
/// Equivalent to `LZ4_compress_fast`.
pub use block::compress::compress_fast;

/// Compress using a caller-supplied state buffer with acceleration.
/// Equivalent to `LZ4_compress_fast_extState`.
pub use block::compress::compress_fast_ext_state;

/// Like `compress_fast_ext_state` but skips full state reset (stream must
/// not have been used across 64 KiB boundaries).
/// Equivalent to `LZ4_compress_fast_extState_fastReset`.
pub use block::compress::compress_fast_ext_state_fast_reset;

/// Compress `src` to fill exactly `dst`; outputs how many source bytes were consumed.
/// Equivalent to `LZ4_compress_destSize`.
pub use block::compress::compress_dest_size;

/// Like `compress_dest_size` but uses a caller-supplied state buffer.
/// Equivalent to `LZ4_compress_destSize_extState` (static/experimental).
pub use block::compress::compress_dest_size_ext_state;

/// Decompress `src` stopping after `target_output_size` bytes are produced.
/// Equivalent to `LZ4_decompress_safe_partial`.
pub use block::decompress_api::decompress_safe_partial;

// ─────────────────────────────────────────────────────────────────────────────
// Streaming compression API  (lz4.h lines 320–410)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming compression context. Equivalent to `LZ4_stream_t` / `LZ4_createStream`.
/// Drop replaces `LZ4_freeStream` (RAII).
pub use block::stream::Lz4Stream;

// ─────────────────────────────────────────────────────────────────────────────
// Streaming decompression API  (lz4.h lines 460–560)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming decompression context. Equivalent to `LZ4_streamDecode_t` / `LZ4_createStreamDecode`.
/// Drop replaces `LZ4_freeStreamDecode` (RAII).
pub use block::decompress_api::Lz4StreamDecode;

/// Initialize / reset a streaming decompression context with an optional dictionary.
/// Equivalent to `LZ4_setStreamDecode`.
pub use block::decompress_api::set_stream_decode;

/// Returns the minimum ring-buffer size for the given maximum block size.
/// Equivalent to `LZ4_decoderRingBufferSize`.
pub use block::decompress_api::decoder_ring_buffer_size;

/// Decompress the next block in a streaming session.
/// Equivalent to `LZ4_decompress_safe_continue`.
pub use block::decompress_api::decompress_safe_continue;

/// Decompress a block using an external dictionary (non-streaming).
/// Equivalent to `LZ4_decompress_safe_usingDict`.
pub use block::decompress_api::decompress_safe_using_dict;

/// Partial decompress a block using an external dictionary.
/// Equivalent to `LZ4_decompress_safe_partial_usingDict`.
pub use block::decompress_api::decompress_safe_partial_using_dict;

// ─────────────────────────────────────────────────────────────────────────────
// Version API  (lz4.h lines 131–143)
// ─────────────────────────────────────────────────────────────────────────────

pub const LZ4_VERSION_MAJOR: i32 = 1;
pub const LZ4_VERSION_MINOR: i32 = 10;
pub const LZ4_VERSION_RELEASE: i32 = 0;
pub const LZ4_VERSION_NUMBER: i32 =
    LZ4_VERSION_MAJOR * 100 * 100 + LZ4_VERSION_MINOR * 100 + LZ4_VERSION_RELEASE;
pub const LZ4_VERSION_STRING: &str = "1.10.0";

/// Returns the library version number (e.g. 11000 for v1.10.0).
/// Equivalent to `LZ4_versionNumber()`.
pub fn version_number() -> i32 {
    LZ4_VERSION_NUMBER
}

/// Returns the library version string (e.g. `"1.10.0"`).
/// Equivalent to `LZ4_versionString()`.
pub fn version_string() -> &'static str {
    LZ4_VERSION_STRING
}

// ─────────────────────────────────────────────────────────────────────────────
// State-size helper  (lz4.h line 245)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the number of bytes needed for an external compression state buffer.
/// Equivalent to `LZ4_sizeofState()`.
pub fn size_of_state() -> i32 {
    core::mem::size_of::<block::types::StreamStateInternal>() as i32
}

// ─────────────────────────────────────────────────────────────────────────────
// In-place buffer size helpers  (lz4.h lines 670–678)
// ─────────────────────────────────────────────────────────────────────────────

/// Margin required for in-place decompression.
/// Equivalent to `LZ4_DECOMPRESS_INPLACE_MARGIN(compressedSize)`.
#[inline]
pub const fn decompress_inplace_margin(compressed_size: usize) -> usize {
    (compressed_size >> 8) + 32
}

/// Minimum buffer size for in-place decompression.
/// Equivalent to `LZ4_DECOMPRESS_INPLACE_BUFFER_SIZE(decompressedSize)`.
#[inline]
pub const fn decompress_inplace_buffer_size(decompressed_size: usize) -> usize {
    decompressed_size + decompress_inplace_margin(decompressed_size)
}

/// Default distance max (history window size). Equivalent to `LZ4_DISTANCE_MAX`.
pub const LZ4_DISTANCE_MAX: usize = 65535;

/// Margin required for in-place compression. Equivalent to `LZ4_COMPRESS_INPLACE_MARGIN`.
pub const LZ4_COMPRESS_INPLACE_MARGIN: usize = LZ4_DISTANCE_MAX + 32;

/// Alias for [`LZ4_COMPRESS_INPLACE_MARGIN`] without the `LZ4_` prefix.
pub const COMPRESS_INPLACE_MARGIN: usize = LZ4_COMPRESS_INPLACE_MARGIN;

/// Minimum buffer size for in-place compression.
/// Equivalent to `LZ4_COMPRESS_INPLACE_BUFFER_SIZE(maxCompressedSize)`.
#[inline]
pub const fn compress_inplace_buffer_size(max_compressed_size: usize) -> usize {
    max_compressed_size + LZ4_COMPRESS_INPLACE_MARGIN
}
