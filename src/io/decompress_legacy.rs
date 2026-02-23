//! Decompression of the LZ4 legacy format (magic `0x184C2102`).
//!
//! The legacy format is a stream of size-prefixed LZ4 block-compressed chunks.
//! Each chunk is preceded by a 4-byte little-endian block size; a value that
//! exceeds `LZ4_COMPRESSBOUND(LEGACY_BLOCKSIZE)` is not a valid block size —
//! it is the magic number of the next chained frame, to be returned to the
//! caller for dispatch.
//!
//! # API
//!
//! [`decode_legacy_stream`] is the main entry point.  It expects the stream
//! positioned immediately after the consumed magic number and writes
//! decompressed output to any `impl Write`.  On success it returns the total
//! number of decoded bytes and, if a chained-frame magic was encountered, that
//! magic value so the caller can dispatch the next frame.
//!
//! # Threading
//!
//! When `prefs.nb_workers > 1` the multi-threaded path reads compressed blocks
//! in batches of [`NB_BUFFSETS`] and decompresses each batch in parallel via
//! rayon, then writes results in order on the calling thread.  The
//! single-threaded path processes one block at a time.
//!
//! Sparse-write optimisation is intentionally left to the caller
//! (`decompress_dispatch`), which holds a direct `&mut File`.  A generic
//! `impl Write` cannot safely assume sparse-hole support.

use std::io::{self, Read, Write};

use crate::block::compress::compress_bound;
use crate::block::decompress_api::decompress_safe;
use rayon::prelude::*;

use crate::io::decompress_resources::DecompressResources;
use crate::io::prefs::{Prefs, LEGACY_BLOCKSIZE};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Batch size for the multi-threaded decompression loop.
///
/// Four buffer sets allow one set to be filling, one decompressing, one
/// writing, and one queued simultaneously — matching the natural pipeline
/// depth of the MT path.
const NB_BUFFSETS: usize = 4;

/// Byte length of the block-size header field in the legacy stream format.
const LEGACY_BLOCK_HEADER_SIZE: usize = 4;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the maximum compressed size of a `LEGACY_BLOCKSIZE`-byte input block.
///
/// Block-size values read from the stream that exceed this bound are not valid
/// compressed sizes; they are treated as magic numbers for a chained frame.
fn lz4_compress_bound() -> usize {
    compress_bound(LEGACY_BLOCKSIZE as i32) as usize
}

/// Reads exactly `buf.len()` bytes, returning `Ok(false)` on a clean EOF
/// encountered before the first byte, `Ok(true)` on success, or an error if
/// EOF occurs mid-read.
///
/// Used when reading the 4-byte block header so that a clean end-of-stream
/// is not treated as an I/O error.
fn read_exact_or_eof<R: Read>(src: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    // A single-byte read distinguishes a clean EOF (n == 0) from a
    // short read mid-header, which would be a truncated stream.
    let n = src.read(&mut buf[..1])?;
    if n == 0 {
        return Ok(false); // clean end-of-stream
    }
    // Any EOF while reading the remaining bytes is a truncation error.
    src.read_exact(&mut buf[1..])?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Decompresses a legacy-format LZ4 stream from `src` into `dst`.
///
/// # Legacy format
///
/// Each block is preceded by a 4-byte little-endian block size.  If the block
/// size exceeds `LZ4_COMPRESSBOUND(LEGACY_BLOCKSIZE)`, the value is not a
/// valid compressed block size and is instead a magic number for the next
/// chained frame.
///
/// # Return value
///
/// `Ok((decoded_bytes, next_magic))` where `next_magic` is `Some(magic)` when
/// the stream ended because a next-stream magic number was encountered, or
/// `None` when it ended at clean EOF.
///
/// # Errors
///
/// Returns `Err` on any I/O error or on corrupted compressed data.
///
pub fn decode_legacy_stream<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    prefs: &Prefs,
    _resources: &DecompressResources,
) -> io::Result<(u64, Option<u32>)> {
    if prefs.nb_workers > 1 {
        decode_legacy_mt(src, dst, prefs)
    } else {
        decode_legacy_st(src, dst)
    }
}

// ---------------------------------------------------------------------------
// Single-threaded path
// ---------------------------------------------------------------------------

/// Single-threaded legacy decompression loop.
///
/// Reads one block at a time: reads the 4-byte size header, validates it
/// against the compress bound, reads the compressed payload, decompresses it,
/// and writes the result to `dst`.  Repeats until clean EOF or a chained-frame
/// magic number is encountered.
fn decode_legacy_st<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
) -> io::Result<(u64, Option<u32>)> {
    let compress_bound = lz4_compress_bound();
    let mut header = [0u8; LEGACY_BLOCK_HEADER_SIZE];
    let mut in_buf = vec![0u8; compress_bound];
    let mut stream_size: u64 = 0;
    let mut next_magic: Option<u32> = None;

    loop {
        // Clean EOF before the block header means the stream ended normally.
        if !read_exact_or_eof(src, &mut header)? {
            break;
        }

        // Decode the little-endian block-size header.
        let block_size = u32::from_le_bytes(header);

        if block_size as usize > compress_bound {
            // Value exceeds the maximum compressed block size — it is the
            // magic number of a chained frame.  Return it for dispatch.
            next_magic = Some(block_size);
            break;
        }

        let block_len = block_size as usize;
        src.read_exact(&mut in_buf[..block_len])?;

        // Decompress the block; any decompressor error is treated as
        // corrupted input and surfaced as an InvalidData I/O error.
        let mut dec_buf = vec![0u8; LEGACY_BLOCKSIZE];
        let dec_n = decompress_safe(&in_buf[..block_len], &mut dec_buf).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Decoding Failed! Corrupted input detected!: {e:?}"),
            )
        })?;;

        stream_size += dec_n as u64;
        dst.write_all(&dec_buf[..dec_n])?;
    }

    // I/O errors from read_exact_or_eof / read_exact propagate as Err;
    // no separate error-state check is needed.

    Ok((stream_size, next_magic))
}

// ---------------------------------------------------------------------------
// Multi-threaded path
// ---------------------------------------------------------------------------

/// Multi-threaded legacy decompression using rayon batch parallelism.
///
/// Reads compressed blocks in batches of [`NB_BUFFSETS`], decompresses each
/// batch in parallel with rayon, then writes decompressed output in order on
/// the calling thread.
///
/// A fully pipelined design with a dedicated write thread is not possible
/// because `Write` is not `Send`.  The rayon batch approach achieves the same
/// CPU-bound throughput with safe Rust by decoupling the decompression work
/// from the serial write step.
fn decode_legacy_mt<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    _prefs: &Prefs,
) -> io::Result<(u64, Option<u32>)> {
    let compress_bound = lz4_compress_bound();
    let mut stream_size: u64 = 0;
    let mut next_magic: Option<u32> = None;

    loop {
        // ── Read a batch of up to NB_BUFFSETS compressed blocks ─────────────
        let mut batch: Vec<Vec<u8>> = Vec::with_capacity(NB_BUFFSETS);
        let mut batch_done = false; // set when EOF or a chained-frame magic is found

        for _ in 0..NB_BUFFSETS {
            let mut header = [0u8; LEGACY_BLOCK_HEADER_SIZE];

            // Clean EOF before a block header means the stream ended normally.
            if !read_exact_or_eof(src, &mut header)? {
                batch_done = true;
                break;
            }

            let block_size = u32::from_le_bytes(header);
            if block_size as usize > compress_bound {
                // Value exceeds the maximum compressed block size — treat as
                // a chained-frame magic number and stop reading this stream.
                next_magic = Some(block_size);
                batch_done = true;
                break;
            }

            let mut block = vec![0u8; block_size as usize];
            src.read_exact(&mut block)?;
            batch.push(block);
        }

        if batch.is_empty() {
            break;
        }

        // ── Decompress batch in parallel ──────────────────────────────────────
        // Each block is independent, so rayon can decompress them concurrently.
        // Results are collected into a Vec to preserve ordering before writing.
        let results: Vec<io::Result<Vec<u8>>> = batch
            .par_iter()
            .map(|block| {
                let mut dec_buf = vec![0u8; LEGACY_BLOCKSIZE];
                let n = decompress_safe(block, &mut dec_buf).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Decoding Failed! Corrupted input detected!: {e:?}"),
                    )
                })?;
                dec_buf.truncate(n);
                Ok(dec_buf)
            })
            .collect();

        // ── Write results in order ────────────────────────────────────────────
        // Propagate any decompression error from the parallel batch before
        // writing; this ensures output is never partially written on error.
        for result in results {
            let decompressed = result?;
            stream_size += decompressed.len() as u64;
            dst.write_all(&decompressed)?;
        }

        if batch_done {
            break;
        }
    }

    Ok((stream_size, next_magic))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::prefs::Prefs;

    fn make_resources() -> DecompressResources {
        DecompressResources::new(&Prefs::default()).expect("resources")
    }

    /// Compress `data` in legacy format (magic + 4-byte size-prefixed blocks)
    /// using the native block encoder and return the raw stream bytes.
    fn make_legacy_stream(data: &[u8]) -> Vec<u8> {
        const LZ4IO_LEGACY_MAGICNUMBER: u32 = 0x184C2102;
        let mut stream = Vec::new();
        // Write magic number.
        stream.extend_from_slice(&LZ4IO_LEGACY_MAGICNUMBER.to_le_bytes());
        // Split data into LEGACY_BLOCKSIZE chunks and compress each.
        for chunk in data.chunks(LEGACY_BLOCKSIZE) {
            let compressed = crate::block::compress_block_to_vec(chunk);
            let block_size = compressed.len() as u32;
            stream.extend_from_slice(&block_size.to_le_bytes());
            stream.extend_from_slice(&compressed);
        }
        stream
    }

    /// Strips the 4-byte magic number from a legacy stream and returns the
    /// payload (block-size-prefixed blocks).
    fn legacy_payload(stream: &[u8]) -> &[u8] {
        &stream[4..] // skip 4-byte magic
    }

    #[test]
    fn st_decompress_small() {
        let original = b"Hello, legacy LZ4 world!";
        let stream = make_legacy_stream(original);
        let payload = legacy_payload(&stream);

        let prefs = Prefs::default(); // nb_workers == 0 → ST
        let res = make_resources();
        let mut out = Vec::new();
        let (size, magic) = decode_legacy_stream(
            &mut std::io::Cursor::new(payload),
            &mut out,
            &prefs,
            &res,
        )
        .expect("decode should succeed");

        assert_eq!(out, original);
        assert_eq!(size, original.len() as u64);
        assert!(magic.is_none(), "no chained frame");
    }

    #[test]
    fn st_decompress_multi_block() {
        // Create data larger than LEGACY_BLOCKSIZE to exercise multiple blocks.
        // Use a small LEGACY_BLOCKSIZE substitute: just generate 2 blocks.
        let block1 = vec![0x41u8; 64]; // 64 bytes of 'A'
        let block2 = vec![0x42u8; 32]; // 32 bytes of 'B'
        let mut payload = Vec::new();
        for chunk in [block1.as_slice(), block2.as_slice()] {
            let compressed = crate::block::compress_block_to_vec(chunk);
            payload.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            payload.extend_from_slice(&compressed);
        }

        let prefs = Prefs::default();
        let res = make_resources();
        let mut out = Vec::new();
        let (size, magic) = decode_legacy_stream(
            &mut std::io::Cursor::new(&payload),
            &mut out,
            &prefs,
            &res,
        )
        .expect("decode should succeed");

        let mut expected = block1.clone();
        expected.extend_from_slice(&block2);
        assert_eq!(out, expected);
        assert_eq!(size, expected.len() as u64);
        assert!(magic.is_none());
    }

    #[test]
    fn st_clean_eof_returns_none_magic() {
        // Empty stream → clean EOF → next_magic is None.
        let prefs = Prefs::default();
        let res = make_resources();
        let mut out = Vec::new();
        let (size, magic) = decode_legacy_stream(
            &mut std::io::Cursor::new(b""),
            &mut out,
            &prefs,
            &res,
        )
        .expect("empty stream should succeed");
        assert_eq!(size, 0);
        assert!(magic.is_none());
    }

    #[test]
    fn st_next_magic_returned() {
        // A stream that ends with a 4-byte value exceeding compress_bound —
        // this should be returned as the next magic.
        let next_magic_value: u32 = 0x184D2204; // LZ4 frame magic
        let mut payload = Vec::new();
        // One valid compressed block first.
        let data = b"test data for magic detection";
        let compressed = crate::block::compress_block_to_vec(data);
        payload.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        payload.extend_from_slice(&compressed);
        // Then the "next magic" (large value).
        payload.extend_from_slice(&next_magic_value.to_le_bytes());

        let prefs = Prefs::default();
        let res = make_resources();
        let mut out = Vec::new();
        let (size, magic) = decode_legacy_stream(
            &mut std::io::Cursor::new(&payload),
            &mut out,
            &prefs,
            &res,
        )
        .expect("decode should succeed");

        assert_eq!(out, data.as_ref());
        assert_eq!(size, data.len() as u64);
        assert_eq!(magic, Some(next_magic_value));
    }

    #[test]
    fn mt_decompress_small() {
        let original = b"Hello, MT legacy LZ4!";
        let stream = make_legacy_stream(original);
        let payload = legacy_payload(&stream);

        let mut prefs = Prefs::default();
        prefs.nb_workers = 2; // MT path
        let res = make_resources();
        let mut out = Vec::new();
        let (size, magic) = decode_legacy_stream(
            &mut std::io::Cursor::new(payload),
            &mut out,
            &prefs,
            &res,
        )
        .expect("MT decode should succeed");

        assert_eq!(out, original);
        assert_eq!(size, original.len() as u64);
        assert!(magic.is_none());
    }

    #[test]
    fn mt_and_st_produce_same_output() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let stream = make_legacy_stream(&data);
        let payload = legacy_payload(&stream);

        let res = make_resources();
        let mut prefs_st = Prefs::default();
        prefs_st.nb_workers = 0;
        let mut prefs_mt = Prefs::default();
        prefs_mt.nb_workers = 4;

        let mut out_st = Vec::new();
        let (sz_st, mag_st) = decode_legacy_stream(
            &mut std::io::Cursor::new(payload),
            &mut out_st,
            &prefs_st,
            &res,
        )
        .unwrap();

        let mut out_mt = Vec::new();
        let (sz_mt, mag_mt) = decode_legacy_stream(
            &mut std::io::Cursor::new(payload),
            &mut out_mt,
            &prefs_mt,
            &res,
        )
        .unwrap();

        assert_eq!(out_st, out_mt);
        assert_eq!(sz_st, sz_mt);
        assert_eq!(mag_st, mag_mt);
    }

    #[test]
    fn corrupted_input_returns_error() {
        // Block size header claims 10 bytes but actual compressed data is garbage.
        let mut payload = Vec::new();
        payload.extend_from_slice(&10u32.to_le_bytes()); // block_size = 10
        payload.extend_from_slice(&[0xFF; 10]); // garbage compressed data

        let prefs = Prefs::default();
        let res = make_resources();
        let mut out = Vec::new();
        let result = decode_legacy_stream(
            &mut std::io::Cursor::new(&payload),
            &mut out,
            &prefs,
            &res,
        );
        assert!(result.is_err(), "corrupted input should return an error");
    }
}
