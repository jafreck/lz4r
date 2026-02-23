//! Decompression binding for the benchmark harness.
//!
//! This module provides three public items that the benchmark loop builds on:
//!
//! - [`DecFunctionF`] — a uniform function-pointer type that lets the harness
//!   call any decompression back-end through a single consistent signature.
//! - [`FrameDecompressor`] — a lightweight session token that owns (or will
//!   own) per-session decompression state.
//! - [`decompress_frame_block`] — decompresses one complete LZ4 frame, enforces
//!   an output-size cap, and verifies that all input bytes were consumed.

use std::io;

use crate::frame::types::LZ4F_VERSION;
use crate::frame::{
    lz4f_create_decompression_context, lz4f_decompress, DecompressOptions, Lz4FDCtx,
};

// ── DecFunctionF ─────────────────────────────────────────────────────────────

/// Uniform function-pointer type for decompression back-ends used by the
/// benchmark harness.
///
/// Every decompression implementation called during benchmarking must match
/// this signature so the measurement loop can swap back-ends without
/// structural changes.
///
/// - `dst_capacity` — maximum bytes the callee may append to `dst`; the callee
///   must return an error rather than exceed this limit.
/// - `skip_checksums` — when `true`, content-checksum verification is skipped,
///   reducing per-call overhead at the cost of integrity coverage.
///
/// Returns the number of bytes written into `dst` on success, or an
/// [`io::Error`] on failure.
pub type DecFunctionF = fn(
    decompressor: &mut FrameDecompressor,
    src: &[u8],
    dst: &mut Vec<u8>,
    dst_capacity: usize,
    skip_checksums: bool,
) -> io::Result<usize>;

// ── FrameDecompressor ────────────────────────────────────────────────────────

/// Owns decompression state for one benchmark session.
///
/// Currently zero-sized: [`decompress_frame_block`] creates a fresh
/// [`Lz4FDCtx`] on each call rather than reusing one across calls, keeping
/// each decompression independent. The named type exists so that future
/// versions can carry persistent state (e.g. a pre-loaded dictionary) without
/// breaking callers.
#[derive(Debug, Default)]
pub struct FrameDecompressor;

impl FrameDecompressor {
    /// Constructs a `FrameDecompressor` with no pre-loaded state.
    pub fn new() -> Self {
        FrameDecompressor
    }
}

// ── decompress_frame_block ───────────────────────────────────────────────────

/// Decompress one complete LZ4 frame from `src`, appending output to `dst`.
///
/// A fresh [`Lz4FDCtx`] is allocated per call, so each invocation is
/// independent regardless of prior calls on the same [`FrameDecompressor`].
///
/// # Parameters
/// - `_decompressor`  — reserved for future per-session state; currently unused.
/// - `src`            — a complete, valid LZ4-frame byte sequence.
/// - `dst`            — output buffer; decompressed bytes are appended.
/// - `dst_capacity`   — maximum bytes that may be appended to `dst`.  Returns
///   an error if the decompressed output would exceed this limit; `dst` is
///   rolled back to its pre-call length on failure.
/// - `skip_checksums` — when `true`, content-checksum verification is skipped;
///   forwarded via [`DecompressOptions`].
///
/// # Returns
/// The number of bytes appended to `dst`, or an [`io::Error`] on failure.
pub fn decompress_frame_block(
    _decompressor: &mut FrameDecompressor,
    src: &[u8],
    dst: &mut Vec<u8>,
    dst_capacity: usize,
    skip_checksums: bool,
) -> io::Result<usize> {
    let mut dctx: Box<Lz4FDCtx> = lz4f_create_decompression_context(LZ4F_VERSION)
        .map_err(|e| io::Error::other(e.to_string()))?;

    let opts = DecompressOptions {
        stable_dst: true,
        skip_checksums,
    };

    // Temporary output chunk buffer — 64 KiB keeps stack usage reasonable while
    // giving `lz4f_decompress` a decent sized destination per iteration.
    let mut tmp = vec![0u8; 64 * 1024];

    let before = dst.len();
    let mut src_pos: usize = 0;
    let mut total_written: usize = 0;

    loop {
        let (src_consumed, dst_written, next_src_hint) =
            lz4f_decompress(&mut dctx, Some(&mut tmp), &src[src_pos..], Some(&opts))
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        if dst_written > 0 {
            total_written += dst_written;

            // Enforce the output-size cap: roll back dst and fail if the
            // decompressed output exceeds dst_capacity.
            if total_written > dst_capacity {
                dst.truncate(before);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "decompressed output exceeds dst_capacity",
                ));
            }

            dst.extend_from_slice(&tmp[..dst_written]);
        }

        src_pos += src_consumed;

        // next_src_hint == 0 means the frame is fully consumed.
        if next_src_hint == 0 {
            break;
        }

        // Safety check: if no progress was made on source or destination, bail
        // to avoid an infinite loop on malformed data.
        if src_consumed == 0 && dst_written == 0 {
            dst.truncate(before);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "decompressor stalled — no progress on source or destination",
            ));
        }
    }

    // Verify that all input bytes were consumed; leftover bytes indicate a
    // malformed or truncated frame.
    if src_pos != src.len() {
        dst.truncate(before);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "decompressor did not consume all input bytes",
        ));
    }

    Ok(total_written)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{lz4f_compress_frame, lz4f_compress_frame_bound};

    /// Compress `data` into an LZ4 frame and return the frame bytes.
    fn compress_frame(data: &[u8]) -> Vec<u8> {
        let bound = lz4f_compress_frame_bound(data.len(), None);
        let mut buf = vec![0u8; bound];
        let n = lz4f_compress_frame(&mut buf, data, None).unwrap();
        buf.truncate(n);
        buf
    }

    #[test]
    fn round_trip_basic() {
        let original = b"hello, lz4 frame decompressor!";
        let frame = compress_frame(original);

        let mut decompressor = FrameDecompressor::new();
        let mut dst = Vec::new();
        let n = decompress_frame_block(&mut decompressor, &frame, &mut dst, original.len(), false)
            .unwrap();

        assert_eq!(n, original.len());
        assert_eq!(dst, original);
    }

    #[test]
    fn round_trip_1mb() {
        // Decompress a 1 MiB cyclic buffer and verify byte-for-byte correctness.
        let original: Vec<u8> = (0u8..=255).cycle().take(1024 * 1024).collect();
        let frame = compress_frame(&original);

        let mut decompressor = FrameDecompressor::new();
        let mut dst = Vec::new();
        let n = decompress_frame_block(&mut decompressor, &frame, &mut dst, original.len(), false)
            .unwrap();

        assert_eq!(n, original.len());
        assert_eq!(dst, original);
    }

    #[test]
    fn skip_checksums_flag_accepted() {
        // skip_checksums=true must not panic or return an error (flag is accepted).
        let data = b"test data";
        let frame = compress_frame(data);
        let mut dec = FrameDecompressor::new();
        let mut dst = Vec::new();
        let result = decompress_frame_block(&mut dec, &frame, &mut dst, data.len(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn dec_function_f_callable_via_type_alias() {
        // Verify that `decompress_frame_block` is assignable to the `DecFunctionF` type alias.
        let f: DecFunctionF = decompress_frame_block;
        let data = b"type alias test";
        let frame = compress_frame(data);
        let mut dec = FrameDecompressor::new();
        let mut dst = Vec::new();
        let n = f(&mut dec, &frame, &mut dst, data.len(), false).unwrap();
        assert_eq!(n, data.len());
    }

    #[test]
    fn invalid_frame_returns_error() {
        let mut dec = FrameDecompressor::new();
        let mut dst = Vec::new();
        let result = decompress_frame_block(&mut dec, b"not valid lz4 data", &mut dst, 1024, false);
        assert!(result.is_err());
    }

    #[test]
    fn dst_capacity_exceeded_returns_error() {
        // Decompressed output larger than dst_capacity must produce an error.
        let data = b"hello, capacity check!";
        let frame = compress_frame(data);
        let mut dec = FrameDecompressor::new();
        let mut dst = Vec::new();
        // Use a capacity smaller than the decompressed size.
        let result = decompress_frame_block(&mut dec, &frame, &mut dst, data.len() - 1, false);
        assert!(result.is_err());
        // dst must not retain any partial output from the failed call.
        assert!(dst.is_empty());
    }
}
