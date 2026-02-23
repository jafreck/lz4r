//! LZ4 block compression and decompression.
//!
//! This module contains the core LZ4 block-format engine, ported from lz4.c v1.10.0.

pub mod compress;
pub mod decompress_api;
pub mod decompress_core;
pub mod stream;
pub mod types;

// Re-export the most important public API items at the module level.
pub use compress::{
    compress_bound, compress_default, compress_dest_size, compress_fast, Lz4Error,
    LZ4_ACCELERATION_DEFAULT, LZ4_ACCELERATION_MAX, LZ4_MAX_INPUT_SIZE,
};
pub use decompress_api::{
    decoder_ring_buffer_size, decompress_safe, decompress_safe_partial, decompress_safe_using_dict,
    Lz4StreamDecode,
};
pub use stream::Lz4Stream;
pub use types::{StreamStateInternal, LZ4_DISTANCE_MAX};

// ---------------------------------------------------------------------------
// Convenience helpers — Vec-returning wrappers over the native block codec.
// ---------------------------------------------------------------------------

/// Compress `input` into a new `Vec<u8>` (raw LZ4 block, no size prefix).
pub fn compress_block_to_vec(input: &[u8]) -> Vec<u8> {
    let cap = compress::compress_bound(input.len() as i32).max(0) as usize;
    let mut dst = vec![0u8; cap];
    match compress::compress_default(input, &mut dst) {
        Ok(n) => {
            dst.truncate(n);
            dst
        }
        Err(_) => Vec::new(),
    }
}

/// Decompress a raw LZ4 block from `src` into a new `Vec<u8>`.
/// `original_size` is the exact expected output length.
pub fn decompress_block_to_vec(src: &[u8], original_size: usize) -> Vec<u8> {
    let mut dst = vec![0u8; original_size];
    match decompress_api::decompress_safe(src, &mut dst) {
        Ok(n) => {
            dst.truncate(n);
            dst
        }
        Err(_) => Vec::new(),
    }
}
