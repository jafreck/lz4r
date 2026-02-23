//! LZ4 Frame format — streaming compression and decompression.
//!
//! The LZ4 frame format wraps one or more LZ4-compressed blocks in a portable,
//! self-describing container defined by the [LZ4 Frame Format Specification].
//! Each frame carries a frame header (magic number, block size, optional flags
//! for block-level and content checksums, and an optional content size), followed
//! by a sequence of compressed or uncompressed data blocks, and terminated by an
//! end-of-stream marker.  An optional 32-bit xxHash content checksum may follow.
//!
//! This module is the pure-Rust equivalent of `lz4frame.c` / `lz4frame.h` from
//! the LZ4 v1.10.0 reference implementation.
//!
//! # Submodules
//!
//! * [`types`]   — shared data types: [`Preferences`], [`FrameInfo`], error codes, etc.
//! * [`header`]  — frame-header encoding/decoding and bound calculation.
//! * [`compress`] — compression context lifecycle and streaming compress API.
//! * [`decompress`] — decompression context lifecycle and streaming decompress API.
//! * [`cdict`]   — compression dictionary support ([`Lz4FCDict`]).
//!
//! # One-shot helpers
//!
//! [`compress_frame_to_vec`] and [`decompress_frame_to_vec`] are thin,
//! allocation-owning wrappers for callers that don't need streaming control.
//!
//! [LZ4 Frame Format Specification]: https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md

pub mod cdict;
pub mod compress;
pub mod decompress;
pub mod header;
pub mod types;

pub use cdict::Lz4FCDict;
pub use compress::{
    lz4f_compress_begin, lz4f_compress_bound, lz4f_compress_end, lz4f_compress_frame,
    lz4f_compress_frame_using_cdict, lz4f_compress_update, lz4f_create_compression_context,
    lz4f_flush, lz4f_free_compression_context, lz4f_uncompressed_update, CompressOptions,
};
pub use decompress::{
    lz4f_create_decompression_context, lz4f_decompress, lz4f_decompress_using_dict,
    lz4f_free_decompression_context, lz4f_get_frame_info, lz4f_header_size,
    lz4f_reset_decompression_context, DecompressOptions, Lz4FDCtx,
};
pub use header::lz4f_compress_frame_bound;
pub use types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, FrameInfo, FrameType, Lz4FCCtx,
    Lz4FError, Preferences,
};

// ---------------------------------------------------------------------------
// One-shot convenience helpers
// ---------------------------------------------------------------------------

/// Compress `data` as a single, complete LZ4 frame and return the result as a
/// freshly-allocated `Vec<u8>`.
///
/// Uses default [`Preferences`] (block size 4 MiB, linked blocks, no content
/// checksum).  For fine-grained control over frame parameters, use the
/// streaming API via [`lz4f_create_compression_context`].
///
/// Returns an empty `Vec` if the underlying codec returns an error, which
/// should not occur for valid inputs under default settings.
pub fn compress_frame_to_vec(data: &[u8]) -> Vec<u8> {
    let prefs = types::Preferences::default();
    let bound = header::lz4f_compress_frame_bound(data.len(), Some(&prefs));
    let mut out = vec![0u8; bound];
    match compress::lz4f_compress_frame(&mut out, data, Some(&prefs)) {
        Ok(n) => {
            out.truncate(n);
            out
        }
        Err(_) => Vec::new(),
    }
}

/// Decompress a complete LZ4 frame from `compressed` into a freshly-allocated
/// `Vec<u8>`.
///
/// Returns `Err(io::Error)` with [`std::io::ErrorKind::InvalidData`] if the
/// input is not a valid LZ4 frame (bad magic, corrupt block, checksum mismatch,
/// etc.).
///
/// For streaming or incremental decompression, use the lower-level
/// [`lz4f_decompress`] API directly.
pub fn decompress_frame_to_vec(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut dctx = decompress::lz4f_create_decompression_context(types::LZ4F_VERSION)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))?;
    let mut out = Vec::new();
    let mut pos = 0usize;
    // 64 KiB output buffer — large enough to amortise Vec growth cost for
    // most real-world block sizes (LZ4 block size ID 4 = 64 KiB max).
    let mut dst_buf = vec![0u8; 65536];
    loop {
        if pos >= compressed.len() {
            break;
        }
        let (consumed, written, hint) =
            decompress::lz4f_decompress(&mut dctx, Some(&mut dst_buf), &compressed[pos..], None)
                .map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}"))
                })?;
        out.extend_from_slice(&dst_buf[..written]);
        pos += consumed;
        // A hint of 0 signals that a complete frame has been decoded.  Per the
        // LZ4 frame protocol the codec sets hint to 0 after consuming the
        // end-of-stream marker; any remaining bytes belong to a subsequent
        // frame or trailing data and are intentionally ignored here.
        if hint == 0 {
            break;
        }
        // Safety valve: if no progress was made in either direction the codec
        // is stalled waiting for input that will never arrive — break rather
        // than looping forever.
        if consumed == 0 && written == 0 {
            break;
        }
    }
    Ok(out)
}
