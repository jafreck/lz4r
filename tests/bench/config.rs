// Unit tests for task-023: bench/config.rs — Configuration Constants and Setters
//
// Verifies parity with bench.c lines 1–145:
//   - Timing constants match C macros (#defines)
//   - Size constants (KB/MB/GB/LZ4_MAX_DICT_SIZE/MAX_MEMORY) are correct
//   - BenchConfig::default() matches C globals (g_displayLevel=2, g_nbSeconds=3, etc.)
//   - All BMK_set* setter functions update the corresponding field and return &mut Self

use lz4::bench::config::{
    BenchConfig, ACTIVEPERIOD_NANOSEC, COOLPERIOD_SEC, DECOMP_MULT, GB, KB, LZ4_MAX_DICT_SIZE,
    MAX_MEMORY, MB, NBSECONDS, TIMELOOP_MICROSEC, TIMELOOP_NANOSEC,
};

// ── Timing constant parity (bench.c lines 68–72) ─────────────────────────────

#[test]
fn nbseconds_equals_3() {
    // C: #define NBSECONDS  3
    assert_eq!(NBSECONDS, 3u32);
}

#[test]
fn timeloop_microsec_equals_1_000_000() {
    // C: #define TIMELOOP_MICROSEC  1*1000000ULL
    assert_eq!(TIMELOOP_MICROSEC, 1_000_000u64);
}

#[test]
fn timeloop_nanosec_equals_1_000_000_000() {
    // C: #define TIMELOOP_NANOSEC   1*1000000000ULL
    assert_eq!(TIMELOOP_NANOSEC, 1_000_000_000u64);
}

#[test]
fn activeperiod_nanosec_equals_70_seconds() {
    // C: #define ACTIVEPERIOD_NANOSEC  70*TIMELOOP_NANOSEC
    assert_eq!(ACTIVEPERIOD_NANOSEC, 70 * 1_000_000_000u64);
}

#[test]
fn coolperiod_sec_equals_10() {
    // C: #define COOLPERIOD_SEC  10
    assert_eq!(COOLPERIOD_SEC, 10u64);
}

#[test]
fn decomp_mult_equals_1() {
    // C: #define DECOMP_MULT  1  (decompression timed 1x longer than compression)
    assert_eq!(DECOMP_MULT, 1u32);
}

// ── Size constant parity (bench.c lines 74–76) ───────────────────────────────

#[test]
fn kb_is_1024() {
    assert_eq!(KB, 1024usize);
}

#[test]
fn mb_is_1048576() {
    assert_eq!(MB, 1024 * 1024usize);
}

#[test]
fn gb_is_1073741824() {
    assert_eq!(GB, 1024 * 1024 * 1024usize);
}

#[test]
fn lz4_max_dict_size_is_64kb() {
    // LZ4 dictionary limit is 64 KiB
    assert_eq!(LZ4_MAX_DICT_SIZE, 64 * 1024usize);
}

#[test]
fn max_memory_is_positive_and_reasonable() {
    // MAX_MEMORY must be > 0 and at most 4 GiB on 32-bit or larger on 64-bit
    assert!(MAX_MEMORY > 0, "MAX_MEMORY must be positive");
    // On 64-bit: 1 << (64 - 31) = 1 << 33 = 8 GiB
    // On 32-bit: 2 GB - 64 MB
    if usize::BITS == 32 {
        assert_eq!(MAX_MEMORY, 2 * GB - 64 * MB);
    } else {
        assert_eq!(MAX_MEMORY, 1usize << (usize::BITS - 31));
    }
}

// ── BenchConfig::default() parity (bench.c globals, lines 122–127) ───────────

#[test]
fn default_display_level_is_2() {
    // C: static U32 g_displayLevel = 2
    assert_eq!(BenchConfig::default().display_level, 2);
}

#[test]
fn default_nb_seconds_is_3() {
    // C: static U32 g_nbSeconds = NBSECONDS  (= 3)
    assert_eq!(BenchConfig::default().nb_seconds, NBSECONDS);
    assert_eq!(BenchConfig::default().nb_seconds, 3);
}

#[test]
fn default_block_size_is_0() {
    // C: static size_t g_blockSize = 0
    assert_eq!(BenchConfig::default().block_size, 0);
}

#[test]
fn default_additional_param_is_0() {
    // C: static int g_additionalParam = 0
    assert_eq!(BenchConfig::default().additional_param, 0);
}

#[test]
fn default_bench_separately_is_false() {
    // C: static int g_benchSeparately = 0
    assert!(!BenchConfig::default().bench_separately);
}

#[test]
fn default_decode_only_is_false() {
    // C: static int g_decodeOnly = 0
    assert!(!BenchConfig::default().decode_only);
}

#[test]
fn default_skip_checksums_is_false() {
    // C: static unsigned g_skipChecksums = 0
    assert!(!BenchConfig::default().skip_checksums);
}

// ── Setter parity (bench.c BMK_set* functions, lines 129–145) ────────────────

#[test]
fn set_notification_level_updates_display_level() {
    // mirrors BMK_setNotificationLevel(level)
    let mut cfg = BenchConfig::default();
    cfg.set_notification_level(4);
    assert_eq!(cfg.display_level, 4);
}

#[test]
fn set_notification_level_to_zero() {
    let mut cfg = BenchConfig::default();
    cfg.set_notification_level(0);
    assert_eq!(cfg.display_level, 0);
}

#[test]
fn set_additional_param_updates_field() {
    // mirrors BMK_setAdditionalParam(additionalParam)
    let mut cfg = BenchConfig::default();
    cfg.set_additional_param(42);
    assert_eq!(cfg.additional_param, 42);
}

#[test]
fn set_additional_param_negative() {
    let mut cfg = BenchConfig::default();
    cfg.set_additional_param(-1);
    assert_eq!(cfg.additional_param, -1);
}

#[test]
fn set_nb_seconds_updates_field() {
    // mirrors BMK_setNbSeconds(nbSeconds)
    let mut cfg = BenchConfig::default();
    cfg.set_nb_seconds(10);
    assert_eq!(cfg.nb_seconds, 10);
}

#[test]
fn set_nb_seconds_to_zero() {
    let mut cfg = BenchConfig::default();
    cfg.set_nb_seconds(0);
    assert_eq!(cfg.nb_seconds, 0);
}

#[test]
fn set_block_size_updates_field() {
    // mirrors BMK_setBlockSize(blockSize)
    let mut cfg = BenchConfig::default();
    cfg.set_block_size(64 * KB);
    assert_eq!(cfg.block_size, 65_536);
}

#[test]
fn set_block_size_to_mb() {
    let mut cfg = BenchConfig::default();
    cfg.set_block_size(4 * MB);
    assert_eq!(cfg.block_size, 4 * 1024 * 1024);
}

#[test]
fn set_bench_separately_true() {
    // mirrors BMK_setBenchSeparately(separate != 0)
    let mut cfg = BenchConfig::default();
    cfg.set_bench_separately(true);
    assert!(cfg.bench_separately);
}

#[test]
fn set_bench_separately_false() {
    let mut cfg = BenchConfig::default();
    cfg.set_bench_separately(true);
    cfg.set_bench_separately(false);
    assert!(!cfg.bench_separately);
}

#[test]
fn set_decode_only_true() {
    // mirrors BMK_setDecodeOnlyMode(set)
    let mut cfg = BenchConfig::default();
    cfg.set_decode_only(true);
    assert!(cfg.decode_only);
}

#[test]
fn set_decode_only_false() {
    let mut cfg = BenchConfig::default();
    cfg.set_decode_only(true);
    cfg.set_decode_only(false);
    assert!(!cfg.decode_only);
}

#[test]
fn set_skip_checksums_true() {
    // mirrors BMK_skipChecksums(skip)
    let mut cfg = BenchConfig::default();
    cfg.set_skip_checksums(true);
    assert!(cfg.skip_checksums);
}

#[test]
fn set_skip_checksums_false() {
    let mut cfg = BenchConfig::default();
    cfg.set_skip_checksums(true);
    cfg.set_skip_checksums(false);
    assert!(!cfg.skip_checksums);
}

// ── Setter method chaining ────────────────────────────────────────────────────

#[test]
fn setters_return_mut_self_for_chaining() {
    // All setters return &mut Self — verify multi-setter chain applies all values
    let mut cfg = BenchConfig::default();
    cfg.set_notification_level(3)
        .set_nb_seconds(5)
        .set_block_size(1 * MB)
        .set_additional_param(7)
        .set_bench_separately(true)
        .set_decode_only(true)
        .set_skip_checksums(true);

    assert_eq!(cfg.display_level, 3);
    assert_eq!(cfg.nb_seconds, 5);
    assert_eq!(cfg.block_size, MB);
    assert_eq!(cfg.additional_param, 7);
    assert!(cfg.bench_separately);
    assert!(cfg.decode_only);
    assert!(cfg.skip_checksums);
}

#[test]
fn chained_setters_last_value_wins() {
    // Calling a setter twice in a chain: the last call should win
    let mut cfg = BenchConfig::default();
    cfg.set_nb_seconds(1).set_nb_seconds(99);
    assert_eq!(cfg.nb_seconds, 99);
}

// ── Clone parity ──────────────────────────────────────────────────────────────

#[test]
fn clone_produces_independent_copy() {
    let mut original = BenchConfig::default();
    original.set_nb_seconds(7);
    let mut cloned = original.clone();
    cloned.set_nb_seconds(100);
    // Original must not be affected
    assert_eq!(original.nb_seconds, 7);
    assert_eq!(cloned.nb_seconds, 100);
}
