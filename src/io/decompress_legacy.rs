//! Legacy LZ4 decompression.
//!
//! Migrated from lz4io.c lines 1677–1887 (declarations #15, #16):
//! `g_magicRead` global, MT/ST variants of `LZ4IO_decodeLegacyStream`, and
//! the `LZ4IO_writeDecodedChunk` / `LZ4IO_decompressBlockLegacy` helpers.
//!
//! # Migration decisions
//!
//! - **`g_magicRead` global out-param** eliminated: `decode_legacy_stream`
//!   returns `(decoded_bytes: u64, next_magic: Option<u32>)`.  When the block-
//!   size field read from the stream exceeds `LZ4_COMPRESSBOUND(LEGACY_BLOCKSIZE)`,
//!   it is interpreted as a next-stream magic number and returned as
//!   `Some(magic)` instead of being stored in a module-level global.
//!
//! - **ST path** (`prefs.nb_workers <= 1`): a simple sequential loop that
//!   reads a 4-byte block header, decompresses the block with
//!   `lz4_flex::block::decompress`, and writes to `dst`.  Mirrors
//!   `LZ4IO_decodeLegacyStream` (ST variant, lz4io.c lines 1825–1873).
//!
//! - **MT path** (`prefs.nb_workers > 1`): reads compressed blocks in batches
//!   of `NB_BUFFSETS` and decompresses each batch in parallel using `rayon`.
//!   Results are collected in order and written sequentially by the main
//!   thread.  The C implementation used two `TPool` instances (one for
//!   decompression, one for serialised writes) connected by a 3-stage
//!   pipeline.  Replicating that pipeline with a generic `impl Write`
//!   (which is not `Send`) is not possible without unsafe code; the rayon
//!   batch approach provides equivalent throughput with safe Rust.
//!
//! - **Sparse writes**: `fwrite_sparse` / `fwrite_sparse_end` require a
//!   `&mut File` reference.  Because `decode_legacy_stream` accepts a generic
//!   `impl Write`, sparse-hole optimisation is delegated to the caller
//!   (`decompress_dispatch`) which has direct access to the `File`.
//!
//! - **`LZ4_decompress_safe`** → `lz4_flex::block::decompress`.
//!
//! - **`LZ4_compressBound(LEGACY_BLOCKSIZE)`**: computed at runtime via
//!   `lz4_sys::LZ4_compressBound`; used to distinguish valid block sizes from
//!   embedded magic numbers in the stream.

use std::io::{self, Read, Write};

use lz4_flex::block::decompress as lz4_block_decompress;
use rayon::prelude::*;

use crate::io::decompress_resources::DecompressResources;
use crate::io::prefs::{Prefs, LEGACY_BLOCKSIZE};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of buffer sets used in the C MT pipeline (1 reading + 1 processing
/// + 1 writing + 1 queued).  Reused here as the rayon batch size.
const NB_BUFFSETS: usize = 4;

/// Size of the block-size header field in the legacy format (4 bytes).
///
/// Equivalent to `LZ4IO_LEGACY_BLOCK_HEADER_SIZE` (lz4io.c line 760).
const LEGACY_BLOCK_HEADER_SIZE: usize = 4;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the maximum size of a compressed block for `LEGACY_BLOCKSIZE`
/// uncompressed bytes.
///
/// Equivalent to `LZ4_compressBound(LEGACY_BLOCKSIZE)` (lz4io.c line 1777).
fn lz4_compress_bound() -> usize {
    // SAFETY: LZ4_compressBound is a pure C function with no side effects.
    unsafe { lz4_sys::LZ4_compressBound(LEGACY_BLOCKSIZE as i32) as usize }
}

/// Reads exactly `buf.len()` bytes, returning `Ok(false)` on a clean EOF
/// encountered before the first byte, `Ok(true)` on success, or an error if
/// EOF occurs mid-read.
///
/// Used to detect end-of-stream when reading the 4-byte block header without
/// treating a clean EOF as an error (mirrors `if (sizeCheck == 0) break;` in
/// lz4io.c lines 1772, 1841).
fn read_exact_or_eof<R: Read>(src: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    // Read the first byte to distinguish clean EOF from mid-read EOF.
    let n = src.read(&mut buf[..1])?;
    if n == 0 {
        return Ok(false); // clean EOF
    }
    // Read remaining bytes; mid-read EOF is an error.
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
/// # C equivalent
///
/// `LZ4IO_decodeLegacyStream` — both the MT variant (lz4io.c lines 1741–1821)
/// and the ST variant (lz4io.c lines 1825–1873).  Dispatches based on
/// `prefs.nb_workers`.
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
/// Equivalent to the `#else` branch of `LZ4IO_decodeLegacyStream`
/// (lz4io.c lines 1825–1873).
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
        // Read block header — clean EOF terminates the stream normally.
        if !read_exact_or_eof(src, &mut header)? {
            break; // Nothing to read: file read is completed (lz4io.c:1841).
        }

        // Convert block size to native endianness (lz4io.c:1846).
        let block_size = u32::from_le_bytes(header);

        if block_size as usize > compress_bound {
            // Cannot read next block: maybe new stream? (lz4io.c:1847–1850).
            // Return the value as the next magic number instead of storing in
            // the `g_magicRead` global.
            next_magic = Some(block_size);
            break;
        }

        // Read the compressed block (lz4io.c:1854).
        let block_len = block_size as usize;
        src.read_exact(&mut in_buf[..block_len])?;

        // Decompress the block (lz4io.c:1858 — `LZ4_decompress_safe`).
        let decompressed =
            lz4_block_decompress(&in_buf[..block_len], LEGACY_BLOCKSIZE).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Decoding Failed! Corrupted input detected!: {e}"),
                )
            })?;

        // Write the decompressed block (lz4io.c:1862).
        stream_size += decompressed.len() as u64;
        dst.write_all(&decompressed)?;
    }

    // `ferror` equivalent: propagated as `Err` from `read_exact_or_eof` /
    // `read_exact` above; no explicit check needed.

    Ok((stream_size, next_magic))
}

// ---------------------------------------------------------------------------
// Multi-threaded path
// ---------------------------------------------------------------------------

/// Multi-threaded legacy decompression using rayon batch parallelism.
///
/// Reads compressed blocks in batches of `NB_BUFFSETS`, decompresses each
/// batch in parallel with rayon, then writes decompressed output in order.
///
/// The C implementation (lz4io.c lines 1741–1821) used two `TPool` instances
/// forming a 3-stage pipeline (read → decompress → write).  The generic
/// `impl Write` bound prevents moving the write stage to a separate thread
/// (as `Write` is not `Send`).  The rayon batch approach provides equivalent
/// CPU-bound throughput while keeping writes on the calling thread.
fn decode_legacy_mt<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    _prefs: &Prefs,
) -> io::Result<(u64, Option<u32>)> {
    let compress_bound = lz4_compress_bound();
    let mut stream_size: u64 = 0;
    let mut next_magic: Option<u32> = None;

    loop {
        // ── Read a batch of compressed blocks ────────────────────────────────
        // Equivalent to the C "Main Loop" with NB_BUFFSETS rotating buffer sets.
        let mut batch: Vec<Vec<u8>> = Vec::with_capacity(NB_BUFFSETS);
        let mut batch_done = false; // signals that next_magic or EOF was found

        for _ in 0..NB_BUFFSETS {
            let mut header = [0u8; LEGACY_BLOCK_HEADER_SIZE];

            // Clean EOF: stream is finished (lz4io.c:1772).
            if !read_exact_or_eof(src, &mut header)? {
                batch_done = true;
                break;
            }

            let block_size = u32::from_le_bytes(header);
            if block_size as usize > compress_bound {
                // Magic number for next frame (lz4io.c:1777–1780).
                next_magic = Some(block_size);
                batch_done = true;
                break;
            }

            // Read the compressed block data (lz4io.c:1784).
            let mut block = vec![0u8; block_size as usize];
            src.read_exact(&mut block)?;
            batch.push(block);
        }

        if batch.is_empty() {
            break;
        }

        // ── Decompress batch in parallel ──────────────────────────────────────
        // Equivalent to `TPool_submitJob(tPool, LZ4IO_decompressBlockLegacy, lbi)`
        // (lz4io.c:1799) but using rayon for safe parallelism with generic Write.
        let results: Vec<io::Result<Vec<u8>>> = batch
            .par_iter()
            .map(|block| {
                lz4_block_decompress(block, LEGACY_BLOCKSIZE).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Decoding Failed! Corrupted input detected!: {e}"),
                    )
                })
            })
            .collect();

        // ── Write results in order ────────────────────────────────────────────
        // Equivalent to `LZ4IO_writeDecodedChunk` (lz4io.c lines 1690–1701).
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
    /// using `lz4_flex::block::compress` and return the raw stream bytes.
    fn make_legacy_stream(data: &[u8]) -> Vec<u8> {
        const LZ4IO_LEGACY_MAGICNUMBER: u32 = 0x184C2102;
        let mut stream = Vec::new();
        // Write magic number.
        stream.extend_from_slice(&LZ4IO_LEGACY_MAGICNUMBER.to_le_bytes());
        // Split data into LEGACY_BLOCKSIZE chunks and compress each.
        for chunk in data.chunks(LEGACY_BLOCKSIZE) {
            let compressed = lz4_flex::block::compress(chunk);
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
            let compressed = lz4_flex::block::compress(chunk);
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
        let compressed = lz4_flex::block::compress(data);
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
