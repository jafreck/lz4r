// Unit tests for task-018: src/io/decompress_frame.rs — LZ4 frame decompression
//
// Verifies behavioural parity with lz4io.c v1.10.0, lines 2015–2275
// (declarations #19, #20 — ST and MT LZ4IO_decompressLZ4F variants).
//
// Public API under test:
//   `lz4::io::decompress_frame::decompress_lz4f`

use lz4::io::decompress_frame::decompress_lz4f;
use lz4::io::decompress_resources::DecompressResources;
use lz4::io::prefs::Prefs;
use std::io::Write;

// ────────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────────

/// Compress `data` with the native frame API, returning the full frame bytes
/// (including the 4-byte magic number prefix).
fn compress_frame(data: &[u8]) -> Vec<u8> {
    lz4::frame::compress_frame_to_vec(data)
}

/// Strip the 4-byte magic prefix, returning `(magic_slice, body)`.
/// Callers pass `body` to `decompress_lz4f` to match the dispatcher convention
/// where the magic number is consumed before the function is called.
fn split_magic(frame: &[u8]) -> (&[u8], &[u8]) {
    assert!(frame.len() >= 4, "frame too short");
    (&frame[..4], &frame[4..])
}

/// Default single-threaded `Prefs`.
fn st_prefs() -> Prefs {
    Prefs::default()
}

/// Multi-threaded `Prefs` (nb_workers = 4).
fn mt_prefs() -> Prefs {
    let mut p = Prefs::default();
    p.nb_workers = 4;
    p
}

fn make_resources(prefs: &Prefs) -> DecompressResources {
    DecompressResources::new(prefs).expect("DecompressResources::new")
}

// ─────────────────────────────────────────────────────────────────────────────
// Basic round-trip — single-threaded path (nb_workers == 0)
// ─────────────────────────────────────────────────────────────────────────────

/// Mirrors C `LZ4IO_decompressLZ4F` #else (ST) branch.
/// Compressed bytes must decode byte-for-byte to the original content.
#[test]
fn round_trip_st() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &prefs, &mut res).unwrap();

    assert_eq!(n as usize, original.len(), "returned byte count must match");
    assert_eq!(output, original, "decompressed bytes must match original");
}

// ─────────────────────────────────────────────────────────────────────────────
// Basic round-trip — multi-threaded path (nb_workers > 1)
// ─────────────────────────────────────────────────────────────────────────────

/// The MT path uses the same ST algorithm (see migration note 3).
/// Output must be byte-identical to the ST path.
#[test]
fn round_trip_mt_byte_identical_to_st() {
    let original: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);

    // ST decompress
    let prefs_st = st_prefs();
    let mut res_st = make_resources(&prefs_st);
    let mut out_st = Vec::new();
    let n_st = decompress_lz4f(&mut &body[..], &mut out_st, &prefs_st, &mut res_st).unwrap();

    // MT decompress (same frame, different prefs)
    let prefs_mt = mt_prefs();
    let mut res_mt = make_resources(&prefs_mt);
    let mut out_mt = Vec::new();
    let n_mt = decompress_lz4f(&mut &body[..], &mut out_mt, &prefs_mt, &mut res_mt).unwrap();

    assert_eq!(n_st, n_mt, "ST and MT byte counts must match");
    assert_eq!(out_st, out_mt, "ST and MT output must be byte-identical");
    assert_eq!(out_st, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// test_mode — output must be discarded, byte count still returned
// ─────────────────────────────────────────────────────────────────────────────

/// Mirrors `if (!prefs->testMode)` guard in C (lz4io.c line 2248).
#[test]
fn test_mode_discards_output_returns_count() {
    let original: Vec<u8> = b"hello, test mode!".to_vec();
    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let mut prefs = st_prefs();
    prefs.test_mode = true;
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &prefs, &mut res).unwrap();

    assert_eq!(
        n as usize,
        original.len(),
        "byte count must be correct even in test mode"
    );
    assert!(output.is_empty(), "test_mode must not write to dst");
}

// ─────────────────────────────────────────────────────────────────────────────
// Empty frame — zero bytes decoded
// ─────────────────────────────────────────────────────────────────────────────

/// A frame encoding 0 bytes of content must return n == 0 and write nothing.
#[test]
fn empty_frame_returns_zero() {
    let frame = compress_frame(&[]);
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &prefs, &mut res).unwrap();

    assert_eq!(n, 0, "empty frame must decode to 0 bytes");
    assert!(output.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// Corrupt / truncated input — must return Err, not panic
// ─────────────────────────────────────────────────────────────────────────────

/// C equivalent: `END_PROCESS(68, "incomplete stream")` → in Rust this is a
/// propagated `io::Error` from `FrameDecoder::read()`.
#[test]
fn corrupt_input_returns_error() {
    // Garbage bytes after the magic number has been "consumed" by the caller.
    let garbage: &[u8] = b"\x00\x01\x02\x03\xFF\xFE\xFD\xFC";
    let mut src = garbage;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let result = decompress_lz4f(&mut src, &mut output, &prefs, &mut res);
    assert!(result.is_err(), "corrupt input must return Err");
}

/// A truncated (incomplete) valid frame must also return Err.
#[test]
fn truncated_frame_returns_error() {
    let original: Vec<u8> = (0u8..128).collect();
    let frame = compress_frame(&original);
    // Keep magic stripped, then truncate mid-stream.
    let truncated = &frame[4..frame.len() / 2];
    let mut src = truncated;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let result = decompress_lz4f(&mut src, &mut output, &prefs, &mut res);
    assert!(result.is_err(), "truncated frame must return Err");
}

// ─────────────────────────────────────────────────────────────────────────────
// Large frame — exercises the multi-read loop (> DECOMP_BUF_SIZE = 64 KiB)
// ─────────────────────────────────────────────────────────────────────────────

/// 256 KiB data causes multiple 64 KiB read iterations in the decompression
/// loop, equivalent to the C while-loop at lz4io.c lines 2224–2256.
#[test]
fn large_frame_round_trip() {
    let original: Vec<u8> = (0u8..=255)
        .cycle()
        .enumerate()
        .map(|(i, b)| b.wrapping_add((i >> 8) as u8))
        .take(256 * 1024)
        .collect();

    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &prefs, &mut res).unwrap();

    assert_eq!(n as usize, original.len(), "large frame byte count");
    assert_eq!(output, original, "large frame content");
}

// ─────────────────────────────────────────────────────────────────────────────
// Single-byte content
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn single_byte_round_trip() {
    let original = vec![0x42u8];
    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &prefs, &mut res).unwrap();

    assert_eq!(n, 1);
    assert_eq!(output, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// All-zeros content (highly compressible)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn all_zeros_round_trip() {
    let original = vec![0u8; 32 * 1024];
    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &prefs, &mut res).unwrap();

    assert_eq!(n as usize, original.len());
    assert_eq!(output, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 6: Dictionary-aware decompression path
// ─────────────────────────────────────────────────────────────────────────────

/// Dict-aware decompression: compress with dict, decompress with same dict.
/// Exercises lines 148, 182, 184, 192 (feed_to_decompressor_dict) and
/// lines 289, 298, 309, 314-317 (decompress_lz4f_st_dict).
#[test]
fn decompress_with_dictionary() {
    // Create a dict file
    let dict_dir = tempfile::tempdir().unwrap();
    let dict_path = dict_dir.path().join("test.dict");
    let dict_data: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    std::fs::write(&dict_path, &dict_data).unwrap();

    // Compress with the dict
    let src_dir = tempfile::tempdir().unwrap();
    let src_path = src_dir.path().join("dict_input.bin");
    let dst_path = src_dir.path().join("dict_input.lz4");
    let original: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src_path, &original).unwrap();

    let mut compress_prefs = Prefs::default();
    compress_prefs.use_dictionary = true;
    compress_prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_string());
    lz4::io::compress_frame::compress_filename(
        src_path.to_str().unwrap(),
        dst_path.to_str().unwrap(),
        1,
        &compress_prefs,
    )
    .expect("compress with dict must succeed");

    // Decompress with the same dict
    let frame = std::fs::read(&dst_path).unwrap();
    let (_, body) = split_magic(&frame);
    let mut src = body;

    let mut decompress_prefs = Prefs::default();
    decompress_prefs.use_dictionary = true;
    decompress_prefs.dictionary_filename = Some(dict_path.to_str().unwrap().to_string());
    let mut res = DecompressResources::from_prefs(&decompress_prefs).unwrap();
    let mut output = Vec::new();

    let n = decompress_lz4f(&mut src, &mut output, &decompress_prefs, &mut res).unwrap();
    assert_eq!(n as usize, original.len());
    assert_eq!(output, original);
}

/// Truncated frame → error (line 224).
#[test]
fn decompress_truncated_frame() {
    let original = vec![b'X'; 1000];
    let frame = compress_frame(&original);
    let (_, body) = split_magic(&frame);
    // Truncate: only give first 10 bytes of body
    let truncated = &body[..10.min(body.len())];
    let mut src = truncated;
    let prefs = st_prefs();
    let mut res = make_resources(&prefs);
    let mut output = Vec::new();
    let result = decompress_lz4f(&mut src, &mut output, &prefs, &mut res);
    assert!(result.is_err(), "truncated frame should return error");
}
