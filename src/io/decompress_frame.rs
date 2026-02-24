//! LZ4 frame decompression — single-threaded and dictionary-aware paths.
//!
//! This module is called by [`crate::io::decompress_dispatch`] after the
//! 4-byte LZ4 frame magic number has been consumed from the stream.  The magic
//! bytes are re-injected here so the [`crate::frame`] API receives a complete,
//! well-formed frame header.
//!
//! # Design notes
//!
//! * **`next_hint`-driven read loop** — [`lz4f_decompress`] returns the number
//!   of additional input bytes the decoder wants before it can make progress.
//!   Each `src.read()` is sized to exactly that hint, minimising syscalls on
//!   buffered sources and avoiding wasteful over-reads.
//!
//! * **Multi-worker path** — When `prefs.nb_workers > 1` the function uses the
//!   same single-threaded algorithm as `nb_workers == 1`.  Output is
//!   byte-for-byte identical.  True read→decompress→write pipelining would
//!   require `dst` to be `Send`, which conflicts with the current
//!   `&mut impl Write` signature.
//!
//! * **Dictionary decompression** — When `resources.dict_buffer` is `Some`,
//!   [`decompress_lz4f_st_dict`] is used.  Each [`lz4f_decompress_using_dict`]
//!   call receives the full dictionary so the decoder can resolve
//!   cross-dictionary backreferences.
//!
//! * **Sparse write optimisation** — Not applied here because `dst` is a
//!   generic `impl Write`.  Callers that hold a concrete `File` handle can
//!   invoke [`crate::io::sparse`] directly.
//!
//! * **Checksum validation** — Frame and block checksums are verified by
//!   default.  Pass [`DecompressOptions`] with `skip_checksums = true` to opt
//!   out (e.g. in latency-sensitive test paths).
//!
//! * **Errors** — All failure modes — I/O errors, invalid frames, checksum
//!   mismatches, truncated input — are surfaced as [`io::Error`].

use std::io::{self, Read, Write};

use crate::frame::types::LZ4F_VERSION;
use crate::frame::{
    lz4f_create_decompression_context, lz4f_decompress, lz4f_decompress_using_dict, Lz4FDCtx,
};
use crate::io::decompress_resources::DecompressResources;
use crate::io::prefs::{display_level, Prefs, DISPLAY_LEVEL, LZ4IO_MAGICNUMBER};

// Read/write buffer capacity for the decompression loop (64 KiB).
// Large enough to amortise syscall overhead; small enough to stay L2-resident
// on typical hardware.
const DECOMP_BUF_SIZE: usize = 64 * 1024;

/// Converts an [`Lz4FError`](crate::frame::Lz4FError) into an [`io::Error`]
/// with [`io::ErrorKind::InvalidData`], suitable for propagation from I/O
/// functions that return `io::Result`.
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

    // Both branches invoke the same ST implementation. True pipelining for
    // nb_workers > 1 is not implemented because `dst: &mut impl Write` is not
    // `Send`. The output is byte-for-byte identical regardless of worker count.
    if prefs.nb_workers > 1 {
        decompress_lz4f_st(src, dst, prefs)
    } else {
        decompress_lz4f_st(src, dst, prefs)
    }
}

// Feeds `input` to the frame decompressor in a loop until the entire slice
// is consumed or the decoder signals frame completion (`next_hint == 0`).
//
// Returns the final `next_hint` value: `0` means the frame is complete;
// any positive value is the number of additional input bytes the decoder
// wants before it can produce more output.
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
            lz4f_decompress(dctx, Some(dst_buf), &input[pos..], None).map_err(lz4f_err_to_io)?;
        pos += src_consumed;
        next_hint = hint;

        if dst_written > 0 {
            *filesize += dst_written as u64;
            if !prefs.test_mode {
                dst.write_all(&dst_buf[..dst_written])
                    .map_err(|e| io::Error::new(e.kind(), format!("Write error: {e}")))?;
            }
            if DISPLAY_LEVEL.load(std::sync::atomic::Ordering::Relaxed) >= 2 {
                display_level(2, &format!("\rDecompressed : {} MiB  ", *filesize >> 20));
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

// Dictionary-aware counterpart of `feed_to_decompressor`. Every
// `lz4f_decompress_using_dict` call receives the full external dictionary
// so the decoder can resolve backreferences that cross the dictionary boundary.
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
                dst.write_all(&dst_buf[..dst_written])
                    .map_err(|e| io::Error::new(e.kind(), format!("Write error: {e}")))?;
            }
            if DISPLAY_LEVEL.load(std::sync::atomic::Ordering::Relaxed) >= 2 {
                display_level(2, &format!("\rDecompressed : {} MiB  ", *filesize >> 20));
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
// Single-threaded decompression path
// ---------------------------------------------------------------------------

/// Decompresses one LZ4 frame from `src` into `dst` using the
/// `next_hint`-driven read loop.  Also serves as the implementation for
/// `nb_workers > 1`; see the module-level note on the multi-worker path.
fn decompress_lz4f_st(src: &mut impl Read, dst: &mut impl Write, prefs: &Prefs) -> io::Result<u64> {
    let mut dctx = lz4f_create_decompression_context(LZ4F_VERSION).map_err(lz4f_err_to_io)?;

    let mut src_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut dst_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut filesize: u64 = 0;

    // Re-inject the 4 magic bytes that the caller already consumed from `src`.
    // The frame decoder needs a complete, contiguous byte stream starting with
    // the magic number to parse the frame header correctly.
    let magic_bytes = LZ4IO_MAGICNUMBER.to_le_bytes();
    let mut next_hint = feed_to_decompressor(
        &mut dctx,
        &magic_bytes,
        &mut dst_buf,
        dst,
        prefs,
        &mut filesize,
    )?;

    // Drive the decoder with hint-sized reads until the frame is complete.
    while next_hint != 0 {
        let to_read = next_hint.min(src_buf.len());
        let read_n = src
            .read(&mut src_buf[..to_read])
            .map_err(|e| io::Error::new(e.kind(), format!("Read error: {e}")))?;
        if read_n == 0 {
            break; // EOF
        }

        next_hint = feed_to_decompressor(
            &mut dctx,
            &src_buf[..read_n],
            &mut dst_buf,
            dst,
            prefs,
            &mut filesize,
        )?;
    }

    // A non-zero next_hint after EOF means the frame was cut short.
    if next_hint != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Truncated LZ4 frame",
        ));
    }

    Ok(filesize)
}

// ---------------------------------------------------------------------------
// Dictionary-aware decompression path
// ---------------------------------------------------------------------------

/// Decompresses one LZ4 frame from `src` into `dst` using an external
/// dictionary. Uses the same `next_hint`-driven read loop as
/// [`decompress_lz4f_st`], but passes `dict` to every decompressor call
/// so the decoder can resolve cross-dictionary backreferences.
///
/// Called by [`decompress_lz4f`] when `resources.dict_buffer` is `Some`.
fn decompress_lz4f_st_dict(
    src: &mut impl Read,
    dst: &mut impl Write,
    prefs: &Prefs,
    dict: &[u8],
) -> io::Result<u64> {
    let mut dctx = lz4f_create_decompression_context(LZ4F_VERSION).map_err(lz4f_err_to_io)?;

    let mut src_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut dst_buf = vec![0u8; DECOMP_BUF_SIZE];
    let mut filesize: u64 = 0;

    // Re-inject the 4 magic bytes the caller already consumed from `src`.
    let magic_bytes = LZ4IO_MAGICNUMBER.to_le_bytes();
    let mut next_hint = feed_to_decompressor_dict(
        &mut dctx,
        &magic_bytes,
        dict,
        &mut dst_buf,
        dst,
        prefs,
        &mut filesize,
    )?;

    // Drive the decoder with hint-sized reads until the frame is complete.
    while next_hint != 0 {
        let to_read = next_hint.min(src_buf.len());
        let read_n = src
            .read(&mut src_buf[..to_read])
            .map_err(|e| io::Error::new(e.kind(), format!("Read error: {e}")))?;
        if read_n == 0 {
            break; // EOF
        }

        next_hint = feed_to_decompressor_dict(
            &mut dctx,
            &src_buf[..read_n],
            dict,
            &mut dst_buf,
            dst,
            prefs,
            &mut filesize,
        )?;
    }

    // A non-zero next_hint after EOF means the frame was cut short.
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

        assert_eq!(
            n as usize,
            original.len(),
            "byte count should match even in test mode"
        );
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
        assert_eq!(
            n as usize,
            original.len(),
            "byte count mismatch (dict path)"
        );
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
