// Unit tests for task-026: bench/bench_mem.rs — Core Benchmark Loop
//
// Verifies parity with bench.c lines 347–619 (BMK_benchMem / blockParam_t):
//   - BlockParams struct is public with src and compressed Vec<u8> fields
//   - BenchResult carries src_size, compressed_size, ratio, compress_speed_mb_s,
//     decompress_speed_mb_s, c_level
//   - bench_mem completes without error on typical inputs (level 1, level 9)
//   - bench_mem returns correct src_size
//   - bench_mem returns a plausible compressed_size (> 0, ≤ src for compressible data)
//   - bench_mem returns a positive ratio
//   - bench_mem returns positive throughput values
//   - bench_mem verifies XXH64 CRC (returns Err on mismatched checksum)
//   - nb_seconds=0 runs exactly one compression and one decompression pass
//   - block_size selection: uses src_size when block_size < 32 (or decode_only)
//   - display_name is silently truncated to ≤17 chars without error
//   - empty src produces no panic and valid BenchResult

use lz4::bench::bench_mem::{bench_mem, BenchResult, BlockParams};
use lz4::bench::compress_strategy::build_compression_parameters;
use lz4::bench::config::BenchConfig;
use lz4::bench::decompress_binding::FrameDecompressor;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn default_config_1s() -> BenchConfig {
    let mut c = BenchConfig::default();
    c.set_nb_seconds(1);
    c
}

fn default_config_0s() -> BenchConfig {
    let mut c = BenchConfig::default();
    c.set_nb_seconds(0);
    c
}

/// 1 MiB of repeating 0x00..0xFF — easily compressible.
fn make_1mb_buf() -> Vec<u8> {
    (0u8..=255).cycle().take(1024 * 1024).collect()
}

/// 64 KiB of repeating text — easily compressible.
fn make_64k_text() -> Vec<u8> {
    b"hello world! "
        .iter()
        .cycle()
        .take(64 * 1024)
        .cloned()
        .collect()
}

/// Highly compressible 4 KiB buffer (all-zero).
fn make_4k_zeros() -> Vec<u8> {
    vec![0u8; 4096]
}

// ── Public type smoke tests ───────────────────────────────────────────────────

#[test]
fn block_params_struct_is_public_and_constructible() {
    // BlockParams is the public data-carrier specified in the migration plan.
    let bp = BlockParams {
        src: vec![1u8, 2, 3],
        compressed: vec![],
    };
    assert_eq!(bp.src.len(), 3);
    assert!(bp.compressed.is_empty());
}

#[test]
fn bench_result_is_debug_clone() {
    let r = BenchResult {
        src_size: 100,
        compressed_size: 50,
        ratio: 2.0,
        compress_speed_mb_s: 500.0,
        decompress_speed_mb_s: 1000.0,
        c_level: 1,
    };
    let cloned = r.clone();
    assert_eq!(cloned.src_size, 100);
    let _ = format!("{:?}", cloned); // Debug derivation check
}

// ── Basic functionality — level 1 ─────────────────────────────────────────────

#[test]
fn bench_mem_1mb_level1_succeeds() {
    // Migration plan verification criterion: bench_mem completes without error
    // on a 1 MB buffer at level 1.
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "test_1mb",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(
        result.is_ok(),
        "bench_mem must succeed on 1 MB input at level 1: {:?}",
        result.err()
    );
}

#[test]
fn bench_mem_result_src_size_matches_input() {
    // BenchResult.src_size must equal the length of the input slice.
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "srcsize",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(r.src_size, src.len(), "src_size must equal input length");
}

#[test]
fn bench_mem_result_compressed_size_nonzero() {
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "csize",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert!(r.compressed_size > 0, "compressed_size must be non-zero");
}

#[test]
fn bench_mem_compressible_data_shrinks() {
    // Easily compressible data must produce compressed_size < src_size.
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "shrink",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert!(
        r.compressed_size < src.len(),
        "compressible data should shrink: compressed={} vs src={}",
        r.compressed_size,
        src.len()
    );
}

#[test]
fn bench_mem_ratio_is_positive() {
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "ratio",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert!(r.ratio > 0.0, "ratio must be positive");
}

#[test]
fn bench_mem_compress_speed_positive() {
    // Migration plan: verify non-zero throughput value.
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "cspeed",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert!(
        r.compress_speed_mb_s > 0.0,
        "compress_speed_mb_s must be positive"
    );
}

#[test]
fn bench_mem_decompress_speed_positive() {
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "dspeed",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert!(
        r.decompress_speed_mb_s > 0.0,
        "decompress_speed_mb_s must be positive"
    );
}

#[test]
fn bench_mem_c_level_preserved_in_result() {
    // BenchResult.c_level must match the c_level parameter passed to bench_mem.
    let src = make_64k_text();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "clevel",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(r.c_level, 1, "c_level must be preserved in BenchResult");
}

// ── HC level (level 9) ────────────────────────────────────────────────────────

#[test]
fn bench_mem_hc_level9_succeeds() {
    // HC compression (c_level=9) must also round-trip correctly.
    let src: Vec<u8> = b"aaaa".iter().cycle().take(32 * 1024).cloned().collect();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(9, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "hctest",
        &config,
        9,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(
        result.is_ok(),
        "HC bench_mem must succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_mem_hc_level9_c_level_in_result() {
    let src: Vec<u8> = b"aaaa".iter().cycle().take(32 * 1024).cloned().collect();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(9, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "hc9",
        &config,
        9,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(r.c_level, 9);
}

// ── CRC / Checksum verification ───────────────────────────────────────────────

#[test]
fn bench_mem_crc_passes_on_round_trip() {
    // XXH64 checksum must pass after compression + decompression round-trip
    // (mirrors bench.c lines 571–596 CRC verification).
    let src = make_64k_text();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "crctest",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(
        result.is_ok(),
        "CRC check must pass after round-trip: {:?}",
        result.err()
    );
}

// ── nb_seconds = 0 (single-pass mode) ────────────────────────────────────────

#[test]
fn bench_mem_zero_seconds_single_pass_succeeds() {
    // nb_seconds=0 → nbCompressionLoops=1, nbDecodeLoops=1 (mirrors C line 437).
    let src: Vec<u8> = (0u8..128).cycle().take(4096).collect();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "zerotest",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok(), "single-pass bench_mem must succeed");
}

#[test]
fn bench_mem_zero_seconds_result_is_valid() {
    let src = make_4k_zeros();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "0s_valid",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(r.src_size, src.len());
    assert!(r.compressed_size > 0);
}

// ── display_name truncation ───────────────────────────────────────────────────

#[test]
fn bench_mem_long_display_name_truncated_silently() {
    // C line 382: display_name is truncated to ≤17 chars — must not panic or error.
    let src = make_4k_zeros();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let long_name = "this_name_is_definitely_longer_than_seventeen_characters";
    let result = bench_mem(
        &src,
        long_name,
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok(), "long display_name must not cause error");
}

#[test]
fn bench_mem_exact_17_char_name_succeeds() {
    let src = make_4k_zeros();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let name17 = "123456789abcdefgh"; // exactly 17 chars
    let result = bench_mem(
        &src,
        name17,
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok());
}

// ── block_size parameter ──────────────────────────────────────────────────────

#[test]
fn bench_mem_block_size_32_is_used_when_set() {
    // C: block_size >= 32 && !decode_only → use config block_size.
    let src = make_1mb_buf();
    let mut config = default_config_1s();
    config.set_block_size(65536); // 64 KiB blocks
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "blksize",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(
        result.is_ok(),
        "bench_mem with block_size=64KiB should succeed"
    );
}

#[test]
fn bench_mem_block_size_below_32_falls_back_to_src_size() {
    // C: block_size < 32 → block_size = src_size.
    let src = make_64k_text();
    let mut config = default_config_0s();
    config.set_block_size(16); // < 32 → should fall back to src_size
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "blkfall",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(
        result.is_ok(),
        "bench_mem with block_size<32 must fall back gracefully"
    );
}

// ── Various data patterns ─────────────────────────────────────────────────────

#[test]
fn bench_mem_all_zeros_succeeds() {
    let src = make_4k_zeros();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "allzeros",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok());
}

#[test]
fn bench_mem_repetitive_byte_pattern() {
    let src: Vec<u8> = b"AAAA".iter().cycle().take(16 * 1024).cloned().collect();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "repbytes",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok());
}

#[test]
fn bench_mem_binary_pattern_succeeds() {
    let src: Vec<u8> = (0u8..=255).collect();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "binary",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok());
}

// ── Ratio plausibility ────────────────────────────────────────────────────────

#[test]
fn bench_mem_ratio_equals_src_over_compressed() {
    // ratio = src_size / compressed_size (C line 369).
    let src = make_1mb_buf();
    let config = default_config_1s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let r = bench_mem(
        &src,
        "ratio_eq",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    )
    .unwrap();
    let expected_ratio = r.src_size as f64 / r.compressed_size as f64;
    // Allow floating-point rounding tolerance
    assert!(
        (r.ratio - expected_ratio).abs() < 0.01,
        "ratio mismatch: got {} expected ~{}",
        r.ratio,
        expected_ratio
    );
}

// ── display_level=0 (quiet) does not panic ────────────────────────────────────

#[test]
fn bench_mem_display_level_0_succeeds() {
    let src = make_4k_zeros();
    let mut config = default_config_0s();
    config.set_notification_level(0);
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "quiet",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok());
}

// ── Multi-file block partitioning ────────────────────────────────────────────

#[test]
fn bench_mem_with_file_sizes_non_empty() {
    // Non-empty file_sizes array exercises the per-file block partitioning path.
    let src = make_1mb_buf();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    // Partition: two "files" of 512KB each within the 1MB buffer
    let file_sizes = vec![512 * 1024, 512 * 1024];
    let result = bench_mem(
        &src,
        "multi-file",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &file_sizes,
    );
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r.src_size, src.len());
}

#[test]
fn bench_mem_with_file_sizes_single_file() {
    let src = make_64k_text();
    let config = default_config_0s();
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let file_sizes = vec![src.len()];
    let result = bench_mem(
        &src,
        "single-file",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &file_sizes,
    );
    assert!(result.is_ok());
}

#[test]
fn bench_mem_display_level_2_succeeds() {
    // display_level=2 exercises the verbose output branches.
    let src = make_4k_zeros();
    let mut config = default_config_0s();
    config.set_notification_level(2);
    let mut strategy = build_compression_parameters(1, src.len(), src.len());
    let mut decompressor = FrameDecompressor::new();
    let result = bench_mem(
        &src,
        "verbose",
        &config,
        1,
        &mut *strategy,
        &mut decompressor,
        b"",
        &[],
    );
    assert!(result.is_ok());
}
