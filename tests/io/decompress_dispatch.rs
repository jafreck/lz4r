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

use lz4::io::decompress_dispatch::{decompress_filename, decompress_multiple_filenames, DecompressStats};
use lz4::io::prefs::{Prefs, LEGACY_BLOCKSIZE};
use lz4_flex::frame::FrameEncoder;
use std::fs;
use std::io::Write;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `data` into a valid LZ4 frame-format stream.
fn make_frame_stream(data: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::new();
    let mut encoder = FrameEncoder::new(&mut compressed);
    encoder.write_all(data).expect("encode");
    encoder.finish().expect("finish");
    compressed
}

/// Build a legacy-format LZ4 stream (magic + size-prefixed compressed blocks).
fn make_legacy_stream(data: &[u8]) -> Vec<u8> {
    const LEGACY_MAGICNUMBER: u32 = 0x184C2102;
    let mut stream = Vec::new();
    stream.extend_from_slice(&LEGACY_MAGICNUMBER.to_le_bytes());
    for chunk in data.chunks(LEGACY_BLOCKSIZE) {
        let compressed = lz4_flex::block::compress(chunk);
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
    let s = DecompressStats { decompressed_bytes: 42 };
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
    assert!(result.is_err(), "existing dst without overwrite must return error");
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

    assert_eq!(fs::read(&expected_dst).unwrap().as_slice(), original.as_ref());
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
    decompress_multiple_filenames(&src_strs, suffix, &prefs).expect("multiple files should succeed");

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
    assert!(result.is_err(), "wrong-extension file must cause an error return");
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

    assert_eq!(fs::read(&expected_dst).unwrap().as_slice(), original.as_ref());
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
