/*
    bench/config.rs — Benchmark configuration constants and setters
    Migrated from lz4-1.10.0/programs/bench.c (lines 1–145) and bench.h

    Original copyright (C) Yann Collet 2012-2020 — GPL v2 License.

    Migration notes:
    - C module-level globals are encapsulated in `BenchConfig`.
    - `g_displayLevel` (U32) → `display_level: u32`
    - `g_nbSeconds` (U32, default 3) → `nb_seconds: u32`
    - `g_blockSize` (size_t, default 0) → `block_size: usize`
    - `g_additionalParam` (int, default 0) → `additional_param: i32`
    - `g_benchSeparately` (int, default 0) → `bench_separately: bool`
    - `g_decodeOnly` (int, default 0) → `decode_only: bool`
    - `g_skipChecksums` (unsigned, default 0) → `skip_checksums: bool`
    - Setter methods mirror the BMK_set* functions and return `&mut Self`
      so they can be chained.
    - `LZ4_isError(errcode)` was `(errcode==0)` in C — the block API returns
      0 on failure (not a negative value). This inverted semantic must NOT
      be carried into Rust. With `lz4_flex`, block compression returns
      `Err(...)` on failure; check with `.is_err()`.
*/

// ── Timing constants (mirrors bench.c lines 68–72) ────────────────────────────

/// Minimum benchmark duration in seconds.
pub const NBSECONDS: u32 = 3;

/// Target duration per compression timing loop (1 second in microseconds).
pub const TIMELOOP_MICROSEC: u64 = 1_000_000;

/// Target duration per compression timing loop (1 second in nanoseconds).
pub const TIMELOOP_NANOSEC: u64 = 1_000_000_000;

/// Active benchmarking period ceiling (70 seconds in nanoseconds).
pub const ACTIVEPERIOD_NANOSEC: u64 = 70 * 1_000_000_000;

/// Cool-down period between active benchmark windows (seconds).
pub const COOLPERIOD_SEC: u64 = 10;

/// Decompression is timed DECOMP_MULT times longer than compression.
pub const DECOMP_MULT: u32 = 1;

// ── Size multiplier constants (mirrors bench.c lines 74–76) ──────────────────

pub const KB: usize = 1 << 10;
pub const MB: usize = 1 << 20;
pub const GB: usize = 1 << 30;

/// Maximum dictionary size accepted by LZ4 (64 KiB).
pub const LZ4_MAX_DICT_SIZE: usize = 64 * KB;

/// Maximum memory the benchmark will attempt to allocate.
/// Mirrors bench.c line 80:
///   (sizeof(size_t)==4) ? (2 GB - 64 MB) : (1ULL << (sizeof(size_t)*8 - 31))
pub const MAX_MEMORY: usize = if usize::BITS == 32 {
    (2 * GB) - (64 * MB)
} else {
    1usize << (usize::BITS - 31)
};

// ── BenchConfig struct ────────────────────────────────────────────────────────

/// Runtime benchmark parameters.
///
/// Encapsulates the C module-level globals `g_*` from bench.c (lines 122–127)
/// and the `g_displayLevel` static (line 90).
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Verbosity level: 0 = silent, 1 = errors, 2 = results+warnings (default),
    /// 3 = progress, 4 = full information.
    pub display_level: u32,

    /// Minimum benchmark duration in seconds (default: [`NBSECONDS`] = 3).
    pub nb_seconds: u32,

    /// Block size for splitting input into independent chunks.
    /// 0 means "use file size" (default: 0).
    pub block_size: usize,

    /// Hidden parameter influencing output format for Python parsing
    /// (mirrors `g_additionalParam`, default: 0).
    pub additional_param: i32,

    /// When true, benchmark each input file separately and report one result
    /// per file (mirrors `g_benchSeparately`, default: false).
    pub bench_separately: bool,

    /// When true, only benchmark decompression; input must be valid LZ4 frame
    /// data (mirrors `g_decodeOnly`, default: false).
    pub decode_only: bool,

    /// When true, skip checksum verification during decode-only benchmarking
    /// (mirrors `g_skipChecksums`, default: false).
    pub skip_checksums: bool,
}

impl Default for BenchConfig {
    /// Returns a `BenchConfig` with the same defaults as the C globals:
    /// - `display_level` = 2
    /// - `nb_seconds`    = 3 (`NBSECONDS`)
    /// - `block_size`    = 0
    /// - `additional_param` = 0
    /// - `bench_separately` = false
    /// - `decode_only`   = false
    /// - `skip_checksums` = false
    fn default() -> Self {
        BenchConfig {
            display_level: 2,
            nb_seconds: NBSECONDS,
            block_size: 0,
            additional_param: 0,
            bench_separately: false,
            decode_only: false,
            skip_checksums: false,
        }
    }
}

impl BenchConfig {
    // ── Setters (mirror the BMK_set* / BMK_skip* functions, bench.c 129–145) ──

    /// Set verbosity level (mirrors `BMK_setNotificationLevel`).
    pub fn set_notification_level(&mut self, level: u32) -> &mut Self {
        self.display_level = level;
        self
    }

    /// Set the hidden additional-param field (mirrors `BMK_setAdditionalParam`).
    pub fn set_additional_param(&mut self, additional_param: i32) -> &mut Self {
        self.additional_param = additional_param;
        self
    }

    /// Set minimum benchmark duration in seconds (mirrors `BMK_setNbSeconds`).
    ///
    /// In the C source, this setter also printed a DISPLAYLEVEL(3, …) message.
    /// That side-effect is omitted here; callers should log if needed.
    pub fn set_nb_seconds(&mut self, nb_seconds: u32) -> &mut Self {
        self.nb_seconds = nb_seconds;
        self
    }

    /// Set the block-split size in bytes (mirrors `BMK_setBlockSize`).
    pub fn set_block_size(&mut self, block_size: usize) -> &mut Self {
        self.block_size = block_size;
        self
    }

    /// Set whether each file is benchmarked separately (mirrors
    /// `BMK_setBenchSeparately`). Mirrors the `(separate!=0)` cast.
    pub fn set_bench_separately(&mut self, separate: bool) -> &mut Self {
        self.bench_separately = separate;
        self
    }

    /// Set decode-only mode (mirrors `BMK_setDecodeOnlyMode`).
    pub fn set_decode_only(&mut self, set: bool) -> &mut Self {
        self.decode_only = set;
        self
    }

    /// Set whether checksums are skipped in decode-only mode
    /// (mirrors `BMK_skipChecksums`).
    pub fn set_skip_checksums(&mut self, skip: bool) -> &mut Self {
        self.skip_checksums = skip;
        self
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_nb_seconds() {
        assert_eq!(BenchConfig::default().nb_seconds, 3);
    }

    #[test]
    fn default_block_size() {
        assert_eq!(BenchConfig::default().block_size, 0);
    }

    #[test]
    fn default_display_level() {
        assert_eq!(BenchConfig::default().display_level, 2);
    }

    #[test]
    fn setter_nb_seconds() {
        let mut cfg = BenchConfig::default();
        cfg.set_nb_seconds(10);
        assert_eq!(cfg.nb_seconds, 10);
    }

    #[test]
    fn setter_block_size() {
        let mut cfg = BenchConfig::default();
        cfg.set_block_size(64 * KB);
        assert_eq!(cfg.block_size, 65536);
    }

    #[test]
    fn setter_chain() {
        let mut cfg = BenchConfig::default();
        cfg.set_nb_seconds(5)
            .set_block_size(1 * MB)
            .set_decode_only(true)
            .set_skip_checksums(true);
        assert_eq!(cfg.nb_seconds, 5);
        assert_eq!(cfg.block_size, MB);
        assert!(cfg.decode_only);
        assert!(cfg.skip_checksums);
    }

    #[test]
    fn setter_bench_separately_false() {
        let mut cfg = BenchConfig::default();
        cfg.set_bench_separately(false);
        assert!(!cfg.bench_separately);
    }

    #[test]
    fn constants_sanity() {
        assert_eq!(KB, 1024);
        assert_eq!(MB, 1024 * 1024);
        assert_eq!(LZ4_MAX_DICT_SIZE, 65536);
        assert_eq!(TIMELOOP_MICROSEC, 1_000_000);
        assert_eq!(TIMELOOP_NANOSEC, 1_000_000_000);
        assert_eq!(ACTIVEPERIOD_NANOSEC, 70_000_000_000);
    }
}
