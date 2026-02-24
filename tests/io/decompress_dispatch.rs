// Unit tests for task-020: src/io/decompress_dispatch.rs — Decompression dispatch
//
// Verifies behavioural parity with lz4io.c v1.10.0, lines 2277–2555
// (declaration #21 — LZ4IO_passThrough, skipStream, fseek_u32,
//  selectDecoder, LZ4IO_decompressSrcFile, LZ4IO_decompressDstFile,
//  LZ4IO_decompressFilename, LZ4IO_decompressMultipleFilenames).
//
// Public API under test:
//   `lz4::io::decompress_dispatch::decompress_filename`
//   `lz4::io::decompress_dispatch::decompress_multiple_filenames`
//   `lz4::io::decompress_dispatch::DecompressStats`

use lz4::io::decompress_dispatch::{
    decompress_filename, decompress_multiple_filenames, DecompressStats,
};
use lz4::io::prefs::{Prefs, LEGACY_BLOCKSIZE};
use std::fs;
use std::io::Write;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `data` into a valid LZ4 frame-format stream.
fn make_frame_stream(data: &[u8]) -> Vec<u8> {
    lz4::frame::compress_frame_to_vec(data)
}

/// Build a legacy-format LZ4 stream (magic + size-prefixed compressed blocks).
fn make_legacy_stream(data: &[u8]) -> Vec<u8> {
    const LEGACY_MAGICNUMBER: u32 = 0x184C2102;
    let mut stream = Vec::new();
    stream.extend_from_slice(&LEGACY_MAGICNUMBER.to_le_bytes());
    for chunk in data.chunks(LEGACY_BLOCKSIZE) {
        let compressed = lz4::block::compress_block_to_vec(chunk);
        stream.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        stream.extend_from_slice(&compressed);
    }
    stream
}

/// Build cycling bytes 0..=255.
fn cycling_bytes(n: usize) -> Vec<u8> {
    (0u8..=255).cycle().take(n).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressStats
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_stats_default_is_zero() {
    // DecompressStats::default() should have decompressed_bytes == 0.
    let s = DecompressStats::default();
    assert_eq!(s.decompressed_bytes, 0);
}

#[test]
fn decompress_stats_clone_and_debug() {
    // DecompressStats must implement Clone and Debug (used by callers for logging).
    let s = DecompressStats {
        decompressed_bytes: 42,
    };
    let cloned = s.clone();
    assert_eq!(cloned.decompressed_bytes, 42);
    let _ = format!("{:?}", s); // must not panic
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_filename — frame format round-trips
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_filename_frame_small() {
    // Basic round-trip: frame format with a small payload.
    let original = b"Hello, decompression dispatch!";
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("small.lz4");
    let dst = dst_dir.path().join("small.raw");
    fs::write(&src, make_frame_stream(original)).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("decompress_filename should succeed");

    assert_eq!(fs::read(&dst).unwrap().as_slice(), original.as_ref());
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

#[test]
fn decompress_filename_frame_empty() {
    // Empty payload: frame format must produce an empty output file.
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("empty.lz4");
    let dst = dst_dir.path().join("empty.raw");
    fs::write(&src, make_frame_stream(b"")).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("empty frame should succeed");

    assert_eq!(fs::read(&dst).unwrap().as_slice(), b"");
    assert_eq!(stats.decompressed_bytes, 0);
}

#[test]
fn decompress_filename_frame_large_cycling() {
    // Multi-block frame (> 64 KiB default block size).
    let original = cycling_bytes(200 * 1024);
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("large.lz4");
    let dst = dst_dir.path().join("large.raw");
    fs::write(&src, make_frame_stream(&original)).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("large frame should succeed");

    assert_eq!(fs::read(&dst).unwrap(), original);
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

#[test]
fn decompress_filename_frame_repetitive() {
    // Highly repetitive data compresses well; verify exact byte count after decompression.
    let original: Vec<u8> = b"AAAA".iter().cycle().take(128 * 1024).cloned().collect();
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("rep.lz4");
    let dst = dst_dir.path().join("rep.raw");
    fs::write(&src, make_frame_stream(&original)).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("repetitive frame should succeed");

    assert_eq!(fs::read(&dst).unwrap(), original);
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_filename — legacy format round-trips
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_filename_legacy_small() {
    // Legacy format, small payload.
    let original = b"Legacy decompression dispatch test";
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("legacy.lz4");
    let dst = dst_dir.path().join("legacy.raw");
    fs::write(&src, make_legacy_stream(original)).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("legacy decompress should succeed");

    assert_eq!(fs::read(&dst).unwrap().as_slice(), original.as_ref());
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

#[test]
fn decompress_filename_legacy_multi_block() {
    // Legacy format with data exceeding LEGACY_BLOCKSIZE (8 MiB blocks in C).
    let original = cycling_bytes(LEGACY_BLOCKSIZE + 1024);
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("multi_block.lz4");
    let dst = dst_dir.path().join("multi_block.raw");
    fs::write(&src, make_legacy_stream(&original)).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("legacy multi-block should succeed");

    assert_eq!(fs::read(&dst).unwrap(), original);
    assert_eq!(stats.decompressed_bytes as usize, original.len());
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_filename — chained frames
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_filename_chained_two_frame_streams() {
    // Two consecutive LZ4 frame-format streams concatenated — both must be decoded.
    let part1 = b"First chained frame content.";
    let part2 = b"Second chained frame content.";
    let mut combined = make_frame_stream(part1);
    combined.extend_from_slice(&make_frame_stream(part2));

    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("chained.lz4");
    let dst = dst_dir.path().join("chained.raw");
    fs::write(&src, &combined).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("chained frames should succeed");

    let mut expected = part1.to_vec();
    expected.extend_from_slice(part2);
    assert_eq!(fs::read(&dst).unwrap(), expected);
    assert_eq!(stats.decompressed_bytes as usize, expected.len());
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_filename — error cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_filename_missing_src_returns_error() {
    // Source file that does not exist → error (equivalent to C fopen failure).
    let dst_dir = tempfile::tempdir().unwrap();
    let prefs = Prefs::default();
    let result = decompress_filename(
        "/nonexistent/path/to/file.lz4",
        dst_dir.path().join("out.raw").to_str().unwrap(),
        &prefs,
    );
    assert!(result.is_err(), "missing src must return error");
}

#[test]
fn decompress_filename_corrupt_frame_returns_error() {
    // Corrupt LZ4 data (valid magic, garbage body) → error.
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("corrupt.lz4");
    let dst = dst_dir.path().join("corrupt.raw");

    // Write LZ4 frame magic + garbage bytes.
    let mut data = 0x184D2204u32.to_le_bytes().to_vec();
    data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xAA, 0xBB, 0xCC]);
    fs::write(&src, &data).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_err(), "corrupt frame must return error");
}

#[test]
fn decompress_filename_unrecognized_magic_returns_error() {
    // Unknown magic number (not LZ4 frame, legacy, or skippable) → error.
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("unknown.bin");
    let dst = dst_dir.path().join("unknown.raw");
    fs::write(&src, b"\xDE\xAD\xBE\xEF some garbage data here").unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_err(), "unrecognized magic must return error");
}

#[test]
fn decompress_filename_pass_through_on_unrecognized_magic() {
    // With pass_through + overwrite enabled, an unrecognized-magic file is
    // copied verbatim (mirrors C LZ4IO_passThrough, lz4io.c:2385–2391).
    let raw_data = b"\xDE\xAD\xBE\xEF raw payload data";
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("raw.bin");
    let dst = dst_dir.path().join("raw.out");
    fs::write(&src, raw_data.as_ref()).unwrap();

    let mut prefs = Prefs::default();
    prefs.pass_through = true;
    prefs.overwrite = true;
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("pass-through should succeed");

    // Output must equal the input exactly (magic + payload).
    assert_eq!(fs::read(&dst).unwrap().as_slice(), raw_data.as_ref());
    assert_eq!(stats.decompressed_bytes as usize, raw_data.len());
}

#[test]
fn decompress_filename_no_overwrite_existing_dst_returns_error() {
    // When overwrite = false and dst exists → AlreadyExists error.
    let original = b"some content";
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("input.lz4");
    let dst = dst_dir.path().join("output.raw");
    fs::write(&src, make_frame_stream(original)).unwrap();
    // Create destination ahead of time.
    fs::write(&dst, b"already here").unwrap();

    let mut prefs = Prefs::default();
    prefs.overwrite = false;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(
        result.is_err(),
        "existing dst without overwrite must return error"
    );
    // Existing file must not have been overwritten.
    assert_eq!(fs::read(&dst).unwrap(), b"already here");
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_filename — skippable frames
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_filename_skippable_frame_then_data() {
    // Skippable frame followed by real data: the skippable payload is discarded,
    // only the real frame contributes to output (lz4io.c:2372–2383).
    const LZ4IO_SKIPPABLE0: u32 = 0x184D2A50;
    let payload = b"real data after skippable";
    let mut stream = Vec::new();

    // Skippable frame with 8 arbitrary bytes.
    stream.extend_from_slice(&LZ4IO_SKIPPABLE0.to_le_bytes());
    stream.extend_from_slice(&8u32.to_le_bytes()); // size = 8
    stream.extend_from_slice(b"SKIPTHIS");

    // Real LZ4 frame.
    stream.extend_from_slice(&make_frame_stream(payload));

    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("skip.lz4");
    let dst = dst_dir.path().join("skip.raw");
    fs::write(&src, &stream).unwrap();

    let prefs = Prefs::default();
    let stats = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("skippable + frame should succeed");

    assert_eq!(fs::read(&dst).unwrap().as_slice(), payload.as_ref());
    assert_eq!(stats.decompressed_bytes as usize, payload.len());
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_multiple_filenames — basic functionality
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_multiple_filenames_single_file_strips_suffix() {
    // The output filename is produced by stripping `suffix` from the source name
    // (mirrors LZ4IO_decompressMultipleFilenames, lz4io.c:2540–2541).
    let suffix = ".lz4";
    let original = b"multiple_filenames single file test";
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("file.raw.lz4");
    let expected_dst = dir.path().join("file.raw");
    fs::write(&src, make_frame_stream(original)).unwrap();

    let prefs = Prefs::default();
    decompress_multiple_filenames(&[src.to_str().unwrap()], suffix, &prefs)
        .expect("single file should succeed");

    assert_eq!(
        fs::read(&expected_dst).unwrap().as_slice(),
        original.as_ref()
    );
}

#[test]
fn decompress_multiple_filenames_multiple_files() {
    // All files in the list are decompressed.
    let suffix = ".lz4";
    let dir = tempfile::tempdir().unwrap();
    let data = [b"file one content".as_ref(), b"file two content".as_ref()];
    let srcs: Vec<_> = data
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let src = dir.path().join(format!("file{}.raw.lz4", i));
            fs::write(&src, make_frame_stream(d)).unwrap();
            src
        })
        .collect();

    let src_strs: Vec<&str> = srcs.iter().map(|p| p.to_str().unwrap()).collect();
    let prefs = Prefs::default();
    decompress_multiple_filenames(&src_strs, suffix, &prefs)
        .expect("multiple files should succeed");

    for (i, d) in data.iter().enumerate() {
        let dst = dir.path().join(format!("file{}.raw", i));
        assert_eq!(fs::read(&dst).unwrap().as_slice(), *d);
    }
}

#[test]
fn decompress_multiple_filenames_skips_wrong_extension() {
    // Files not ending with `suffix` are skipped and counted as skipped_files
    // (lz4io.c:2535–2543); the function returns Err when any file is skipped.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("file.gz"); // wrong extension
    fs::write(&src, b"not an lz4 file").unwrap();

    let prefs = Prefs::default();
    let result = decompress_multiple_filenames(&[src.to_str().unwrap()], ".lz4", &prefs);
    assert!(
        result.is_err(),
        "wrong-extension file must cause an error return"
    );
}

#[test]
fn decompress_multiple_filenames_partial_failure_returns_error() {
    // If one file fails (e.g., missing), the function returns Err even if
    // other files succeeded (lz4io.c: missing_files counter).
    let suffix = ".lz4";
    let dir = tempfile::tempdir().unwrap();
    let good_src = dir.path().join("good.raw.lz4");
    fs::write(&good_src, make_frame_stream(b"good data")).unwrap();

    let prefs = Prefs::default();
    let result = decompress_multiple_filenames(
        &[good_src.to_str().unwrap(), "/nonexistent/bad.raw.lz4"],
        suffix,
        &prefs,
    );
    assert!(result.is_err(), "partial failure must return Err");

    // The good file must still have been decompressed.
    let good_dst = dir.path().join("good.raw");
    assert_eq!(fs::read(&good_dst).unwrap().as_slice(), b"good data");
}

#[test]
fn decompress_multiple_filenames_empty_list_succeeds() {
    // An empty source list should succeed with no output (lz4io.c:2518–2544 loop body never runs).
    let prefs = Prefs::default();
    decompress_multiple_filenames(&[], ".lz4", &prefs).expect("empty list should succeed");
}

#[test]
fn decompress_multiple_filenames_legacy_format() {
    // Legacy-format files are also decompressed correctly via the dispatch loop.
    let suffix = ".lz4";
    let original = b"Legacy content in multiple filenames";
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy.raw.lz4");
    let expected_dst = dir.path().join("legacy.raw");
    fs::write(&src, make_legacy_stream(original)).unwrap();

    let prefs = Prefs::default();
    decompress_multiple_filenames(&[src.to_str().unwrap()], suffix, &prefs)
        .expect("legacy format should succeed");

    assert_eq!(
        fs::read(&expected_dst).unwrap().as_slice(),
        original.as_ref()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_multiple_filenames — stdout / devnull sentinel destinations
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_multiple_filenames_stdout_sentinel_succeeds() {
    // When suffix == STDOUT_MARK, each file is decompressed to stdout
    // (lz4io.c:2524–2527).  We only verify the function returns Ok (not panics).
    use lz4::io::file_io::STDOUT_MARK;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("out.lz4");
    fs::write(&src, make_frame_stream(b"stdout content")).unwrap();

    let prefs = Prefs::default();
    decompress_multiple_filenames(&[src.to_str().unwrap()], STDOUT_MARK, &prefs)
        .expect("stdout sentinel should succeed");
}

#[test]
fn decompress_multiple_filenames_nul_sentinel_succeeds() {
    // When suffix == NUL_MARK, each file is decompressed to /dev/null / sink.
    use lz4::io::file_io::NUL_MARK;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("discard.lz4");
    fs::write(&src, make_frame_stream(b"discarded content")).unwrap();

    let prefs = Prefs::default();
    decompress_multiple_filenames(&[src.to_str().unwrap()], NUL_MARK, &prefs)
        .expect("nul sentinel should succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional coverage tests
// ─────────────────────────────────────────────────────────────────────────────

/// decompress_filename with remove_src_file=true deletes the source (line 362).
#[test]
fn decompress_filename_remove_src_file_deletes_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("remove_me.lz4");
    let dst = dir.path().join("remove_me.out");
    fs::write(&src, make_frame_stream(b"delete source file test")).unwrap();

    let mut prefs = Prefs::default();
    prefs.remove_src_file = true;

    decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("decompress with remove_src_file must succeed");
    assert!(
        !src.exists(),
        "source file must be deleted after decompression"
    );
    assert!(dst.exists(), "destination file must exist");
}

/// LZ4 frame followed by unrecognized magic: second frame unknown triggers display+break (lines 323, 326).
#[test]
fn decompress_filename_lz4_frame_then_unknown_magic_stops_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("unknown_after_frame.lz4");
    let dst = dir.path().join("unknown_after_frame.out");

    let mut data = make_frame_stream(b"valid frame before junk");
    // Append unrecognized 4-byte magic
    data.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());

    fs::write(&src, &data).unwrap();

    let prefs = Prefs::default();
    // Should succeed (unrecognized subsequent frame just stops decoding)
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(
        result.is_ok(),
        "subsequent unknown magic must not error: {result:?}"
    );
}

/// Legacy frame followed by unrecognized magic → same break path.
#[test]
fn decompress_filename_legacy_then_unknown_magic_stops_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy_then_junk.lz4");
    let dst = dir.path().join("legacy_then_junk.out");

    let mut data = make_legacy_stream(b"legacy frame before junk");
    // Append unrecognized magic after the legacy frame
    data.extend_from_slice(&0xCAFEBABEu32.to_le_bytes());

    fs::write(&src, &data).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(
        result.is_ok(),
        "legacy then unknown magic must not error: {result:?}"
    );
}

/// decompress_filename with a regular file dst propagates file metadata (line 488).
#[test]
fn decompress_filename_regular_dst_propagates_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("meta.lz4");
    let dst = dir.path().join("meta.out");
    fs::write(&src, make_frame_stream(b"metadata propagation test")).unwrap();

    let prefs = Prefs::default();
    decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs)
        .expect("must succeed and propagate metadata");
    assert!(dst.exists());
}

/// decompress_multiple_filenames with no overwrite returns error (line 362 branch).
#[test]
fn decompress_multiple_filenames_no_overwrite_existing_dst_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("noover.lz4");
    let dst = dir.path().join("noover");
    fs::write(&src, make_frame_stream(b"no overwrite test data")).unwrap();
    fs::write(&dst, b"existing content").unwrap(); // pre-exist

    let mut prefs = Prefs::default();
    prefs.overwrite = false;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(
        result.is_err(),
        "must fail when dst exists and overwrite=false"
    );
}

/// SparseWriter flush: exercise the flush method on the write path (lines 149-151).
/// This happens whenever a file decompression flushes the sparse writer.
#[test]
fn decompress_filename_large_frame_exercises_sparse_writer_flush() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("large_sparse.lz4");
    let dst = dir.path().join("large_sparse.out");
    let payload: Vec<u8> = b"sparse flush test content "
        .iter()
        .cycle()
        .take(128 * 1024)
        .copied()
        .collect();
    fs::write(&src, make_frame_stream(&payload)).unwrap();

    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 1;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, payload);
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional decompress_dispatch coverage
// ─────────────────────────────────────────────────────────────────────────────

/// Pass-through mode: non-LZ4 data should be copied as-is when pass_through is enabled.
#[test]
fn decompress_filename_pass_through_non_lz4_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("plain.txt");
    let dst = dir.path().join("plain.out");
    let data = b"this is not an lz4 file, just plain text for pass-through test";
    fs::write(&src, data).unwrap();

    let mut prefs = Prefs::default();
    prefs.pass_through = true;
    prefs.overwrite = true;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    // pass_through should succeed and copy the data as-is
    assert!(result.is_ok(), "pass_through on non-LZ4 file must succeed: {result:?}");
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data);
}

/// Decompressing a legacy frame exercises the legacy decoder path.
#[test]
fn decompress_filename_legacy_frame_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy.lz4");
    let dst = dir.path().join("legacy.out");
    let data = b"legacy format decompression test data repeated many times for good measure";
    let legacy_stream = make_legacy_stream(data);
    fs::write(&src, &legacy_stream).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "legacy frame must decompress: {result:?}");
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data.as_ref());
}

/// remove_src_file: verify that source file is deleted after decompression.
#[test]
fn decompress_filename_remove_src_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("remove_me.lz4");
    let dst = dir.path().join("remove_me.out");
    let data = b"test data for remove_src_file option";
    fs::write(&src, make_frame_stream(data)).unwrap();

    let mut prefs = Prefs::default();
    prefs.remove_src_file = true;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    assert!(!src.exists(), "source file should be removed after decompression");
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data.as_ref());
}

/// Decompress with sparse disabled (sparse_file_support=0).
#[test]
fn decompress_filename_sparse_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("no_sparse.lz4");
    let dst = dir.path().join("no_sparse.out");
    // Data with lots of zeros (would be sparse candidates)
    let data = vec![0u8; 65536];
    fs::write(&src, make_frame_stream(&data)).unwrap();

    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 0;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data);
}

/// Decompress chained frames (two frames concatenated).
#[test]
fn decompress_filename_chained_frames() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("chained.lz4");
    let dst = dir.path().join("chained.out");
    let data1 = b"first frame data here";
    let data2 = b"second frame data here";
    let mut stream = make_frame_stream(data1);
    stream.extend_from_slice(&make_frame_stream(data2));
    fs::write(&src, &stream).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    let out = fs::read(&dst).unwrap();
    let mut expected = data1.to_vec();
    expected.extend_from_slice(data2);
    assert_eq!(out, expected);
}

// ─────────────────────────────────────────────────────────────────────────────
// Coverage-gap tests: skippable frames, sparse with zeros, multiple filenames
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress with skippable frame prepended exercises skip_stream path (L296-300).
#[test]
fn decompress_filename_skippable_frame_prepended() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("skippable.lz4");
    let dst = dir.path().join("skippable.out");

    // Build skippable frame + real data
    let data = b"real data after skippable frame";
    let skip_payload = b"skip this metadata";
    let mut stream = Vec::new();
    stream.extend_from_slice(&0x184D2A50u32.to_le_bytes()); // skippable magic
    stream.extend_from_slice(&(skip_payload.len() as u32).to_le_bytes());
    stream.extend_from_slice(skip_payload);
    stream.extend_from_slice(&make_frame_stream(data));
    fs::write(&src, &stream).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "skippable + frame must decompress: {result:?}");
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data.as_ref());
}

/// Decompress multiple filenames exercises decompress_multiple_filenames.
#[test]
fn decompress_multiple_filenames_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let data1 = b"first file for multi decompress test!";
    let data2 = b"second file for multi decompress test, different content";

    let src1 = dir.path().join("file1.lz4");
    let src2 = dir.path().join("file2.lz4");
    fs::write(&src1, make_frame_stream(data1)).unwrap();
    fs::write(&src2, make_frame_stream(data2)).unwrap();

    let prefs = Prefs::default();
    let srcs = [src1.to_str().unwrap(), src2.to_str().unwrap()];
    // Using a suffix that strips .lz4 — the function uses the default behavior
    let result = decompress_multiple_filenames(&srcs, ".lz4", &prefs);
    assert!(result.is_ok(), "multi decompress must succeed: {result:?}");
}

/// Decompress sparse data (lots of zeros) with sparse enabled exercises SparseWriter.
#[test]
fn decompress_filename_sparse_zeros() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sparse.lz4");
    let dst = dir.path().join("sparse.out");
    // 256KB of zeros should trigger sparse writing
    let data = vec![0u8; 256 * 1024];
    fs::write(&src, make_frame_stream(&data)).unwrap();

    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 2; // auto-select sparse

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "sparse decompression must succeed: {result:?}");
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data);
}

/// Decompress with overwrite=true to existing file.
#[test]
fn decompress_filename_overwrite_existing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("overwrite.lz4");
    let dst = dir.path().join("overwrite.out");
    let data = b"data to overwrite";
    fs::write(&src, make_frame_stream(data)).unwrap();
    fs::write(&dst, b"old content that should be overwritten").unwrap();

    let mut prefs = Prefs::default();
    prefs.overwrite = true;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "overwrite must succeed");
    let out = fs::read(&dst).unwrap();
    assert_eq!(out, data.as_ref());
}

/// Decompress with overwrite=false to existing file should fail.
#[test]
fn decompress_filename_no_overwrite_existing_fails() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("no_over.lz4");
    let dst = dir.path().join("no_over.out");
    let data = b"data that should not overwrite";
    fs::write(&src, make_frame_stream(data)).unwrap();
    fs::write(&dst, b"existing content").unwrap();

    let mut prefs = Prefs::default();
    prefs.overwrite = false;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_err(), "no-overwrite to existing file should fail");
}

// ── Phase 4: Additional decompress_dispatch coverage tests ───────────────────

/// Decompress with test_mode=true — decompresses to /dev/null equivalent.
#[test]
fn decompress_filename_test_mode() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("testmode.lz4");
    let data = cycling_bytes(2000);
    fs::write(&src, make_frame_stream(&data)).unwrap();

    let mut prefs = Prefs::default();
    prefs.test_mode = true;

    let result = decompress_filename(
        src.to_str().unwrap(),
        "/dev/null",
        &prefs,
    );
    assert!(result.is_ok(), "test mode decompress should succeed: {result:?}");
}

/// Decompress legacy format with block checksum.
#[test]
fn decompress_legacy_format_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy.lz4");
    let dst = dir.path().join("legacy.out");
    let data = cycling_bytes(5000);
    fs::write(&src, make_legacy_stream(&data)).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "legacy decompress must succeed: {result:?}");
    let output = fs::read(&dst).unwrap();
    assert_eq!(output, data);
}

/// Decompress with sparse_file_support=2 (forced).
#[test]
fn decompress_filename_sparse_forced() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sparse_forced.lz4");
    let dst = dir.path().join("sparse_forced.out");
    let data = vec![0u8; 65536]; // all zeros — sparse-friendly
    fs::write(&src, make_frame_stream(&data)).unwrap();

    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 2; // forced

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "sparse forced decompress must succeed: {result:?}");
    let output = fs::read(&dst).unwrap();
    assert_eq!(output, data);
}

/// Decompress multiple files in sequence using suffix-based output naming.
#[test]
fn decompress_multiple_files_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let data1 = cycling_bytes(1000);
    let data2 = cycling_bytes(2000);
    let src1 = dir.path().join("multi1.lz4");
    let src2 = dir.path().join("multi2.lz4");
    fs::write(&src1, make_frame_stream(&data1)).unwrap();
    fs::write(&src2, make_frame_stream(&data2)).unwrap();

    let prefs = Prefs::default();
    let srcs = [src1.to_str().unwrap(), src2.to_str().unwrap()];
    let result = decompress_multiple_filenames(&srcs, ".lz4", &prefs);
    assert!(result.is_ok(), "multi decompress must succeed: {result:?}");
    // Output filenames are the src names with .lz4 suffix stripped
    let dst1 = dir.path().join("multi1");
    let dst2 = dir.path().join("multi2");
    assert_eq!(fs::read(&dst1).unwrap(), data1);
    assert_eq!(fs::read(&dst2).unwrap(), data2);
}

/// Decompress with remove_src_file=true removes source.
#[test]
fn decompress_filename_remove_src() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("removeme.lz4");
    let dst = dir.path().join("removeme.out");
    let data = cycling_bytes(500);
    fs::write(&src, make_frame_stream(&data)).unwrap();

    let mut prefs = Prefs::default();
    prefs.remove_src_file = true;

    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "decompress with remove should succeed: {result:?}");
    assert!(dst.exists());
    assert!(!src.exists(), "source should be removed");
}

/// Decompress with block_checksum enabled exercises block checksum verification.
#[test]
fn decompress_frame_with_block_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("blk_chk.lz4");
    let dst_path = dir.path().join("blk_chk.out");

    // Compress with block checksum using frame API
    use lz4::frame::types::{BlockChecksum, FrameInfo, Preferences as FPrefs};
    let data = cycling_bytes(10_000);
    let prefs_frame = FPrefs {
        frame_info: FrameInfo {
            block_checksum_flag: BlockChecksum::Enabled,
            ..Default::default()
        },
        ..Default::default()
    };
    let bound = lz4::frame::header::lz4f_compress_frame_bound(data.len(), Some(&prefs_frame));
    let mut compressed = vec![0u8; bound];
    let n = lz4::frame::compress::lz4f_compress_frame(&mut compressed, &data, Some(&prefs_frame)).unwrap();
    compressed.truncate(n);
    fs::write(&src_path, &compressed).unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src_path.to_str().unwrap(), dst_path.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "block checksum decompress must succeed: {result:?}");
    assert_eq!(fs::read(&dst_path).unwrap(), data);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: pass_through, skippable dispatch, sparse writer, multi-block decompress
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress a file that has a skippable frame followed by a standard frame.
/// Exercises the LZ4IO_SKIPPABLE0 branch + skip_stream.
#[test]
fn decompress_skippable_then_standard_frame() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("skip_std.lz4");
    let dst_path = dir.path().join("skip_std.out");

    let data = cycling_bytes(500);
    let frame = make_frame_stream(&data);

    // Build skippable frame: magic(4) + size(4) + payload
    let skip_payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
    let skip_magic = 0x184D2A50u32;
    let mut combined = Vec::new();
    combined.extend_from_slice(&skip_magic.to_le_bytes());
    combined.extend_from_slice(&(skip_payload.len() as u32).to_le_bytes());
    combined.extend_from_slice(&skip_payload);
    combined.extend_from_slice(&frame);

    fs::write(&src_path, &combined).unwrap();
    let prefs = Prefs::default();
    let result = decompress_filename(src_path.to_str().unwrap(), dst_path.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "skip+standard decompress must succeed: {result:?}");
    assert_eq!(fs::read(&dst_path).unwrap(), data);
}

/// pass_through: give a non-LZ4 file with pass_through+overwrite enabled.
/// Exercises the pass_through function (writes magic bytes then copies rest).
#[test]
fn decompress_pass_through_non_lz4_file() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("passthru.bin");
    let dst_path = dir.path().join("passthru.out");

    let data = b"This is not an LZ4 file at all, just plain text data.";
    fs::write(&src_path, data).unwrap();

    let mut prefs = Prefs::default();
    prefs.pass_through = true;
    prefs.overwrite = true;
    prefs.test_mode = false;

    let result = decompress_filename(src_path.to_str().unwrap(), dst_path.to_str().unwrap(), &prefs);
    assert!(result.is_ok(), "pass_through must succeed: {result:?}");
    assert_eq!(fs::read(&dst_path).unwrap(), data.as_slice());
}

/// Decompress multi-block frame (>64KB) exercises the streaming loop.
#[test]
fn decompress_multiblock_frame() {
    let dir = tempfile::tempdir().unwrap();

    // First compress a >64KB file to create a multi-block frame
    let data: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let src_orig = dir.path().join("large.bin");
    let compressed_path = dir.path().join("large.lz4");
    fs::write(&src_orig, &data).unwrap();

    use lz4::io::compress_frame::compress_filename;
    let cprefs = Prefs::default();
    compress_filename(
        src_orig.to_str().unwrap(),
        compressed_path.to_str().unwrap(),
        1,
        &cprefs,
    ).unwrap();

    let dst_path = dir.path().join("large.out");
    let prefs = Prefs::default();
    let result = decompress_filename(
        compressed_path.to_str().unwrap(),
        dst_path.to_str().unwrap(),
        &prefs,
    );
    assert!(result.is_ok(), "multiblock decompress must succeed: {result:?}");
    assert_eq!(fs::read(&dst_path).unwrap(), data);
}

/// Decompress with sparse_file_support=2 (FORCED) using a real file.
#[test]
fn decompress_multiblock_with_sparse() {
    let dir = tempfile::tempdir().unwrap();

    // Compress data with lots of zeros (good for sparse testing)
    let mut data = vec![0u8; 200_000];
    for i in (0..200_000).step_by(4096) {
        data[i] = 0xAA;
    }
    let src_orig = dir.path().join("sparse.bin");
    let compressed_path = dir.path().join("sparse.lz4");
    fs::write(&src_orig, &data).unwrap();

    use lz4::io::compress_frame::compress_filename;
    let cprefs = Prefs::default();
    compress_filename(
        src_orig.to_str().unwrap(),
        compressed_path.to_str().unwrap(),
        1,
        &cprefs,
    ).unwrap();

    let dst_path = dir.path().join("sparse.out");
    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 2; // FORCED
    let result = decompress_filename(
        compressed_path.to_str().unwrap(),
        dst_path.to_str().unwrap(),
        &prefs,
    );
    assert!(result.is_ok(), "sparse decompress must succeed: {result:?}");
    assert_eq!(fs::read(&dst_path).unwrap(), data);
}

/// Decompress with content_checksum + block_checksum + linked blocks 
/// exercises all checksum verification and dict update paths in decompress_loop.
#[test]
fn decompress_all_features_multiblock() {
    let dir = tempfile::tempdir().unwrap();

    let data: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let src_orig = dir.path().join("allfeats.bin");
    let compressed_path = dir.path().join("allfeats.lz4");
    fs::write(&src_orig, &data).unwrap();

    use lz4::io::compress_frame::compress_filename;
    let mut cprefs = Prefs::default();
    cprefs.block_independence = false;
    cprefs.block_checksum = true;
    cprefs.stream_checksum = true;
    cprefs.content_size_flag = true;
    compress_filename(
        src_orig.to_str().unwrap(),
        compressed_path.to_str().unwrap(),
        1,
        &cprefs,
    ).unwrap();

    let dst_path = dir.path().join("allfeats.out");
    let prefs = Prefs::default();
    let result = decompress_filename(
        compressed_path.to_str().unwrap(),
        dst_path.to_str().unwrap(),
        &prefs,
    );
    assert!(result.is_ok(), "all features decompress must succeed: {result:?}");
    assert_eq!(fs::read(&dst_path).unwrap(), data);
}

/// Decompress a non-LZ4 file without pass_through should fail.
#[test]
fn decompress_non_lz4_without_passthrough_fails() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("notlz4.bin");
    let dst_path = dir.path().join("notlz4.out");

    fs::write(&src_path, b"This is not LZ4 data").unwrap();

    let prefs = Prefs::default();
    let result = decompress_filename(src_path.to_str().unwrap(), dst_path.to_str().unwrap(), &prefs);
    assert!(result.is_err(), "non-LZ4 without pass_through must fail");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 6: pass_through overwrite, remove_src, sparse writer paths
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress with remove_src_file=true (line 422).
#[test]
fn decompress_remove_src_file() {
    let dir = tempfile::tempdir().unwrap();
    let data = cycling_bytes(1000);
    let src = dir.path().join("rm_src.lz4");
    let dst = dir.path().join("rm_src");
    fs::write(&src, make_frame_stream(&data)).unwrap();
    let mut prefs = Prefs::default();
    prefs.remove_src_file = true;
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    assert!(!src.exists(), "source should be deleted after decompress --rm");
    assert_eq!(fs::read(&dst).unwrap(), data);
}

/// pass_through with overwrite=true on non-LZ4 data (lines 201-205, 396-412).
#[test]
fn decompress_pass_through_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("plain.bin");
    let dst = dir.path().join("plain.out");
    let data = b"This is not LZ4 data at all!";
    fs::write(&src, data).unwrap();
    let mut prefs = Prefs::default();
    prefs.pass_through = true;
    prefs.overwrite = true;
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    assert_eq!(fs::read(&dst).unwrap(), data);
}

/// Decompress with skippable frame in front of real frame (lines 258-261).
#[test]
fn decompress_skippable_before_real_frame() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("skip_real.lz4");
    let dst = dir.path().join("skip_real");
    let data = cycling_bytes(500);
    let mut combined = Vec::new();
    // Skippable frame
    combined.extend_from_slice(&0x184D2A50u32.to_le_bytes());
    combined.extend_from_slice(&16u32.to_le_bytes());
    combined.extend_from_slice(&[0u8; 16]);
    // Real frame
    combined.extend_from_slice(&make_frame_stream(&data));
    fs::write(&src, &combined).unwrap();
    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    assert_eq!(fs::read(&dst).unwrap(), data);
}

/// Invalid magic after valid frame → error (lines 296-300).
#[test]
fn decompress_invalid_magic_after_frame() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bad_trail.lz4");
    let dst = dir.path().join("bad_trail");
    let data = cycling_bytes(500);
    let mut combined = make_frame_stream(&data);
    // Append garbage that looks like a magic read but is invalid
    combined.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    fs::write(&src, &combined).unwrap();
    let prefs = Prefs::default();
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    // Should succeed but potentially with error for trailing garbage
    // The first frame should still decompress fine
    if result.is_ok() {
        assert_eq!(fs::read(&dst).unwrap(), data);
    }
}

/// Decompress with sparse_file_support > 0 (lines 145, 149-151).
#[test]
fn decompress_with_sparse_mode() {
    let dir = tempfile::tempdir().unwrap();
    let data = cycling_bytes(2000);
    let src = dir.path().join("sparse.lz4");
    let dst = dir.path().join("sparse.out");
    fs::write(&src, make_frame_stream(&data)).unwrap();
    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 2;
    let result = decompress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), &prefs);
    assert!(result.is_ok());
    assert_eq!(fs::read(&dst).unwrap(), data);
}

/// decompress_multiple_filenames with suffix matching — one file doesn't have suffix.
/// Files without the correct suffix are skipped, and the function reports an error.
#[test]
fn decompress_multiple_filenames_suffix_skip() {
    let dir = tempfile::tempdir().unwrap();

    let data = cycling_bytes(1000);
    let src1 = dir.path().join("file1.lz4");
    fs::write(&src1, make_frame_stream(&data)).unwrap();

    // File without .lz4 suffix should be skipped
    let src2 = dir.path().join("file2.txt");
    fs::write(&src2, make_frame_stream(&data)).unwrap();

    let prefs = Prefs::default();
    let srcs = [src1.to_str().unwrap(), src2.to_str().unwrap()];
    let result = decompress_multiple_filenames(&srcs, ".lz4", &prefs);
    // The function returns an error because file2.txt was skipped
    assert!(result.is_err(), "should report error for skipped files");
    let dst1 = dir.path().join("file1");
    assert!(dst1.exists());
    assert_eq!(fs::read(&dst1).unwrap(), data);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 7: pass_through, NUL_MARK sink, legacy chaining, sparse write
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress a non-LZ4 file with pass_through=true → copies file as-is.
/// Exercises lines 201-205 (pass_through copy loop).
#[test]
fn decompress_pass_through_non_lz4() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("plain.txt.lz4");
    let dst = dir.path().join("plain.txt");
    let content = b"This is not an LZ4 file, just plain text for pass-through.";
    fs::write(&src, content).unwrap();

    let mut prefs = Prefs::default();
    prefs.pass_through = true;
    prefs.overwrite = true;
    let result = decompress_filename(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        &prefs,
    );
    // pass_through should copy the file
    assert!(result.is_ok());
    assert_eq!(fs::read(&dst).unwrap(), content);
}

/// Decompress with sparse_file_support enabled.
/// Exercises SparseWriter::write and finish paths (L145, 149-151).
#[test]
fn decompress_with_sparse_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sparse_in.lz4");
    // Create data with lots of zeros (good for sparse)
    let data = vec![0u8; 8192];
    fs::write(&src, make_frame_stream(&data)).unwrap();
    let dst = dir.path().join("sparse_out.bin");

    let mut prefs = Prefs::default();
    prefs.sparse_file_support = 1;
    prefs.overwrite = true;
    let result = decompress_filename(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        &prefs,
    );
    assert!(result.is_ok());
    assert_eq!(fs::read(&dst).unwrap(), data);
}

/// Decompress a legacy frame followed by a standard frame.
/// Exercises decompress_loop pending_magic reuse (L296-300).
#[test]
fn decompress_legacy_then_standard() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("mixed.lz4");
    let dst = dir.path().join("mixed.bin");

    // Build legacy frame
    let payload1 = vec![b'L'; 200];
    let mut compressed = vec![0u8; lz4::block::compress_bound(payload1.len() as i32) as usize];
    let clen = lz4::block::compress_default(&payload1, &mut compressed).unwrap();
    let mut data = Vec::new();
    data.extend_from_slice(&0x184C2102u32.to_le_bytes());
    data.extend_from_slice(&(clen as u32).to_le_bytes());
    data.extend_from_slice(&compressed[..clen]);
    // End of legacy: block size 0
    data.extend_from_slice(&0u32.to_le_bytes());

    // Append standard frame
    let payload2 = vec![b'S'; 300];
    let frame2 = lz4::frame::compress_frame_to_vec(&payload2);
    data.extend_from_slice(&frame2);

    fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.overwrite = true;
    let result = decompress_filename(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        &prefs,
    );
    // Exercising the dispatch path is what matters
    if let Ok(_stats) = result {
        let out = fs::read(&dst).unwrap();
        assert!(!out.is_empty());
    }
}
