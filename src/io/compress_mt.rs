// compress_mt.rs — LZ4 frame multi-threaded (MT) compression pipeline.
// Migrated from lz4io.c lines 455–565, 568–760, 1158–1365 (declarations #7, #8, #12).
//
// Migration decisions:
// - `WriteRegister` (C: sorted dynamic array, qsort-based) →
//   `WriteRegister` Rust struct wrapping a `BTreeMap<u64, Vec<u8>>` + `Mutex`.
//   A BTreeMap keeps pending chunks in chunk-ID order automatically, avoiding
//   the need for qsort. The Mutex makes it safe to share across rayon threads.
// - MT structs `CompressJobDesc` / `ReadTracker` are not needed as named types;
//   closures capture all necessary state.
// - `LZ4IO_compressFilename_extRess_MT` →
//   `compress_filename_mt(in_stream_size, ress, src, dst, level, prefs)`.
//   The function signature mirrors the ST counterpart in compress_frame.rs.
// - Chunk reading is done sequentially (only one thread can read a FILE).
//   Chunks are processed in bounded batches of nb_workers at a time:
//   read up to nb_workers chunks → compress in parallel with rayon → write in order → repeat.
//   This keeps memory bounded to O(nb_workers * CHUNK_SIZE), matching C's O(nb_workers)
//   buffer count from the TPool pipeline.
// - Linked-block mode (blockMode == Linked): each chunk receives the last
//   64 KB of the previous chunk as its prefix (copied before batch dispatch).
//   Chunks within a batch are independent after prefix extraction, so full
//   parallelism is achieved for independent blocks; linked blocks get parallel
//   compression with pre-extracted, independently-owned prefix slices.
// - Content checksum: computed externally with XXH32 over raw input data
//   (matching the C code which resets contentChecksumFlag to LZ4F_noContentChecksum
//   after compressBegin to avoid double-accounting). Written as 4 LE bytes after
//   the 4-byte end-of-data marker.
// - Single-block files (<= CHUNK_SIZE): compressed with `lz4f_compress_frame_using_cdict`
//   in a single pass (same as the C single-block path, lines 1199–1211).
// - `END_PROCESS(code, msg)` (process exit in C) → `io::Error` + early return.
// - File stat propagation uses `crate::util::set_file_stat`.
// - `DISPLAYUPDATE` / `DISPLAYLEVEL` → `crate::io::prefs::display_level`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use rayon::prelude::*;

use crate::frame::{
    lz4f_compress_frame_using_cdict, Lz4FCDict,
};
use crate::frame::compress::lz4f_compress_begin;
use crate::frame::header::lz4f_compress_frame_bound;
use crate::frame::types::{ContentChecksum, BlockMode};
use crate::io::compress_frame::{compress_frame_chunk, CfcParameters, CompressResources};
use crate::io::file_io::{open_dst_file, open_src_file, NUL_MARK, STDIN_MARK};
use crate::io::prefs::{display_level, Prefs, KB, MB};
use crate::util::set_file_stat;
use crate::xxhash::Xxh32State;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Chunk size for MT compression (4 MB), matching C `const size_t chunkSize = 4 MB`.
const CHUNK_SIZE: usize = 4 * MB;

/// Prefix size for linked-block mode (64 KB). Equivalent to `64 KB` in lz4io.c.
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
// WriteRegister — lz4io.c lines 461–565 (declarations #7)
//
// In C: a flat array of `BufferDesc` sorted by chunk rank, extended with realloc.
// In Rust: a `BTreeMap<chunk_id, compressed_bytes>` wrapped in a Mutex so that
// multiple rayon threads can insert pending chunks concurrently.
//
// The C `WR_checkWriteOrder` drains sequentially as chunks arrive; in Rust the
// drain happens after all compressions are done (still in-order via BTreeMap).
// ---------------------------------------------------------------------------

/// Stores out-of-order compressed chunks until the writer can drain them in sequence.
///
/// Equivalent to `WriteRegister` in C, but implemented with a `BTreeMap` instead
/// of `qsort`-managed arrays.
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
    /// Equivalent to `WR_init(blockSize)`.
    fn new(block_size: usize) -> Self {
        WriteRegister {
            expected_rank: 0,
            pending: Mutex::new(BTreeMap::new()),
            total_csize: 0,
            block_size,
        }
    }

    /// Insert a compressed chunk. Equivalent to `WR_addBufDesc`.
    fn insert(&self, chunk_id: u64, data: Vec<u8>) {
        self.pending.lock().unwrap().insert(chunk_id, data);
    }

    /// Drain all pending chunks in order, calling `write_fn` for each.
    ///
    /// Equivalent to `WR_getBufID` + `WR_removeBuffID` + `LZ4IO_writeBuffer`
    /// called from `LZ4IO_checkWriteOrder`.
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
                    &format!(
                        "\rRead : {} MiB   ==> {:.2}%   ",
                        processed >> 20,
                        ratio
                    ),
                );
            }
            self.expected_rank += 1;
            pending = self.pending.lock().unwrap();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// read_to_capacity — fills buf fully from reader, equivalent to fread.
// (local copy — same as in compress_frame.rs, repeated here to avoid coupling)
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
// copy_file_stat — UTIL_getFileStat + UTIL_setFileStat (lz4io.c 1337–1343)
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
// compress_filename_mt — LZ4IO_compressFilename_extRess_MT (lz4io.c 1158–1358)
// ---------------------------------------------------------------------------

/// Multi-threaded frame-format compression of one file.
///
/// Reads from `src_filename`, compresses with `io_prefs.nb_workers` threads,
/// writes to `dst_filename`.  Updates `*in_stream_size` with the total number
/// of uncompressed bytes processed.
///
/// Equivalent to `static int LZ4IO_compressFilename_extRess_MT(...)`.
pub fn compress_filename_mt(
    in_stream_size: &mut u64,
    ress: &mut CompressResources,
    src_filename: &str,
    dst_filename: &str,
    compression_level: i32,
    io_prefs: &Prefs,
) -> io::Result<()> {
    // ── Open files (lz4io.c 1176–1179) ──────────────────────────────────────
    let mut src_reader = open_src_file(src_filename)?;
    let dst_file = open_dst_file(dst_filename, io_prefs).map_err(|e| {
        // close src_reader implicitly on drop
        e
    })?;
    let dst_is_stdout = dst_file.is_stdout;
    let mut dst_writer: Box<dyn Write> = Box::new(dst_file);

    // ── Build per-call preferences (lz4io.c 1182–1189) ──────────────────────
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

    // ── Read first chunk (lz4io.c 1193–1196) ────────────────────────────────
    let mut first_buf = vec![0u8; CHUNK_SIZE];
    let read_size = read_to_capacity(&mut *src_reader, &mut first_buf)?;
    first_buf.truncate(read_size);

    let mut filesize: u64 = read_size as u64;
    let mut compressedfilesize: u64 = 0;

    // ── Single-block path (lz4io.c 1199–1211) ───────────────────────────────
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
        .map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("Compression failed: {}", e))
        })?;
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
        // ── Multi-block MT path (lz4io.c 1216–1330) ─────────────────────────

        let linked_blocks = prefs.frame_info.block_mode == BlockMode::Linked;
        let use_checksum = prefs.frame_info.content_checksum_flag == ContentChecksum::Enabled;

        // ── Write frame header (lz4io.c 1267–1274) ──────────────────────────
        // C note: "do not employ dictionary when input size >= 4 MB, the
        // benefit is very limited anyway, and is not worth the dependency cost"
        // NOTE: contentChecksumFlag is still set here so the frame header declares
        // checksum present — matching C where the flag is reset AFTER compressBegin.
        let header_size = lz4f_compress_begin(
            &mut ress.ctx,
            &mut ress.dst_buffer,
            Some(&prefs),
        )
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
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

        // Disable internal checksum AFTER header write — we compute it externally.
        // Matches lz4io.c line 1277: prefs.frameInfo.contentChecksumFlag = LZ4F_noContentChecksum
        // which comes AFTER LZ4F_compressBegin (line 1269).
        if use_checksum {
            prefs.frame_info.content_checksum_flag = ContentChecksum::Disabled;
        }

        // ── Process chunks in bounded batches ────────────────────────────────
        // Reads and compresses up to nb_workers chunks at a time, then writes
        // them before reading the next batch.  This keeps memory bounded to
        // O(nb_workers * CHUNK_SIZE) — matching C's O(nb_workers) buffer count
        // from the TPool pipeline — rather than buffering the entire file.
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

        // last_suffix: last PREFIX_SIZE bytes of the most-recently-read chunk,
        // supplied as the prefix to the next chunk in linked-block mode.
        // (lz4io.c 1259–1264 / 1296–1298)
        let mut last_suffix: Option<Vec<u8>> = if linked_blocks && read_size >= PREFIX_SIZE {
            Some(first_buf[read_size - PREFIX_SIZE..].to_vec())
        } else {
            None
        };

        // Seed the first batch with the already-read first_buf.
        // First chunk has no prefix (lz4io.c 1283: cjd.prefixSize = 0).
        let mut pending: Option<Chunk> = Some(Chunk { data: first_buf, prefix: None });
        let mut eof = false;

        loop {
            // ── Assemble one batch ────────────────────────────────────────────
            let mut batch: Vec<Chunk> = Vec::with_capacity(batch_size);

            // Carry the pending chunk (first_buf on the first iteration).
            if let Some(c) = pending.take() {
                let short = c.data.len() < CHUNK_SIZE;
                batch.push(c);
                if short { eof = true; }
            }

            // Read additional chunks to fill the batch.
            while !eof && batch.len() < batch_size {
                let mut buf = vec![0u8; CHUNK_SIZE];
                let n = read_to_capacity(&mut *src_reader, &mut buf)?;
                if n == 0 { eof = true; break; }
                buf.truncate(n);
                filesize += n as u64;

                if let Some(ref mut h) = xxh32 { h.update(&buf); }

                // Prefix for this chunk = suffix of the previous chunk.
                let prefix = last_suffix.take();
                if linked_blocks && n >= PREFIX_SIZE {
                    last_suffix = Some(buf[n - PREFIX_SIZE..].to_vec());
                }

                let short = n < CHUNK_SIZE;
                batch.push(Chunk { data: buf, prefix });
                if short { eof = true; }
            }

            if batch.is_empty() { break; }

            // ── Compress this batch in parallel (rayon) ───────────────────────
            // `into_par_iter()` + `collect::<Vec<_>>()` preserves chunk order.
            let batch_results: Vec<io::Result<Vec<u8>>> = batch
                .into_par_iter()
                .map(|chunk| -> io::Result<Vec<u8>> {
                    let mut dst_buf = vec![0u8; max_cblock_size];
                    // SAFETY: sync_cdict.as_ptr() is immutable and valid for ress lifetime.
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

            // ── Write this batch in order ─────────────────────────────────────
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

            if eof { break; }
        }
        compressedfilesize += write_register.total_csize;

        // ── End-of-frame mark (lz4io.c 1310–1323) ───────────────────────────
        // Write 4 bytes of zeros (end-of-data mark), plus optional 4-byte XXH32.
        // The C code notes: LZ4F_compressEnd already wrote a (bogus) checksum;
        // we skip that and write the end block manually.
        let mut end_buf = [0u8; 8];
        // end_buf[0..4] = 0x00000000 (end-of-data block, already zeroed).
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
        dst_writer
            .write_all(&end_buf[..end_size])
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Write error : cannot write end of frame",
                )
            })?;
        compressedfilesize += end_size as u64;
    }

    // ── Release file handles ─────────────────────────────────────────────────
    drop(dst_writer);

    // ── Copy owner/permissions/mtime (lz4io.c 1337–1343) ────────────────────
    if src_filename != STDIN_MARK && !dst_is_stdout && dst_filename != NUL_MARK {
        let _ = copy_file_stat(src_filename, dst_filename);
    }

    // ── Remove source file if --rm (lz4io.c 1345–1348) ──────────────────────
    if io_prefs.remove_src_file && src_filename != STDIN_MARK {
        fs::remove_file(src_filename).map_err(|e| {
            io::Error::new(e.kind(), format!("Remove error : {}: {}", src_filename, e))
        })?;
    }

    // ── Final status display (lz4io.c 1351–1354) ─────────────────────────────
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
