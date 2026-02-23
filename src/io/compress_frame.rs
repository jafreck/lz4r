//! Single-threaded LZ4 frame-format compression.
//!
//! This module implements frame-format compression for the `lz4r` I/O pipeline
//! (see the [LZ4 frame format specification](https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md)).
//! It exposes:
//!
//! - [`CompressResources`] — a reusable bundle of I/O buffers, a streaming
//!   compression context ([`Lz4FCCtx`]), and an optional pre-digested
//!   compression dictionary ([`Lz4FCDict`]).  Allocated once and shared across
//!   all files in a single invocation.
//! - [`CfcParameters`] — lightweight parameter struct passed to
//!   [`compress_frame_chunk`], the per-chunk primitive also consumed by the
//!   multi-threaded path in `io::compress_mt`.
//! - [`compress_filename`] — end-to-end single-file compression.
//! - [`compress_multiple_filenames`] — batch compression with a shared suffix.
//!
//! # Single-threaded vs multi-threaded
//!
//! [`compress_filename_ext`] always calls the single-threaded path.  When the
//! `multithread` feature is enabled and `nb_workers > 1`, callers should
//! dispatch to `io::compress_mt::compress_filename_mt` instead.
//!
//! # Dictionary support
//!
//! A dictionary file is read once via `load_dict_file`, which uses a circular
//! buffer to retain only the final 64 KB regardless of the file's total size
//! (matching the 64-KB sliding-window limit of LZ4's block format).  The raw
//! bytes are then digested into an [`Lz4FCDict`] for efficient reuse across
//! all compressed files.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::ptr;
use std::time::SystemTime;

use crate::frame::compress::{
    lz4f_compress_begin_using_cdict, lz4f_compress_begin_using_dict, LZ4F_VERSION,
};
use crate::frame::header::lz4f_compress_frame_bound;
use crate::frame::types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, FrameInfo, FrameType, Preferences,
};
use crate::frame::{
    lz4f_compress_end, lz4f_compress_frame_using_cdict, lz4f_compress_update,
    lz4f_create_compression_context, Lz4FCCtx, Lz4FCDict,
};
use crate::io::file_io::{open_dst_file, open_src_file, NUL_MARK, STDIN_MARK, STDOUT_MARK};
use crate::io::prefs::{display_level, final_time_display, Prefs, KB, LZ4_MAX_DICT_SIZE, MB};
use crate::timefn::get_time;
use crate::util::set_file_stat;

extern "C" {
    fn clock() -> libc::clock_t;
}

// ---------------------------------------------------------------------------
// Source chunk size (lz4io.c line 1079: `const size_t chunkSize = 4 MB`)
// ---------------------------------------------------------------------------

const CHUNK_SIZE: usize = 4 * MB;

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

/// Statistics returned from a successful frame-format compression run.
///
/// Equivalent to the `*inStreamSize` out-parameter and return code in C.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompressStats {
    /// Total uncompressed source bytes processed.
    pub bytes_in: u64,
    /// Total compressed bytes written to the destination.
    pub bytes_out: u64,
}

// ---------------------------------------------------------------------------
// CfcParameters — parameters for compress_frame_chunk.
// Also consumed by the multi-threaded path in io::compress_mt.
// ---------------------------------------------------------------------------

/// Parameters for `compress_frame_chunk`, equivalent to `LZ4IO_CfcParameters`.
pub struct CfcParameters<'a> {
    /// Frame compression preferences (already has the desired level set).
    pub prefs: &'a Preferences,
    /// Pre-digested dictionary pointer, or null if no dictionary.
    /// SAFETY: must remain valid for the duration of the call.
    pub cdict: *const Lz4FCDict,
}

// SAFETY: *const Lz4FCDict is an immutable reference after construction.
unsafe impl<'a> Send for CfcParameters<'a> {}
unsafe impl<'a> Sync for CfcParameters<'a> {}

// ---------------------------------------------------------------------------
// CompressResources — cRess_t (lz4io.c lines 978-988)
// ---------------------------------------------------------------------------

/// Compression resources allocated once and reused across multiple files.
///
/// Equivalent to `cRess_t` in the reference implementation.  Thread-pool
/// fields (`tPool`, `wPool`) are not stored here; they belong to `io::compress_mt`.
pub struct CompressResources {
    /// Source I/O buffer (CHUNK_SIZE = 4 MB). Equivalent to `srcBuffer`.
    pub src_buffer: Vec<u8>,
    /// Destination I/O buffer (worst-case frame size). Equivalent to `dstBuffer`.
    pub dst_buffer: Vec<u8>,
    /// Streaming compression context. Equivalent to `ctx`.
    pub ctx: Box<Lz4FCCtx>,
    /// Frame preferences prepared from `Prefs` at allocation time.
    /// Equivalent to `preparedPrefs` (without per-file `compressionLevel`/`contentSize`).
    pub prepared_prefs: Preferences,
    /// Pre-digested dictionary, or `None` when no dictionary is active.
    /// Equivalent to `cdict` (an `LZ4F_CDict*`).
    pub cdict: Option<Box<Lz4FCDict>>,
}

// SAFETY: The raw *const Lz4FCDict pointer derived from `cdict` is only used
// inside compression calls and never stored beyond CompressResources' lifetime.
unsafe impl Send for CompressResources {}

// ---------------------------------------------------------------------------
// Helper: build Preferences from Prefs (maps io::prefs types to frame types)
// ---------------------------------------------------------------------------

fn build_preferences(io_prefs: &Prefs) -> Preferences {
    let block_size_id = match io_prefs.block_size_id {
        4 => BlockSizeId::Max64Kb,
        5 => BlockSizeId::Max256Kb,
        6 => BlockSizeId::Max1Mb,
        _ => BlockSizeId::Max4Mb, // 7 (default) → 4 MB
    };
    let block_mode = if io_prefs.block_independence {
        BlockMode::Independent
    } else {
        BlockMode::Linked
    };
    Preferences {
        frame_info: FrameInfo {
            block_size_id,
            block_mode,
            content_checksum_flag: if io_prefs.stream_checksum {
                ContentChecksum::Enabled
            } else {
                ContentChecksum::Disabled
            },
            block_checksum_flag: if io_prefs.block_checksum {
                BlockChecksum::Enabled
            } else {
                BlockChecksum::Disabled
            },
            frame_type: FrameType::Frame,
            content_size: 0, // overridden per-call when content_size_flag is set
            dict_id: 0,
        },
        compression_level: 0, // overridden per-call
        auto_flush: true,     // mirrors ress.preparedPrefs.autoFlush = 1
        favor_dec_speed: io_prefs.favor_dec_speed,
    }
}

// ---------------------------------------------------------------------------
// Helper: effective block size
// ---------------------------------------------------------------------------

/// Returns the actual block size in bytes, deriving it from block_size_id when
/// block_size is 0. Equivalent to `io_prefs->blockSize` in the C code after
/// `LZ4IO_createCResources` has been called (which fills in the default).
fn effective_block_size(io_prefs: &Prefs) -> usize {
    if io_prefs.block_size > 0 {
        io_prefs.block_size
    } else {
        match io_prefs.block_size_id {
            4 => 64 * KB,
            5 => 256 * KB,
            6 => MB,
            _ => 4 * MB, // 7 → 4 MB
        }
    }
}

// ---------------------------------------------------------------------------
// load_dict_file — LZ4IO_createDict (lz4io.c lines 1005-1062)
// Reads at most LZ4_MAX_DICT_SIZE (64 KB) bytes from the end of a file.
// Uses a circular buffer so only one read pass is needed.
// ---------------------------------------------------------------------------

fn load_dict_file(dict_filename: &str) -> io::Result<Vec<u8>> {
    let circ_size = LZ4_MAX_DICT_SIZE; // 64 KB
    let mut circular_buf = vec![0u8; circ_size];
    let mut dict_end: usize = 0;
    let mut dict_len: usize = 0;

    // Open the dict file (stdin sentinel or a real file).
    let mut reader: Box<dyn Read> = if dict_filename == STDIN_MARK {
        Box::new(io::stdin())
    } else {
        let mut f = fs::File::open(dict_filename).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("Dictionary error: could not open {}: {}", dict_filename, e),
            )
        })?;
        // Opportunistically seek to the last 64 KB (lz4io.c:1027-1029).
        // If this fails (e.g. pipes), we just read from the current position.
        {
            use std::io::Seek;
            let _ = f.seek(std::io::SeekFrom::End(-(circ_size as i64)));
        }
        Box::new(f)
    };

    // Fill the circular buffer (lz4io.c:1031-1035).
    loop {
        let n = reader.read(&mut circular_buf[dict_end..])?;
        if n == 0 {
            break; // EOF
        }
        dict_end = (dict_end + n) % circ_size;
        dict_len += n;
    }

    // Clamp to 64 KB (lz4io.c:1037-1039).
    if dict_len > LZ4_MAX_DICT_SIZE {
        dict_len = LZ4_MAX_DICT_SIZE;
    }

    // Reconstruct a contiguous slice from the circular buffer (lz4io.c:1043-1056).
    let dict_start = (circ_size + dict_end - dict_len) % circ_size;

    if dict_start == 0 {
        // Simple case: dict is already contiguous from offset 0.
        circular_buf.truncate(dict_len);
        Ok(circular_buf)
    } else {
        // Wrapped case: copy dict_start..end, then 0..remaining.
        let first_len = (circ_size - dict_start).min(dict_len);
        let second_len = dict_len - first_len;
        let mut dict_buf = vec![0u8; dict_len.max(1)];
        dict_buf[..first_len].copy_from_slice(&circular_buf[dict_start..dict_start + first_len]);
        if second_len > 0 {
            dict_buf[first_len..].copy_from_slice(&circular_buf[..second_len]);
        }
        Ok(dict_buf)
    }
}

// ---------------------------------------------------------------------------
// create_cdict — LZ4IO_createCDict (lz4io.c lines 1064-1075)
// ---------------------------------------------------------------------------

fn create_cdict(io_prefs: &Prefs) -> io::Result<Option<Box<Lz4FCDict>>> {
    if !io_prefs.use_dictionary {
        return Ok(None);
    }
    let dict_filename = io_prefs.dictionary_filename.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Dictionary error: no filename provided",
        )
    })?;
    let dict_buf = load_dict_file(dict_filename)?;
    let cdict = Lz4FCDict::create(&dict_buf)
        .ok_or_else(|| io::Error::other("Dictionary error: could not create CDict"))?;
    Ok(Some(cdict))
}

// ---------------------------------------------------------------------------
// CompressResources::new — LZ4IO_createCResources (lz4io.c lines 1077-1113)
// ---------------------------------------------------------------------------

impl CompressResources {
    /// Allocate compression resources for the given preferences.
    ///
    /// Equivalent to `LZ4IO_createCResources`.
    pub fn new(io_prefs: &Prefs) -> io::Result<Self> {
        let prepared_prefs = build_preferences(io_prefs);

        // Allocate the LZ4F compression context (lz4io.c:1092-1095).
        let ctx = lz4f_create_compression_context(LZ4F_VERSION).map_err(|e| {
            io::Error::other(format!(
                "Allocation error: can't create LZ4F context: {}",
                e
            ))
        })?;

        // Allocate source and destination buffers (lz4io.c:1099-1104).
        let src_buffer = vec![0u8; CHUNK_SIZE];
        let dst_buffer_size = lz4f_compress_frame_bound(CHUNK_SIZE, Some(&prepared_prefs));
        let dst_buffer = vec![0u8; dst_buffer_size];

        // Load dictionary if one is configured (lz4io.c:1106).
        let cdict = create_cdict(io_prefs)?;

        Ok(CompressResources {
            src_buffer,
            dst_buffer,
            ctx,
            prepared_prefs,
            cdict,
        })
    }

    /// Return a raw pointer to the CDict, or null if none.
    ///
    /// SAFETY: The returned pointer is valid for the lifetime of `self`.
    pub fn cdict_ptr(&self) -> *const Lz4FCDict {
        self.cdict
            .as_deref()
            .map_or(ptr::null(), |c| c as *const Lz4FCDict)
    }
}

// ---------------------------------------------------------------------------
// read_to_capacity: fill buf[..capacity] from reader (equivalent to fread)
// ---------------------------------------------------------------------------

fn read_to_capacity(reader: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break, // EOF
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// copy_file_stat helper — UTIL_setFileStat (lz4io.c lines 1467-1473)
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
// compress_frame_chunk — LZ4IO_compressFrameChunk (lz4io.c lines 1120-1150)
// ---------------------------------------------------------------------------

/// Compress `src` into `dst` as a single LZ4 frame chunk.
///
/// A temporary `Lz4FCCtx` is created and destroyed per call (equivalent to
/// the C code which calls `LZ4F_createCompressionContext` / `LZ4F_freeCompressionContext`
/// internally).
///
/// The compression begin call writes a frame header to `dst[0..]` to initialise
/// stream state, then `lz4f_compress_update` *overwrites* `dst` from offset 0
/// with the actual compressed block data (see C comment: "will be overwritten at
/// next stage").  The return value is the number of bytes of compressed block
/// data in `dst[0..return_value]` — the frame header is discarded.
///
/// `prefix_data`: if `Some`, the bytes immediately preceding `src` used as a
/// 64-KB linked-block prefix (`LZ4F_compressBegin_usingDict` path in C).
/// If `None`, the CDict from `params.cdict` is used instead.
///
/// Equivalent to `LZ4IO_compressFrameChunk`.
pub fn compress_frame_chunk(
    params: &CfcParameters<'_>,
    dst: &mut [u8],
    src: &[u8],
    prefix_data: Option<&[u8]>,
) -> io::Result<usize> {
    // Create a fresh per-chunk context (lz4io.c:1126-1129).
    let mut cctx = lz4f_create_compression_context(LZ4F_VERSION).map_err(|e| {
        io::Error::other(format!(
            "unable to create a LZ4F compression context: {}",
            e
        ))
    })?;

    // Write frame header to dst (lz4io.c:1132-1141).
    // The header is overwritten by compressUpdate in the next step.
    if let Some(prefix) = prefix_data {
        // LZ4F_compressBegin_usingDict (lz4io.c:1133)
        lz4f_compress_begin_using_dict(&mut cctx, dst, prefix, Some(params.prefs)).map_err(
            |e| {
                io::Error::other(format!(
                    "error initializing LZ4F compression context with prefix: {}",
                    e
                ))
            },
        )?;
    } else {
        // LZ4F_compressBegin_usingCDict (lz4io.c:1138)
        // SAFETY: params.cdict is valid for the duration of this call.
        unsafe {
            lz4f_compress_begin_using_cdict(&mut cctx, dst, params.cdict, Some(params.prefs))
        }
        .map_err(|e| {
            io::Error::other(format!(
                "error initializing LZ4F compression context: {}",
                e
            ))
        })?;
    }

    // Compress data, overwriting the header (lz4io.c:1143-1149).
    let c_size = lz4f_compress_update(&mut cctx, dst, src, None).map_err(|e| {
        io::Error::other(format!("error compressing with LZ4F_compressUpdate: {}", e))
    })?;

    // cctx is dropped here (equivalent to LZ4F_freeCompressionContext).
    Ok(c_size)
}

// ---------------------------------------------------------------------------
// compress_filename_st — LZ4IO_compressFilename_extRess_ST (lz4io.c 1366-1488)
// ---------------------------------------------------------------------------

/// Single-threaded frame-format compression of one file.
///
/// Returns the number of uncompressed source bytes processed via `in_stream_size`.
/// Equivalent to `LZ4IO_compressFilename_extRess_ST`.
fn compress_filename_st(
    in_stream_size: &mut u64,
    ress: &mut CompressResources,
    src_filename: &str,
    dst_filename: &str,
    compression_level: i32,
    io_prefs: &Prefs,
) -> io::Result<()> {
    let block_size = effective_block_size(io_prefs);

    // Open source (lz4io.c:1384-1385).
    let mut src_reader = open_src_file(src_filename)?;

    // Build per-call preferences (lz4io.c:1391-1398).
    let mut prefs = ress.prepared_prefs;
    prefs.compression_level = compression_level;
    if io_prefs.content_size_flag {
        // UTIL_getOpenFileSize equivalent: stat before reading.
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

    // Open destination (lz4io.c:1386-1387).
    let dst_file = open_dst_file(dst_filename, io_prefs)?;
    let dst_is_stdout = dst_file.is_stdout;
    let mut dst_writer: Box<dyn Write> = Box::new(dst_file);

    let cdict_ptr = ress.cdict_ptr();

    let mut filesize: u64 = 0;
    let mut compressedfilesize: u64 = 0;

    // Read first block (lz4io.c:1401-1403).
    let mut read_size = read_to_capacity(&mut *src_reader, &mut ress.src_buffer[..block_size])?;
    filesize += read_size as u64;

    if read_size < block_size {
        // Single-block file: one-shot frame compression (lz4io.c:1406-1418).
        let c_size = lz4f_compress_frame_using_cdict(
            &mut ress.ctx,
            &mut ress.dst_buffer,
            &ress.src_buffer[..read_size],
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
                compressedfilesize as f64 / (filesize.max(1)) as f64 * 100.0,
            ),
        );

        dst_writer
            .write_all(&ress.dst_buffer[..c_size])
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Write error: failed writing single-block compressed frame",
                )
            })?;
    } else {
        // Multi-block file: streaming frame compression (lz4io.c:1423-1460).

        // Write frame header (lz4io.c:1425-1430).
        // SAFETY: cdict_ptr is valid for the lifetime of ress.
        let header_size = unsafe {
            lz4f_compress_begin_using_cdict(
                &mut ress.ctx,
                &mut ress.dst_buffer,
                cdict_ptr,
                Some(&prefs),
            )
        }
        .map_err(|e| io::Error::other(format!("File header generation failed: {}", e)))?;

        dst_writer
            .write_all(&ress.dst_buffer[..header_size])
            .map_err(|_| {
                io::Error::new(io::ErrorKind::WriteZero, "Write error: cannot write header")
            })?;
        compressedfilesize += header_size as u64;

        // Main loop — one block at a time (lz4io.c:1433-1449).
        while read_size > 0 {
            let out_size = lz4f_compress_update(
                &mut ress.ctx,
                &mut ress.dst_buffer,
                &ress.src_buffer[..read_size],
                None,
            )
            .map_err(|e| io::Error::other(format!("Compression failed: {}", e)))?;
            compressedfilesize += out_size as u64;

            display_level(
                2,
                &format!(
                    "\rRead : {} MiB   ==> {:.2}%   ",
                    filesize >> 20,
                    compressedfilesize as f64 / filesize as f64 * 100.0,
                ),
            );

            dst_writer
                .write_all(&ress.dst_buffer[..out_size])
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Write error: cannot write compressed block",
                    )
                })?;

            // Read next block (lz4io.c:1447-1448).
            read_size = read_to_capacity(&mut *src_reader, &mut ress.src_buffer[..block_size])?;
            filesize += read_size as u64;
        }

        // End-of-frame mark (lz4io.c:1452-1459).
        let end_size = lz4f_compress_end(&mut ress.ctx, &mut ress.dst_buffer, None)
            .map_err(|e| io::Error::other(format!("End of frame error: {}", e)))?;
        dst_writer
            .write_all(&ress.dst_buffer[..end_size])
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Write error: cannot write end of frame",
                )
            })?;
        compressedfilesize += end_size as u64;
    }

    // Release file handles (lz4io.c:1463-1464):
    // dst_writer is dropped here; for stdout the DstFile wrapper does not close it.
    drop(dst_writer);

    // Copy owner/permissions/mtime from src to dst (lz4io.c:1467-1473).
    if src_filename != STDIN_MARK && !dst_is_stdout && dst_filename != NUL_MARK {
        let _ = copy_file_stat(src_filename, dst_filename);
    }

    // Remove source file if requested (lz4io.c:1475-1478).
    if io_prefs.remove_src_file && src_filename != STDIN_MARK {
        fs::remove_file(src_filename).map_err(|e| {
            io::Error::new(e.kind(), format!("Remove error: {}: {}", src_filename, e))
        })?;
    }

    // Final status display (lz4io.c:1481-1484).
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
// compress_filename_ext — LZ4IO_compressFilename_extRess (lz4io.c 1490-1501)
// ---------------------------------------------------------------------------

/// Compresses a single file using external [`CompressResources`], dispatching
/// to the single-threaded path.  When `io_prefs.nb_workers > 1` and the
/// `multithread` feature is enabled, callers should use `io::compress_mt`
/// instead.
///
/// Equivalent to `LZ4IO_compressFilename_extRess`.
pub fn compress_filename_ext(
    in_stream_size: &mut u64,
    ress: &mut CompressResources,
    src_filename: &str,
    dst_filename: &str,
    compression_level: i32,
    io_prefs: &Prefs,
) -> io::Result<()> {
    // The multi-threaded path lives in io::compress_mt; this function always
    // delegates to the single-threaded path.
    compress_filename_st(
        in_stream_size,
        ress,
        src_filename,
        dst_filename,
        compression_level,
        io_prefs,
    )
}

// ---------------------------------------------------------------------------
// Public: compress_filename — LZ4IO_compressFilename (lz4io.c 1503-1519)
// ---------------------------------------------------------------------------

/// Compress a single file to the LZ4 frame format.
///
/// `src` may be `"stdin"` to read from standard input;
/// `dst` may be `"stdout"` to write to standard output.
///
/// Equivalent to `int LZ4IO_compressFilename(srcFileName, dstFileName, compressionLevel, prefs)`.
pub fn compress_filename(
    src: &str,
    dst: &str,
    compression_level: i32,
    prefs: &Prefs,
) -> io::Result<CompressStats> {
    let time_start = get_time();
    let cpu_start = unsafe { clock() };
    let mut ress = CompressResources::new(prefs)?;
    let mut processed: u64 = 0;

    let result = compress_filename_ext(
        &mut processed,
        &mut ress,
        src,
        dst,
        compression_level,
        prefs,
    );

    // Free resources (ress drops automatically at end of scope).
    final_time_display(time_start, cpu_start, processed);

    result?;
    Ok(CompressStats {
        bytes_in: processed,
        bytes_out: 0,
    })
}

// ---------------------------------------------------------------------------
// Public: compress_multiple_filenames — LZ4IO_compressMultipleFilenames (1521-1575)
// ---------------------------------------------------------------------------

/// Compress multiple files to the LZ4 frame format, appending `suffix` to each
/// output filename.  If `suffix` is `"stdout"`, all files are written to stdout.
///
/// Returns the number of files that could not be compressed (equivalent to the
/// C return value `missed_files`).
///
/// Equivalent to `int LZ4IO_compressMultipleFilenames(inFileNamesTable, ifntSize, suffix, compressionLevel, prefs)`.
pub fn compress_multiple_filenames(
    srcs: &[&str],
    suffix: &str,
    compression_level: i32,
    prefs: &Prefs,
) -> io::Result<usize> {
    let time_start = get_time();
    let cpu_start = unsafe { clock() };
    let mut ress = CompressResources::new(prefs)?;
    let mut total_processed: u64 = 0;
    let mut missed_files: usize = 0;

    for &src_name in srcs {
        let mut processed: u64 = 0;

        // Determine destination filename (lz4io.c:1544-1565).
        let dst_name: String = if suffix == STDOUT_MARK {
            STDOUT_MARK.to_owned()
        } else {
            format!("{}{}", src_name, suffix)
        };

        if compress_filename_ext(
            &mut processed,
            &mut ress,
            src_name,
            &dst_name,
            compression_level,
            prefs,
        )
        .is_err()
        {
            missed_files += 1;
        }

        total_processed += processed;
    }

    // Free resources and display timing (lz4io.c:1570-1573).
    final_time_display(time_start, cpu_start, total_processed);

    Ok(missed_files)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::prefs::Prefs;
    use tempfile::TempDir;

    // ── CompressResources ────────────────────────────────────────────────────

    #[test]
    fn compress_resources_new_default_prefs() {
        let prefs = Prefs::default();
        let ress = CompressResources::new(&prefs).expect("new() should succeed");
        assert_eq!(ress.src_buffer.len(), CHUNK_SIZE);
        assert!(ress.dst_buffer.len() >= CHUNK_SIZE);
        assert!(ress.cdict.is_none());
    }

    #[test]
    fn compress_resources_new_with_dict() {
        let dir = TempDir::new().unwrap();
        let dict_path = dir.path().join("dict.bin");
        std::fs::write(&dict_path, b"hello world dictionary content for testing").unwrap();

        let mut prefs = Prefs::default();
        prefs.use_dictionary = true;
        prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_owned());

        let ress = CompressResources::new(&prefs).expect("new() with dict should succeed");
        assert!(ress.cdict.is_some());
    }

    // ── load_dict_file ────────────────────────────────────────────────────────

    #[test]
    fn load_dict_file_small() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.dict");
        let data = b"small dictionary data";
        std::fs::write(&path, data).unwrap();

        let dict = load_dict_file(path.to_str().unwrap()).unwrap();
        assert_eq!(dict.as_slice(), data.as_slice());
    }

    #[test]
    fn load_dict_file_large_truncated_to_64kb() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("large.dict");
        // Write 96 KB of data; only last 64 KB should be retained.
        let data: Vec<u8> = (0u8..=255).cycle().take(96 * 1024).collect();
        std::fs::write(&path, &data).unwrap();

        let dict = load_dict_file(path.to_str().unwrap()).unwrap();
        assert_eq!(dict.len(), LZ4_MAX_DICT_SIZE);
        // Must be the last 64 KB.
        assert_eq!(dict.as_slice(), &data[data.len() - LZ4_MAX_DICT_SIZE..]);
    }

    // ── effective_block_size ──────────────────────────────────────────────────

    #[test]
    fn effective_block_size_uses_block_size_when_set() {
        let mut p = Prefs::default();
        p.block_size = 128 * KB;
        assert_eq!(effective_block_size(&p), 128 * KB);
    }

    #[test]
    fn effective_block_size_derives_from_id_when_zero() {
        let mut p = Prefs::default();
        p.block_size = 0;
        p.block_size_id = 4;
        assert_eq!(effective_block_size(&p), 64 * KB);
        p.block_size_id = 7;
        assert_eq!(effective_block_size(&p), 4 * MB);
    }

    // ── compress_filename round-trip ──────────────────────────────────────────

    #[test]
    fn compress_filename_round_trip_small_file() {
        let dir = TempDir::new().unwrap();
        let src_path = dir.path().join("input.txt");
        let dst_path = dir.path().join("output.lz4");
        let original = b"Hello, LZ4 frame format! This is a test of the compression.";
        std::fs::write(&src_path, original).unwrap();

        let prefs = Prefs::default();
        compress_filename(
            src_path.to_str().unwrap(),
            dst_path.to_str().unwrap(),
            1,
            &prefs,
        )
        .expect("compress_filename should succeed");

        assert!(dst_path.exists(), "output file must exist");
        let compressed = std::fs::read(&dst_path).unwrap();
        // LZ4 frame magic: 0x184D2204
        assert!(compressed.len() >= 7, "must be at least header size");
        assert_eq!(
            &compressed[..4],
            &[0x04, 0x22, 0x4D, 0x18],
            "must start with LZ4 magic"
        );

        // Decompress to verify round-trip.
        let decompressed =
            crate::frame::decompress_frame_to_vec(&compressed).expect("decompression must succeed");
        assert_eq!(decompressed.as_slice(), original.as_slice());
    }

    #[test]
    fn compress_filename_round_trip_large_file() {
        // Large enough to trigger the multi-block path (block_size_id=4 → 64 KB blocks).
        let dir = TempDir::new().unwrap();
        let src_path = dir.path().join("large.bin");
        let dst_path = dir.path().join("large.lz4");

        // 200 KB of pseudo-random data (compressible).
        let original: Vec<u8> = (0u8..=255).cycle().take(200 * 1024).collect();
        std::fs::write(&src_path, &original).unwrap();

        let mut prefs = Prefs::default();
        prefs.block_size_id = 4; // 64 KB blocks → multi-block path
        prefs.block_size = 64 * KB;

        compress_filename(
            src_path.to_str().unwrap(),
            dst_path.to_str().unwrap(),
            1,
            &prefs,
        )
        .expect("compress_filename large should succeed");

        let compressed = std::fs::read(&dst_path).unwrap();
        let decompressed =
            crate::frame::decompress_frame_to_vec(&compressed).expect("decompression must succeed");
        assert_eq!(decompressed, original);
    }

    #[test]
    fn compress_multiple_filenames_produces_outputs() {
        let dir = TempDir::new().unwrap();
        let src1 = dir.path().join("a.txt");
        let src2 = dir.path().join("b.txt");
        std::fs::write(&src1, b"file a content").unwrap();
        std::fs::write(&src2, b"file b content").unwrap();

        let prefs = Prefs::default();
        let missed = compress_multiple_filenames(
            &[src1.to_str().unwrap(), src2.to_str().unwrap()],
            ".lz4",
            1,
            &prefs,
        )
        .expect("compress_multiple_filenames should succeed");

        assert_eq!(missed, 0, "no files should be missed");
        assert!(dir.path().join("a.txt.lz4").exists());
        assert!(dir.path().join("b.txt.lz4").exists());
    }

    #[test]
    fn compress_multiple_filenames_missing_file_counted() {
        let prefs = Prefs::default();
        let missed = compress_multiple_filenames(
            &["/nonexistent/__lz4_missing_file__.txt"],
            ".lz4",
            1,
            &prefs,
        )
        .expect("should return Ok even when some files are missing");
        assert_eq!(missed, 1, "one file should be missed");
    }

    // ── compress_frame_chunk ──────────────────────────────────────────────────

    #[test]
    fn compress_frame_chunk_returns_nonzero_for_compressible_input() {
        let prefs_val = build_preferences(&Prefs::default());
        let params = CfcParameters {
            prefs: &prefs_val,
            cdict: ptr::null(),
        };

        let src: Vec<u8> = b"abcdefghij".iter().cycle().take(4096).copied().collect();
        // Destination must be large enough: at minimum src.len() + BH_SIZE.
        let mut dst = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs_val))];

        let c_size = compress_frame_chunk(&params, &mut dst, &src, None)
            .expect("compress_frame_chunk should succeed");
        assert!(c_size > 0, "compressed output must be non-empty");
        assert!(c_size <= dst.len(), "must not exceed dst capacity");
    }

    #[test]
    fn compress_frame_chunk_with_dict_returns_output() {
        let dict_data: Vec<u8> = b"dictionary content"
            .iter()
            .cycle()
            .take(1024)
            .copied()
            .collect();
        let cdict = Lz4FCDict::create(&dict_data).expect("CDict creation failed");
        let cdict_ptr: *const Lz4FCDict = &*cdict;

        let prefs_val = build_preferences(&Prefs::default());
        let params = CfcParameters {
            prefs: &prefs_val,
            cdict: cdict_ptr,
        };

        let src: Vec<u8> = b"hello world".iter().cycle().take(512).copied().collect();
        let mut dst = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs_val))];

        let c_size = compress_frame_chunk(&params, &mut dst, &src, None)
            .expect("compress_frame_chunk with dict should succeed");
        assert!(c_size > 0);
    }
}
