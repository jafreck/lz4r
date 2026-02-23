// Unit tests for task-013: src/io/sparse.rs — Sparse file write utilities.
//
// Verifies behavioural parity with lz4io.c v1.10.0 lines 1583–1676:
//   - `LZ4IO_readLE32`       → `read_le32`
//   - `LZ4IO_fwriteSparse`   → `fwrite_sparse`
//   - `LZ4IO_fwriteSparseEnd`→ `fwrite_sparse_end`
//
// Coverage:
//   - read_le32: zero, one, max, known vector, extra bytes ignored
//   - SPARSE_SEGMENT_SIZE constant value
//   - fwrite_sparse_end: no-op at zero, extends file by correct byte count
//   - fwrite_sparse (Unix): plain data, all-zeros accumulation, zeros+data,
//     data+trailing zeros, mixed round-trip, non-word-aligned tail,
//     accumulated skips > ONE_GB guard, initial stored_skips carry-in

use lz4::io::sparse::{fwrite_sparse_end, read_le32, SPARSE_SEGMENT_SIZE};
use std::io::{Read, Seek, SeekFrom, Write};

// ═════════════════════════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════════════════════════

/// Read the full contents of a tempfile from its start.
fn read_all(f: &mut std::fs::File) -> Vec<u8> {
    f.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    buf
}

// ═════════════════════════════════════════════════════════════════════════════
// Constants
// ═════════════════════════════════════════════════════════════════════════════

/// SPARSE_SEGMENT_SIZE must equal 32 KiB (C: `(32 KB) / sizeof(size_t)` bytes).
#[test]
fn sparse_segment_size_is_32kib() {
    assert_eq!(SPARSE_SEGMENT_SIZE, 32 * 1024);
}

// ═════════════════════════════════════════════════════════════════════════════
// read_le32  (LZ4IO_readLE32)
// ═════════════════════════════════════════════════════════════════════════════

/// All-zero bytes decode to 0.
#[test]
fn read_le32_zero_bytes() {
    assert_eq!(read_le32(&[0x00, 0x00, 0x00, 0x00]), 0u32);
}

/// Least-significant byte first: [1,0,0,0] → 1.
#[test]
fn read_le32_value_one() {
    assert_eq!(read_le32(&[0x01, 0x00, 0x00, 0x00]), 1u32);
}

/// All-0xFF bytes decode to u32::MAX.
#[test]
fn read_le32_max_value() {
    assert_eq!(read_le32(&[0xFF, 0xFF, 0xFF, 0xFF]), u32::MAX);
}

/// Known vector: [0x01, 0x02, 0x03, 0x04] → 0x04030201 (little-endian).
#[test]
fn read_le32_known_vector() {
    assert_eq!(read_le32(&[0x01, 0x02, 0x03, 0x04]), 0x0403_0201u32);
}

/// Only the first 4 bytes are consumed; extra bytes are ignored.
#[test]
fn read_le32_extra_bytes_ignored() {
    let result = read_le32(&[0x01, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(
        result, 1u32,
        "extra bytes beyond the first 4 must be ignored"
    );
}

/// High byte only: [0,0,0,0x80] → 0x80000000.
#[test]
fn read_le32_high_byte_set() {
    assert_eq!(read_le32(&[0x00, 0x00, 0x00, 0x80]), 0x8000_0000u32);
}

/// Each byte position is correctly weighted.
#[test]
fn read_le32_byte_weight() {
    // byte 0 (LSB) = 0x01 → 1
    assert_eq!(read_le32(&[0x01, 0x00, 0x00, 0x00]), 1);
    // byte 1 = 0x01 → 256
    assert_eq!(read_le32(&[0x00, 0x01, 0x00, 0x00]), 256);
    // byte 2 = 0x01 → 65536
    assert_eq!(read_le32(&[0x00, 0x00, 0x01, 0x00]), 65_536);
    // byte 3 (MSB) = 0x01 → 16777216
    assert_eq!(read_le32(&[0x00, 0x00, 0x00, 0x01]), 16_777_216);
}

// ═════════════════════════════════════════════════════════════════════════════
// fwrite_sparse_end  (LZ4IO_fwriteSparseEnd)
// ═════════════════════════════════════════════════════════════════════════════

/// stored_skips == 0: no-op; file remains empty.
#[test]
fn fwrite_sparse_end_zero_skips_is_noop() {
    let mut f = tempfile::tempfile().unwrap();
    fwrite_sparse_end(&mut f, 0).unwrap();
    let contents = read_all(&mut f);
    assert!(
        contents.is_empty(),
        "stored_skips=0 must leave the file empty"
    );
}

/// stored_skips == 1: writes exactly 1 zero byte.
#[test]
fn fwrite_sparse_end_one_skip_writes_one_byte() {
    let mut f = tempfile::tempfile().unwrap();
    fwrite_sparse_end(&mut f, 1).unwrap();
    let len = f.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(len, 1, "stored_skips=1 should produce a 1-byte file");
    let contents = read_all(&mut f);
    assert_eq!(contents, &[0u8], "the byte written must be zero");
}

/// stored_skips == 4: file ends at logical offset 4 (3 seeked + 1 written).
#[test]
fn fwrite_sparse_end_four_skips_extends_file() {
    let mut f = tempfile::tempfile().unwrap();
    fwrite_sparse_end(&mut f, 4).unwrap();
    let len = f.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(
        len, 4,
        "stored_skips=4 should produce a 4-byte logical file"
    );
}

/// stored_skips == 1024: file logical size equals 1024.
#[test]
fn fwrite_sparse_end_large_skips_correct_size() {
    let mut f = tempfile::tempfile().unwrap();
    fwrite_sparse_end(&mut f, 1024).unwrap();
    let len = f.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(len, 1024);
}

/// fwrite_sparse_end is idempotent-ish when called with skips on a non-empty file:
/// the file position before the call is the start offset that gets extended.
#[test]
fn fwrite_sparse_end_appends_after_existing_data() {
    let mut f = tempfile::tempfile().unwrap();
    // Write 8 bytes of data first.
    f.write_all(&[0xABu8; 8]).unwrap();
    // Now 8 pending skip bytes.
    fwrite_sparse_end(&mut f, 8).unwrap();
    let len = f.seek(SeekFrom::End(0)).unwrap();
    // Total logical size: 8 (written) + 8 (skips) = 16.
    assert_eq!(
        len, 16,
        "sparse end after written data should produce correct total size"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// fwrite_sparse — Unix-only tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(unix)]
mod unix_fwrite_sparse {
    use super::*;
    use lz4::io::sparse::fwrite_sparse;
    use std::mem;

    const WORD: usize = mem::size_of::<usize>();

    // ── Basic plain-data write ────────────────────────────────────────────────

    /// Non-zero buffer is written verbatim; returned skips == 0.
    #[test]
    fn plain_nonzero_data_written_in_full() {
        let mut f = tempfile::tempfile().unwrap();
        let data: Vec<u8> = (1u8..=32).collect();
        let skips = fwrite_sparse(&mut f, &data, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0, "no trailing zeros → skips must be 0");
        let contents = read_all(&mut f);
        assert_eq!(contents, data, "file contents must match input exactly");
    }

    /// Single non-zero byte is written; skips == 0.
    #[test]
    fn single_nonzero_byte_written() {
        let mut f = tempfile::tempfile().unwrap();
        let skips = fwrite_sparse(&mut f, &[0x42u8], SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0);
        let contents = read_all(&mut f);
        assert_eq!(contents, &[0x42u8]);
    }

    // ── All-zeros accumulation ────────────────────────────────────────────────

    /// A word-aligned all-zeros buffer accumulates all bytes as skips.
    /// No data is written; file position remains 0.
    #[test]
    fn all_zeros_word_aligned_accumulates_skips() {
        let mut f = tempfile::tempfile().unwrap();
        let zeros = vec![0u8; WORD * 4]; // 4 full words, all zero
        let skips = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, zeros.len() as u64, "all zeros must be accumulated");
        let pos = f.seek(SeekFrom::Current(0)).unwrap();
        assert_eq!(
            pos, 0,
            "no seek/write should have occurred for all-zero buffer"
        );
    }

    /// A single zero byte accumulates 1 skip (trailing-bytes path).
    #[test]
    fn single_zero_byte_accumulates_one_skip() {
        let mut f = tempfile::tempfile().unwrap();
        let skips = fwrite_sparse(&mut f, &[0u8], SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 1);
        let pos = f.seek(SeekFrom::Current(0)).unwrap();
        assert_eq!(pos, 0);
    }

    /// 64-byte all-zeros buffer accumulates exactly 64 skips.
    #[test]
    fn zeros_64_bytes_accumulates_64_skips() {
        let mut f = tempfile::tempfile().unwrap();
        let zeros = vec![0u8; 64];
        let skips = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 64);
    }

    // ── carry-in stored_skips ─────────────────────────────────────────────────

    /// Passing an initial stored_skips value carries it forward correctly.
    /// Non-zero data flushes both the carry-in and any segment zeros.
    #[test]
    fn carry_in_stored_skips_flushed_on_nonzero_data() {
        let mut f = tempfile::tempfile().unwrap();
        let data = vec![0xABu8; WORD]; // one non-zero word
        let initial_skips = 8u64;
        let skips = fwrite_sparse(&mut f, &data, SPARSE_SEGMENT_SIZE, initial_skips, true).unwrap();
        assert_eq!(skips, 0, "non-zero data must flush all accumulated skips");
        // File should start at offset 8 (the carry-in skip) then write WORD bytes.
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, initial_skips + WORD as u64);
    }

    /// All-zeros with carry-in: skips accumulate additively.
    #[test]
    fn carry_in_stored_skips_added_to_zero_accumulation() {
        let mut f = tempfile::tempfile().unwrap();
        let zeros = vec![0u8; WORD];
        let initial_skips = 5u64;
        let skips =
            fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, initial_skips, true).unwrap();
        assert_eq!(skips, initial_skips + WORD as u64);
    }

    // ── zeros + data (sparse hole at front) ───────────────────────────────────

    /// [zero words | non-zero words]: seek over zeros, then write non-zero data.
    /// Returned skips == 0 after the data flushes the hole.
    #[test]
    fn zero_words_then_nonzero_writes_correct_data() {
        let mut f = tempfile::tempfile().unwrap();
        let mut buf = vec![0u8; WORD]; // one zero word
        buf.extend_from_slice(&[0xCDu8].repeat(WORD)); // one non-zero word

        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0, "non-zero data at end must flush pending skips");

        // Logical file size: WORD (hole) + WORD (data) = 2*WORD.
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, (2 * WORD) as u64);
    }

    // ── data + trailing zeros (same segment → zeros are written, not accumulated) ──

    /// [non-zero word | zero word] in one segment: the entire segment from the
    /// first non-zero word to the end is written verbatim (including the trailing
    /// zero word).  Trailing zeros are only accumulated when they form a
    /// complete zero segment or are sub-word bytes at the end of the buffer.
    #[test]
    fn nonzero_then_intra_segment_trailing_zeros_are_written() {
        let mut f = tempfile::tempfile().unwrap();
        let mut buf = vec![0xFFu8; WORD]; // non-zero word
        buf.extend_from_slice(&[0u8; WORD]); // trailing zero word (same segment)

        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        // Both words are in one segment; the whole segment is written → skips == 0.
        assert_eq!(
            skips, 0,
            "intra-segment trailing zeros are written, not accumulated"
        );
        let contents = read_all(&mut f);
        assert_eq!(contents, buf, "file must contain both words verbatim");
    }

    // ── Round-trip: read-back matches original ────────────────────────────────

    /// All-zeros → fwrite_sparse → fwrite_sparse_end → read back == original.
    #[test]
    fn all_zeros_round_trip() {
        let mut f = tempfile::tempfile().unwrap();
        let original = vec![0u8; 64];
        let skips = fwrite_sparse(&mut f, &original, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        fwrite_sparse_end(&mut f, skips).unwrap();
        let contents = read_all(&mut f);
        assert_eq!(
            contents, original,
            "all-zeros round-trip must match original"
        );
    }

    /// Non-zero data → fwrite_sparse → read back == original.
    #[test]
    fn nonzero_round_trip() {
        let mut f = tempfile::tempfile().unwrap();
        let original: Vec<u8> = (1u8..=16).collect();
        let skips = fwrite_sparse(&mut f, &original, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        fwrite_sparse_end(&mut f, skips).unwrap();
        let contents = read_all(&mut f);
        assert_eq!(
            contents, original,
            "plain data round-trip must match original"
        );
    }

    /// Mixed buffer [non-zero | zeros | non-zero] round-trips correctly.
    #[test]
    fn mixed_content_round_trip() {
        let mut f = tempfile::tempfile().unwrap();
        let mut original = Vec::new();
        original.extend_from_slice(&[0xABu8; 8]); // 8 non-zero bytes
        original.extend_from_slice(&[0u8; 16]); // 16 zero bytes (hole)
        original.extend_from_slice(&[0xCDu8; 8]); // 8 non-zero bytes

        let skips = fwrite_sparse(&mut f, &original, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        fwrite_sparse_end(&mut f, skips).unwrap();

        let contents = read_all(&mut f);
        assert_eq!(
            contents, original,
            "mixed content round-trip must match original"
        );
    }

    /// Large buffer that spans multiple segments round-trips correctly.
    #[test]
    fn multi_segment_round_trip() {
        let mut f = tempfile::tempfile().unwrap();
        // 128 KiB: 4 × SPARSE_SEGMENT_SIZE.
        let original: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();

        let skips = fwrite_sparse(&mut f, &original, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        fwrite_sparse_end(&mut f, skips).unwrap();

        let contents = read_all(&mut f);
        assert_eq!(
            contents.len(),
            original.len(),
            "multi-segment round-trip length must match"
        );
        assert_eq!(
            contents, original,
            "multi-segment round-trip content must match"
        );
    }

    // ── Trailing-bytes path (non-word-aligned) ────────────────────────────────

    /// Buffer length is not a multiple of WORD: trailing bytes handled correctly.
    #[test]
    fn non_word_aligned_nonzero_trailing_bytes() {
        let mut f = tempfile::tempfile().unwrap();
        // Exactly WORD+1 bytes with the extra byte being non-zero.
        let mut buf = vec![0xAAu8; WORD]; // one non-zero word
        buf.push(0xBBu8); // one trailing non-zero byte
        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0);
        let contents = read_all(&mut f);
        assert_eq!(contents, buf);
    }

    /// Buffer length == WORD+1 with trailing zero byte: that byte is accumulated.
    #[test]
    fn non_word_aligned_zero_trailing_byte_accumulated() {
        let mut f = tempfile::tempfile().unwrap();
        let mut buf = vec![0xFFu8; WORD]; // non-zero word
        buf.push(0u8); // one trailing zero byte
        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(
            skips, 1,
            "single trailing zero byte must be accumulated as 1 skip"
        );
    }

    /// Buffer of 3 bytes (sub-word): all non-zero — written, skips == 0.
    #[test]
    fn sub_word_nonzero_buffer_written() {
        let mut f = tempfile::tempfile().unwrap();
        let buf = vec![0x01u8, 0x02, 0x03]; // 3 bytes, all non-zero
        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0);
        let contents = read_all(&mut f);
        assert_eq!(contents, buf);
    }

    /// Buffer of 3 bytes (sub-word): all zero — accumulated as 3 skips.
    #[test]
    fn sub_word_zero_buffer_accumulated() {
        let mut f = tempfile::tempfile().unwrap();
        let buf = vec![0u8; 3];
        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 3);
        let pos = f.seek(SeekFrom::Current(0)).unwrap();
        assert_eq!(pos, 0);
    }

    // ── Empty buffer ──────────────────────────────────────────────────────────

    /// Empty buffer is a no-op: returns stored_skips unchanged.
    #[test]
    fn empty_buffer_is_noop() {
        let mut f = tempfile::tempfile().unwrap();
        let initial_skips = 7u64;
        let skips = fwrite_sparse(&mut f, &[], SPARSE_SEGMENT_SIZE, initial_skips, true).unwrap();
        assert_eq!(
            skips, initial_skips,
            "empty buffer must not modify stored_skips"
        );
        let pos = f.seek(SeekFrom::Current(0)).unwrap();
        assert_eq!(pos, 0, "empty buffer must not move the file pointer");
    }

    // ── stored_skips > ONE_GB guard ───────────────────────────────────────────

    /// When stored_skips > 1 GiB, the function flushes ONE_GB worth of skip before
    /// processing the new buffer (mirrors C check at lz4io.c line 1616).
    /// After the call, stored_skips must be < ONE_GB + buf_len.
    #[test]
    fn stored_skips_over_one_gb_triggers_overflow_guard() {
        // Use a small non-zero buffer to force flushing.
        let buf = vec![0x01u8; WORD];
        let mut f = tempfile::tempfile().unwrap();

        // Pass in slightly more than 1 GiB of stored skips.
        let over_one_gb: u64 = (1u64 << 30) + 1; // ONE_GB + 1
        let skips = fwrite_sparse(&mut f, &buf, SPARSE_SEGMENT_SIZE, over_one_gb, true).unwrap();

        // After the non-zero word flushes everything, skips must be 0.
        assert_eq!(
            skips, 0,
            "non-zero buf must flush all pending skips after ONE_GB guard"
        );
        // File logical length = over_one_gb + WORD (the non-zero word was written).
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, over_one_gb + WORD as u64);
    }

    // ── fwrite_sparse_end completes a sparse sequence ─────────────────────────

    /// Zero buffer followed by fwrite_sparse_end produces correct file size.
    #[test]
    fn zeros_then_sparse_end_gives_correct_size() {
        let mut f = tempfile::tempfile().unwrap();
        let zeros = vec![0u8; 16];
        let skips = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        fwrite_sparse_end(&mut f, skips).unwrap();
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, 16, "file logical size must match buffer length");
    }

    /// Multiple fwrite_sparse calls accumulate correctly across invocations.
    #[test]
    fn multiple_calls_accumulate_skips() {
        let mut f = tempfile::tempfile().unwrap();
        let zeros = vec![0u8; WORD];

        let s1 = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(s1, WORD as u64);

        let s2 = fwrite_sparse(&mut f, &zeros, SPARSE_SEGMENT_SIZE, s1, true).unwrap();
        assert_eq!(s2, 2 * WORD as u64);

        fwrite_sparse_end(&mut f, s2).unwrap();
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, 2 * WORD as u64);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// fwrite_sparse — non-Unix fallback tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(not(unix))]
mod non_unix_fwrite_sparse {
    use super::*;
    use lz4::io::sparse::fwrite_sparse;

    /// Non-Unix fallback writes the buffer as-is and returns 0 skips.
    #[test]
    fn fallback_writes_full_buffer() {
        let mut f = tempfile::tempfile().unwrap();
        let data = vec![0u8; 32]; // zeros
        let skips = fwrite_sparse(&mut f, &data, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0, "non-Unix fallback always returns 0");
        let len = f.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(len, 32, "non-Unix fallback must write all bytes");
    }

    /// Non-Unix: non-zero data is written and skips == 0.
    #[test]
    fn fallback_nonzero_data_written() {
        let mut f = tempfile::tempfile().unwrap();
        let data: Vec<u8> = (1u8..=16).collect();
        let skips = fwrite_sparse(&mut f, &data, SPARSE_SEGMENT_SIZE, 0, true).unwrap();
        assert_eq!(skips, 0);
        let contents = read_all(&mut f);
        assert_eq!(contents, data);
    }
}
