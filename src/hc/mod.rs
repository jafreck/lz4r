//! LZ4 HC (high-compression) block codec.
//!
//! Ported from `lz4hc.c` / `lz4hc.h` v1.10.0.

pub mod api;
pub mod compress_hc;
pub mod dispatch;
pub mod encode;
pub mod lz4mid;
pub mod search;
pub mod types;

// Re-export key public API items at the module level.
pub use api::{
    attach_hc_dictionary, compress_hc, compress_hc_continue, compress_hc_dest_size, compress_hc_ext_state,
    favor_decompression_speed, init_stream_hc, load_dict_hc, reset_stream_hc,
    reset_stream_hc_fast, save_dict_hc, set_compression_level, sizeof_state_hc, Lz4StreamHc,
};
pub use types::{
    LZ4HC_CLEVEL_DEFAULT, LZ4HC_CLEVEL_MAX, LZ4HC_CLEVEL_MIN, LZ4HC_CLEVEL_OPT_MIN,
};
