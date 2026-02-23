//! LZ4 file-level streaming I/O — Rust port of lz4file.c / lz4file.h v1.10.0.
//!
//! Provides a thin streaming wrapper around the LZ4 Frame API using generic
//! `R: Read` / `W: Write` parameters in place of C `FILE*` handles.
//!
//! # Public API
//! - [`Lz4ReadFile`]  — streaming decompressor (`LZ4_readFile_t` / `LZ4F_readOpen/read/readClose`)
//! - [`Lz4WriteFile`] — streaming compressor   (`LZ4_writeFile_t` / `LZ4F_writeOpen/write/writeClose`)
//! - [`lz4_read_frame`]  — convenience: decompress one complete frame
//! - [`lz4_write_frame`] — convenience: compress a buffer as one complete frame

use std::io::{self, Read, Write};

use crate::frame::compress::{
    lz4f_compress_begin, lz4f_compress_bound, lz4f_compress_end, lz4f_compress_update,
    lz4f_create_compression_context,
};
use crate::frame::decompress::{
    lz4f_create_decompression_context, lz4f_decompress, lz4f_get_frame_info, Lz4FDCtx,
};
use crate::frame::types::{
    BlockSizeId, Lz4FCCtx, Lz4FError, Preferences, LZ4F_VERSION, MAX_FH_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Map a `BlockSizeId` to its corresponding maximum block size in bytes.
/// Equivalent to the `switch (info.blockSizeID)` in `LZ4F_readOpen` / `LZ4F_writeOpen`.
fn block_size_from_id(id: BlockSizeId) -> usize {
    match id {
        BlockSizeId::Default | BlockSizeId::Max64Kb => 64 * 1024,
        BlockSizeId::Max256Kb => 256 * 1024,
        BlockSizeId::Max1Mb => 1024 * 1024,
        BlockSizeId::Max4Mb => 4 * 1024 * 1024,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4ReadFile<R>  (LZ4_readFile_s in lz4file.c:49-56)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming LZ4 frame decompressor backed by any `R: Read`.
///
/// Implements [`std::io::Read`] for transparent decompression.
///
/// Corresponds to `LZ4_readFile_s` / `LZ4_readFile_t` in lz4file.h.
pub struct Lz4ReadFile<R: Read> {
    /// LZ4 decompression context (C: `dctxPtr`).
    dctx: Box<Lz4FDCtx>,
    /// Underlying reader (C: `fp`).
    inner: R,
    /// Internal source buffer holding raw (compressed) bytes (C: `srcBuf`).
    src_buf: Vec<u8>,
    /// Number of valid bytes in `src_buf` (C: `srcBufSize`).
    src_buf_size: usize,
    /// Read offset within `src_buf` (C: `srcBufNext`).
    src_buf_next: usize,
}

impl<R: Read> Lz4ReadFile<R> {
    /// Open a streaming LZ4 frame reader.
    ///
    /// Reads the frame header immediately to determine the block size and
    /// validate the magic number, leaving any post-header bytes in the internal
    /// source buffer so that the first `read()` call sees them.
    ///
    /// Equivalent to `LZ4F_readOpen` (lz4file.c:73–138).
    pub fn open(mut reader: R) -> Result<Self, Lz4FError> {
        // Create a fresh decompression context.
        let mut dctx = lz4f_create_decompression_context(LZ4F_VERSION)?;

        // Read up to MAX_FH_SIZE (19) bytes for the frame header.
        // Mirrors `fread(buf, 1, sizeof(buf), fp)` in C — reads what's available.
        let mut header_buf = [0u8; MAX_FH_SIZE];
        let mut total_read = 0usize;
        while total_read < MAX_FH_SIZE {
            let n = reader
                .read(&mut header_buf[total_read..])
                .map_err(|_| Lz4FError::IoRead)?;
            if n == 0 {
                break; // EOF — might still be enough for the header
            }
            total_read += n;
        }
        if total_read == 0 {
            return Err(Lz4FError::IoRead);
        }

        // Parse the header using only the bytes we actually read.
        let (frame_info, consumed, _hint) =
            lz4f_get_frame_info(&mut dctx, &header_buf[..total_read])?;

        // Determine source buffer capacity from the negotiated block size.
        let src_buf_max_size = block_size_from_id(frame_info.block_size_id);

        // Allocate source buffer and copy leftover bytes after the header
        // (C: `memcpy(srcBuf, buf + consumedSize, srcBufSize)`).
        let leftover = total_read - consumed;
        let mut src_buf = vec![0u8; src_buf_max_size];
        src_buf[..leftover].copy_from_slice(&header_buf[consumed..total_read]);

        Ok(Lz4ReadFile {
            dctx,
            inner: reader,
            src_buf,
            src_buf_size: leftover,
            src_buf_next: 0,
        })
    }
}

impl<R: Read> Read for Lz4ReadFile<R> {
    /// Decompress LZ4 frame data into `buf`.
    ///
    /// Returns the number of decompressed bytes written, or `0` at
    /// end-of-frame / EOF.
    ///
    /// Equivalent to `LZ4F_read` (lz4file.c:140–181).
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = buf.len();
        let mut next: usize = 0;

        while next < size {
            let src_avail = self.src_buf_size - self.src_buf_next;

            // Refill the source buffer when it is exhausted.
            if src_avail == 0 {
                let n = self.inner.read(&mut self.src_buf)?;
                if n == 0 {
                    break; // EOF on compressed stream
                }
                self.src_buf_size = n;
                self.src_buf_next = 0;
            }

            let src_avail = self.src_buf_size - self.src_buf_next;

            // Copy source bytes to a local buffer to avoid simultaneous field
            // borrows (src_buf and dctx are separate struct fields, but the
            // borrow checker sees both as borrows of `self`).
            let src_copy = self.src_buf[self.src_buf_next..self.src_buf_next + src_avail].to_vec();

            let (src_consumed, dst_written, _hint) =
                lz4f_decompress(&mut self.dctx, Some(&mut buf[next..]), &src_copy, None)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

            self.src_buf_next += src_consumed;
            next += dst_written;
        }

        Ok(next)
    }
}

impl<R: Read> Drop for Lz4ReadFile<R> {
    /// The `dctx` Box is dropped automatically; no explicit free is needed.
    fn drop(&mut self) {}
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4WriteFile<W>  (LZ4_writeFile_s in lz4file.c:193-200)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming LZ4 frame compressor backed by any `W: Write`.
///
/// Implements [`std::io::Write`] for transparent compression.
///
/// Corresponds to `LZ4_writeFile_s` / `LZ4_writeFile_t` in lz4file.h.
///
/// # Usage
/// ```no_run
/// use lz4::file::Lz4WriteFile;
/// use std::io::Write;
///
/// let mut lz4w = Lz4WriteFile::open(std::io::sink(), None).unwrap();
/// lz4w.write_all(b"hello").unwrap();
/// lz4w.finish().unwrap();
/// ```
pub struct Lz4WriteFile<W: Write> {
    /// LZ4 compression context (C: `cctxPtr`).
    cctx: Box<Lz4FCCtx>,
    /// Underlying writer; wrapped in `Option` so `finish()` can take ownership.
    /// (C: `fp`)
    inner: Option<W>,
    /// Output buffer for compressed data (C: `dstBuf`).
    dst_buf: Vec<u8>,
    /// Maximum input chunk size per `compress_update` call (C: `maxWriteSize`).
    max_write_size: usize,
    /// Sticky error flag: once `true`, `Drop` and `finish()` skip `compress_end`.
    /// (C: `errCode` sticky pattern)
    errored: bool,
}

impl<W: Write> Lz4WriteFile<W> {
    /// Open a streaming LZ4 frame writer.
    ///
    /// Writes the LZ4 frame header to `writer` immediately.
    ///
    /// Equivalent to `LZ4F_writeOpen` (lz4file.c:217–279).
    pub fn open(mut writer: W, prefs: Option<&Preferences>) -> Result<Self, Lz4FError> {
        // Determine the maximum per-chunk input size from the block size preference.
        let max_write_size = prefs
            .map(|p| block_size_from_id(p.frame_info.block_size_id))
            .unwrap_or(64 * 1024); // C default: LZ4F_max64KB

        // Allocate output buffer sized for the worst-case compressed output.
        let dst_buf_max_size = lz4f_compress_bound(max_write_size, prefs);
        let mut dst_buf = vec![0u8; dst_buf_max_size];

        // Create compression context and write the frame header.
        let mut cctx = lz4f_create_compression_context(LZ4F_VERSION)?;

        let header_size = lz4f_compress_begin(&mut cctx, &mut dst_buf, prefs)?;
        writer
            .write_all(&dst_buf[..header_size])
            .map_err(|_| Lz4FError::IoWrite)?;

        Ok(Lz4WriteFile {
            cctx,
            inner: Some(writer),
            dst_buf,
            max_write_size,
            errored: false,
        })
    }

    /// Flush any buffered data, write the end-mark (+ optional checksum), and
    /// return the underlying writer.
    ///
    /// This is the preferred way to finish writing.  If you let the
    /// [`Lz4WriteFile`] be `drop`ped instead, errors during finalization are
    /// silently discarded.
    ///
    /// Equivalent to `LZ4F_writeClose` (lz4file.c:317–341); the C `goto out`
    /// cleanup is replaced by the `?` operator + RAII.
    pub fn finish(mut self) -> Result<W, Lz4FError> {
        // Only flush the end-mark if no prior error was recorded (mirrors the
        // C `if (lz4fWrite->errCode == LZ4F_OK_NoError)` guard).
        if !self.errored {
            let writer = self.inner.as_mut().expect("inner writer already taken");
            let end_size = lz4f_compress_end(&mut self.cctx, &mut self.dst_buf, None)?;
            writer
                .write_all(&self.dst_buf[..end_size])
                .map_err(|_| Lz4FError::IoWrite)?;
        }
        // Take the writer out of the Option so Drop does not double-finalize.
        Ok(self.inner.take().expect("inner writer already taken"))
    }
}

impl<W: Write> Write for Lz4WriteFile<W> {
    /// Compress `buf` and write the compressed output to the inner writer.
    ///
    /// Chunks the input into `max_write_size` pieces and calls
    /// `compress_update` for each, mirroring the loop in `LZ4F_write`
    /// (lz4file.c:281–315).
    ///
    /// Returns `Ok(buf.len())` on success (all bytes consumed).
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut remain = buf.len();
        let mut p = 0usize;

        while remain > 0 {
            let chunk = remain.min(self.max_write_size);

            let compressed =
                lz4f_compress_update(&mut self.cctx, &mut self.dst_buf, &buf[p..p + chunk], None)
                    .map_err(|e| {
                    self.errored = true;
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;

            self.inner
                .as_mut()
                .expect("inner writer already taken")
                .write_all(&self.dst_buf[..compressed])
                .map_err(|e| {
                    self.errored = true;
                    e
                })?;

            p += chunk;
            remain -= chunk;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner
            .as_mut()
            .expect("inner writer already taken")
            .flush()
    }
}

impl<W: Write> Drop for Lz4WriteFile<W> {
    /// Attempt to finalize the frame on drop (e.g. in case of panic).
    ///
    /// Errors during drop are silently ignored per Rust convention.
    /// If you need to handle finalization errors, call [`Lz4WriteFile::finish`]
    /// explicitly before the value is dropped.
    fn drop(&mut self) {
        if self.inner.is_none() || self.errored {
            return; // finish() was already called, or a prior write error occurred
        }
        // Attempt to write the end-mark; ignore errors.
        let _ = lz4f_compress_end(&mut self.cctx, &mut self.dst_buf, None).and_then(|end_size| {
            self.inner
                .as_mut()
                .unwrap()
                .write_all(&self.dst_buf[..end_size])
                .map_err(|_| Lz4FError::IoWrite)
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Convenience functions  (equivalent to LZ4_writeFile / LZ4_readFile in lz4file.h)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `data` as a complete LZ4 frame and write it to `writer`.
///
/// Returns the inner writer on success.
///
/// High-level convenience equivalent of `LZ4_writeFile`.
pub fn lz4_write_frame<W: Write>(data: &[u8], writer: W) -> Result<W, Lz4FError> {
    let mut lz4w = Lz4WriteFile::open(writer, None)?;
    lz4w.write_all(data).map_err(|_| Lz4FError::IoWrite)?;
    lz4w.finish()
}

/// Decompress one complete LZ4 frame from `reader` and write the raw bytes to `writer`.
///
/// High-level convenience equivalent of `LZ4_readFile`.
pub fn lz4_read_frame<R: Read>(reader: R, writer: &mut impl Write) -> Result<(), Lz4FError> {
    let mut lz4r = Lz4ReadFile::open(reader)?;
    let mut tmp = [0u8; 64 * 1024];
    loop {
        let n = lz4r.read(&mut tmp).map_err(|_| Lz4FError::IoRead)?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&tmp[..n])
            .map_err(|_| Lz4FError::IoWrite)?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Round-trip: lz4_write_frame → lz4_read_frame recovers original data.
    #[test]
    fn round_trip_small() {
        let original = b"Hello, LZ4 world! This is a test of the lz4 file streaming API.";

        // Compress
        let compressed = lz4_write_frame(original, Vec::new()).unwrap();

        // Decompress
        let mut recovered = Vec::new();
        lz4_read_frame(Cursor::new(&compressed), &mut recovered).unwrap();

        assert_eq!(recovered, original);
    }

    /// Round-trip with data larger than a single 64 KiB block.
    #[test]
    fn round_trip_multi_block() {
        let original: Vec<u8> = (0u8..=255).cycle().take(200 * 1024).collect();

        let compressed = lz4_write_frame(&original, Vec::new()).unwrap();

        let mut recovered = Vec::new();
        lz4_read_frame(Cursor::new(&compressed), &mut recovered).unwrap();

        assert_eq!(recovered, original);
    }

    /// Round-trip using the streaming Write/Read trait implementations.
    #[test]
    fn streaming_write_read() {
        let original: Vec<u8> = b"streaming test data"
            .iter()
            .cycle()
            .take(4096)
            .cloned()
            .collect();

        let mut lz4w = Lz4WriteFile::open(Vec::new(), None).unwrap();
        // Write in small chunks to exercise the chunking loop.
        for chunk in original.chunks(256) {
            lz4w.write_all(chunk).unwrap();
        }
        let compressed = lz4w.finish().unwrap();

        let mut lz4r = Lz4ReadFile::open(Cursor::new(&compressed)).unwrap();
        let mut recovered = Vec::new();
        let mut tmp = [0u8; 512];
        loop {
            let n = lz4r.read(&mut tmp).unwrap();
            if n == 0 {
                break;
            }
            recovered.extend_from_slice(&tmp[..n]);
        }

        assert_eq!(recovered, original);
    }

    /// Small-but-nonzero input that still fits in one block.
    /// (The C LZ4F_readOpen reads exactly MAX_FH_SIZE=19 bytes for the frame header,
    /// so an LZ4 frame shorter than 19 bytes cannot be decoded — same constraint applies here.)
    #[test]
    fn round_trip_one_byte() {
        let original = b"x";
        let compressed = lz4_write_frame(original, Vec::new()).unwrap();
        let mut recovered = Vec::new();
        lz4_read_frame(Cursor::new(&compressed), &mut recovered).unwrap();
        assert_eq!(recovered.as_slice(), original.as_ref());
    }

    /// Empty input round-trip: the writer produces a valid LZ4 frame with just
    /// a header + end-mark; the reader should decompress it to empty bytes.
    #[test]
    fn round_trip_empty() {
        let original: &[u8] = b"";
        let compressed = lz4_write_frame(original, Vec::new()).unwrap();
        let mut recovered = Vec::new();
        lz4_read_frame(Cursor::new(&compressed), &mut recovered).unwrap();
        assert_eq!(recovered.as_slice(), original);
    }
}
