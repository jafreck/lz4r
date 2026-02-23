//! LZ4 Frame format — streaming compression and decompression.
//!
//! Corresponds to lz4frame.c / lz4frame.h / lz4frame_static.h from LZ4 v1.10.0.

pub mod cdict;
pub mod compress;
pub mod decompress;
pub mod header;
pub mod types;

// Re-export key public API items at the module level.
pub use cdict::Lz4FCDict;
pub use compress::{
    lz4f_compress_begin, lz4f_compress_bound, lz4f_compress_end, lz4f_compress_frame,
    lz4f_compress_frame_using_cdict, lz4f_compress_update, lz4f_create_compression_context,
    lz4f_flush, lz4f_free_compression_context, lz4f_uncompressed_update, CompressOptions,
};
pub use header::lz4f_compress_frame_bound;
pub use decompress::{
    lz4f_create_decompression_context, lz4f_decompress, lz4f_decompress_using_dict,
    lz4f_free_decompression_context, lz4f_get_frame_info, lz4f_header_size,
    lz4f_reset_decompression_context, DecompressOptions, Lz4FDCtx,
};
pub use types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, FrameInfo, FrameType, Lz4FCCtx,
    Lz4FError, Preferences,
};
