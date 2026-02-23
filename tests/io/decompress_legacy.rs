// Unit tests for task-017: src/io/decompress_legacy.rs — decode_legacy_stream
//
// Verifies behavioural parity with lz4io.c v1.10.0, lines 1677–1887
// (declarations #15, #16):
//   - `g_magicRead` global → replaced by `next_magic: Option<u32>` return value
//   - `LZ4IO_decodeLegacyStream` ST variant (lines 1825–1873)
//   - `LZ4IO_decodeLegacyStream` MT variant (lines 1741–1821)
//
// All tests exercise the public API only:
//   `lz4::io::decompress_legacy::decode_legacy_stream`

use lz4::io::decompress_legacy::decode_legacy_stream;
use lz4::io::decompress_resources::DecompressResources;
use lz4::io::prefs::{Prefs, LEGACY_BLOCKSIZE};
use std::io::Cursor;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a `DecompressResources` using default `Prefs` (no dictionary).
fn make_resources() -> DecompressResources {
    DecompressResources::new(&Prefs::default()).expect("DecompressResources::new should not fail")
}

/// Returns default ST prefs (nb_workers = 0).
fn st_prefs() -> Prefs {
    Prefs::default()
}

/// Returns MT prefs (nb_workers = 2).
fn mt_prefs() -> Prefs {
    let mut p = Prefs::default();
    p.nb_workers = 2;
    p
}

/// Compresses `data` in legacy format (4-byte LE block sizes + raw lz4 blocks)
/// using `lz4::block::compress_block_to_vec`.  Does NOT prepend the magic number —
/// callers receive the raw block payload that `decode_legacy_stream` expects.
///
/// Mirrors the stream format produced by `LZ4IO_compressFilename_Legacy`.
fn make_legacy_payload(data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    for chunk in data.chunks(LEGACY_BLOCKSIZE) {
        let compressed = lz4::block::compress_block_to_vec(chunk);
        let block_size = compressed.len() as u32;
        payload.extend_from_slice(&block_size.to_le_bytes());
        payload.extend_from_slice(&compressed);
    }
    payload
}

/// A 4-byte value that exceeds `LZ4_compressBound(LEGACY_BLOCKSIZE)` so it is
/// interpreted by `decode_legacy_stream` as a next-stream magic number.
const LZ4_FRAME_MAGIC: u32 = 0x184D_2204;

// ═════════════════════════════════════════════════════════════════════════════
// Single-threaded path (prefs.nb_workers == 0)
// ═════════════════════════════════════════════════════════════════════════════

/// Parity: ST path decompresses a small single-block stream correctly.
/// Mirrors `LZ4IO_decodeLegacyStream` ST (lz4io.c:1841–1862).
#[test]
fn st_decompress_single_block() {
    let original = b"Hello, legacy LZ4 decompression!";
    let payload = make_legacy_payload(original);

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("ST decompress should succeed");

    assert_eq!(out, original.as_ref());
    assert_eq!(size, original.len() as u64);
    assert!(magic.is_none(), "clean EOF: no next magic");
}

/// Parity: ST path with empty input returns (0, None) — clean EOF immediately.
/// Mirrors the `if (sizeCheck == 0) break;` at lz4io.c:1841.
#[test]
fn st_empty_input_returns_zero_and_no_magic() {
    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) =
        decode_legacy_stream(&mut Cursor::new(b""), &mut out, &prefs, &res)
            .expect("empty ST stream should succeed");

    assert_eq!(size, 0);
    assert!(out.is_empty());
    assert!(magic.is_none());
}

/// Parity: ST path handles multiple blocks (data larger than LEGACY_BLOCKSIZE).
/// Each LEGACY_BLOCKSIZE chunk is compressed separately; all must be reconstructed.
#[test]
fn st_decompress_multiple_blocks() {
    // Build data that spans multiple LEGACY_BLOCKSIZE chunks.
    let original: Vec<u8> = (0u8..=255).cycle().take(LEGACY_BLOCKSIZE * 3 + 100).collect();
    let payload = make_legacy_payload(&original);

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("ST multi-block decompress should succeed");

    assert_eq!(out, original);
    assert_eq!(size, original.len() as u64);
    assert!(magic.is_none());
}

/// Parity: ST path returns `Some(magic)` when it reads a value exceeding
/// `LZ4_compressBound(LEGACY_BLOCKSIZE)` — the "next stream" case.
/// Mirrors lz4io.c:1847–1850 and the `g_magicRead` global replacement.
#[test]
fn st_detects_next_magic_number() {
    let original = b"data before next frame";
    let mut payload = make_legacy_payload(original);
    // Append a fake "next frame" magic number.
    payload.extend_from_slice(&LZ4_FRAME_MAGIC.to_le_bytes());

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("ST stream with chained magic should succeed");

    assert_eq!(out, original.as_ref());
    assert_eq!(size, original.len() as u64);
    assert_eq!(magic, Some(LZ4_FRAME_MAGIC));
}

/// Parity: ST path with a stream that is ONLY a next-magic value (no data blocks)
/// returns (0, Some(magic)).
#[test]
fn st_magic_only_stream_returns_zero_bytes_and_magic() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&LZ4_FRAME_MAGIC.to_le_bytes());

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("magic-only stream should succeed");

    assert_eq!(size, 0);
    assert!(out.is_empty());
    assert_eq!(magic, Some(LZ4_FRAME_MAGIC));
}

/// Parity: ST path returns an error for corrupted compressed data.
/// Mirrors `LZ4_decompress_safe` → `Err(InvalidData)` path (lz4io.c:1858–1866).
#[test]
fn st_corrupted_block_data_returns_error() {
    let mut payload = Vec::new();
    // 10-byte block with all-garbage content.
    payload.extend_from_slice(&10u32.to_le_bytes());
    payload.extend_from_slice(&[0xFF; 10]);

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let result = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res);

    assert!(result.is_err(), "corrupted ST block should return Err");
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

/// Parity: ST path returns an error when the stream is truncated mid-block
/// (header promises N bytes but only M < N are present).
#[test]
fn st_truncated_mid_block_returns_error() {
    let original = b"some data to compress";
    let compressed = lz4::block::compress_block_to_vec(original);
    // Promise the full block size but only supply half the bytes.
    let block_size = compressed.len() as u32;
    let mut payload = Vec::new();
    payload.extend_from_slice(&block_size.to_le_bytes());
    payload.extend_from_slice(&compressed[..compressed.len() / 2]); // truncated!

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let result = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res);

    assert!(result.is_err(), "truncated ST block should return Err");
}

/// Parity: ST path with highly-compressible data (all zeros) decompresses
/// correctly and the decoded output equals the original.
#[test]
fn st_all_zeros_decompresses_correctly() {
    let original = vec![0u8; 4096];
    let payload = make_legacy_payload(&original);

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("all-zeros decompress should succeed");

    assert_eq!(out, original);
    assert_eq!(size, original.len() as u64);
    assert!(magic.is_none());
}

/// Parity: ST path with incompressible data (random-like) decompresses correctly.
#[test]
fn st_incompressible_data_decompresses_correctly() {
    // Use cycling byte pattern — not easily compressible.
    let original: Vec<u8> = (0u8..=255).cycle().take(512).collect();
    let payload = make_legacy_payload(&original);

    let prefs = st_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, _) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("incompressible ST decompress should succeed");

    assert_eq!(out, original);
    assert_eq!(size, 512);
}

// ═════════════════════════════════════════════════════════════════════════════
// Multi-threaded path (prefs.nb_workers > 1)
// ═════════════════════════════════════════════════════════════════════════════

/// Parity: MT path decompresses a small single-block stream correctly.
#[test]
fn mt_decompress_single_block() {
    let original = b"Hello, MT legacy LZ4!";
    let payload = make_legacy_payload(original);

    let prefs = mt_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("MT decompress should succeed");

    assert_eq!(out, original.as_ref());
    assert_eq!(size, original.len() as u64);
    assert!(magic.is_none());
}

/// Parity: MT path with empty input returns (0, None).
#[test]
fn mt_empty_input_returns_zero_and_no_magic() {
    let prefs = mt_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) =
        decode_legacy_stream(&mut Cursor::new(b""), &mut out, &prefs, &res)
            .expect("empty MT stream should succeed");

    assert_eq!(size, 0);
    assert!(out.is_empty());
    assert!(magic.is_none());
}

/// Parity: MT path handles multiple blocks (> NB_BUFFSETS=4 blocks) correctly,
/// ensuring multi-batch behaviour is exercised.
#[test]
fn mt_decompress_more_than_nb_buffsets_blocks() {
    // Produce 6 blocks (2 full batches of NB_BUFFSETS=4 would be 8; 6 crosses boundary).
    let block_data: Vec<u8> = vec![0xA5u8; 64];
    let mut payload = Vec::new();
    for _ in 0..6 {
        let compressed = lz4::block::compress_block_to_vec(&block_data);
        payload.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        payload.extend_from_slice(&compressed);
    }

    let prefs = mt_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("MT multi-batch decompress should succeed");

    let expected: Vec<u8> = block_data.iter().cloned().cycle().take(6 * 64).collect();
    assert_eq!(out, expected);
    assert_eq!(size, 6 * 64);
    assert!(magic.is_none());
}

/// Parity: MT path returns `Some(magic)` when encountering a next-stream magic
/// number within a batch — mirrors lz4io.c:1777–1780.
#[test]
fn mt_detects_next_magic_number() {
    let original = b"MT data before next frame";
    let mut payload = make_legacy_payload(original);
    payload.extend_from_slice(&LZ4_FRAME_MAGIC.to_le_bytes());

    let prefs = mt_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let (size, magic) = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
        .expect("MT chained frame should succeed");

    assert_eq!(out, original.as_ref());
    assert_eq!(size, original.len() as u64);
    assert_eq!(magic, Some(LZ4_FRAME_MAGIC));
}

/// Parity: MT path returns an error for corrupted compressed data.
#[test]
fn mt_corrupted_block_data_returns_error() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&10u32.to_le_bytes());
    payload.extend_from_slice(&[0xFF; 10]);

    let prefs = mt_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let result = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res);

    assert!(result.is_err(), "corrupted MT block should return Err");
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

/// Parity: MT path returns an error when the stream is truncated mid-block.
#[test]
fn mt_truncated_mid_block_returns_error() {
    let original = b"mt data to compress";
    let compressed = lz4::block::compress_block_to_vec(original);
    let block_size = compressed.len() as u32;
    let mut payload = Vec::new();
    payload.extend_from_slice(&block_size.to_le_bytes());
    payload.extend_from_slice(&compressed[..compressed.len() / 2]);

    let prefs = mt_prefs();
    let res = make_resources();
    let mut out = Vec::new();
    let result = decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res);

    assert!(result.is_err(), "truncated MT block should return Err");
}

// ═════════════════════════════════════════════════════════════════════════════
// ST / MT parity
// ═════════════════════════════════════════════════════════════════════════════

/// Parity: ST and MT paths produce identical output for the same input stream.
/// Covers the core migration concern that the rayon-batch MT implementation is
/// equivalent to the C `TPool` pipeline.
#[test]
fn st_and_mt_produce_identical_output_small() {
    let original: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
    let payload = make_legacy_payload(&original);
    let res = make_resources();

    let mut out_st = Vec::new();
    let (sz_st, mag_st) = decode_legacy_stream(
        &mut Cursor::new(&payload),
        &mut out_st,
        &st_prefs(),
        &res,
    )
    .unwrap();

    let mut out_mt = Vec::new();
    let (sz_mt, mag_mt) = decode_legacy_stream(
        &mut Cursor::new(&payload),
        &mut out_mt,
        &mt_prefs(),
        &res,
    )
    .unwrap();

    assert_eq!(out_st, out_mt, "ST and MT output must match");
    assert_eq!(sz_st, sz_mt, "ST and MT decoded sizes must match");
    assert_eq!(mag_st, mag_mt, "ST and MT next_magic must match");
}

/// Parity: ST and MT produce identical output for data spanning multiple batches
/// (exercises the multi-batch loop in the MT path).
#[test]
fn st_and_mt_produce_identical_output_multi_batch() {
    // 10 blocks ensures 3 batches (4 + 4 + 2) in the MT path.
    let block: Vec<u8> = vec![0x7Bu8; 128];
    let mut payload = Vec::new();
    for _ in 0..10 {
        let compressed = lz4::block::compress_block_to_vec(&block);
        payload.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        payload.extend_from_slice(&compressed);
    }
    let res = make_resources();

    let mut out_st = Vec::new();
    let (sz_st, _) = decode_legacy_stream(
        &mut Cursor::new(&payload),
        &mut out_st,
        &st_prefs(),
        &res,
    )
    .unwrap();

    let mut out_mt = Vec::new();
    let (sz_mt, _) = decode_legacy_stream(
        &mut Cursor::new(&payload),
        &mut out_mt,
        &mt_prefs(),
        &res,
    )
    .unwrap();

    assert_eq!(out_st, out_mt);
    assert_eq!(sz_st, sz_mt);
    assert_eq!(sz_st, 10 * 128);
}

/// Parity: ST and MT agree on the next_magic when a chained frame follows data.
#[test]
fn st_and_mt_agree_on_chained_frame_magic() {
    let original = b"chained frame parity test";
    let mut payload = make_legacy_payload(original);
    payload.extend_from_slice(&LZ4_FRAME_MAGIC.to_le_bytes());
    let res = make_resources();

    let mut out_st = Vec::new();
    let (_, mag_st) = decode_legacy_stream(
        &mut Cursor::new(&payload),
        &mut out_st,
        &st_prefs(),
        &res,
    )
    .unwrap();

    let mut out_mt = Vec::new();
    let (_, mag_mt) = decode_legacy_stream(
        &mut Cursor::new(&payload),
        &mut out_mt,
        &mt_prefs(),
        &res,
    )
    .unwrap();

    assert_eq!(mag_st, Some(LZ4_FRAME_MAGIC));
    assert_eq!(mag_mt, Some(LZ4_FRAME_MAGIC));
    assert_eq!(out_st, out_mt);
}

// ═════════════════════════════════════════════════════════════════════════════
// nb_workers boundary — dispatch routing
// ═════════════════════════════════════════════════════════════════════════════

/// nb_workers == 1 must route to the ST path (same as nb_workers == 0).
#[test]
fn nb_workers_one_routes_to_st_path() {
    let original = b"nb_workers=1 should use ST";
    let payload = make_legacy_payload(original);
    let res = make_resources();

    let mut prefs = Prefs::default();
    prefs.nb_workers = 1;

    let mut out = Vec::new();
    let (size, magic) =
        decode_legacy_stream(&mut Cursor::new(&payload), &mut out, &prefs, &res)
            .expect("nb_workers=1 should succeed");

    assert_eq!(out, original.as_ref());
    assert_eq!(size, original.len() as u64);
    assert!(magic.is_none());
}

/// nb_workers == 4 must produce the same output as nb_workers == 0 (ST).
#[test]
fn nb_workers_four_matches_st_output() {
    let original: Vec<u8> = (0u8..128).collect();
    let payload = make_legacy_payload(&original);
    let res = make_resources();

    let mut out_st = Vec::new();
    decode_legacy_stream(&mut Cursor::new(&payload), &mut out_st, &st_prefs(), &res).unwrap();

    let mut prefs4 = Prefs::default();
    prefs4.nb_workers = 4;
    let mut out_mt4 = Vec::new();
    decode_legacy_stream(&mut Cursor::new(&payload), &mut out_mt4, &prefs4, &res).unwrap();

    assert_eq!(out_st, out_mt4);
}
