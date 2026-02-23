//! Decompression dispatch and public API.
//!
//! This module is the top-level entry point for decompression.  It implements
//! the frame-format dispatch loop: it reads the 4-byte magic number at the
//! start of each chained frame and routes to the appropriate decoder:
//!
//! - [`crate::io::decompress_frame`] for LZ4 frame-format streams
//! - [`crate::io::decompress_legacy`] for the legacy block-stream format
//! - [`pass_through`] for unrecognised headers when pass-through mode is active
//! - Skippable frames (`0x184D2A50`–`0x184D2A5F`) are silently discarded
//!
//! The public API exposes two functions:
//! - [`decompress_filename`] — decompresses a single source/destination pair
//! - [`decompress_multiple_filenames`] — decompresses a list of source files,
//!   deriving destination names by stripping a suffix (e.g., `.lz4`)
//!
//! Corresponds to `LZ4IO_passThrough`, `skipStream`, `selectDecoder`,
//! `LZ4IO_decompressSrcFile`, `LZ4IO_decompressDstFile`,
//! `LZ4IO_decompressFilename`, and `LZ4IO_decompressMultipleFilenames` in
//! the reference implementation (`lz4io.c` lines 2277–2555).
//!
//! # Design notes
//!
//! - **Magic number passing**: `decode_legacy_stream` returns
//!   `(bytes, Option<u32>)`.  When the legacy stream is immediately followed
//!   by another chained frame, the embedded next-stream magic is returned as
//!   `Some(u32)` and consumed by the next dispatch iteration without an extra
//!   read, avoiding the `g_magicRead` module-level global in the C reference
//!   implementation (lz4io.c line 1677).
//!
//! - **Skippable-frame seeking**: Skippable-frame payloads are consumed via
//!   `skip_stream` (read-and-discard) rather than `fseek`.  This is correct
//!   for both seekable files and non-seekable pipes; regular-file inputs pay a
//!   minor read overhead compared to `fseek` but are functionally identical.
//!
//! - **Frame counter scoping**: The C `selectDecoder` function maintains a
//!   `static unsigned nbFrames` local that persists across calls.  The
//!   equivalent `nb_frames` is a local variable scoped to a single call of
//!   `decompress_loop`, eliminating implicit state shared between invocations.
//!
//! - **Sparse writes**: The frame and legacy decoders write to `impl Write`
//!   and cannot call `fwrite_sparse` themselves.  Concrete `File` outputs are
//!   wrapped in [`SparseWriter`], which forwards every write through
//!   `fwrite_sparse` and defers trailing-zeros finalisation until the entire
//!   dispatch loop completes.  The C implementation calls `fwriteSparseEnd`
//!   at the end of each frame decoder, but because no intervening real data is
//!   written between frames, the deferred approach is functionally equivalent.
//!
//! - **File stat propagation**: Uses the `filetime` crate for mtime and
//!   `std::fs::set_permissions` for mode bits, matching the behaviour of
//!   `UTIL_setFileStat` in the reference implementation.
//!
//! - **Error handling**: The C `END_PROCESS(n, msg)` macro calls `exit()`.
//!   All errors are returned as `io::Error` with descriptive messages so that
//!   callers can handle or propagate them without terminating the process.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::Ordering;

// `libc::clock` is not exposed directly on macOS/Linux via the `libc` crate
// (it is a macro in C); declare it via a private extern block, matching the
// pattern used by `compress_frame.rs` and `compress_legacy.rs`.
extern "C" {
    fn clock() -> libc::clock_t;
}

use crate::io::decompress_frame::decompress_lz4f;
use crate::io::decompress_legacy::decode_legacy_stream;
use crate::io::decompress_resources::DecompressResources;
use crate::io::file_io::{
    is_skippable_magic_number, open_src_file, NUL_MARK, STDIN_MARK, STDOUT_MARK,
};
use crate::io::prefs::{
    display_level, final_time_display, Prefs, DISPLAY_LEVEL, LEGACY_MAGICNUMBER, LZ4IO_MAGICNUMBER,
    LZ4IO_SKIPPABLE0, MAGICNUMBER_SIZE,
};
use crate::io::sparse::{fwrite_sparse, fwrite_sparse_end, SPARSE_SEGMENT_SIZE};
use crate::timefn::get_time;

// ---------------------------------------------------------------------------
// Public stats
// ---------------------------------------------------------------------------

/// Statistics returned by `decompress_filename`.
/// Equivalent to the `*outGenSize` out-parameter in the C API.
#[derive(Debug, Clone, Default)]
pub struct DecompressStats {
    /// Total number of decompressed bytes written to the output.
    pub decompressed_bytes: u64,
}

// ---------------------------------------------------------------------------
// Buffer sizes (lz4io.c lines 2282–2307)
// ---------------------------------------------------------------------------

/// Copy buffer used by pass-through mode.
/// C uses `size_t buffer[PTSIZET]`; 16 KiB matches the typical value.
const PT_BUF_SIZE: usize = 16 * 1024;

/// Buffer for `skip_stream` — mirrors `SKIP_BUFF_SIZE` (lz4io.c line 2304).
const SKIP_BUF_SIZE: usize = 16 * 1024;

// ---------------------------------------------------------------------------
// SparseWriter
// ---------------------------------------------------------------------------

/// A `Write`-implementing wrapper around a concrete `File` that routes every
/// buffer through `fwrite_sparse`, accumulating pending zero-byte skips.
///
/// Callers must call `finish()` after the last write to materialise trailing
/// zero bytes (equivalent to `LZ4IO_fwriteSparseEnd`).
struct SparseWriter {
    file: File,
    stored_skips: u64,
    sparse_mode: bool,
}

impl SparseWriter {
    fn new(file: File, sparse_mode: bool) -> Self {
        SparseWriter {
            file,
            stored_skips: 0,
            sparse_mode,
        }
    }

    /// Finalises sparse writes: writes any pending trailing zeros (lz4io.c:1665).
    fn finish(&mut self) -> io::Result<()> {
        let skips = self.stored_skips;
        self.stored_skips = 0;
        fwrite_sparse_end(&mut self.file, skips)
    }
}

impl Write for SparseWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stored_skips = fwrite_sparse(
            &mut self.file,
            buf,
            SPARSE_SEGMENT_SIZE,
            self.stored_skips,
            self.sparse_mode,
        )?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

// ---------------------------------------------------------------------------
// pass_through (lz4io.c lines 2277–2299)
// ---------------------------------------------------------------------------

/// Copies the full source stream to `dst`, prepending the already-read
/// `magic_bytes`.
///
/// Equivalent to `LZ4IO_passThrough`.  The sparse-file optimisation present
/// in the C version is delegated to the `SparseWriter` wrapping `dst` in the
/// calling layer; this function writes through the provided writer as-is.
fn pass_through<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    magic_bytes: [u8; MAGICNUMBER_SIZE],
) -> io::Result<u64> {
    // Write the 4 magic bytes that the dispatcher consumed (lz4io.c:2287).
    dst.write_all(&magic_bytes)?;
    let mut total = MAGICNUMBER_SIZE as u64;

    let mut buf = [0u8; PT_BUF_SIZE];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        dst.write_all(&buf[..n])?;
    }

    Ok(total)
}

// ---------------------------------------------------------------------------
// skip_stream (lz4io.c lines 2305–2316)
// ---------------------------------------------------------------------------

/// Reads and discards `offset` bytes from `src`.
///
/// Equivalent to `skipStream`.  Used to skip over skippable-frame payloads.
/// The C version tries `fseek` first (via `fseek_u32`) and falls back to
/// reading when `fseek` fails.  Here we always read-and-discard, which is
/// correct for both seekable and non-seekable sources.
fn skip_stream<R: Read>(src: &mut R, mut offset: u32) -> io::Result<()> {
    let mut buf = [0u8; SKIP_BUF_SIZE];
    while offset > 0 {
        let to_read = (offset as usize).min(SKIP_BUF_SIZE);
        src.read_exact(&mut buf[..to_read]).map_err(|_| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Stream error : cannot skip skippable area",
            )
        })?;
        offset -= to_read as u32;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// decompress_loop (selectDecoder + LZ4IO_decompressSrcFile inner loop)
// ---------------------------------------------------------------------------

/// Decompresses all chained frames from `src` into `dst`.
///
/// Returns the total number of decompressed bytes produced.
///
/// Equivalent to the `for(;;)` dispatch loop in `LZ4IO_decompressSrcFile`
/// combined with `selectDecoder`.  The C `static unsigned nbFrames` local of
/// `selectDecoder` is modelled as a local `nb_frames` counter that resets
/// implicitly when this function returns (no global state).
///
/// The type parameters `R: Read` and `W: Write` allow the same function body
/// to call `decompress_lz4f` and `decode_legacy_stream` which require `impl
/// Read` / `impl Write` (sized concrete types, not trait objects).
fn decompress_loop<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    prefs: &Prefs,
    resources: &mut DecompressResources,
) -> io::Result<u64> {
    let mut filesize: u64 = 0;
    // Equivalent to C's `static unsigned nbFrames = 0` in `selectDecoder`.
    let mut nb_frames: u64 = 0;
    // When the legacy decoder encounters a chained-stream magic number embedded
    // at the end of its own stream, it returns it as `Some(u32)`.  Storing it
    // here lets the next iteration reuse that value without an extra 4-byte
    // read (lz4io.c lines 1677, 2352–2354).
    let mut pending_magic: Option<u32> = None;

    loop {
        // ── Read 4-byte magic number (or reuse from previous legacy frame) ──
        let (magic, magic_bytes) = if let Some(m) = pending_magic.take() {
            // Legacy decoder yielded a next-stream magic; reuse it
            // (equivalent to `if (g_magicRead)` branch, lz4io.c:2352–2354).
            (m, m.to_le_bytes())
        } else {
            let mut mb = [0u8; MAGICNUMBER_SIZE];
            // Use a single-byte read to distinguish clean EOF from mid-read EOF
            // (mirrors `if (nbReadBytes==0) { nbFrames = 0; return ENDOFSTREAM; }`
            // at lz4io.c:2357).
            match src.read(&mut mb[..1])? {
                0 => break, // Clean EOF — end of stream.
                _ => {}
            }
            if let Err(e) = src.read_exact(&mut mb[1..]) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unrecognized header : Magic Number unreadable: {}", e),
                ));
            }
            (u32::from_le_bytes(mb), mb)
        };

        // Fold all skippable magic numbers to the canonical value (lz4io.c:2362–2363).
        let folded = if is_skippable_magic_number(magic) {
            LZ4IO_SKIPPABLE0
        } else {
            magic
        };

        // ── Dispatch (lz4io.c:2365–2400) ────────────────────────────────────
        match folded {
            LZ4IO_MAGICNUMBER => {
                // LZ4 frame format (lz4io.c:2367–2368).
                let bytes = decompress_lz4f(src, dst, prefs, resources)?;
                filesize += bytes;
            }

            LEGACY_MAGICNUMBER => {
                // Legacy block format (lz4io.c:2369–2371).
                display_level(4, "Detected : Legacy format \n");
                let (bytes, next) = decode_legacy_stream(src, dst, prefs, resources)?;
                filesize += bytes;
                // `next` replaces g_magicRead: carry the embedded magic number
                // to the next iteration instead of storing in a global.
                pending_magic = next;
            }

            LZ4IO_SKIPPABLE0 => {
                // Skippable frame: read 4-byte payload size then skip (lz4io.c:2372–2383).
                display_level(4, "Skipping detected skippable area \n");
                let mut sb = [0u8; 4];
                src.read_exact(&mut sb).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Stream error : skippable size unreadable",
                    )
                })?;
                let skip_size = u32::from_le_bytes(sb);
                skip_stream(src, skip_size)?;
                // Returns 0 decoded bytes (lz4io.c:2383).
            }

            _ => {
                // Unrecognized magic (lz4io.c:2384–2399).
                // In C, `nbFrames` is incremented *before* this check, so
                // `nbFrames == 1` means this is the first frame.  Here
                // `nb_frames` has not been incremented yet, so we check `== 0`.
                if nb_frames == 0 {
                    // First frame: pass-through if configured (lz4io.c:2385–2391).
                    if !prefs.test_mode && prefs.overwrite && prefs.pass_through {
                        let bytes = pass_through(src, dst, magic_bytes)?;
                        return Ok(bytes);
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Unrecognized header : file cannot be decoded",
                    ));
                }
                // Subsequent frames: log and stop (lz4io.c:2393–2399).
                display_level(2, "Stream followed by undecodable data \n");
                // Equivalent to returning DECODING_ERROR from selectDecoder,
                // which causes decompressSrcFile to set result=1 and break.
                break;
            }
        }

        nb_frames += 1;
    }

    Ok(filesize)
}

// ---------------------------------------------------------------------------
// decompress_src_file (lz4io.c lines 2404–2442)
// ---------------------------------------------------------------------------

/// Opens `src_path` for reading and decompresses all frames into `dst`.
///
/// Returns the total decompressed byte count.
///
/// The type parameter `W: Write` allows callers to pass concrete write targets
/// (stdout lock, sink, `SparseWriter`) without an extra heap allocation;
/// `decompress_loop` requires `W: Write + Sized` because it calls into
/// `decompress_lz4f` which uses `impl Write`.
///
/// Equivalent to `LZ4IO_decompressSrcFile` (the `output_filename` parameter
/// present in C is unused inside that function and is omitted here).
fn decompress_src_file<W: Write>(
    src_path: &str,
    dst: &mut W,
    prefs: &Prefs,
    resources: &mut DecompressResources,
) -> io::Result<u64> {
    let mut src = open_src_file(src_path)?; // Box<dyn Read>: Read via impl<R: Read + ?Sized> Read for Box<R>
    let filesize = decompress_loop(&mut src, dst, prefs, resources)?;

    // `--rm`: remove source file after successful decompression (lz4io.c:2430–2432).
    if prefs.remove_src_file {
        fs::remove_file(src_path)
            .map_err(|e| io::Error::new(e.kind(), format!("Remove error : {}: {}", src_path, e)))?;
    }

    // Progress display (lz4io.c:2436–2437).
    if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 2 {
        display_level(2, &format!("\r{:79}\r", ""));
        display_level(
            2,
            &format!("{:<30.30} : decoded {} bytes \n", src_path, filesize),
        );
    }

    Ok(filesize)
}

// ---------------------------------------------------------------------------
// open_regular_dst — overwrite-checked file open for decompress_dst_file
// ---------------------------------------------------------------------------

/// Opens a regular destination file, honouring the `overwrite` preference.
///
/// Inlines the overwrite-prompt logic from `LZ4IO_openDstFile` (lz4io.c
/// lines 2455, 419–435) but returns a raw `File` rather than a `FILE*`
/// so that the caller can wrap it in `SparseWriter`.
fn open_regular_dst(dst_path: &str, prefs: &Prefs) -> io::Result<File> {
    if !prefs.overwrite && Path::new(dst_path).exists() {
        let level = DISPLAY_LEVEL.load(Ordering::Relaxed);
        if level <= 1 {
            eprintln!("{} already exists; not overwritten  ", dst_path);
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{}: already exists; not overwritten", dst_path),
            ));
        }
        // Interactive prompt (lz4io.c:422–436).
        eprint!(
            "{} already exists; do you want to overwrite (y/N) ? ",
            dst_path
        );
        let _ = io::stderr().flush();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let first = line.trim_start().chars().next().unwrap_or('\0');
        if first != 'y' && first != 'Y' {
            eprintln!("    not overwritten  ");
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{}: not overwritten", dst_path),
            ));
        }
    }

    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst_path)
        .map_err(|e| {
            if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
                eprintln!("{}: {}", dst_path, e);
            }
            e
        })
}

// ---------------------------------------------------------------------------
// decompress_dst_file (lz4io.c lines 2445–2476)
// ---------------------------------------------------------------------------

/// Opens `dst_path` for writing, calls `decompress_src_file`, and copies
/// file metadata (mtime, permissions) from the source to the destination.
///
/// Returns the total decompressed byte count.
///
/// Equivalent to `LZ4IO_decompressDstFile`.
fn decompress_dst_file(
    src_path: &str,
    dst_path: &str,
    prefs: &Prefs,
    resources: &mut DecompressResources,
) -> io::Result<u64> {
    // Read source metadata for stat propagation (lz4io.c:2458–2460).
    // Only meaningful when `src_path` is a regular file (not stdin sentinel).
    let src_stat = if src_path != STDIN_MARK {
        fs::metadata(src_path).ok()
    } else {
        None
    };

    // ── Open destination and decompress ──────────────────────────────────────
    let filesize = if dst_path == STDOUT_MARK {
        // Write to stdout (no sparse).
        let mut dst = io::stdout();
        decompress_src_file(src_path, &mut dst, prefs, resources)?
    } else if dst_path == NUL_MARK {
        // Discard output (no sparse).
        let mut dst = io::sink();
        decompress_src_file(src_path, &mut dst, prefs, resources)?
    } else {
        // Regular file: sparse-write-capable output.
        let file = open_regular_dst(dst_path, prefs)?;
        // C: `sparseMode = (sparseFileSupport - (f==stdout)) > 0`
        // Since `f != stdout` here: `sparseMode = prefs->sparseFileSupport > 0`.
        let sparse_mode = prefs.sparse_file_support > 0;
        let mut sparse_writer = SparseWriter::new(file, sparse_mode);
        let result = decompress_src_file(src_path, &mut sparse_writer, prefs, resources);
        // Finalise sparse regardless of success/failure to keep the file
        // in a consistent state (lz4io.c: fwriteSparseEnd at end of each frame).
        let finish_result = sparse_writer.finish();
        let sz = result?;
        finish_result?;
        sz
    };

    // ── Copy file metadata (lz4io.c:2467–2473) ───────────────────────────────
    let is_special_dst = dst_path == STDOUT_MARK || dst_path == NUL_MARK;
    if !is_special_dst {
        if let Some(meta) = &src_stat {
            // Modification time via the `filetime` crate.
            if let Ok(mtime) = meta.modified() {
                let ft = filetime::FileTime::from_system_time(mtime);
                let _ = filetime::set_file_mtime(dst_path, ft);
            }
            // Permissions via std::fs.
            let _ = fs::set_permissions(dst_path, meta.permissions());
        }
    }

    Ok(filesize)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Decompresses the file at `src` into `dst`.
///
/// Supports frame format, legacy format, skippable frames, and chained frames.
/// When `prefs.pass_through` is set and the input has an unrecognised header,
/// the file is copied verbatim.
///
/// The function mirrors the timing display behaviour of `LZ4IO_decompressFilename`
/// (lz4io.c lines 2483–2495): timing is displayed only when an error occurs.
///
/// # Errors
///
/// Returns an error on I/O failure, corrupted data, or unrecognised format
/// (when pass-through is not active).
pub fn decompress_filename(src: &str, dst: &str, prefs: &Prefs) -> io::Result<DecompressStats> {
    let mut resources = DecompressResources::from_prefs(prefs)?;
    let time_start = get_time();
    // SAFETY: clock() is declared in the module-level extern "C" block.
    let cpu_start = unsafe { clock() };

    let result = decompress_dst_file(src, dst, prefs, &mut resources);

    // C lz4io.c:2491–2492: LZ4IO_finalTimeDisplay is called only on error.
    match result {
        Ok(bytes) => Ok(DecompressStats {
            decompressed_bytes: bytes,
        }),
        Err(e) => {
            final_time_display(time_start, cpu_start, 0);
            Err(e)
        }
    }
}

/// Decompresses multiple source files, deriving each output filename by
/// stripping `suffix` from the source name.
///
/// When `suffix` is the stdout sentinel (`"stdout"`) or the devnull sentinel
/// (`"/dev/null"` / `"nul"`), all files are decompressed to that special
/// destination.  Otherwise, files whose names do not end with `suffix` are
/// logged and skipped.
///
/// Always displays a timing summary (equivalent to the unconditional
/// `LZ4IO_finalTimeDisplay` call in `LZ4IO_decompressMultipleFilenames`,
/// lz4io.c:2548).
///
/// Returns `Ok(())` when all files succeed; `Err` summarising the counts of
/// missing and skipped files otherwise.
///
/// Equivalent to `LZ4IO_decompressMultipleFilenames` (lz4io.c lines 2498–2550).
pub fn decompress_multiple_filenames(srcs: &[&str], suffix: &str, prefs: &Prefs) -> io::Result<()> {
    let mut resources = DecompressResources::from_prefs(prefs)?;
    let time_start = get_time();
    // SAFETY: clock() is declared in the module-level extern "C" block.
    let cpu_start = unsafe { clock() };

    // Inform the user when checksums are disabled (lz4io.c:2515–2517).
    if !prefs.block_checksum && !prefs.stream_checksum {
        display_level(4, "disabling checksum validation during decoding \n");
    }

    let mut total_processed: u64 = 0;
    let mut missing_files: i32 = 0;
    let mut skipped_files: i32 = 0;

    let dst_is_special = suffix == STDOUT_MARK || suffix == NUL_MARK;

    for &src_path in srcs {
        if dst_is_special {
            // Decompress directly to stdout / devnull (lz4io.c:2524–2527).
            // The `ress.dstFile` in C is already set to the special handle;
            // here we just write to the same special destination each iteration.
            let result = if suffix == NUL_MARK {
                let mut sink = io::sink();
                decompress_src_file(src_path, &mut sink, prefs, &mut resources)
            } else {
                let mut stdout = io::stdout();
                decompress_src_file(src_path, &mut stdout, prefs, &mut resources)
            };
            match result {
                Ok(n) => total_processed += n,
                Err(_) => missing_files += 1,
            }
        } else {
            // Check that the source filename ends with `suffix` (lz4io.c:2535–2543).
            if src_path.len() <= suffix.len() || !src_path.ends_with(suffix) {
                display_level(
                    1,
                    &format!(
                        "File extension doesn't match expected LZ4_EXTENSION ({:4}); \
                         will not process file: {}\n",
                        suffix, src_path
                    ),
                );
                skipped_files += 1;
                continue;
            }

            // Strip suffix to produce the output filename (lz4io.c:2540–2541).
            let out_path = &src_path[..src_path.len() - suffix.len()];

            match decompress_dst_file(src_path, out_path, prefs, &mut resources) {
                Ok(n) => total_processed += n,
                Err(_) => missing_files += 1,
            }
        }
    }

    // Always display timing (lz4io.c:2548).
    final_time_display(time_start, cpu_start, total_processed);

    let total_failures = missing_files + skipped_files;
    if total_failures > 0 {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "{} file(s) could not be decompressed; {} file(s) skipped",
                missing_files, skipped_files
            ),
        ))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::prefs::Prefs;
    use std::io::{Cursor, Write};

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Builds a minimal LZ4 frame-format stream for `data`.
    fn make_frame_stream(data: &[u8]) -> Vec<u8> {
        crate::frame::compress_frame_to_vec(data)
    }

    /// Builds a legacy-format LZ4 stream for `data` (magic + size-prefixed blocks).
    fn make_legacy_stream(data: &[u8]) -> Vec<u8> {
        use crate::io::prefs::LEGACY_BLOCKSIZE;
        let mut stream = Vec::new();
        stream.extend_from_slice(&LEGACY_MAGICNUMBER.to_le_bytes());
        for chunk in data.chunks(LEGACY_BLOCKSIZE) {
            let compressed = crate::block::compress_block_to_vec(chunk);
            stream.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            stream.extend_from_slice(&compressed);
        }
        stream
    }

    // ── pass_through ─────────────────────────────────────────────────────────

    #[test]
    fn pass_through_copies_magic_and_data() {
        let magic = [0x01, 0x02, 0x03, 0x04u8];
        let payload = b"hello world payload";
        let mut src = Cursor::new(payload.as_ref());
        let mut dst = Vec::new();

        let total = pass_through(&mut src, &mut dst, magic).expect("pass_through should succeed");

        let mut expected = magic.to_vec();
        expected.extend_from_slice(payload);
        assert_eq!(dst, expected);
        assert_eq!(total, (MAGICNUMBER_SIZE + payload.len()) as u64);
    }

    #[test]
    fn pass_through_empty_payload() {
        let magic = [0xAA, 0xBB, 0xCC, 0xDDu8];
        let mut src = Cursor::new(b"" as &[u8]);
        let mut dst = Vec::new();

        let total = pass_through(&mut src, &mut dst, magic).expect("pass_through should succeed");

        assert_eq!(dst, magic.as_ref());
        assert_eq!(total, MAGICNUMBER_SIZE as u64);
    }

    // ── skip_stream ──────────────────────────────────────────────────────────

    #[test]
    fn skip_stream_discards_bytes() {
        let data = b"ABCDEFGHIJ";
        let mut src = Cursor::new(data.as_ref());
        skip_stream(&mut src, 5).expect("skip should succeed");

        let mut remaining = Vec::new();
        src.read_to_end(&mut remaining).unwrap();
        assert_eq!(remaining, b"FGHIJ");
    }

    #[test]
    fn skip_stream_zero_is_noop() {
        let data = b"XYZ";
        let mut src = Cursor::new(data.as_ref());
        skip_stream(&mut src, 0).expect("skip 0 should succeed");

        let mut remaining = Vec::new();
        src.read_to_end(&mut remaining).unwrap();
        assert_eq!(remaining, b"XYZ");
    }

    #[test]
    fn skip_stream_exact_length() {
        let data = b"HELLO";
        let mut src = Cursor::new(data.as_ref());
        skip_stream(&mut src, 5).expect("skip exact should succeed");

        let mut remaining = Vec::new();
        src.read_to_end(&mut remaining).unwrap();
        assert!(remaining.is_empty());
    }

    // ── decompress_loop: frame format ────────────────────────────────────────

    #[test]
    fn decompress_loop_frame_format() {
        let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let frame_stream = make_frame_stream(&original);

        let prefs = Prefs::default();
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(frame_stream);
        let mut dst = Vec::new();

        let bytes = decompress_loop(&mut src, &mut dst, &prefs, &mut resources)
            .expect("frame decompress should succeed");

        assert_eq!(bytes as usize, original.len());
        assert_eq!(dst, original);
    }

    // ── decompress_loop: legacy format ───────────────────────────────────────

    #[test]
    fn decompress_loop_legacy_format() {
        let original = b"Hello, legacy world!";
        let legacy_stream = make_legacy_stream(original);

        let prefs = Prefs::default();
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(legacy_stream);
        let mut dst = Vec::new();

        let bytes = decompress_loop(&mut src, &mut dst, &prefs, &mut resources)
            .expect("legacy decompress should succeed");

        assert_eq!(bytes as usize, original.len());
        assert_eq!(dst.as_slice(), original.as_ref());
    }

    // ── decompress_loop: skippable frame ─────────────────────────────────────

    #[test]
    fn decompress_loop_skippable_frame() {
        // Build: [skippable frame: 5 bytes payload] [frame format: original]
        let original = b"After skippable frame";
        let mut stream = Vec::new();

        // Skippable frame (lz4io.c:2372–2383).
        stream.extend_from_slice(&LZ4IO_SKIPPABLE0.to_le_bytes()); // magic
        let skip_payload = b"XXXXX"; // 5 arbitrary bytes
        stream.extend_from_slice(&(skip_payload.len() as u32).to_le_bytes()); // size
        stream.extend_from_slice(skip_payload); // payload

        // Real LZ4 frame.
        stream.extend_from_slice(&make_frame_stream(original));

        let prefs = Prefs::default();
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(stream);
        let mut dst = Vec::new();

        let bytes = decompress_loop(&mut src, &mut dst, &prefs, &mut resources)
            .expect("skippable + frame should succeed");

        assert_eq!(bytes as usize, original.len());
        assert_eq!(dst.as_slice(), original.as_ref());
    }

    // ── decompress_loop: chained frames (frame + legacy) ─────────────────────

    #[test]
    fn decompress_loop_chained_frame_then_legacy() {
        let part1 = b"Part one in frame format.";
        let part2 = b"Part two in legacy format.";
        let mut stream = Vec::new();
        stream.extend_from_slice(&make_frame_stream(part1));
        stream.extend_from_slice(&make_legacy_stream(part2));

        let prefs = Prefs::default();
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(stream);
        let mut dst = Vec::new();

        let bytes = decompress_loop(&mut src, &mut dst, &prefs, &mut resources)
            .expect("chained frames should succeed");

        let mut expected = part1.to_vec();
        expected.extend_from_slice(part2);
        assert_eq!(bytes as usize, expected.len());
        assert_eq!(dst, expected);
    }

    // ── decompress_loop: empty input ─────────────────────────────────────────

    #[test]
    fn decompress_loop_empty_input_returns_zero() {
        let prefs = Prefs::default();
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(b"" as &[u8]);
        let mut dst = Vec::new();

        let bytes = decompress_loop(&mut src, &mut dst, &prefs, &mut resources)
            .expect("empty input should succeed");

        assert_eq!(bytes, 0);
        assert!(dst.is_empty());
    }

    // ── decompress_loop: unrecognized magic on first frame → error ────────────

    #[test]
    fn decompress_loop_unrecognized_magic_first_frame_returns_error() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&0xDEADBEEFu32.to_le_bytes()); // unrecognized

        let prefs = Prefs::default(); // pass_through = false
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(stream);
        let mut dst = Vec::new();

        let result = decompress_loop(&mut src, &mut dst, &prefs, &mut resources);
        assert!(result.is_err(), "unrecognized magic must return error");
    }

    // ── decompress_loop: pass-through on first frame ─────────────────────────

    #[test]
    fn decompress_loop_pass_through_first_frame() {
        let magic: u32 = 0xDEADBEEF;
        let payload = b"raw data payload";
        let mut stream = Vec::new();
        stream.extend_from_slice(&magic.to_le_bytes());
        stream.extend_from_slice(payload);

        let mut prefs = Prefs::default();
        prefs.pass_through = true;
        prefs.overwrite = true;
        prefs.test_mode = false;

        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(stream.clone());
        let mut dst = Vec::new();

        let bytes = decompress_loop(&mut src, &mut dst, &prefs, &mut resources)
            .expect("pass-through should succeed");

        // Should reproduce the full stream (magic + payload).
        assert_eq!(bytes as usize, stream.len());
        assert_eq!(dst, stream);
    }

    // ── decompress_loop: corrupt compressed data → error ─────────────────────

    #[test]
    fn decompress_loop_corrupt_frame_returns_error() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&LZ4IO_MAGICNUMBER.to_le_bytes());
        stream.extend_from_slice(b"\xFF\xFF\xFF\xFF\xFF"); // garbage

        let prefs = Prefs::default();
        let mut resources = DecompressResources::new(&prefs).unwrap();
        let mut src = Cursor::new(stream);
        let mut dst = Vec::new();

        let result = decompress_loop(&mut src, &mut dst, &prefs, &mut resources);
        assert!(result.is_err(), "corrupt frame must return error");
    }

    // ── SparseWriter ─────────────────────────────────────────────────────────

    #[test]
    fn sparse_writer_write_and_finish() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = File::create(tmp.path()).unwrap();
        let mut sw = SparseWriter::new(file, false); // sparse_mode=false → plain write

        let data = b"hello sparse";
        write!(&mut sw, "{}", std::str::from_utf8(data).unwrap()).unwrap();
        sw.finish().unwrap();

        let written = fs::read(tmp.path()).unwrap();
        assert_eq!(written, data);
    }

    // ── Integration: decompress_filename round-trip ───────────────────────────

    #[test]
    fn decompress_filename_frame_format_round_trip() {
        let original: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
        let compressed = make_frame_stream(&original);

        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path().join("input.lz4");
        let dst_path = dst_dir.path().join("output.raw");
        fs::write(&src_path, &compressed).unwrap();

        let prefs = Prefs::default();
        let stats = decompress_filename(
            src_path.to_str().unwrap(),
            dst_path.to_str().unwrap(),
            &prefs,
        )
        .expect("decompress_filename should succeed");

        let decompressed = fs::read(&dst_path).unwrap();
        assert_eq!(decompressed, original);
        assert_eq!(stats.decompressed_bytes as usize, original.len());
    }

    #[test]
    fn decompress_filename_legacy_format_round_trip() {
        let original = b"Legacy format round-trip test data";
        let compressed = make_legacy_stream(original);

        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path().join("input.lz4");
        let dst_path = dst_dir.path().join("output.raw");
        fs::write(&src_path, &compressed).unwrap();

        let prefs = Prefs::default();
        let stats = decompress_filename(
            src_path.to_str().unwrap(),
            dst_path.to_str().unwrap(),
            &prefs,
        )
        .expect("decompress_filename should succeed");

        let decompressed = fs::read(&dst_path).unwrap();
        assert_eq!(decompressed.as_slice(), original.as_ref());
        assert_eq!(stats.decompressed_bytes as usize, original.len());
    }

    // ── Integration: decompress_multiple_filenames ────────────────────────────

    #[test]
    fn decompress_multiple_filenames_strips_suffix() {
        let suffix = ".lz4";
        let original = b"multiple filenames test";
        let compressed = make_frame_stream(original);

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("file.raw.lz4");
        let expected_dst = dir.path().join("file.raw");
        fs::write(&src, &compressed).unwrap();

        let prefs = Prefs::default();
        let src_str = src.to_str().unwrap();
        decompress_multiple_filenames(&[src_str], suffix, &prefs).expect("should succeed");

        let decompressed = fs::read(&expected_dst).unwrap();
        assert_eq!(decompressed.as_slice(), original.as_ref());
    }

    #[test]
    fn decompress_multiple_filenames_skips_wrong_extension() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("file.gz"); // wrong extension
        fs::write(&src, b"not an lz4 file").unwrap();

        let prefs = Prefs::default();
        let src_str = src.to_str().unwrap();
        // Should return Err because the file was skipped.
        let result = decompress_multiple_filenames(&[src_str], ".lz4", &prefs);
        assert!(
            result.is_err(),
            "wrong-extension file should cause a skip error"
        );
    }
}
