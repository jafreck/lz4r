//! Sparse file write utilities.
//!
//! Mirrors `LZ4IO_readLE32`, `LZ4IO_fwriteSparse`, and `LZ4IO_fwriteSparseEnd`
//! from lz4io.c (lines 1583–1676).
//!
//! On Unix, `fwrite_sparse` scans decompressed output for contiguous runs of
//! zero bytes and advances the file position with `seek(SeekFrom::Current(n))`
//! rather than writing the zeros, creating a sparse file hole on filesystems
//! that support it.  On non-Unix platforms the optimisation is skipped and the
//! function performs a plain write.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::mem;

/// One gigabyte — used as a safe upper bound for a single `seek` call to avoid
/// integer-overflow in the stored-skips counter (mirrors the C guard at line 1616).
const ONE_GB: u64 = 1 << 30;

/// Size of a native word (usize) in bytes.
const WORD: usize = mem::size_of::<usize>();

// ── Public constants ──────────────────────────────────────────────────────────

/// Default sparse-write segment size in bytes (32 KiB), matching the C macro
/// `(32 KB) / sizeof(size_t)` scaled back to bytes.
pub const SPARSE_SEGMENT_SIZE: usize = 32 * 1024;

// ── read_le32 ─────────────────────────────────────────────────────────────────

/// Decodes a little-endian `u32` from the first four bytes of `src`.
///
/// Equivalent to `LZ4IO_readLE32` (lz4io.c line 1583).
#[inline]
pub fn read_le32(src: &[u8]) -> u32 {
    u32::from_le_bytes([src[0], src[1], src[2], src[3]])
}

// ── fwrite_sparse (Unix) ──────────────────────────────────────────────────────

/// Writes `buf` to `file`, using `seek` to create sparse holes for zero runs.
///
/// `sparse_mode` must be `true` to enable sparse writes; when `false` the
/// buffer is written with a plain `write_all` and `Ok(0)` is returned — this
/// mirrors the `if (!sparseMode)` path in `LZ4IO_fwriteSparse` (lz4io.c:1609).
/// Callers should compute `sparse_mode` as
/// `(sparse_file_support - (file_is_stdout as i32)) > 0`, matching the C
/// expression `sparseMode = (sparseFileSupport - (file==stdout)) > 0`.
///
/// The buffer is processed in segments of `sparse_threshold` bytes.  Leading
/// zero *words* (native `usize`-sized) in each segment are accumulated in
/// `stored_skips` rather than written.  When a non-zero word is found the
/// accumulated skip is applied with `file.seek(SeekFrom::Current(…))` before
/// writing the rest of the segment.  Trailing bytes that are not a multiple of
/// `usize` are handled byte-by-byte.
///
/// Returns the updated `stored_skips` value, which the caller must pass back
/// on the next call and ultimately hand to [`fwrite_sparse_end`].
///
/// On non-Unix platforms falls back to [`fwrite_sparse`]'s `#[cfg(not(unix))]`
/// variant which performs a plain write.
///
/// Equivalent to `LZ4IO_fwriteSparse` (lz4io.c line 1594).
#[cfg(unix)]
pub fn fwrite_sparse(
    file: &mut File,
    buf: &[u8],
    sparse_threshold: usize,
    stored_skips: u64,
    sparse_mode: bool,
) -> io::Result<u64> {
    // Non-sparse path: plain write, mirrors C `if (!sparseMode)` at line 1609.
    if !sparse_mode {
        file.write_all(buf)?;
        return Ok(0);
    }

    let mut stored_skips = stored_skips;

    // Guard: avoid integer overflow by flushing if accumulated skips > 1 GB
    // (mirrors the C check at line 1616).
    if stored_skips > ONE_GB {
        file.seek(SeekFrom::Current(ONE_GB as i64))?;
        stored_skips -= ONE_GB;
    }

    let seg_size_words = (sparse_threshold / WORD).max(1);
    let aligned_len = buf.len() / WORD; // number of full usize words in buf
    let mut buf_remaining = aligned_len;
    let mut buf_pos = 0usize; // byte offset into buf

    // Process the word-aligned portion of the buffer in segments.
    while buf_pos < aligned_len * WORD {
        let seg_words = seg_size_words.min(buf_remaining);
        buf_remaining -= seg_words;

        // Count leading zero usize-words in this segment.
        let mut nb_zeros = 0usize;
        for i in 0..seg_words {
            let start = buf_pos + i * WORD;
            // Safety: bounds are guaranteed by `aligned_len` calculation.
            let word = usize::from_ne_bytes(
                buf[start..start + WORD].try_into().unwrap(),
            );
            if word != 0 {
                break;
            }
            nb_zeros += 1;
        }
        stored_skips += (nb_zeros * WORD) as u64;

        if nb_zeros != seg_words {
            // Segment contains non-zero data: apply accumulated seek, then
            // write from the first non-zero word to the end of the segment.
            file.seek(SeekFrom::Current(stored_skips as i64))?;
            stored_skips = 0;
            let write_start = buf_pos + nb_zeros * WORD;
            let write_len = (seg_words - nb_zeros) * WORD;
            file.write_all(&buf[write_start..write_start + write_len])?;
        }

        buf_pos += seg_words * WORD;
    }

    // Handle trailing bytes (buf.len() is not a multiple of WORD).
    let rest = &buf[aligned_len * WORD..];
    if !rest.is_empty() {
        let nb_zero_bytes = rest.iter().take_while(|&&b| b == 0).count();
        stored_skips += nb_zero_bytes as u64;
        if nb_zero_bytes < rest.len() {
            // There is non-zero content in the trailing bytes.
            file.seek(SeekFrom::Current(stored_skips as i64))?;
            stored_skips = 0;
            file.write_all(&rest[nb_zero_bytes..])?;
        }
    }

    Ok(stored_skips)
}

/// Non-Unix fallback: writes the buffer as-is without sparse optimisation.
#[cfg(not(unix))]
pub fn fwrite_sparse(
    file: &mut File,
    buf: &[u8],
    _sparse_threshold: usize,
    _stored_skips: u64,
    _sparse_mode: bool,
) -> io::Result<u64> {
    file.write_all(buf)?;
    Ok(0)
}

// ── fwrite_sparse_end ─────────────────────────────────────────────────────────

/// Finalises a sparse write sequence.
///
/// If there are pending accumulated skips (trailing zeros that were seeked over
/// but not written), advances the file position by `stored_skips - 1` bytes
/// and writes a single zero byte.  This materialises the end of the file at
/// the correct logical offset, matching the behaviour of
/// `LZ4IO_fwriteSparseEnd` (lz4io.c line 1665).
///
/// Must be called exactly once after the last [`fwrite_sparse`] call to ensure
/// the output file has the correct size.
pub fn fwrite_sparse_end(file: &mut File, stored_skips: u64) -> io::Result<()> {
    if stored_skips > 0 {
        file.seek(SeekFrom::Current((stored_skips - 1) as i64))?;
        file.write_all(&[0u8])?;
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};

    // ── read_le32 ──────────────────────────────────────────────────────────────

    #[test]
    fn test_read_le32_zero() {
        assert_eq!(read_le32(&[0, 0, 0, 0]), 0);
    }

    #[test]
    fn test_read_le32_one() {
        assert_eq!(read_le32(&[1, 0, 0, 0]), 1);
    }

    #[test]
    fn test_read_le32_max() {
        assert_eq!(read_le32(&[0xFF, 0xFF, 0xFF, 0xFF]), u32::MAX);
    }

    #[test]
    fn test_read_le32_known_value() {
        // 0x04030201 = little-endian bytes [0x01, 0x02, 0x03, 0x04]
        assert_eq!(read_le32(&[0x01, 0x02, 0x03, 0x04]), 0x04030201);
    }

    // ── fwrite_sparse_end ──────────────────────────────────────────────────────

    #[test]
    fn test_fwrite_sparse_end_no_skips() {
        let mut f = tempfile::tempfile().unwrap();
        // No skips — should be a no-op.
        fwrite_sparse_end(&mut f, 0).unwrap();
        let mut buf = Vec::new();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn test_fwrite_sparse_end_extends_file() {
        let mut f = tempfile::tempfile().unwrap();
        // Simulate 4 bytes of pending skips.
        fwrite_sparse_end(&mut f, 4).unwrap();
        // File should be exactly 4 bytes (3 seeked + 1 zero written).
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, 4);
    }

    // ── fwrite_sparse (platform-specific) ─────────────────────────────────────

    #[cfg(unix)]
    mod unix_tests {
        use super::*;

        #[test]
        fn test_fwrite_sparse_plain_data() {
            let mut f = tempfile::tempfile().unwrap();
            let data: Vec<u8> = (1u8..=16).collect();
            let skips = fwrite_sparse(&mut f, &data, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
            assert_eq!(skips, 0, "no trailing zeros expected");
            f.seek(SeekFrom::Start(0)).unwrap();
            let mut out = vec![0u8; 16];
            f.read_exact(&mut out).unwrap();
            assert_eq!(out, data);
        }

        #[test]
        fn test_fwrite_sparse_all_zeros_accumulates() {
            let mut f = tempfile::tempfile().unwrap();
            let zeros = vec![0u8; 64];
            let skips = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
            // All bytes are zero → should be accumulated as skips, not written.
            assert_eq!(skips, 64);
            // File should have no content yet (no seek+write issued).
            let pos = f.seek(SeekFrom::Current(0)).unwrap();
            assert_eq!(pos, 0);
        }

        #[test]
        fn test_fwrite_sparse_zeros_then_data() {
            // [0,0,0,0,0,0,0,0,  1,2,3,4,5,6,7,8]
            // First 8 bytes zero, then 8 bytes non-zero (assumes 64-bit usize).
            let mut buf = vec![0u8; WORD]; // one zero word
            buf.extend_from_slice(&[1u8, 2, 3, 4, 5, 6, 7, 8]);
            let mut f = tempfile::tempfile().unwrap();
            let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
            assert_eq!(skips, 0, "non-zero data should have flushed pending skips");
            // File should be WORD + 8 bytes in logical size but have
            // `WORD` bytes of hole at the start.
            let logical_pos = f.seek(SeekFrom::Current(0)).unwrap();
            assert_eq!(logical_pos as usize, WORD + 8);
        }

        #[test]
        fn test_fwrite_sparse_end_after_sparse_write() {
            let mut f = tempfile::tempfile().unwrap();
            let zeros = vec![0u8; 16];
            let skips = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
            fwrite_sparse_end(&mut f, skips).unwrap();
            let len = f.seek(SeekFrom::End(0)).unwrap();
            assert_eq!(len, 16, "file logical size should equal buffer size");
        }

        #[test]
        fn test_fwrite_sparse_mixed_content_round_trip() {
            // Write a buffer with a zero hole in the middle and verify the
            // decoded content matches the original.
            let mut buf = Vec::new();
            buf.extend_from_slice(&[0xABu8; 8]); // 8 non-zero bytes
            buf.extend_from_slice(&[0u8; 16]); // 16 zero bytes (hole)
            buf.extend_from_slice(&[0xCDu8; 8]); // 8 non-zero bytes

            let mut f = tempfile::tempfile().unwrap();
            let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
            fwrite_sparse_end(&mut f, skips).unwrap();

            f.seek(SeekFrom::Start(0)).unwrap();
            let mut out = vec![0u8; buf.len()];
            f.read_exact(&mut out).unwrap();
            assert_eq!(out, buf);
        }
    }

    #[cfg(not(unix))]
    mod non_unix_tests {
        use super::*;

        #[test]
        fn test_fwrite_sparse_fallback() {
            let mut f = tempfile::tempfile().unwrap();
            let data = vec![0u8; 32];
            let skips = fwrite_sparse(&mut f, &data, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
            assert_eq!(skips, 0);
            let len = f.seek(SeekFrom::End(0)).unwrap();
            assert_eq!(len, 32);
        }
    }
}
