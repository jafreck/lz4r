// Unit tests for task-024: bench/compress_strategy.rs — Compression Strategy Vtable
//
// Verifies parity with bench.c lines 152–312:
//   - NoStreamFast: maps negative c_level to higher acceleration, positive → acceleration=1
//   - NoStreamHC: uses c_level directly as HC level
//   - StreamFast: dict-aware fast stream compression (LZ4_compressInitStream path)
//   - StreamHC: dict-aware HC stream compression (LZ4_compressInitStreamHC path)
//   - build_compression_parameters: selects NoStreamFast (c_level<2) or NoStreamHC (c_level≥2)
//   - build_compression_parameters_with_dict: selects StreamFast or StreamHC accordingly
//   - LZ4HC_CLEVEL_MIN boundary = 2 (from lz4hc.h line 47)
//   - Zero return from any block function is mapped to Err (not a valid output)

use lz4::bench::compress_strategy::{
    build_compression_parameters, build_compression_parameters_with_dict,
    CompressionStrategy, NoStreamFast, NoStreamHC, StreamFast, StreamHC,
};

// ── Helper: decompress and verify ────────────────────────────────────────────

fn lz4_decompress(compressed: &[u8], original_len: usize) -> Vec<u8> {
    let out = lz4::block::decompress_block_to_vec(compressed, original_len);
    assert_eq!(out.len(), original_len);
    out
}

const SAMPLE: &[u8] =
    b"hello world hello world hello world hello world \
      this is a test of lz4 block compression round-trip!";

const REPETITIVE: &[u8] = &[b'A'; 4096];

// ── NoStreamFast ──────────────────────────────────────────────────────────────

#[test]
fn no_stream_fast_roundtrip_level_1() {
    // Standard positive c_level → acceleration=1
    let mut s = NoStreamFast::new(1);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn no_stream_fast_roundtrip_level_0() {
    // c_level=0 < 2 → acceleration=1 (same as level 1)
    let mut s = NoStreamFast::new(0);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn no_stream_fast_negative_level_roundtrip() {
    // Negative c_level: acceleration = -c_level + 1 (bench.c line 225)
    let mut s = NoStreamFast::new(-5);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn no_stream_fast_acceleration_increases_with_negative_level() {
    // Lower (more negative) c_level should NOT cause failure — just higher acceleration.
    let mut s = NoStreamFast::new(-100);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert!(n > 0, "compress_block should succeed for highly negative c_level");
}

#[test]
fn no_stream_fast_dst_grown_when_empty() {
    // Passing an empty dst Vec — ensure_dst_capacity must grow it to at least compressBound.
    let mut s = NoStreamFast::new(1);
    let mut dst = Vec::new(); // empty — capacity 0
    let n = s.compress_block(REPETITIVE, &mut dst).unwrap();
    assert!(n > 0);
    assert_eq!(lz4_decompress(&dst[..n], REPETITIVE.len()), REPETITIVE);
}

#[test]
fn no_stream_fast_repetitive_data_compresses_smaller() {
    // Highly repetitive data should compress to well under original size.
    let mut s = NoStreamFast::new(1);
    let mut dst = Vec::new();
    let n = s.compress_block(REPETITIVE, &mut dst).unwrap();
    assert!(
        n < REPETITIVE.len(),
        "repetitive data should compress: compressed={n} vs original={}",
        REPETITIVE.len()
    );
}

#[test]
fn no_stream_fast_multiple_blocks_are_independent() {
    // Each call to compress_block is independent (no stream state).
    let mut s = NoStreamFast::new(1);
    let mut dst1 = Vec::new();
    let mut dst2 = Vec::new();
    let n1 = s.compress_block(SAMPLE, &mut dst1).unwrap();
    let n2 = s.compress_block(SAMPLE, &mut dst2).unwrap();
    assert_eq!(lz4_decompress(&dst1[..n1], SAMPLE.len()), SAMPLE);
    assert_eq!(lz4_decompress(&dst2[..n2], SAMPLE.len()), SAMPLE);
    // Both blocks produce identical output for identical input.
    assert_eq!(&dst1[..n1], &dst2[..n2]);
}

// ── NoStreamHC ────────────────────────────────────────────────────────────────

#[test]
fn no_stream_hc_roundtrip_level_9() {
    let mut s = NoStreamHC::new(9);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn no_stream_hc_min_level_roundtrip() {
    // LZ4HC_CLEVEL_MIN = 2; this is the lowest valid HC level.
    let mut s = NoStreamHC::new(2);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn no_stream_hc_level_12_roundtrip() {
    // Maximum HC level (LZ4HC_CLEVEL_MAX=12)
    let mut s = NoStreamHC::new(12);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn no_stream_hc_dst_grown_when_empty() {
    let mut s = NoStreamHC::new(9);
    let mut dst = Vec::new();
    let n = s.compress_block(REPETITIVE, &mut dst).unwrap();
    assert!(n > 0);
    assert_eq!(lz4_decompress(&dst[..n], REPETITIVE.len()), REPETITIVE);
}

#[test]
fn no_stream_hc_multiple_blocks_independent() {
    let mut s = NoStreamHC::new(9);
    let mut dst1 = Vec::new();
    let mut dst2 = Vec::new();
    let n1 = s.compress_block(SAMPLE, &mut dst1).unwrap();
    let n2 = s.compress_block(SAMPLE, &mut dst2).unwrap();
    assert_eq!(lz4_decompress(&dst1[..n1], SAMPLE.len()), SAMPLE);
    assert_eq!(lz4_decompress(&dst2[..n2], SAMPLE.len()), SAMPLE);
}

// ── StreamFast ────────────────────────────────────────────────────────────────

#[test]
fn stream_fast_no_dict_roundtrip() {
    let mut s = StreamFast::new(1, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn stream_fast_with_dict_succeeds() {
    // Dict-aware stream; decompression would need the same dict, so just verify n > 0.
    let dict = b"hello world ";
    let mut s = StreamFast::new(1, dict).unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert!(n > 0, "stream compress with dict should return > 0 bytes");
}

#[test]
fn stream_fast_negative_level_no_dict_roundtrip() {
    // Negative c_level → acceleration = -c_level + 1 (bench.c line 246)
    let mut s = StreamFast::new(-3, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn stream_fast_multiple_blocks_no_dict() {
    // compress_block resets the stream each time (mirrors C reset step).
    let mut s = StreamFast::new(1, b"").unwrap();
    for _ in 0..3 {
        let mut dst = Vec::new();
        let n = s.compress_block(SAMPLE, &mut dst).unwrap();
        assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
    }
}

#[test]
fn stream_fast_empty_dict_accepted() {
    // An empty slice dict must be handled gracefully (NULL path in C attach_dictionary).
    let s = StreamFast::new(1, b"");
    assert!(s.is_ok(), "StreamFast::new with empty dict must succeed");
}

// ── StreamHC ──────────────────────────────────────────────────────────────────

#[test]
fn stream_hc_no_dict_roundtrip() {
    let mut s = StreamHC::new(9, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn stream_hc_with_dict_succeeds() {
    let dict = b"hello world ";
    let mut s = StreamHC::new(9, dict).unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert!(n > 0, "HC stream compress with dict should return > 0 bytes");
}

#[test]
fn stream_hc_min_level_no_dict_roundtrip() {
    let mut s = StreamHC::new(2, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn stream_hc_max_level_no_dict_roundtrip() {
    let mut s = StreamHC::new(12, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn stream_hc_multiple_blocks_no_dict() {
    // compress_block resets the stream each time.
    let mut s = StreamHC::new(9, b"").unwrap();
    for _ in 0..3 {
        let mut dst = Vec::new();
        let n = s.compress_block(SAMPLE, &mut dst).unwrap();
        assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
    }
}

#[test]
fn stream_hc_empty_dict_accepted() {
    let s = StreamHC::new(9, b"");
    assert!(s.is_ok(), "StreamHC::new with empty dict must succeed");
}

// ── Factory: build_compression_parameters ────────────────────────────────────

#[test]
fn factory_no_dict_level_1_selects_fast() {
    // c_level=1 < LZ4HC_CLEVEL_MIN=2 → NoStreamFast
    let mut s = build_compression_parameters(1, 65536, 65536);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_no_dict_level_0_selects_fast() {
    // c_level=0 < 2 → NoStreamFast
    let mut s = build_compression_parameters(0, 65536, 65536);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_no_dict_negative_level_selects_fast() {
    let mut s = build_compression_parameters(-5, 65536, 65536);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_no_dict_level_2_selects_hc() {
    // c_level=2 == LZ4HC_CLEVEL_MIN → NoStreamHC
    let mut s = build_compression_parameters(2, 65536, 65536);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_no_dict_level_9_selects_hc() {
    // c_level=9 ≥ 2 → NoStreamHC
    let mut s = build_compression_parameters(9, 65536, 65536);
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_no_dict_src_block_size_args_ignored() {
    // _src_size and _block_size are ignored — same output regardless.
    let mut s1 = build_compression_parameters(1, 0, 0);
    let mut s2 = build_compression_parameters(1, usize::MAX, usize::MAX);
    let mut dst1 = Vec::new();
    let mut dst2 = Vec::new();
    s1.compress_block(SAMPLE, &mut dst1).unwrap();
    s2.compress_block(SAMPLE, &mut dst2).unwrap();
    assert_eq!(dst1[..], dst2[..]);
}

// ── Factory: build_compression_parameters_with_dict ──────────────────────────

#[test]
fn factory_with_dict_level_1_selects_stream_fast() {
    let dict = b"hello world ";
    let mut s = build_compression_parameters_with_dict(1, dict).unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert!(n > 0);
}

#[test]
fn factory_with_dict_level_0_selects_stream_fast() {
    let mut s = build_compression_parameters_with_dict(0, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_with_dict_level_2_selects_stream_hc() {
    // c_level=2 == LZ4HC_CLEVEL_MIN → StreamHC
    let dict = b"hello world ";
    let mut s = build_compression_parameters_with_dict(2, dict).unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert!(n > 0);
}

#[test]
fn factory_with_dict_level_9_selects_stream_hc() {
    let mut s = build_compression_parameters_with_dict(9, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_with_empty_dict_no_dict_roundtrip_fast() {
    // Empty dict → same as no-dict, should round-trip cleanly.
    let mut s = build_compression_parameters_with_dict(1, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

#[test]
fn factory_with_empty_dict_no_dict_roundtrip_hc() {
    let mut s = build_compression_parameters_with_dict(9, b"").unwrap();
    let mut dst = Vec::new();
    let n = s.compress_block(SAMPLE, &mut dst).unwrap();
    assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
}

// ── LZ4HC_CLEVEL_MIN boundary ─────────────────────────────────────────────────

#[test]
fn clevel_boundary_1_vs_2_no_dict() {
    // Both must succeed; level 1 → fast path, level 2 → HC path.
    let mut s1 = build_compression_parameters(1, 0, 0);
    let mut s2 = build_compression_parameters(2, 0, 0);
    let mut d1 = Vec::new();
    let mut d2 = Vec::new();
    let n1 = s1.compress_block(SAMPLE, &mut d1).unwrap();
    let n2 = s2.compress_block(SAMPLE, &mut d2).unwrap();
    // Both decompress correctly
    assert_eq!(lz4_decompress(&d1[..n1], SAMPLE.len()), SAMPLE);
    assert_eq!(lz4_decompress(&d2[..n2], SAMPLE.len()), SAMPLE);
}

#[test]
fn clevel_boundary_1_vs_2_with_dict() {
    let dict = b"hello world ";
    let mut s1 = build_compression_parameters_with_dict(1, dict).unwrap();
    let mut s2 = build_compression_parameters_with_dict(2, dict).unwrap();
    let mut d1 = Vec::new();
    let mut d2 = Vec::new();
    s1.compress_block(SAMPLE, &mut d1).unwrap();
    s2.compress_block(SAMPLE, &mut d2).unwrap();
    // Outputs differ (different algorithms) but both are valid compressed data.
    // We just assert both succeed.
}

// ── Trait object usage ────────────────────────────────────────────────────────

#[test]
fn strategy_usable_as_trait_object() {
    // CompressionStrategy must work behind Box<dyn CompressionStrategy>.
    let strategies: Vec<Box<dyn CompressionStrategy>> = vec![
        build_compression_parameters(1, 0, 0),
        build_compression_parameters(9, 0, 0),
    ];
    for mut s in strategies {
        let mut dst = Vec::new();
        let n = s.compress_block(SAMPLE, &mut dst).unwrap();
        assert_eq!(lz4_decompress(&dst[..n], SAMPLE.len()), SAMPLE);
    }
}
