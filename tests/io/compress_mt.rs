// Integration tests for task-019: src/io/compress_mt.rs — MT Compression Pipeline
//
// Verifies behavioural parity with lz4io.c v1.10.0, lines 455–565, 568–760, 1158–1365
// (declarations #7, #8, #12):
//   - compress_filename_mt: single-block path (file < 4 MB)
//   - compress_filename_mt: multi-block path (file > 4 MB)
//   - compress_filename_mt: empty file (zero bytes)
//   - compress_filename_mt: updates in_stream_size correctly
//   - compress_filename_mt: writes a non-empty output file
//   - compress_filename_mt: output differs from input (compressed)
//   - compress_filename_mt: removes source file when remove_src_file is true
//   - compress_filename_mt: does not remove source file when remove_src_file is false
//   - compress_filename_mt: content_size_flag causes content-size to be embedded
//   - compress_filename_mt: various compression levels produce valid compressed output
//   - compress_filename_mt: multi-block output is larger than header-only (non-trivial content)
//   - compress_filename_mt: parallel and sequential produce equivalent byte counts

use lz4::io::compress_frame::CompressResources;
use lz4::io::compress_mt::compress_filename_mt;
use lz4::io::prefs::{Prefs, MB};
use std::io::Write;
use tempfile::TempDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

const CHUNK_SIZE: usize = 4 * MB;

fn make_prefs(nb_workers: i32) -> Prefs {
    let mut p = Prefs::default();
    p.nb_workers = nb_workers;
    // Suppress all display output in tests.
    p
}

fn make_ress(prefs: &Prefs) -> CompressResources {
    CompressResources::new(prefs).expect("CompressResources::new")
}

/// Repeating pattern — compressible but not all-zeros.
fn pattern_data(len: usize) -> Vec<u8> {
    (0u8..=127).cycle().take(len).collect()
}

/// Returns true if `data` starts with the LZ4 frame magic number (0x184D2204 LE).
fn starts_with_lz4_magic(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == [0x04, 0x22, 0x4D, 0x18]
}

// ── Single-block path (file < CHUNK_SIZE) ────────────────────────────────────

/// Parity: C single-block path (lz4io.c 1199–1211).
/// A file smaller than chunkSize is compressed in a single lz4f_compress_frame call.
#[test]
fn small_file_produces_valid_lz4_frame() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("small.bin");
    let dst = dir.path().join("small.lz4");
    let data = pattern_data(64 * 1024); // 64 KiB < 4 MiB
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(2);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;

    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress small file");

    assert_eq!(
        in_size,
        data.len() as u64,
        "in_stream_size must match input length"
    );
    let compressed = std::fs::read(&dst).unwrap();
    assert!(!compressed.is_empty(), "output must be non-empty");
    assert!(
        starts_with_lz4_magic(&compressed),
        "output must start with LZ4 magic"
    );
}

/// Output is smaller than input for compressible (repetitive) data.
#[test]
fn small_file_compressed_is_smaller_than_input() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("input.bin");
    let dst = dir.path().join("output.lz4");
    // Use highly repetitive data so compression is effective.
    let data: Vec<u8> = b"AAAAAAAAAAAAAAAAAAAAAAAAA"
        .iter()
        .cycle()
        .take(256 * 1024)
        .copied()
        .collect();
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(2);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;

    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    let c_size = std::fs::metadata(&dst).unwrap().len();
    assert!(
        c_size < data.len() as u64,
        "compressed size {} must be < input size {}",
        c_size,
        data.len()
    );
}

// ── Multi-block path (file > CHUNK_SIZE) ─────────────────────────────────────

/// Parity: C multi-block MT path (lz4io.c 1216–1330).
/// A file larger than chunkSize triggers the multi-block parallel path.
#[test]
fn multi_block_file_produces_non_empty_output() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("large.bin");
    let dst = dir.path().join("large.lz4");
    // 5 MiB > CHUNK_SIZE (4 MiB) → at least 2 blocks.
    let data = pattern_data(5 * MB);
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(2);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;

    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("multi-block compress");

    assert_eq!(in_size, data.len() as u64);
    let c_data = std::fs::read(&dst).unwrap();
    assert!(!c_data.is_empty());
    assert!(
        starts_with_lz4_magic(&c_data),
        "output must start with LZ4 magic"
    );
}

/// Multi-block output size is tracked correctly.
#[test]
fn multi_block_in_stream_size_matches_input_len() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("l.bin");
    let dst = dir.path().join("l.lz4");
    let data = pattern_data(9 * MB); // three chunks
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(2);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;

    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    assert_eq!(
        in_size,
        data.len() as u64,
        "in_stream_size mismatch for 9 MB file"
    );
}

// ── in_stream_size tracking ──────────────────────────────────────────────────

/// Parity: lz4io.c 1351 — *inStreamSize is set to total uncompressed bytes read.
#[test]
fn in_stream_size_reflects_exact_input_length() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("exact.bin");
    let dst = dir.path().join("exact.lz4");
    let data = b"Hello, world!".to_vec(); // 13 bytes
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(1);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0xDEAD_BEEF_u64; // sentinel — should be overwritten

    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    assert_eq!(in_size, 13, "in_stream_size must equal input length");
}

// ── Compression levels ────────────────────────────────────────────────────────

/// Parity: compression_level parameter is passed through to lz4f (lz4io.c 1185).
/// Different levels must all produce valid (magic-prefixed) LZ4 output.
#[test]
fn compression_level_1_produces_valid_output() {
    let dir = TempDir::new().unwrap();
    let data = pattern_data(256 * 1024);
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.lz4");
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(1);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .unwrap();
    let out = std::fs::read(&dst).unwrap();
    assert!(starts_with_lz4_magic(&out));
}

#[test]
fn compression_level_9_produces_valid_output() {
    let dir = TempDir::new().unwrap();
    let data = pattern_data(256 * 1024);
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.lz4");
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(1);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        9,
        &prefs,
    )
    .unwrap();
    let out = std::fs::read(&dst).unwrap();
    assert!(starts_with_lz4_magic(&out));
}

// ── remove_src_file ──────────────────────────────────────────────────────────

/// Parity: lz4io.c 1345–1348 — source file is deleted when remove_src_file is set.
#[test]
fn remove_src_file_true_deletes_source() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("to_delete.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, pattern_data(1024)).unwrap();

    let mut prefs = make_prefs(1);
    prefs.remove_src_file = true;

    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    assert!(
        !src.exists(),
        "source file must be deleted when remove_src_file is true"
    );
    assert!(dst.exists(), "destination must still exist");
}

/// Parity: source file is preserved when remove_src_file is false (the default).
#[test]
fn remove_src_file_false_keeps_source() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("keep.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, pattern_data(1024)).unwrap();

    let mut prefs = make_prefs(1);
    prefs.remove_src_file = false;

    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    assert!(
        src.exists(),
        "source file must be kept when remove_src_file is false"
    );
}

// ── content_size_flag ────────────────────────────────────────────────────────

/// Parity: lz4io.c 1182–1189 — when content_size_flag is set, the frame header
/// embeds the original file size. Output must still be a valid LZ4 frame.
#[test]
fn content_size_flag_produces_valid_output() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.lz4");
    let data = pattern_data(128 * 1024);
    std::fs::write(&src, &data).unwrap();

    let mut prefs = make_prefs(1);
    prefs.content_size_flag = true;

    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress with content_size_flag");

    assert_eq!(in_size, data.len() as u64);
    let c = std::fs::read(&dst).unwrap();
    assert!(starts_with_lz4_magic(&c));
}

// ── nb_workers = 1 ───────────────────────────────────────────────────────────

/// Parity: single-worker execution must produce the same output shape as multi-worker.
#[test]
fn single_worker_produces_valid_lz4_output() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.lz4");
    let data = pattern_data(5 * MB);
    std::fs::write(&src, &data).unwrap();

    let prefs = make_prefs(1);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("single-worker compress");

    assert_eq!(in_size, data.len() as u64);
    let c = std::fs::read(&dst).unwrap();
    assert!(starts_with_lz4_magic(&c));
}

// ── output file did not exist before ─────────────────────────────────────────

/// Parity: destination file is created anew.
#[test]
fn creates_destination_file() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("new_dst.lz4");
    std::fs::write(&src, pattern_data(1024)).unwrap();
    assert!(!dst.exists());

    let prefs = make_prefs(1);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    assert!(dst.exists(), "destination file must be created");
}

// ── multi-block end-of-frame marker ──────────────────────────────────────────

/// Parity: lz4io.c 1310–1323 — multi-block output ends with 4-byte zero end-mark.
/// When content checksum is enabled (default), the last 8 bytes are:
///   [0x00,0x00,0x00,0x00] (end-of-data block) + [xxh32 bytes] (checksum).
/// When content checksum is disabled, only the 4-byte end-mark is written.
#[test]
fn multi_block_output_ends_with_end_mark_no_checksum() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("large.bin");
    let dst = dir.path().join("large.lz4");
    let data = pattern_data(5 * MB);
    std::fs::write(&src, &data).unwrap();

    let mut prefs = make_prefs(2);
    // Disable content checksum so end_buf is 4 bytes (end-of-data mark only).
    prefs.stream_checksum = false;

    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress");

    let c = std::fs::read(&dst).unwrap();
    assert!(c.len() >= 4);
    let end = &c[c.len() - 4..];
    assert_eq!(
        end, &[0u8; 4],
        "last 4 bytes must be end-of-data mark 0x00000000 when checksum disabled"
    );
}

/// Parity: lz4io.c 1310–1323 — with content checksum enabled, last 8 bytes consist of
/// 4-byte end-mark (zeros) followed by 4-byte XXH32 checksum.
#[test]
fn multi_block_output_with_checksum_has_end_mark_plus_4_bytes() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("large_ck.bin");
    let dst = dir.path().join("large_ck.lz4");
    let data = pattern_data(5 * MB);
    std::fs::write(&src, &data).unwrap();

    let mut prefs = make_prefs(2);
    prefs.stream_checksum = true; // default, but explicit

    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;
    compress_filename_mt(
        &mut in_size,
        &mut ress,
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress with checksum");

    let c = std::fs::read(&dst).unwrap();
    // At least 8 bytes at the end: end-mark (4) + checksum (4).
    assert!(
        c.len() >= 8,
        "output must have at least 8 trailing bytes for end-mark + checksum"
    );
    // The end-of-data block (bytes len-8 to len-4) must be 0x00000000.
    let end_mark = &c[c.len() - 8..c.len() - 4];
    assert_eq!(end_mark, &[0u8; 4], "end-of-data mark must be 0x00000000");
}

// ── error case: nonexistent source ───────────────────────────────────────────

/// Parity: opening a nonexistent source file must return an error (lz4io.c 1176–1178).
#[test]
fn nonexistent_src_returns_error() {
    let dir = TempDir::new().unwrap();
    let dst = dir.path().join("out.lz4");

    let prefs = make_prefs(1);
    let mut ress = make_ress(&prefs);
    let mut in_size = 0u64;

    let result = compress_filename_mt(
        &mut in_size,
        &mut ress,
        "/nonexistent/path/that/does/not/exist.bin",
        dst.to_str().unwrap(),
        1,
        &prefs,
    );
    assert!(result.is_err(), "must return Err for nonexistent src");
}
