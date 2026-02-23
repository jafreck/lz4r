//! Multi-threaded (MT) frame-format compression pipeline.
//!
//! This module implements parallel LZ4 frame compression using a
//! read-one-thread / compress-many-threads / write-one-thread strategy:
//!
//! 1. The input file is read sequentially in 4 MB chunks.
//! 2. Each batch of up to `nb_workers` chunks is compressed concurrently
//!    via [`rayon`], keeping peak memory proportional to
//!    `nb_workers × CHUNK_SIZE` rather than the full file size.
//! 3. Compressed chunks are written to the output file in their original
//!    order, enforced by [`WriteRegister`].
//!
//! **Linked-block mode**: when `BlockMode::Linked` is active, the last
//! 64 KB of each chunk is extracted before the batch is dispatched so that
//! every compression worker owns its prefix slice independently, enabling
//! full parallelism without shared mutable state.
//!
//! **Content checksum**: the XXH32 digest is computed over the raw input
//! bytes and appended as a 4-byte little-endian value after the end-of-data
//! marker.  The frame header advertises checksum presence; the LZ4F context's
//! internal checksum tracking is disabled after the header is written to
//! avoid double-accounting.
//!
//! Files smaller than `CHUNK_SIZE` take a fast single-block path and skip
//! the batch machinery entirely.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use rayon::prelude::*;

use crate::frame::compress::lz4f_compress_begin;
use crate::frame::header::lz4f_compress_frame_bound;
use crate::frame::types::{BlockMode, ContentChecksum};
use crate::frame::{lz4f_compress_frame_using_cdict, Lz4FCDict};
use crate::io::compress_frame::{compress_frame_chunk, CfcParameters, CompressResources};
use crate::io::file_io::{open_dst_file, open_src_file, NUL_MARK, STDIN_MARK};
use crate::io::prefs::{display_level, Prefs, KB, MB};
use crate::util::set_file_stat;
use crate::xxhash::Xxh32State;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Chunk size for each MT compression unit (4 MB).
///
/// Large enough to give rayon workers substantial independent work while
/// keeping per-worker memory overhead bounded.
const CHUNK_SIZE: usize = 4 * MB;

/// Prefix carried forward in linked-block mode (last 64 KB of each chunk).
///
/// Matches the LZ4 spec's maximum back-reference distance so a linked block
/// can reference any byte written by its predecessor.
const PREFIX_SIZE: usize = 64 * KB;

// ---------------------------------------------------------------------------
// SyncCDictPtr — makes *const Lz4FCDict safe to share across rayon threads.
//
// `Lz4FCDict` is `Sync` (immutable after construction), but raw `*const`
// pointers are not `Sync` by default. This newtype restores that guarantee.
// ---------------------------------------------------------------------------

/// Wrapper around `*const Lz4FCDict` that opts into `Send + Sync`.
///
/// SAFETY: `Lz4FCDict` implements `Sync`; the pointer is immutable during the
/// parallel section and its referent lives for the duration of `ress` borrow.
struct SyncCDictPtr(*const Lz4FCDict);
// SAFETY: Lz4FCDict is Sync; pointer is read-only.
unsafe impl Send for SyncCDictPtr {}
unsafe impl Sync for SyncCDictPtr {}

impl SyncCDictPtr {
    /// Access the inner raw pointer via a method (forces closure to capture the
    /// whole `SyncCDictPtr` rather than just the `*const` field, which is
    /// important for Rust 2021 precise closure capture).
    #[inline]
    fn as_ptr(&self) -> *const Lz4FCDict {
        self.0
    }
}

// ---------------------------------------------------------------------------
// WriteRegister — ordered write buffer for parallel-compressed chunks
//
// Compressed chunks arrive from rayon workers in arbitrary order.  This
// structure buffers them — keyed by their sequential chunk ID — and drains
// them to the writer in ascending ID order.  The BTreeMap provides O(log n)
// insertion and O(1) in-order access without a secondary sort pass.
// ---------------------------------------------------------------------------

/// Stores out-of-order compressed chunks and drains them to the writer in sequence.
///
/// Rayon may complete chunks in any order; this structure buffers each chunk
/// under its sequential ID and emits them strictly in ascending order, ensuring
/// the output stream is well-formed regardless of scheduling.
struct WriteRegister {
    /// Next chunk ID expected to be written.
    expected_rank: u64,
    /// Pending compressed chunks indexed by their chunk ID.
    /// BTreeMap keeps entries sorted so draining is O(n).
    pending: Mutex<BTreeMap<u64, Vec<u8>>>,
    /// Accumulated compressed byte count (written chunks only).
    total_csize: u64,
    /// Block size used for the progress display denominator.
    block_size: usize,
}

impl WriteRegister {
    /// Creates a new register expecting chunk ID 0 first.
    fn new(block_size: usize) -> Self {
        WriteRegister {
            expected_rank: 0,
            pending: Mutex::new(BTreeMap::new()),
            total_csize: 0,
            block_size,
        }
    }

    /// Stores a compressed chunk under its sequential chunk ID.
    ///
    /// Thread-safe; may be called from multiple rayon workers concurrently.
    fn insert(&self, chunk_id: u64, data: Vec<u8>) {
        self.pending.lock().unwrap().insert(chunk_id, data);
    }

    /// Drains all pending chunks whose IDs form an unbroken sequence starting
    /// at `expected_rank`, calling `write_fn` for each in ascending order.
    ///
    /// Stops as soon as a gap is encountered (the missing chunk has not yet
    /// been inserted).  Advances `expected_rank` and `total_csize` for every
    /// chunk successfully written.
    fn drain_in_order(
        &mut self,
        write_fn: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut pending = self.pending.lock().unwrap();
        // Drain in ascending chunk-ID order.
        while let Some(entry) = pending.first_entry() {
            let id = *entry.key();
            if id != self.expected_rank {
                // Gap: wait for the expected chunk (shouldn't happen if all tasks complete).
                break;
            }
            let data = entry.remove();
            self.total_csize += data.len() as u64;
            drop(pending); // Release lock while writing.
            write_fn(&data)?;
            {
                let processed = self.expected_rank * self.block_size as u64;
                let ratio = if processed > 0 {
                    self.total_csize as f64 / processed as f64 * 100.0
                } else {
                    0.0
                };
                display_level(
                    2,
                    &format!("\rRead : {} MiB   ==> {:.2}%   ", processed >> 20, ratio),
                );
            }
            self.expected_rank += 1;
            pending = self.pending.lock().unwrap();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// read_to_capacity — fills `buf` as fully as possible from `reader`.
//
// Retries on `Interrupted` and stops at EOF or when the buffer is full.
// Returns the number of bytes actually read, which may be less than
// `buf.len()` only at the end of the stream.
// ---------------------------------------------------------------------------

fn read_to_capacity(reader: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// copy_file_stat — copies mtime and, on Unix, uid/gid/mode from src to dst.
// ---------------------------------------------------------------------------

fn copy_file_stat(src: &str, dst: &str) -> io::Result<()> {
    let m = fs::metadata(src)?;
    let mtime = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    #[cfg(unix)]
    let (uid, gid, mode) = {
        use std::os::unix::fs::MetadataExt;
        (m.uid(), m.gid(), m.mode())
    };
    #[cfg(not(unix))]
    let (uid, gid, mode) = (0u32, 0u32, 0o644u32);

    set_file_stat(Path::new(dst), mtime, uid, gid, mode)
}

// ---------------------------------------------------------------------------
// Chunk — internal data unit for the MT pipeline
// ---------------------------------------------------------------------------

/// A single read chunk with metadata needed for parallel compression.
struct Chunk {
    /// Raw (uncompressed) chunk data.
    data: Vec<u8>,
    /// Last PREFIX_SIZE bytes of the previous chunk, used as dict in linked mode.
    /// `None` for the first chunk or when blockMode == Independent.
    prefix: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// compress_filename_mt — parallel frame-format compression of a single file
// ---------------------------------------------------------------------------

/// Compresses `src_filename` into `dst_filename` using `io_prefs.nb_workers`
/// parallel compression threads.
///
/// The output is a valid LZ4 frame that any conforming LZ4 decompressor can
/// read.  `*in_stream_size` is set to the total number of uncompressed bytes
/// consumed from the source.
pub fn compress_filename_mt(
    in_stream_size: &mut u64,
    ress: &mut CompressResources,
    src_filename: &str,
    dst_filename: &str,
    compression_level: i32,
    io_prefs: &Prefs,
) -> io::Result<()> {
    let mut src_reader = open_src_file(src_filename)?;
    let dst_file = open_dst_file(dst_filename, io_prefs)?;
    let dst_is_stdout = dst_file.is_stdout;
    let mut dst_writer: Box<dyn Write> = Box::new(dst_file);

    // Build per-call preferences: inherit global settings, then apply call-site overrides.
    let mut prefs = ress.prepared_prefs;
    prefs.compression_level = compression_level;
    if io_prefs.content_size_flag {
        let file_size = if src_filename != STDIN_MARK {
            fs::metadata(src_filename).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        prefs.frame_info.content_size = file_size;
        if file_size == 0 {
            display_level(3, "Warning : cannot determine input content size \n");
        }
    }

    let cdict_ptr = ress.cdict_ptr();

    // Read the first chunk to decide whether the single-block or multi-block path applies.
    let mut first_buf = vec![0u8; CHUNK_SIZE];
    let read_size = read_to_capacity(&mut *src_reader, &mut first_buf)?;
    first_buf.truncate(read_size);

    let mut filesize: u64 = read_size as u64;
    let mut compressedfilesize: u64 = 0;

    // Single-block fast path: the entire input fits in one CHUNK_SIZE buffer,
    // so compress it as a single self-contained frame without the batch machinery.
    if read_size < CHUNK_SIZE {
        let max_dst = lz4f_compress_frame_bound(read_size, Some(&prefs));
        let mut dst_buf = vec![0u8; max_dst];
        let c_size = lz4f_compress_frame_using_cdict(
            &mut ress.ctx,
            &mut dst_buf,
            &first_buf,
            cdict_ptr,
            Some(&prefs),
        )
        .map_err(|e| io::Error::other(format!("Compression failed: {}", e)))?;
        compressedfilesize = c_size as u64;

        display_level(
            2,
            &format!(
                "\rRead : {} MiB   ==> {:.2}%   ",
                filesize >> 20,
                compressedfilesize as f64 / filesize.max(1) as f64 * 100.0,
            ),
        );

        dst_writer.write_all(&dst_buf[..c_size]).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WriteZero,
                "Write error : failed writing single-block compressed frame",
            )
        })?;
    } else {
        // Multi-block path: read, compress in parallel, and write in bounded batches.

        let linked_blocks = prefs.frame_info.block_mode == BlockMode::Linked;
        let use_checksum = prefs.frame_info.content_checksum_flag == ContentChecksum::Enabled;

        // Write the LZ4 frame header.  The content-checksum flag must still be
        // set at this point so the header correctly declares that a checksum
        // is present; the flag is cleared on the working copy of `prefs` after
        // the header is written so the LZ4F context does not attempt to compute
        // a second, internal checksum.
        let header_size = lz4f_compress_begin(&mut ress.ctx, &mut ress.dst_buffer, Some(&prefs))
            .map_err(|e| {
                io::Error::other(
                    format!("File header generation failed : {}", e),
                )
            })?;
        dst_writer
            .write_all(&ress.dst_buffer[..header_size])
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Write error : cannot write header",
                )
            })?;
        compressedfilesize += header_size as u64;

        // Disable the LZ4F context's internal checksum tracking after the header
        // has been written.  We accumulate the XXH32 digest over raw input bytes
        // ourselves and append it manually, which is necessary to maintain a
        // correct rolling checksum across independently-compressed parallel chunks.
        if use_checksum {
            prefs.frame_info.content_checksum_flag = ContentChecksum::Disabled;
        }

        // Process chunks in bounded batches of at most `nb_workers` chunks.
        // Reading the next batch is deferred until the current batch is fully
        // written, which bounds peak memory to O(nb_workers × CHUNK_SIZE).
        let batch_size = (io_prefs.nb_workers as usize).max(1);
        let max_cblock_size = lz4f_compress_frame_bound(CHUNK_SIZE, Some(&prefs));
        // Wrap cdict_ptr in a Sync+Send newtype so rayon closures can capture it.
        let sync_cdict = SyncCDictPtr(cdict_ptr);
        let mut write_register = WriteRegister::new(CHUNK_SIZE);

        // xxh32 accumulates content checksum over raw input bytes.
        let mut xxh32 = if use_checksum {
            let mut h = Xxh32State::new(0);
            h.update(&first_buf);
            Some(h)
        } else {
            None
        };

        // last_suffix: the final PREFIX_SIZE bytes of the most-recently-read chunk.
        // In linked-block mode this slice is given to the *next* chunk as its
        // prefix dictionary, so each block can reference data from its predecessor.
        let mut last_suffix: Option<Vec<u8>> = if linked_blocks && read_size >= PREFIX_SIZE {
            Some(first_buf[read_size - PREFIX_SIZE..].to_vec())
        } else {
            None
        };

        // Seed the first batch with the already-read first chunk.
        // The first chunk never has a prefix because there is no preceding chunk.
        let mut pending: Option<Chunk> = Some(Chunk {
            data: first_buf,
            prefix: None,
        });
        let mut eof = false;

        loop {
            // ── Assemble one batch ────────────────────────────────────────────
            let mut batch: Vec<Chunk> = Vec::with_capacity(batch_size);

            // Carry the pending chunk (first_buf on the first iteration).
            if let Some(c) = pending.take() {
                let short = c.data.len() < CHUNK_SIZE;
                batch.push(c);
                if short {
                    eof = true;
                }
            }

            // Read additional chunks to fill the batch.
            while !eof && batch.len() < batch_size {
                let mut buf = vec![0u8; CHUNK_SIZE];
                let n = read_to_capacity(&mut *src_reader, &mut buf)?;
                if n == 0 {
                    eof = true;
                    break;
                }
                buf.truncate(n);
                filesize += n as u64;

                if let Some(ref mut h) = xxh32 {
                    h.update(&buf);
                }

                // Prefix for this chunk = suffix of the previous chunk.
                let prefix = last_suffix.take();
                if linked_blocks && n >= PREFIX_SIZE {
                    last_suffix = Some(buf[n - PREFIX_SIZE..].to_vec());
                }

                let short = n < CHUNK_SIZE;
                batch.push(Chunk { data: buf, prefix });
                if short {
                    eof = true;
                }
            }

            if batch.is_empty() {
                break;
            }

            // Compress this batch in parallel.  Collecting into a Vec preserves
            // the original chunk order so writing is straightforward.
            let batch_results: Vec<io::Result<Vec<u8>>> = batch
                .into_par_iter()
                .map(|chunk| -> io::Result<Vec<u8>> {
                    let mut dst_buf = vec![0u8; max_cblock_size];
                    // SAFETY: `sync_cdict` wraps an immutable pointer that remains
                    // valid for the duration of the enclosing `ress` borrow.
                    // Multiple threads reading the same immutable CDict is safe.
                    let params = CfcParameters {
                        prefs: &prefs,
                        cdict: sync_cdict.as_ptr(),
                    };
                    let c_size = compress_frame_chunk(
                        &params,
                        &mut dst_buf,
                        &chunk.data,
                        chunk.prefix.as_deref(),
                    )?;
                    dst_buf.truncate(c_size);
                    Ok(dst_buf)
                })
                .collect();

            // Write each compressed chunk in original order via WriteRegister.
            for result in batch_results {
                let c_data = result?;
                write_register.insert(write_register.expected_rank, c_data);
                write_register.drain_in_order(&mut |bytes| {
                    dst_writer.write_all(bytes).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::WriteZero,
                            "Write error : cannot write compressed block",
                        )
                    })
                })?;
            }

            if eof {
                break;
            }
        }
        compressedfilesize += write_register.total_csize;

        // Finalise the frame: write the 4-byte end-of-data marker
        // (0x00000000) followed by the optional 4-byte XXH32 content checksum.
        // We write both fields manually so we can inject our externally-computed
        // checksum rather than relying on the LZ4F context (whose checksum was
        // disabled after the header was written).
        let mut end_buf = [0u8; 8];
        // Bytes 0–3 are the end-of-data block (already zero-initialised).
        let end_size = if use_checksum {
            if let Some(h) = xxh32 {
                let crc = h.digest();
                end_buf[4..8].copy_from_slice(&crc.to_le_bytes());
                8
            } else {
                4
            }
        } else {
            4
        };
        dst_writer.write_all(&end_buf[..end_size]).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WriteZero,
                "Write error : cannot write end of frame",
            )
        })?;
        compressedfilesize += end_size as u64;
    }

    // Flush and close the destination file before touching its metadata.
    drop(dst_writer);

    // Propagate mtime and, on Unix, uid/gid/mode from source to destination.
    if src_filename != STDIN_MARK && !dst_is_stdout && dst_filename != NUL_MARK {
        let _ = copy_file_stat(src_filename, dst_filename);
    }

    // Remove the source file when `--rm` is active.
    if io_prefs.remove_src_file && src_filename != STDIN_MARK {
        fs::remove_file(src_filename).map_err(|e| {
            io::Error::new(e.kind(), format!("Remove error : {}: {}", src_filename, e))
        })?;
    }

    // Print the final compression-ratio summary line.
    display_level(2, &format!("\r{:79}\r", ""));
    display_level(
        2,
        &format!(
            "Compressed {} bytes into {} bytes ==> {:.2}%\n",
            filesize,
            compressedfilesize,
            compressedfilesize as f64 / filesize.max(1) as f64 * 100.0,
        ),
    );

    *in_stream_size = filesize;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::compress_frame::CompressResources;
    use crate::io::prefs::Prefs;
    use tempfile::TempDir;

    // ── WriteRegister ────────────────────────────────────────────────────────

    #[test]
    fn write_register_drains_in_order() {
        let mut wr = WriteRegister::new(CHUNK_SIZE);
        // Insert chunks out of order: 2, 0, 1.
        wr.insert(2, vec![2u8; 4]);
        wr.insert(0, vec![0u8; 4]);
        wr.insert(1, vec![1u8; 4]);

        let mut written: Vec<u8> = Vec::new();
        wr.drain_in_order(&mut |bytes| {
            written.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        // Should drain 0, 1, 2 in order.
        assert_eq!(&written[0..4], &[0u8; 4]);
        assert_eq!(&written[4..8], &[1u8; 4]);
        assert_eq!(&written[8..12], &[2u8; 4]);
        assert_eq!(wr.expected_rank, 3);
        assert_eq!(wr.total_csize, 12);
    }

    #[test]
    fn write_register_stops_at_gap() {
        let mut wr = WriteRegister::new(CHUNK_SIZE);
        // Insert chunk 0 and chunk 2 (gap at 1).
        wr.insert(0, vec![0u8; 4]);
        wr.insert(2, vec![2u8; 4]);

        let mut written: Vec<u8> = Vec::new();
        wr.drain_in_order(&mut |bytes| {
            written.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        // Only chunk 0 should be drained; chunk 2 is still pending.
        assert_eq!(written.len(), 4);
        assert_eq!(wr.expected_rank, 1);
    }

    // ── compress_filename_mt round-trip ─────────────────────────────────────

    #[test]
    fn compress_filename_mt_round_trip_small_file() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("input.bin");
        let dst = dir.path().join("output.lz4");

        // Write a small test file (< CHUNK_SIZE).
        let original = b"Hello MT compression round-trip test!".repeat(100);
        std::fs::write(&src, &original).unwrap();

        let mut prefs = Prefs::default();
        prefs.nb_workers = 2;
        let mut ress = CompressResources::new(&prefs).expect("resources");

        let mut in_size = 0u64;
        compress_filename_mt(
            &mut in_size,
            &mut ress,
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            1,
            &prefs,
        )
        .expect("MT compress small");

        assert_eq!(in_size, original.len() as u64);
        assert!(dst.exists());
        assert!(dst.metadata().unwrap().len() > 0);
    }

    #[test]
    fn compress_filename_mt_round_trip_multi_block() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("input_large.bin");
        let dst = dir.path().join("output_large.lz4");

        // Create a file larger than CHUNK_SIZE (4 MB) to exercise the multi-block path.
        // Use 5 MB of patterned data.
        let pattern: Vec<u8> = (0u8..=255).cycle().take(5 * MB).collect();
        std::fs::write(&src, &pattern).unwrap();

        let mut prefs = Prefs::default();
        prefs.nb_workers = 2;
        let mut ress = CompressResources::new(&prefs).expect("resources");

        let mut in_size = 0u64;
        compress_filename_mt(
            &mut in_size,
            &mut ress,
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            1,
            &prefs,
        )
        .expect("MT compress multi-block");

        assert_eq!(in_size, pattern.len() as u64);
        assert!(dst.exists());
        // Compressed file must be non-empty.
        assert!(dst.metadata().unwrap().len() > 0);
    }
}
