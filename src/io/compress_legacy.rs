// compress_legacy.rs — LZ4 legacy format compression.
// Migrated from lz4io.c lines 764–977 (declaration #9) and lz4io.h
// (lz4-1.10.0/programs).
//
// Migration decisions:
// - `LZ4IO_writeLE32` → Rust's `u32::to_le_bytes()`.
// - The C implementation uses a multi-threaded TPool + WriteRegister +
//   ReadTracker pipeline even for the legacy path.  The Rust port provides a
//   functionally equivalent single-threaded loop that produces byte-identical
//   output (blocks are independently compressed and written in order).  A
//   future refactor could add rayon parallelism; the public API is stable.
// - `CompressLegacyState.cLevel` → local variable `clevel: i32`.
// - `LZ4IO_compressBlockLegacy_fast` / `_HC` → private `compress_block_fast`
//   / `compress_block_hc` (same logic; LE32 header written inline).
// - `LZ4IO_compressLegacy_internal` → private `compress_legacy_internal`
//   returning `io::Result<LegacyResult>`.
// - `LZ4IO_compressFilename_Legacy` → `compress_filename_legacy`.
// - `LZ4IO_compressMultipleFilenames_Legacy` →
//   `compress_multiple_filenames_legacy`.
// - C `END_PROCESS(code, msg)` (exits the process) → `io::Error` + early
//   return, preserving recoverability.
// - Timing: `TIME_getTime` / `clock()` → `crate::timefn::get_time` /
//   `libc::clock()`; `LZ4IO_finalTimeDisplay` → `crate::io::prefs::final_time_display`.
// - `FNSPACE` / manual realloc loop for dst filename → `String` allocation.
// - For HC block compression the Rust port of `LZ4_compress_HC`
//   (`crate::hc::api::compress_hc`) is used; this avoids an FFI dependency
//   on `lz4-sys` while preserving bit-identical output.

use std::io::{self, Read, Write};

use crate::block::compress::{compress_bound, compress_fast};
use crate::io::file_io::{open_dst_file, open_src_file, STDOUT_MARK};
use crate::io::prefs::{final_time_display, Prefs, LEGACY_BLOCKSIZE, LEGACY_MAGICNUMBER,
                       MAGICNUMBER_SIZE};
use crate::timefn::get_time;

extern "C" {
    fn clock() -> libc::clock_t;
}

// The 4-byte little-endian size field that precedes each compressed block in
// the legacy format (lz4io.c line 760).
const LEGACY_BLOCK_HEADER_SIZE: usize = 4;

// ---------------------------------------------------------------------------
// Public result type
// ---------------------------------------------------------------------------

/// Statistics produced by a successful legacy-format compression run.
///
/// Equivalent to the (readSize, wr.totalCSize) pair from C.
#[derive(Debug, Clone, Copy, Default)]
pub struct LegacyResult {
    /// Total uncompressed bytes read from the source.
    pub bytes_read: u64,
    /// Total bytes written to the destination (including the 4-byte header).
    pub bytes_written: u64,
}

// ---------------------------------------------------------------------------
// Private: block-level compressors
// ---------------------------------------------------------------------------

/// Compress one block using the fast (non-HC) compressor.
///
/// Equivalent to `LZ4IO_compressBlockLegacy_fast` (lz4io.c lines 778–795).
///
/// Writes a 4-byte LE size prefix into `dst[..4]`, then compressed data from
/// `dst[4..]`.  `dst` is resized to exactly hold the result.
/// Returns the total byte count written (header + compressed payload).
fn compress_block_fast(src: &[u8], dst: &mut Vec<u8>, clevel: i32) -> io::Result<usize> {
    // acceleration = (-clevel) when clevel < 0, else 0  (mirrors C line 788)
    let acceleration = if clevel < 0 { -clevel } else { 0 };

    let bound = compress_bound(src.len() as i32) as usize;
    dst.resize(bound + LEGACY_BLOCK_HEADER_SIZE, 0);

    let c_size = compress_fast(src, &mut dst[LEGACY_BLOCK_HEADER_SIZE..], acceleration)
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("fast compression failed: {:?}", e),
            )
        })?;

    // Write LE32 compressed-block size (lz4io.c line 792)
    dst[..4].copy_from_slice(&(c_size as u32).to_le_bytes());
    Ok(c_size + LEGACY_BLOCK_HEADER_SIZE)
}

/// Compress one block using the HC compressor.
///
/// Equivalent to `LZ4IO_compressBlockLegacy_HC` (lz4io.c lines 797–814).
///
/// Writes a 4-byte LE size prefix into `dst[..4]`, then compressed data from
/// `dst[4..]`.  `dst` is resized to exactly hold the result.
/// Returns the total byte count written (header + compressed payload).
fn compress_block_hc(src: &[u8], dst: &mut Vec<u8>, clevel: i32) -> io::Result<usize> {
    let bound = compress_bound(src.len() as i32) as usize;
    dst.resize(bound + LEGACY_BLOCK_HEADER_SIZE, 0);

    // SAFETY: src and dst slices are valid, non-overlapping, and correctly sized.
    let c_size = unsafe {
        crate::hc::api::compress_hc(
            src.as_ptr(),
            dst[LEGACY_BLOCK_HEADER_SIZE..].as_mut_ptr(),
            src.len() as i32,
            bound as i32,
            clevel,
        )
    };

    if c_size < 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "HC compression failed",
        ));
    }

    let c_size = c_size as usize;
    // Write LE32 compressed-block size (lz4io.c line 811)
    dst[..4].copy_from_slice(&(c_size as u32).to_le_bytes());
    Ok(c_size + LEGACY_BLOCK_HEADER_SIZE)
}

// ---------------------------------------------------------------------------
// Private: internal compression loop
// ---------------------------------------------------------------------------

/// Core legacy-format compression implementation.
///
/// Equivalent to `LZ4IO_compressLegacy_internal` (lz4io.c lines 820–900).
///
/// Dispatches to `compress_block_fast` when `compressionlevel < 3`, otherwise
/// to `compress_block_hc` (mirrors C line 827).
fn compress_legacy_internal(
    input_filename: &str,
    output_filename: &str,
    compressionlevel: i32,
    prefs: &Prefs,
) -> io::Result<LegacyResult> {
    let mut src_reader = open_src_file(input_filename)?;
    let mut dst_file = open_dst_file(output_filename, prefs)?;

    // Write archive header: 4-byte LE magic (lz4io.c lines 854–858)
    let magic_bytes = LEGACY_MAGICNUMBER.to_le_bytes();
    dst_file.write_all(&magic_bytes)?;

    let mut bytes_read: u64 = 0;
    let mut bytes_written: u64 = MAGICNUMBER_SIZE as u64;

    // Reusable buffers for one chunk cycle
    let mut src_buf = vec![0u8; LEGACY_BLOCKSIZE];
    let mut cmp_buf: Vec<u8> = Vec::with_capacity(
        compress_bound(LEGACY_BLOCKSIZE as i32) as usize + LEGACY_BLOCK_HEADER_SIZE,
    );

    // Use fast or HC compressor based on level (mirrors C line 827)
    let use_hc = compressionlevel >= 3;

    loop {
        // Read up to LEGACY_BLOCKSIZE bytes
        let mut total_read = 0usize;
        while total_read < LEGACY_BLOCKSIZE {
            match src_reader.read(&mut src_buf[total_read..]) {
                Ok(0) => break,
                Ok(n) => total_read += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }

        if total_read == 0 {
            break; // EOF
        }

        bytes_read += total_read as u64;

        // Compress the chunk
        let chunk = &src_buf[..total_read];
        let written = if use_hc {
            compress_block_hc(chunk, &mut cmp_buf, compressionlevel)?
        } else {
            compress_block_fast(chunk, &mut cmp_buf, compressionlevel)?
        };

        // Write header + compressed data
        dst_file.write_all(&cmp_buf[..written])?;
        bytes_written += written as u64;
    }

    dst_file.flush()?;

    // Status display (lz4io.c lines 884–888)
    let ratio = if bytes_read == 0 {
        100.0
    } else {
        (bytes_written as f64) / (bytes_read as f64) * 100.0
    };
    crate::io::prefs::display_level(
        2,
        &format!(
            "\r{:79}\r",
            "" // blank line
        ),
    );
    crate::io::prefs::display_level(
        2,
        &format!(
            "Compressed {} bytes into {} bytes ==> {:.2}% \n",
            bytes_read, bytes_written, ratio
        ),
    );

    Ok(LegacyResult {
        bytes_read,
        bytes_written,
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compress a single file to legacy LZ4 format.
///
/// If `dst` is the `"stdout"` sentinel, output is written to stdout.
///
/// Equivalent to `LZ4IO_compressFilename_Legacy` (lz4io.c lines 907–918).
///
/// Returns `Ok(LegacyResult)` with byte statistics on success, or an
/// `io::Error` on failure.
pub fn compress_filename_legacy(
    src: &str,
    dst: &str,
    compressionlevel: i32,
    prefs: &Prefs,
) -> io::Result<LegacyResult> {
    let time_start = get_time();
    let cpu_start = unsafe { clock() };

    let result = compress_legacy_internal(src, dst, compressionlevel, prefs);

    // Always display timing (lz4io.c line 916), using bytes_read on success
    // or 0 on failure (mirrors C: `LZ4IO_finalTimeDisplay(timeStart, cpuStart, processed)`).
    let processed = result.as_ref().map(|r| r.bytes_read).unwrap_or(0);
    final_time_display(time_start, cpu_start, processed);

    result
}

/// Compress multiple files to legacy LZ4 format.
///
/// Each input file `srcs[i]` is compressed to `srcs[i] + suffix`.  If
/// `suffix` is the `"stdout"` sentinel, all output is written to stdout.
///
/// Returns the number of files that could not be compressed (0 on complete
/// success), mirroring `LZ4IO_compressMultipleFilenames_Legacy` (lz4io.c
/// lines 924–973).
pub fn compress_multiple_filenames_legacy(
    srcs: &[&str],
    suffix: &str,
    compressionlevel: i32,
    prefs: &Prefs,
) -> io::Result<()> {
    let time_start = get_time();
    let cpu_start = unsafe { clock() };
    let mut missed_files: usize = 0;
    let mut total_processed: u64 = 0;

    let suffix_is_stdout = suffix == STDOUT_MARK;

    for &src in srcs {
        let dst: String = if suffix_is_stdout {
            STDOUT_MARK.to_owned()
        } else {
            // Build destination filename: src + suffix (mirrors C lines 952–959)
            format!("{}{}", src, suffix)
        };

        match compress_legacy_internal(src, &dst, compressionlevel, prefs) {
            Ok(res) => total_processed += res.bytes_read,
            Err(_) => missed_files += 1,
        }
    }

    // Display cumulative timing (lz4io.c line 969)
    final_time_display(time_start, cpu_start, total_processed);

    if missed_files > 0 {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{} file(s) could not be compressed", missed_files),
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

    // Helper: compress `data` in legacy format and return the raw bytes.
    fn compress_to_bytes(data: &[u8], clevel: i32) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("input.bin");
        let dst_path = dir.path().join("output.lz4");
        std::fs::write(&src_path, data).unwrap();

        let prefs = Prefs::default();
        let _result = compress_filename_legacy(
            src_path.to_str().unwrap(),
            dst_path.to_str().unwrap(),
            clevel,
            &prefs,
        )
        .unwrap();

        std::fs::read(&dst_path).unwrap()
    }

    #[test]
    fn magic_number_written_first() {
        let output = compress_to_bytes(b"hello world", 1);
        assert!(output.len() >= 4);
        // First 4 bytes must be the legacy magic number in LE
        let magic = u32::from_le_bytes([output[0], output[1], output[2], output[3]]);
        assert_eq!(magic, LEGACY_MAGICNUMBER);
    }

    #[test]
    fn fast_mode_block_header_present() {
        let data = vec![0u8; 1024];
        let output = compress_to_bytes(&data, 1); // fast mode (clevel < 3)
        assert!(output.len() > MAGICNUMBER_SIZE + LEGACY_BLOCK_HEADER_SIZE);
    }

    #[test]
    fn hc_mode_block_header_present() {
        let data = vec![0u8; 1024];
        let output = compress_to_bytes(&data, 9); // HC mode (clevel >= 3)
        assert!(output.len() > MAGICNUMBER_SIZE + LEGACY_BLOCK_HEADER_SIZE);
    }

    #[test]
    fn hc_produces_output_no_larger_than_fast_for_compressible() {
        // Compressible data: long run of repeated bytes
        let data = vec![b'A'; 16 * 1024];
        let fast_out = compress_to_bytes(&data, 1);
        let hc_out = compress_to_bytes(&data, 9);
        assert!(
            hc_out.len() <= fast_out.len(),
            "HC ({}) should not be larger than fast ({})",
            hc_out.len(),
            fast_out.len()
        );
    }

    #[test]
    fn round_trip_fast_mode() {
        // Compress with Rust, then verify the block can be decompressed.
        let original = b"The quick brown fox jumps over the lazy dog.";
        let compressed = compress_to_bytes(original, 1);

        // Parse the legacy format manually:
        // - bytes 0..4: magic
        // - bytes 4..8: LE32 block size
        // - bytes 8..8+block_size: compressed block data
        assert!(compressed.len() >= 8);
        let block_size =
            u32::from_le_bytes([compressed[4], compressed[5], compressed[6], compressed[7]])
                as usize;
        assert!(compressed.len() >= 8 + block_size);

        let compressed_block = &compressed[8..8 + block_size];
        let decompressed =
            lz4_flex::block::decompress(compressed_block, original.len() * 2).unwrap();
        assert_eq!(&decompressed[..original.len()], original);
    }

    #[test]
    fn round_trip_hc_mode() {
        let original = b"The quick brown fox jumps over the lazy dog.";
        let compressed = compress_to_bytes(original, 9);

        assert!(compressed.len() >= 8);
        let block_size =
            u32::from_le_bytes([compressed[4], compressed[5], compressed[6], compressed[7]])
                as usize;
        assert!(compressed.len() >= 8 + block_size);

        let compressed_block = &compressed[8..8 + block_size];
        let decompressed =
            lz4_flex::block::decompress(compressed_block, original.len() * 2).unwrap();
        assert_eq!(&decompressed[..original.len()], original);
    }

    #[test]
    fn bytes_read_matches_input_size() {
        let data = b"sample data for size check";
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("input.bin");
        let dst_path = dir.path().join("output.lz4");
        std::fs::write(&src_path, data).unwrap();

        let prefs = Prefs::default();
        let result = compress_filename_legacy(
            src_path.to_str().unwrap(),
            dst_path.to_str().unwrap(),
            1,
            &prefs,
        )
        .unwrap();

        assert_eq!(result.bytes_read, data.len() as u64);
        assert!(result.bytes_written > MAGICNUMBER_SIZE as u64);
    }

    #[test]
    fn compress_multiple_filenames_legacy_ok() {
        let dir = tempfile::tempdir().unwrap();

        let src1 = dir.path().join("a.txt");
        let src2 = dir.path().join("b.txt");
        std::fs::write(&src1, b"file a content").unwrap();
        std::fs::write(&src2, b"file b content").unwrap();

        let prefs = Prefs::default();
        let srcs = [src1.to_str().unwrap(), src2.to_str().unwrap()];
        let result =
            compress_multiple_filenames_legacy(&srcs, ".lz4", 1, &prefs);
        assert!(result.is_ok());

        // Both output files should exist
        assert!(dir.path().join("a.txt.lz4").exists());
        assert!(dir.path().join("b.txt.lz4").exists());
    }

    #[test]
    fn compress_nonexistent_src_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let dst_path = dir.path().join("out.lz4");
        let prefs = Prefs::default();
        let result = compress_filename_legacy(
            "/nonexistent/file/that/cannot/exist.bin",
            dst_path.to_str().unwrap(),
            1,
            &prefs,
        );
        assert!(result.is_err());
    }
}
