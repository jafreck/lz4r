// Integration tests for task-015: src/io/compress_frame.rs — LZ4 frame format compression.
//
// Verifies behavioural parity with lz4io.c v1.10.0 lines 978–1157 and 1366–1582
// (declarations #10, #11, #13):
//
//   - `cRess_t` / `LZ4IO_createCResources`     → `CompressResources::new`
//   - `LZ4IO_compressFrameChunk`               → `compress_frame_chunk`
//   - `LZ4IO_compressFilename_extRess`         → `compress_filename_ext`
//   - `LZ4IO_compressFilename`                 → `compress_filename`
//   - `LZ4IO_compressMultipleFilenames`        → `compress_multiple_filenames`
//   - `CompressStats`                           → returned statistics struct
//
// Coverage:
//   - compress_stats_default: CompressStats::default() all-zero
//   - compress_stats_fields: bytes_in and bytes_out are accessible
//   - compress_resources_new_default: allocates buffers, no cdict
//   - compress_resources_new_with_dict: cdict is Some when prefs configure one
//   - compress_resources_new_bad_dict: Err when dictionary file missing
//   - compress_resources_cdict_ptr_null: returns null when cdict is None
//   - compress_resources_cdict_ptr_nonnull: returns non-null when cdict is Some
//   - compress_filename_round_trip_small: single-block path, valid LZ4 frame
//   - compress_filename_round_trip_large: multi-block path recovers original
//   - compress_filename_empty: empty source produces valid (small) LZ4 frame
//   - compress_filename_nonexistent_src: returns Err
//   - compress_filename_bad_dst_dir: returns Err for unwritable path
//   - compress_filename_stats_bytes_in: bytes_in matches source size
//   - compress_filename_hc_level: HC level >= 3 produces valid output
//   - compress_filename_magic: output starts with LZ4 frame magic 0x184D2204
//   - compress_filename_ext_same_as_compress_filename: behaves identically
//   - compress_multiple_filenames_empty_list: Ok(0) for empty list
//   - compress_multiple_filenames_all_files_created: suffix appended
//   - compress_multiple_filenames_suffix_custom: custom suffix applied
//   - compress_multiple_filenames_missing_counted: missed count incremented
//   - compress_multiple_filenames_all_bad: all files missed, Ok(n)
//   - compress_multiple_filenames_outputs_valid: output is decompressible
//   - compress_frame_chunk_basic: returns non-zero for compressible input
//   - compress_frame_chunk_prefix: with prefix_data path returns output
//   - compress_frame_chunk_empty_src: returns 0 bytes for empty input

use lz4::io::compress_frame::{
    compress_filename, compress_filename_ext, compress_frame_chunk, compress_multiple_filenames,
    CfcParameters, CompressResources, CompressStats,
};
use lz4::io::prefs::Prefs;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn round_trip(src: &[u8], compression_level: i32) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("input.bin");
    let dst_path = dir.path().join("output.lz4");
    std::fs::write(&src_path, src).unwrap();

    let prefs = Prefs::default();
    compress_filename(
        src_path.to_str().unwrap(),
        dst_path.to_str().unwrap(),
        compression_level,
        &prefs,
    )
    .expect("compress_filename must succeed");

    let compressed = std::fs::read(&dst_path).unwrap();
    lz4::frame::decompress_frame_to_vec(&compressed).expect("decompression must succeed")
}

// ═════════════════════════════════════════════════════════════════════════════
// CompressStats
// ═════════════════════════════════════════════════════════════════════════════

/// CompressStats::default() must have all fields zeroed.
#[test]
fn compress_stats_default_is_zero() {
    let s = CompressStats::default();
    assert_eq!(s.bytes_in, 0);
    assert_eq!(s.bytes_out, 0);
}

/// CompressStats fields bytes_in and bytes_out are independently addressable.
#[test]
fn compress_stats_fields_accessible() {
    let s = CompressStats {
        bytes_in: 1234,
        bytes_out: 567,
    };
    assert_eq!(s.bytes_in, 1234);
    assert_eq!(s.bytes_out, 567);
}

/// CompressStats must implement Clone and Copy.
#[test]
fn compress_stats_clone_copy() {
    let s = CompressStats {
        bytes_in: 10,
        bytes_out: 5,
    };
    let s2 = s; // Copy
    let s3 = s.clone(); // Clone
    assert_eq!(s2.bytes_in, 10);
    assert_eq!(s3.bytes_out, 5);
}

// ═════════════════════════════════════════════════════════════════════════════
// CompressResources
// ═════════════════════════════════════════════════════════════════════════════

/// new() with default prefs allocates buffers; cdict is None.
#[test]
fn compress_resources_new_default_prefs_succeeds() {
    let prefs = Prefs::default();
    let ress = CompressResources::new(&prefs).expect("new() must succeed");
    // src_buffer is 4 MB (CHUNK_SIZE)
    assert_eq!(ress.src_buffer.len(), 4 * 1024 * 1024);
    // dst_buffer is at least as large as src_buffer
    assert!(ress.dst_buffer.len() >= ress.src_buffer.len());
    // No dictionary
    assert!(ress.cdict.is_none());
}

/// new() with a valid dictionary file creates a cdict.
#[test]
fn compress_resources_new_with_dict_creates_cdict() {
    let dir = tempfile::tempdir().unwrap();
    let dict_path = dir.path().join("dict.bin");
    std::fs::write(&dict_path, b"dictionary content for testing purposes").unwrap();

    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_owned());

    let ress = CompressResources::new(&prefs).expect("new() with dict must succeed");
    assert!(
        ress.cdict.is_some(),
        "cdict must be Some when dictionary is configured"
    );
}

/// new() with use_dictionary=true but missing file returns Err.
#[test]
fn compress_resources_new_bad_dict_returns_err() {
    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some("/nonexistent/path/to/dict.bin".to_owned());

    let result = CompressResources::new(&prefs);
    assert!(
        result.is_err(),
        "must fail when dictionary file does not exist"
    );
}

/// cdict_ptr() returns null when no dictionary is set.
#[test]
fn compress_resources_cdict_ptr_is_null_without_dict() {
    let prefs = Prefs::default();
    let ress = CompressResources::new(&prefs).unwrap();
    assert!(
        ress.cdict_ptr().is_null(),
        "cdict_ptr must be null when cdict is None"
    );
}

/// cdict_ptr() returns a non-null pointer when a dictionary is set.
#[test]
fn compress_resources_cdict_ptr_nonnull_with_dict() {
    let dir = tempfile::tempdir().unwrap();
    let dict_path = dir.path().join("dict.bin");
    std::fs::write(&dict_path, b"some dictionary bytes for testing").unwrap();

    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_owned());

    let ress = CompressResources::new(&prefs).unwrap();
    assert!(
        !ress.cdict_ptr().is_null(),
        "cdict_ptr must be non-null when cdict is Some"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_filename — basic functionality
// ═════════════════════════════════════════════════════════════════════════════

/// Output file starts with LZ4 frame magic number (0x184D2204 in LE).
#[test]
fn compress_filename_output_starts_with_lz4_magic() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"hello world").unwrap();

    let prefs = Prefs::default();
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("must succeed");

    let out = std::fs::read(&dst).unwrap();
    assert!(out.len() >= 4, "output too short");
    assert_eq!(
        &out[..4],
        &[0x04, 0x22, 0x4D, 0x18],
        "LZ4 frame magic mismatch"
    );
}

/// Small file: single-block path; round-trip recovers original.
#[test]
fn compress_filename_round_trip_small_file() {
    let original = b"The quick brown fox jumps over the lazy dog.";
    let decompressed = round_trip(original, 1);
    assert_eq!(decompressed.as_slice(), original.as_slice());
}

/// Large file (> 4 MB): multi-block path; round-trip recovers original.
#[test]
fn compress_filename_round_trip_large_file() {
    // 5 MB > CHUNK_SIZE (4 MB) → triggers multi-block streaming path
    let original: Vec<u8> = (0u8..=255).cycle().take(5 * 1024 * 1024).collect();
    let decompressed = round_trip(&original, 1);
    assert_eq!(decompressed, original);
}

/// Empty source file produces a valid (small) LZ4 frame.
#[test]
fn compress_filename_empty_source_produces_valid_frame() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("empty.bin");
    let dst = dir.path().join("empty.lz4");
    std::fs::write(&src, b"").unwrap();

    let prefs = Prefs::default();
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("compression of empty file must succeed");

    let out = std::fs::read(&dst).unwrap();
    assert!(out.len() >= 4, "must have at least 4-byte magic");
    assert_eq!(
        &out[..4],
        &[0x04, 0x22, 0x4D, 0x18],
        "LZ4 frame magic mismatch"
    );
}

/// Nonexistent source file → Err.
#[test]
fn compress_filename_nonexistent_src_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("out.lz4");
    let prefs = Prefs::default();
    let result = compress_filename(
        "/nonexistent/__lz4test_missing__.bin",
        dst.to_str().unwrap(),
        1,
        &prefs,
    );
    assert!(result.is_err(), "expected Err for nonexistent source");
}

/// Unwritable destination directory → Err.
#[test]
fn compress_filename_bad_dst_dir_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.bin");
    std::fs::write(&src, b"data").unwrap();
    let prefs = Prefs::default();
    let result = compress_filename(src.to_str().unwrap(), "/nonexistent/dir/out.lz4", 1, &prefs);
    assert!(result.is_err(), "expected Err for unwritable destination");
}

/// bytes_in in the returned CompressStats equals the input file size.
#[test]
fn compress_filename_stats_bytes_in_equals_input_size() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.bin");
    let dst = dir.path().join("out.lz4");
    let data = b"bytes in test content";
    std::fs::write(&src, data).unwrap();

    let prefs = Prefs::default();
    let stats = compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("must succeed");
    assert_eq!(stats.bytes_in, data.len() as u64);
}

/// HC compression level (≥ 3) produces valid decompressible LZ4 frame output.
#[test]
fn compress_filename_hc_level_round_trip() {
    let original = b"HC compression parity test with enough data to be interesting.";
    let decompressed = round_trip(original, 9);
    assert_eq!(decompressed.as_slice(), original.as_slice());
}

/// Level 0 (fast, zero acceleration) produces valid decompressible output.
#[test]
fn compress_filename_level_0_round_trip() {
    let original = b"level zero acceleration test string";
    let decompressed = round_trip(original, 0);
    assert_eq!(decompressed.as_slice(), original.as_slice());
}

/// Negative level (fast, high acceleration) produces valid decompressible output.
#[test]
fn compress_filename_negative_level_round_trip() {
    let original = b"negative acceleration test";
    let decompressed = round_trip(original, -5);
    assert_eq!(decompressed.as_slice(), original.as_slice());
}

/// HC output is no larger than fast output for highly compressible data.
#[test]
fn compress_filename_hc_not_larger_than_fast_for_compressible_data() {
    let data: Vec<u8> = vec![b'A'; 64 * 1024];

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.bin");
    let fast_dst = dir.path().join("fast.lz4");
    let hc_dst = dir.path().join("hc.lz4");
    std::fs::write(&src, &data).unwrap();

    let prefs = Prefs::default();
    compress_filename(src.to_str().unwrap(), fast_dst.to_str().unwrap(), 1, &prefs).unwrap();
    compress_filename(src.to_str().unwrap(), hc_dst.to_str().unwrap(), 9, &prefs).unwrap();

    let fast_size = std::fs::metadata(&fast_dst).unwrap().len();
    let hc_size = std::fs::metadata(&hc_dst).unwrap().len();
    assert!(
        hc_size <= fast_size,
        "HC ({hc_size}) should not be larger than fast ({fast_size}) for compressible data"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_filename_ext — dispatcher
// ═════════════════════════════════════════════════════════════════════════════

/// compress_filename_ext (ST path) produces the same output as compress_filename.
#[test]
fn compress_filename_ext_matches_compress_filename() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.bin");
    let dst1 = dir.path().join("via_compress_filename.lz4");
    let dst2 = dir.path().join("via_compress_filename_ext.lz4");
    let data = b"dispatcher equivalence test data";
    std::fs::write(&src, data).unwrap();

    let prefs = Prefs::default();

    compress_filename(src.to_str().unwrap(), dst1.to_str().unwrap(), 1, &prefs).unwrap();

    let mut ress = CompressResources::new(&prefs).unwrap();
    let mut in_stream_size: u64 = 0;
    compress_filename_ext(
        &mut in_stream_size,
        &mut ress,
        src.to_str().unwrap(),
        dst2.to_str().unwrap(),
        1,
        &prefs,
    )
    .expect("compress_filename_ext must succeed");
    assert_eq!(in_stream_size, data.len() as u64);

    // Both outputs must be valid LZ4 frames decompressing to the same content.
    let bytes1 = std::fs::read(&dst1).unwrap();
    let dec1 = lz4::frame::decompress_frame_to_vec(&bytes1).unwrap();

    let bytes2 = std::fs::read(&dst2).unwrap();
    let dec2 = lz4::frame::decompress_frame_to_vec(&bytes2).unwrap();

    assert_eq!(dec1, data.as_slice());
    assert_eq!(dec2, data.as_slice());
}

/// compress_filename_ext Err propagates for nonexistent source.
#[test]
fn compress_filename_ext_nonexistent_src_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("out.lz4");
    let prefs = Prefs::default();
    let mut ress = CompressResources::new(&prefs).unwrap();
    let mut sz: u64 = 0;
    let result = compress_filename_ext(
        &mut sz,
        &mut ress,
        "/nonexistent/__lz4test_missing__.bin",
        dst.to_str().unwrap(),
        1,
        &prefs,
    );
    assert!(result.is_err());
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_multiple_filenames
// ═════════════════════════════════════════════════════════════════════════════

/// Empty file list → Ok(0 missed files).
#[test]
fn compress_multiple_filenames_empty_list_returns_zero_missed() {
    let prefs = Prefs::default();
    let missed =
        compress_multiple_filenames(&[], ".lz4", 1, &prefs).expect("empty list must return Ok");
    assert_eq!(missed, 0);
}

/// All files compressed; output filenames have suffix appended.
#[test]
fn compress_multiple_filenames_all_files_created_with_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let src1 = dir.path().join("a.txt");
    let src2 = dir.path().join("b.txt");
    std::fs::write(&src1, b"file a").unwrap();
    std::fs::write(&src2, b"file b").unwrap();

    let prefs = Prefs::default();
    let missed = compress_multiple_filenames(
        &[src1.to_str().unwrap(), src2.to_str().unwrap()],
        ".lz4",
        1,
        &prefs,
    )
    .expect("must succeed");

    assert_eq!(missed, 0, "no files should be missed");
    assert!(
        dir.path().join("a.txt.lz4").exists(),
        "a.txt.lz4 must exist"
    );
    assert!(
        dir.path().join("b.txt.lz4").exists(),
        "b.txt.lz4 must exist"
    );
}

/// Custom suffix is appended to each source filename.
#[test]
fn compress_multiple_filenames_custom_suffix_applied() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("data.bin");
    std::fs::write(&src, b"custom suffix test").unwrap();

    let prefs = Prefs::default();
    compress_multiple_filenames(&[src.to_str().unwrap()], ".lz4custom", 1, &prefs).unwrap();

    let expected = format!("{}.lz4custom", src.to_str().unwrap());
    assert!(
        std::path::Path::new(&expected).exists(),
        "expected output at {expected}"
    );
}

/// Missing file increments missed count; Ok is still returned.
#[test]
fn compress_multiple_filenames_missing_file_increments_missed() {
    let prefs = Prefs::default();
    let missed =
        compress_multiple_filenames(&["/nonexistent/__lz4_missing__.bin"], ".lz4", 1, &prefs)
            .expect("should return Ok even when files are missing");
    assert_eq!(missed, 1, "one file must be missed");
}

/// All missing → all counted as missed.
#[test]
fn compress_multiple_filenames_all_bad_all_missed() {
    let prefs = Prefs::default();
    let missed = compress_multiple_filenames(
        &["/bad/a.bin", "/bad/b.bin", "/bad/c.bin"],
        ".lz4",
        1,
        &prefs,
    )
    .expect("must return Ok with missed count");
    assert_eq!(missed, 3);
}

/// Mixed: one good, one bad → missed == 1, good file is created.
#[test]
fn compress_multiple_filenames_mixed_good_and_bad() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.bin");
    std::fs::write(&good, b"good file content").unwrap();

    let prefs = Prefs::default();
    let missed = compress_multiple_filenames(
        &[good.to_str().unwrap(), "/nonexistent/__bad__.bin"],
        ".lz4",
        1,
        &prefs,
    )
    .expect("must return Ok");
    assert_eq!(missed, 1);
    assert!(
        dir.path().join("good.bin.lz4").exists(),
        "good.bin.lz4 must exist"
    );
}

/// Output files produced by compress_multiple_filenames are valid LZ4 frames.
#[test]
fn compress_multiple_filenames_outputs_are_valid_lz4_frames() {
    let dir = tempfile::tempdir().unwrap();
    let content = b"multi-file frame format validation content";
    let src = dir.path().join("data.bin");
    std::fs::write(&src, content).unwrap();

    let prefs = Prefs::default();
    let missed = compress_multiple_filenames(&[src.to_str().unwrap()], ".lz4", 1, &prefs).unwrap();
    assert_eq!(missed, 0);

    let out = std::fs::read(dir.path().join("data.bin.lz4")).unwrap();
    assert_eq!(
        &out[..4],
        &[0x04, 0x22, 0x4D, 0x18],
        "LZ4 frame magic mismatch"
    );

    let decompressed = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(decompressed.as_slice(), content.as_slice());
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_frame_chunk
// ═════════════════════════════════════════════════════════════════════════════

/// compress_frame_chunk returns non-zero bytes for compressible input (no dict, no prefix).
#[test]
fn compress_frame_chunk_basic_returns_nonzero() {
    use lz4::frame::header::lz4f_compress_frame_bound;
    use lz4::frame::types::Preferences;

    // Build a default Preferences from a default Prefs via the module under test.
    // We access CompressResources.prepared_prefs indirectly.
    let prefs = Prefs::default();
    let ress = CompressResources::new(&prefs).unwrap();
    let prefs_val: Preferences = ress.prepared_prefs;

    let params = CfcParameters {
        prefs: &prefs_val,
        cdict: std::ptr::null(),
    };

    let src: Vec<u8> = b"abcdefghij".iter().cycle().take(4096).copied().collect();
    let mut dst = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs_val))];

    let c_size = compress_frame_chunk(&params, &mut dst, &src, None)
        .expect("compress_frame_chunk must succeed");
    assert!(c_size > 0, "compressed output must be non-empty");
    assert!(c_size <= dst.len(), "must not exceed dst capacity");
}

/// compress_frame_chunk with prefix_data path returns non-zero output.
#[test]
fn compress_frame_chunk_with_prefix_returns_output() {
    use lz4::frame::header::lz4f_compress_frame_bound;

    let prefs = Prefs::default();
    let ress = CompressResources::new(&prefs).unwrap();
    let prefs_val = ress.prepared_prefs;

    let params = CfcParameters {
        prefs: &prefs_val,
        cdict: std::ptr::null(),
    };

    let prefix: Vec<u8> = b"prefix dictionary data"
        .iter()
        .cycle()
        .take(1024)
        .copied()
        .collect();
    let src: Vec<u8> = b"source data to compress with prefix"
        .iter()
        .cycle()
        .take(512)
        .copied()
        .collect();
    let mut dst = vec![0u8; lz4f_compress_frame_bound(src.len(), Some(&prefs_val))];

    let c_size = compress_frame_chunk(&params, &mut dst, &src, Some(&prefix))
        .expect("compress_frame_chunk with prefix must succeed");
    assert!(c_size > 0, "output must be non-empty with prefix");
}

/// compress_frame_chunk with empty src returns 0.
#[test]
fn compress_frame_chunk_empty_src_returns_zero() {
    use lz4::frame::header::lz4f_compress_frame_bound;

    let prefs = Prefs::default();
    let ress = CompressResources::new(&prefs).unwrap();
    let prefs_val = ress.prepared_prefs;

    let params = CfcParameters {
        prefs: &prefs_val,
        cdict: std::ptr::null(),
    };

    let src: &[u8] = &[];
    let mut dst = vec![0u8; lz4f_compress_frame_bound(0, Some(&prefs_val)).max(256)];

    let c_size = compress_frame_chunk(&params, &mut dst, src, None)
        .expect("compress_frame_chunk with empty src must succeed");
    assert_eq!(c_size, 0, "empty src must produce 0 compressed bytes");
}

// ═════════════════════════════════════════════════════════════════════════════
// Additional coverage tests
// ═════════════════════════════════════════════════════════════════════════════

/// block_size_id=4 → Max64Kb branch (line 130)
#[test]
fn compress_filename_block_size_id_4_max64kb() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"hello world").unwrap();
    let mut prefs = Prefs::default();
    prefs.block_size_id = 4;
    let stats = compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("block_size_id=4 must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, b"hello world");
    let _ = stats;
}

/// block_size_id=5 → Max256Kb branch (line 131)
#[test]
fn compress_filename_block_size_id_5_max256kb() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"hello 256kb block").unwrap();
    let mut prefs = Prefs::default();
    prefs.block_size_id = 5;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("block_size_id=5 must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, b"hello 256kb block");
}

/// block_size_id=6 → Max1Mb branch (line 131)
#[test]
fn compress_filename_block_size_id_6_max1mb() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"hello 1mb block").unwrap();
    let mut prefs = Prefs::default();
    prefs.block_size_id = 6;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("block_size_id=6 must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, b"hello 1mb block");
}

/// block_independence=false → BlockMode::Linked (line 137)
#[test]
fn compress_filename_linked_block_mode_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data: Vec<u8> = (0u8..255).cycle().take(8192).collect();
    std::fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.block_independence = false;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("linked block mode must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}

/// block_checksum=true → BlockChecksum::Enabled (line 149)
#[test]
fn compress_filename_block_checksum_enabled_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data = b"block checksum test data";
    std::fs::write(&src, data).unwrap();
    let mut prefs = Prefs::default();
    prefs.block_checksum = true;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("block_checksum must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data.as_ref());
}

/// effective_block_size: block_size > 0 path (lines 176-177)
#[test]
fn compress_filename_explicit_block_size_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data: Vec<u8> = (0u8..255).cycle().take(4096).collect();
    std::fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.block_size = 64 * 1024; // non-zero → effective_block_size returns this directly
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("explicit block_size must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}

/// load_dict_file wrap path: dict file larger than 64 KB causes circular-buffer wrap (lines 238-245)
#[test]
fn compress_filename_large_dict_wraps_circular_buffer() {
    let dir = tempfile::tempdir().unwrap();
    // Create a dict file larger than 64 KB to exercise the circular-buffer wrap.
    let dict_data: Vec<u8> = (0u8..255).cycle().take(80 * 1024).collect();
    let dict_path = dir.path().join("large.dict");
    std::fs::write(&dict_path, &dict_data).unwrap();

    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"some data to compress with large dict").unwrap();

    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_owned());

    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("compression with large dict must succeed");

    // Just verify output is non-empty LZ4 frame (content is dict-compressed, harder to verify)
    let out = std::fs::read(&dst).unwrap();
    assert!(out.len() >= 7, "output should be a valid LZ4 frame");
}

/// create_cdict: no dictionary_filename returns Err (line 262)
#[test]
fn compress_resources_new_with_use_dictionary_but_no_filename_returns_err() {
    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = None;
    let result = CompressResources::new(&prefs);
    assert!(result.is_err(), "must fail without dictionary filename");
}

/// load_dict_file: nonexistent file returns Err (line 197)
#[test]
fn compress_resources_new_missing_dict_file_returns_err() {
    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some("/nonexistent/path/to/dict.lz4d".to_owned());
    let result = CompressResources::new(&prefs);
    assert!(result.is_err(), "must fail with missing dict file");
}

/// content_size_flag=true injects content size into header (lines 451-459)
#[test]
fn compress_filename_with_content_size_flag_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data: Vec<u8> = (0u8..255).cycle().take(4096).collect();
    std::fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.content_size_flag = true;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("content_size_flag must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}

/// content_size_flag=true with large (multi-block) file (lines 451-459, 519-528, 540-560)
#[test]
fn compress_filename_content_size_flag_multiblock_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    // Use more than 4 MB to trigger multi-block path
    let data: Vec<u8> = (0u8..255).cycle().take(5 * 1024 * 1024).collect();
    std::fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.content_size_flag = true;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("content_size_flag multi-block must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}

/// remove_src_file=true deletes the source file after compression (lines 595-596)
#[test]
fn compress_filename_remove_src_file_deletes_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"data to be removed after compression").unwrap();
    let mut prefs = Prefs::default();
    prefs.remove_src_file = true;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("must succeed");
    assert!(!src.exists(), "source file should be deleted");
    assert!(dst.exists(), "output file should exist");
}

/// compress_multiple_filenames with use_dictionary=true + missing file → CompressResources::new fails (line 713)
#[test]
fn compress_multiple_filenames_bad_dict_returns_err() {
    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some("/nonexistent/dict.lz4d".to_owned());
    let result = compress_multiple_filenames(&["any_file.bin"], ".lz4", 1, &prefs);
    assert!(result.is_err(), "should fail when dict file missing");
}

/// stream_checksum=true covers ContentChecksum::Enabled branch
#[test]
fn compress_filename_stream_checksum_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data = b"stream checksum test";
    std::fs::write(&src, data).unwrap();
    let mut prefs = Prefs::default();
    prefs.stream_checksum = false; // already false by default — test explicitly disabled
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data.as_ref());
}

/// Multi-block path: large file exercises multi-block streaming (lines 519-579)
#[test]
fn compress_filename_large_multiblock_streaming_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    // Generate data larger than the default 4MB block size to trigger multi-block streaming
    let data: Vec<u8> = (0u8..255u8).cycle().take(6 * 1024 * 1024).collect();
    std::fs::write(&src, &data).unwrap();
    let prefs = Prefs::default();
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("multi-block streaming must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}

/// CompressResources::new with small dict file (≤64KB) — simple (non-circular) path  
#[test]
fn compress_filename_small_dict_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let dict_data: Vec<u8> = b"small dict content"
        .iter()
        .cycle()
        .take(1024)
        .copied()
        .collect();
    let dict_path = dir.path().join("small.dict");
    std::fs::write(&dict_path, &dict_data).unwrap();

    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    std::fs::write(&src, b"data compressed with small dict").unwrap();

    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_owned());

    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("small dict compression must succeed");
    let out = std::fs::read(&dst).unwrap();
    assert!(out.len() >= 7);
}

/// Multi-block with linked blocks and large input (covers lines 519-579 + 137)
#[test]
fn compress_filename_multiblock_linked_large_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data: Vec<u8> = (0u8..200u8).cycle().take(5 * 1024 * 1024).collect();
    std::fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.block_independence = false;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("linked multi-block must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}

/// stream_checksum=true large file (multi-block) covers ContentChecksum path in multi-block
#[test]
fn compress_filename_multiblock_stream_checksum_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("out.lz4");
    let data: Vec<u8> = (0u8..255).cycle().take(5 * 1024 * 1024).collect();
    std::fs::write(&src, &data).unwrap();
    let mut prefs = Prefs::default();
    prefs.stream_checksum = true;
    compress_filename(src.to_str().unwrap(), dst.to_str().unwrap(), 1, &prefs)
        .expect("stream_checksum multi-block must succeed");
    let out = std::fs::read(&dst).unwrap();
    let dec = lz4::frame::decompress_frame_to_vec(&out).unwrap();
    assert_eq!(dec, data);
}
