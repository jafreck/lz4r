/*
    bench/decompress_binding.rs — LZ4 Frame decompression binding
    Migrated from lz4-1.10.0/programs/bench.c (lines 315–341)

    Original copyright (C) Yann Collet 2012-2020 — GPL v2 License.

    Migration notes:
    - `DecFunction_f` (C typedef, lines 315–317): a function-pointer type used
      by the benchmark loop to call any decompression implementation through a
      uniform signature. Represented in Rust as a type alias `DecFunctionF`.
      `dst_capacity` is preserved (mirrors the C parameter); `dictStart`/
      `dictSize` are dropped because the C source explicitly ignores them too.
    - `g_dctx` (C global `LZ4F_dctx*`, line 319): never freed in C; lives for
      the process lifetime. In Rust, replaced by `FrameDecompressor` which
      creates a fresh `Lz4FDCtx` per call (`decompress_frame_block`) rather than
      reusing across calls. This avoids unsafe statics while preserving the same
      observable behaviour (each call fully decompresses one independent LZ4
      frame).
    - `LZ4F_decompress_binding` (lines 322–341): decompresses one LZ4-frame
      block.  In C it returned -1 on error; in Rust we return `io::Error`.
      `dst_capacity` is now enforced (returns an error if exceeded, matching C).
      The input-consumption check (`readSize == srcSize`) is replicated by
      tracking the cumulative `src_consumed` returned by `lz4f_decompress`.
    - `dictStart`/`dictSize` parameters: the C source explicitly ignores them
      (`(void)dictStart; (void)dictSize;`).  The Rust signature omits them
      entirely for clarity.
    - `skip_checksums`: the C code forwarded this via `LZ4F_decompressOptions_t`.
      Now forwarded through `DecompressOptions { skip_checksums, .. }`.
    - `stableData = 1`: set in C's `LZ4F_decompressOptions_t` as a performance
      hint; mapped to `DecompressOptions { stable_dst: true, .. }`.
*/

use std::io;

use crate::frame::{
    lz4f_create_decompression_context, lz4f_decompress, DecompressOptions, Lz4FDCtx,
};
use crate::frame::types::LZ4F_VERSION;

// ── DecFunction_f (bench.c lines 315–317) ────────────────────────────────────

/// Uniform function-pointer type for decompression implementations.
///
/// Mirrors `typedef int (*DecFunction_f)(const char* src, char* dst,
///   int srcSize, int dstCapacity, const char* dictStart, int dictSize)`.
///
/// `dst_capacity` caps the number of bytes that may be appended to `dst`
/// (mirrors the C `dstCapacity` parameter).  `dictStart`/`dictSize` are
/// omitted because the C source explicitly ignores them.
///
/// Returns the number of bytes written into `dst` on success, or an `io::Error`
/// on failure.  The `skip_checksums` parameter replaces the global
/// `g_skipChecksums` reference in the C binding.
pub type DecFunctionF =
    fn(
        decompressor: &mut FrameDecompressor,
        src: &[u8],
        dst: &mut Vec<u8>,
        dst_capacity: usize,
        skip_checksums: bool,
    ) -> io::Result<usize>;

// ── FrameDecompressor (replaces g_dctx, bench.c line 319) ────────────────────

/// Owns the decompression context for one benchmark session.
///
/// The C equivalent was a process-lifetime `static LZ4F_dctx* g_dctx`.
/// In Rust we create a fresh context per call inside `decompress_frame_block`,
/// so `FrameDecompressor` is currently a zero-sized sentinel.  It is kept as a
/// named type so that the public API is stable and future versions can add
/// state (e.g. a dictionary) without breaking callers.
#[derive(Debug, Default)]
pub struct FrameDecompressor;

impl FrameDecompressor {
    /// Create a new `FrameDecompressor`.
    pub fn new() -> Self {
        FrameDecompressor
    }
}

// ── LZ4F_decompress_binding (bench.c lines 322–341) ──────────────────────────

/// Decompress one LZ4-frame block from `src`, appending output to `dst`.
///
/// Mirrors `LZ4F_decompress_binding`.  A fresh [`Lz4FDCtx`] is created for
/// every call, replacing the reuse of the global `g_dctx` in C.
///
/// # Parameters
/// - `_decompressor`  — reserved for future state; unused today.
/// - `src`            — complete, valid LZ4 frame data.
/// - `dst`            — output buffer; decompressed bytes are appended.
/// - `dst_capacity`   — maximum bytes that may be appended to `dst`; mirrors
///   the C `dstCapacity` parameter.  Returns an error if the frame decompresses
///   to more bytes than this limit (same as C returning −1).
/// - `skip_checksums` — mirrors `g_skipChecksums`; forwarded via
///   [`DecompressOptions`].
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
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

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

            // dstCapacity check: mirror C passing dstSize=dstCapacity to LZ4F_decompress.
            // C returns -1 (error) if the frame decompresses to more than dstCapacity bytes.
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

    // Input-consumption check: mirror C's `(int)readSize == srcSize` assertion.
    // C returns -1 if the decompressor did not consume all srcSize input bytes.
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
        let n = decompress_frame_block(&mut decompressor, &frame, &mut dst, original.len(), false).unwrap();

        assert_eq!(n, original.len());
        assert_eq!(dst, original);
    }

    #[test]
    fn round_trip_1mb() {
        // Parity check: decompress a 1 MiB buffer and verify correctness.
        let original: Vec<u8> = (0u8..=255).cycle().take(1024 * 1024).collect();
        let frame = compress_frame(&original);

        let mut decompressor = FrameDecompressor::new();
        let mut dst = Vec::new();
        let n = decompress_frame_block(&mut decompressor, &frame, &mut dst, original.len(), false).unwrap();

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
        // Mirrors C returning -1 when decompressed output > dstCapacity.
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
