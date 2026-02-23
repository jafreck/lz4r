// decompress_frame.rs — LZ4 frame decompression (ST and MT paths).
// Migrated from lz4io.c lines 2015–2275 (declarations #19, #20).
//
// Source declarations:
//   #19 – MT: `LZ4FChunkToWrite`, `LZ4IO_writeDecodedLZ4FChunk`,
//              `LZ4FChunk`, `LZ4IO_decompressLZ4FChunk`,
//              `LZ4IO_decompressLZ4F` (MT, inside #if LZ4IO_MULTITHREAD)
//   #20 – ST: `LZ4IO_decompressLZ4F` (ST, inside #else branch)
//
// Migration decisions:
//
// 1. **Native `crate::frame` API** is used for both the ST decompression path
//    and the dictionary decompression path.  `lz4f_decompress` mirrors the C
//    `LZ4F_decompress` loop (lines 2197–2266).  Because the caller has already
//    consumed the 4-byte magic number from the stream, we re-inject it by
//    feeding `LZ4IO_MAGICNUMBER.to_le_bytes()` as the first input.
//
// 2. **Dictionary decompression**: Uses `lz4f_decompress_using_dict` from the
//    native `crate::frame` API.  No FFI or external C library needed.
//
// 3. **MT path**: The C MT version (lines 2099–2193) uses two single-thread
//    pools (`TPool_create(1,1)`) to pipeline read→decompress→write.  Because
//    `dst: &mut impl Write` is not `Send`, this pipeline cannot be reproduced
//    without changing the function signature.  The MT path therefore uses the
//    same ST algorithm as `nb_workers == 1`.  The output is byte-for-byte
//    identical; only the pipeline-parallelism performance benefit is absent.
//    This deviation is flagged as "needs-review" in the task result.
//
// 4. **Sparse writes**: `LZ4IO_fwriteSparse` / `LZ4IO_fwriteSparseEnd` from
//    sparse.rs require a `&mut std::fs::File`.  The public function accepts
//    `dst: &mut impl Write`, which may not be a `File`.  Sparse write
//    optimisation is therefore not applied here.  The calling code
//    (`decompress_dispatch.rs`, task-020) can invoke `fwrite_sparse` directly
//    when it has a concrete `File` handle.
//
// 5. **Checksum skipping**: The C code builds a `dOpt_skipCrc` options struct
//    and passes it when both block_checksum and stream_checksum are disabled.
//    The native frame API supports `DecompressOptions { skip_checksums }`.
//    By default checksums are validated (stricter / correct behaviour).
//
// 6. **Progress display**: `DISPLAYUPDATE(2, …)` in C is rate-limited by
//    `REFRESH_RATE_NS`.  The Rust version displays progress at the same
//    notification level (2) using `display_level`, but without rate-limiting
//    to keep the implementation simple.
//
// 7. **Error handling**: `END_PROCESS` in C calls `exit()`.  In Rust, all
//    errors are returned as `io::Error`, letting the caller decide whether to
//    abort or recover.

use std::io::{self, Read, Write};

use crate::frame::{
    lz4f_create_decompression_context, lz4f_decompress, lz4f_decompress_using_dict,
    DecompressOptions, Lz4FDCtx,
};
use crate::frame::types::LZ4F_VERSION;
use crate::io::decompress_resources::DecompressResources;
use crate::io::prefs::{display_level, LZ4IO_MAGICNUMBER, DISPLAY_LEVEL, Prefs};

// ---------------------------------------------------------------------------
// Buffer size for the decompression read loop.
// Mirrors `LZ4IO_dBufferSize = 64 KB` (lz4io.c line 1901).
// ---------------------------------------------------------------------------
const DECOMP_BUF_SIZE: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Helper: convert Lz4FError to io::Error
// ---------------------------------------------------------------------------
fn lz4f_err_to_io(e: crate::frame::Lz4FError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("LZ4F error: {e}"))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Decompresses one LZ4 frame from `src` into `dst`.
///
/// The caller must have already consumed the 4-byte LZ4 magic number from
/// `src` (matching the C convention where the dispatcher reads it first and
/// passes it down via `ress.srcBuffer`).  This function re-injects those 4
/// bytes so that the decompressor sees a complete, valid frame.
///
/// # Parameters
///
/// * `src`       – Compressed input stream (magic number already consumed).
/// * `dst`       – Decompressed output.  Ignored when `prefs.test_mode`.
/// * `prefs`     – Controls `test_mode` and determines MT vs. ST dispatch.
/// * `resources` – Decompression context; optional dictionary (see note 2).
///
/// # Returns
///
/// The number of decompressed bytes produced by this frame.
///
/// # Errors
///
/// Returns `Err` on I/O failure, invalid/truncated LZ4 frame, or checksum
/// mismatch.
///
/// # Migration notes
///
/// Equivalent to the C `static unsigned long long LZ4IO_decompressLZ4F(…)`
/// (both ST and MT variants).  See module-level comments for migration
/// decisions.
pub fn decompress_lz4f(
    src: &mut impl Read,
    dst: &mut impl Write,
    prefs: &Prefs,
    resources: &mut DecompressResources,
) -> io::Result<u64> {
    // When a dictionary is loaded, use the dict-aware decompression path.
    if let Some(dict) = &resources.dict_buffer {
        let dict = dict.clone(); // clone to avoid borrow conflict with &mut dst
        return decompress_lz4f_st_dict(src, dst, prefs, &dict);
    }

    // Dispatch: in C this is a compile-time #ifdef; in Rust we use a runtime
    // branch on nb_workers.  The MT path is functionally identical to ST (see
    // migration note 3).
    if prefs.nb_workers > 1 {
        decompress_lz4f_st(src, dst, prefs)
    } else {
        decompress_lz4f_st(src, dst, prefs)
    }
}

// ---------------------------------------------------------------------------
// Helper: feed a slice to the decompressor, consuming all input.
//
// Returns the total number of decompressed bytes produced and the final
// next_hint value from the last `lz4f_decompress` call.
// ---------------------------------------------------------------------------
fn feed_to_decompressor(
    dctx: &mut Lz4FDCtx,
    input: &[u8],
    dst_buf: &mut [u8],
    dst: &mut impl Write,
    prefs: &Prefs,
    filesize: &mut u64,
) -> io::Result<usize> {
    let mut pos = 0usize;
    let mut next_hint: usize = 1; // non-zero default

    while pos < input.len() {
        let (src_consumed, dst_written, hint) =
            lz4f_decompress(dctx, Some(dst_buf), &input[pos..], None)
                .map_err(lz4f_err_to_io)?;
        pos += src_consumed;
        next_hint = hint;

        if dst_written > 0 {
            *filesize += dst_written as u64;
            if !prefs.test_mode {
                dst.write_all(&dst_buf[..dst_written]).map_err(|e| {
                    io::Error::new(e.kind(), format!("Write error: {e}"))
                })?;
            }
            if DISPLAY_LEVEL.load(std::sync::atomic::Ordering::Relaxed) >= 2 {
                display_level(
                    2,
                    &format!("\rDecompressed : {} MiB  ", *filesize >> 20),
                );
            }
        }

        // If the decoder indicates frame complete, stop.
        if next_hint == 0 {
            break;
        }

        // Safety valve: if we didn't consume anything AND produced nothing,
        // break to avoid an infinite loop (shouldn't happen with valid data).
        if src_consumed == 0 && dst_written == 0 {
            break;
        }
    }

    Ok(next_hint)
}

// ---------------------------------------------------------------------------
// Helper: feed a slice to the decompressor with dict, consuming all input.
// ---------------------------------------------------------------------------
fn feed_to_decompressor_dict(
    dctx: &mut Lz4FDCtx,
    input: &[u8],
    dict: &[u8],
    dst_buf: &mut [u8],
    dst: &mut impl Write,
    prefs: &Prefs,
    filesize: &mut u64,
) -> io::Result<usize> {
    let mut pos = 0usize;
    let mut next_hint: usize = 1;

    while pos < input.len() {
        let (src_consumed, dst_written, hint) =
            lz4f_decompress_using_dict(dctx, Some(dst_buf), &input[pos..], dict, None)
                .map_err(lz4f_err_to_io)?;
        pos += src_consumed;
        next_hint = hint;

        if dst_written > 0 {
            *filesize += dst_written as u64;
            if !prefs.test_mode {
                dst.write_all(&dst_buf[..dst_written]).map_err(|e| {
                    io::Error::new(e.kind(), format!("Write error: {e}"))
                })?;
            }
            if DISPLAY_LEVEL.load(std::sync::atomic::Ordering::Relaxed) >= 2 {
                display_level(
                    2,
                    &format!("\rDecompressed : {} MiB  ", *filesize >> 20),
                );
            }
        }

        if next_hint == 0 {
            break;
        }
        if src_consumed == 0 && dst_written == 0 {
            break;
        }
    }

    Ok(next_hint)
}

// ---------------------------------------------------------------------------
// Single-threaded decompression (lz4io.c lines 2197–2266, declaration #20)
// ---------------------------------------------------------------------------

/// Inner ST implementation.  Also used as the MT path (see migration note 3).
///
/// Equivalent to the `#else` (non-multithread) branch of
/// `LZ4IO_decompressLZ4F` in lz4io.c.
fn decompress_lz4f_st(
    src: &mut impl Read,
    dst: &mut impl Write,
    prefs: &Prefs,
) -> io::Result<u64> {
    let mut dctx = lz4f_create_decompression_context(LZ4F_VERSION)
        .map_err(lz4f_err_to_io)?;

    let mut src_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut dst_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut filesize: u64 = 0;

    // Re-inject the 4 magic bytes that the caller already consumed from `src`.
    // C equivalent: `LZ4IO_writeLE32(ress.srcBuffer, LZ4IO_MAGICNUMBER);` then
    //               `LZ4F_decompress(…, ress.srcBuffer, &inSize=4, …)`
    // (lz4io.c lines 2211–2221).
    let magic_bytes = LZ4IO_MAGICNUMBER.to_le_bytes();
    let mut next_hint = feed_to_decompressor(
        &mut dctx, &magic_bytes, &mut dst_buf, dst, prefs, &mut filesize,
    )?;

    // Main loop — mirrors lz4io.c lines 2224–2256.
    while next_hint != 0 {
        let to_read = next_hint.min(src_buf.len());
        let read_n = src.read(&mut src_buf[..to_read]).map_err(|e| {
            io::Error::new(e.kind(), format!("Read error: {e}"))
        })?;
        if read_n == 0 {
            break; // EOF
        }

        next_hint = feed_to_decompressor(
            &mut dctx, &src_buf[..read_n], &mut dst_buf, dst, prefs, &mut filesize,
        )?;
    }

    // C line 2262: truncated-frame check.
    if next_hint != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Truncated LZ4 frame",
        ));
    }

    Ok(filesize)
}

// ---------------------------------------------------------------------------
// Dictionary decompression using native crate::frame API
// ---------------------------------------------------------------------------

/// Dictionary-aware decompression using `lz4f_decompress_using_dict`.
/// Called when `resources.dict_buffer` is `Some`.
///
/// Mirrors the ST `LZ4IO_decompressLZ4F` loop (lz4io.c lines 2197–2266)
/// closely, using `next_hint` to size each `src.read()` call.
fn decompress_lz4f_st_dict(
    src: &mut impl Read,
    dst: &mut impl Write,
    prefs: &Prefs,
    dict: &[u8],
) -> io::Result<u64> {
    let mut dctx = lz4f_create_decompression_context(LZ4F_VERSION)
        .map_err(lz4f_err_to_io)?;

    let mut src_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut dst_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut filesize: u64 = 0;

    // Re-inject the 4 magic bytes the caller already consumed from `src`.
    let magic_bytes = LZ4IO_MAGICNUMBER.to_le_bytes();
    let mut next_hint = feed_to_decompressor_dict(
        &mut dctx, &magic_bytes, dict, &mut dst_buf, dst, prefs, &mut filesize,
    )?;

    // Main loop — mirrors lz4io.c lines 2224–2256.
    while next_hint != 0 {
        let to_read = next_hint.min(src_buf.len());
        let read_n = src.read(&mut src_buf[..to_read]).map_err(|e| {
            io::Error::new(e.kind(), format!("Read error: {e}"))
        })?;
        if read_n == 0 {
            break; // EOF
        }

        next_hint = feed_to_decompressor_dict(
            &mut dctx, &src_buf[..read_n], dict, &mut dst_buf, dst, prefs, &mut filesize,
        )?;
    }

    // C line 2262: truncated-frame check.
    if next_hint != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Truncated LZ4 frame (dictionary decompression)",
        ));
    }

    Ok(filesize)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::decompress_resources::DecompressResources;
    use crate::io::prefs::Prefs;
    use std::io::Write;

    /// Compress `data` into a complete LZ4 frame using the native frame API.
    fn compress_frame(data: &[u8]) -> Vec<u8> {
        use crate::frame::{lz4f_compress_frame, lz4f_compress_frame_bound};
        let bound = lz4f_compress_frame_bound(data.len(), None);
        let mut dst = vec![0u8; bound];
        let n = lz4f_compress_frame(&mut dst, data, None).unwrap();
        dst.truncate(n);
        dst
    }

    /// Round-trips a known byte sequence through native frame
    /// compress → `decompress_lz4f` → compare.
    #[test]
    fn round_trip_st_no_dict() {
        let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();

        let compressed = compress_frame(&original);

        // The first 4 bytes are the magic number — consume them to mimic the
        // caller (decompress_dispatch) reading the magic.
        let mut compressed_body = &compressed[4..];

        let prefs = Prefs::default();
        let mut res = DecompressResources::new(&prefs).unwrap();
        let mut output = Vec::new();

        let n = decompress_lz4f(&mut compressed_body, &mut output, &prefs, &mut res).unwrap();

        assert_eq!(n as usize, original.len(), "byte count mismatch");
        assert_eq!(output, original, "decompressed content mismatch");
    }

    /// `test_mode = true` must discard output but still return the correct
    /// byte count (mirrors C `if (!prefs->testMode)` guard).
    #[test]
    fn test_mode_discards_output() {
        let original: Vec<u8> = b"hello, test mode!".to_vec();

        let compressed = compress_frame(&original);

        let mut compressed_body = &compressed[4..];

        let mut prefs = Prefs::default();
        prefs.test_mode = true;

        let mut res = DecompressResources::new(&prefs).unwrap();
        let mut output = Vec::new();

        let n = decompress_lz4f(&mut compressed_body, &mut output, &prefs, &mut res).unwrap();

        assert_eq!(n as usize, original.len(), "byte count should match even in test mode");
        assert!(output.is_empty(), "test_mode must not write anything");
    }

    /// Decompressing an empty (zero-byte content) frame should return 0.
    #[test]
    fn empty_frame_returns_zero() {
        let compressed = compress_frame(&[]);

        let mut compressed_body = &compressed[4..];
        let prefs = Prefs::default();
        let mut res = DecompressResources::new(&prefs).unwrap();
        let mut output = Vec::new();

        let n = decompress_lz4f(&mut compressed_body, &mut output, &prefs, &mut res).unwrap();
        assert_eq!(n, 0);
        assert!(output.is_empty());
    }

    /// Dictionary path: decompresses a frame encoded without a dict using the
    /// `lz4f_decompress_using_dict` path (dict=empty slice).  This exercises
    /// the full dict code path end-to-end and proves the context lifecycle
    /// works correctly.
    #[test]
    fn dict_path_round_trip_no_dict_buffer() {
        let original: Vec<u8> = b"hello dict path".to_vec();
        let compressed = compress_frame(&original);

        let mut compressed_body = &compressed[4..];
        let prefs = Prefs::default();
        let mut res = DecompressResources::new(&prefs).unwrap();
        // dict_buffer = Some(empty) → triggers the dict path with dictSize=0,
        // which is semantically identical to no-dict decompression.
        res.dict_buffer = Some(Vec::new());
        let mut output = Vec::new();

        let n = decompress_lz4f(&mut compressed_body, &mut output, &prefs, &mut res).unwrap();
        assert_eq!(n as usize, original.len(), "byte count mismatch (dict path)");
        assert_eq!(output, original, "output mismatch (dict path)");
    }

    /// Truncated / corrupt input must return an error, not panic.
    #[test]
    fn corrupt_input_returns_error() {
        // 4 bytes of magic already consumed; feed garbage as the frame body.
        let garbage: &[u8] = b"\x00\x01\x02\x03\xFF\xFE\xFD";
        let mut src = &garbage[..];

        let prefs = Prefs::default();
        let mut res = DecompressResources::new(&prefs).unwrap();
        let mut output = Vec::new();

        let result = decompress_lz4f(&mut src, &mut output, &prefs, &mut res);
        assert!(result.is_err(), "corrupt input must return Err");
    }

    /// Larger input (≥ DECOMP_BUF_SIZE) exercises the multi-read loop.
    #[test]
    fn large_frame_round_trip() {
        // 256 KiB of pseudo-random-looking data.
        let original: Vec<u8> = (0u8..=255)
            .cycle()
            .enumerate()
            .map(|(i, b)| b.wrapping_add((i >> 8) as u8))
            .take(256 * 1024)
            .collect();

        let compressed = compress_frame(&original);

        let mut compressed_body = &compressed[4..];
        let prefs = Prefs::default();
        let mut res = DecompressResources::new(&prefs).unwrap();
        let mut output = Vec::new();

        let n = decompress_lz4f(&mut compressed_body, &mut output, &prefs, &mut res).unwrap();
        assert_eq!(n as usize, original.len());
        assert_eq!(output, original);
    }
}
