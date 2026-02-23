//! LZ4 HC (high-compression) block codec.
//!
//! This module implements the LZ4 HC variant of the LZ4 block format (see
//! `lz4hc.c` / `lz4hc.h` in the reference implementation).  LZ4 HC produces
//! smaller output than the standard LZ4 block compressor by spending more time
//! searching for longer and better matches, at the cost of significantly slower
//! compression.  Decompression speed is identical to standard LZ4: any LZ4
//! block decompressor handles HC-compressed data without modification.
//!
//! # Compression levels
//!
//! HC compression is parameterised by a level in the range
//! [`LZ4HC_CLEVEL_MIN`]..=[`LZ4HC_CLEVEL_MAX`] (2–12).  Level
//! [`LZ4HC_CLEVEL_DEFAULT`] (9) balances compression ratio and speed well for
//! most workloads.  Levels ≥ [`LZ4HC_CLEVEL_OPT_MIN`] (10–12) activate an
//! optimal parser that can improve ratios further at a substantial additional
//! time cost.
//!
//! # Submodules
//!
//! | Submodule       | Responsibility                                                    |
//! |-----------------|-------------------------------------------------------------------|
//! | [`types`]       | Compression-level table, hash constants, and compression context  |
//! | [`encode`]      | Core token-encoding helpers shared by all HC strategies           |
//! | [`search`]      | Hash-chain insertion and match-search routines                    |
//! | [`lz4mid`]      | Mid-level path (levels ≤ 9): hash-table fill and compress loop    |
//! | [`compress_hc`] | Main HC compress loop — greedy and optimal-parser variants        |
//! | [`dispatch`]    | Strategy dispatch: selects hc / lz4mid / optimal at runtime      |
//! | [`api`]         | Public API — one-shot and streaming compression entry points      |
//!
//! The items most commonly needed by callers are re-exported at this level.

pub mod api;
pub mod compress_hc;
pub mod dispatch;
pub mod encode;
pub mod lz4mid;
pub mod search;
pub mod types;

pub use api::{
    attach_hc_dictionary, compress_hc, compress_hc_continue, compress_hc_dest_size,
    compress_hc_ext_state, favor_decompression_speed, init_stream_hc, load_dict_hc,
    reset_stream_hc, reset_stream_hc_fast, save_dict_hc, set_compression_level, sizeof_state_hc,
    Lz4StreamHc,
};
pub use types::{LZ4HC_CLEVEL_DEFAULT, LZ4HC_CLEVEL_MAX, LZ4HC_CLEVEL_MIN, LZ4HC_CLEVEL_OPT_MIN};
