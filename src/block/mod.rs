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
pub use decompress_api::{decoder_ring_buffer_size, decompress_safe, decompress_safe_partial, decompress_safe_using_dict, Lz4StreamDecode};
pub use stream::Lz4Stream;
pub use types::{StreamStateInternal, LZ4_DISTANCE_MAX};
