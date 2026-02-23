//! Benchmark configuration: constants and runtime parameters for the `bench` subsystem.
//!
//! [`BenchConfig`] holds all tuneable settings for a benchmark run — duration, block
//! size, verbosity, and mode flags. Its builder-style setters allow callers to
//! construct a configuration incrementally before passing it to the benchmark runner.
//!
//! The timing and size constants defined here are shared across the compression and
//! decompression timing loops in [`super::runner`].

// ── Timing constants ─────────────────────────────────────────────────────────

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

/// Multiplier applied to decompression timing budget relative to compression.
/// A value of 1 means both phases receive equal wall-clock time.
pub const DECOMP_MULT: u32 = 1;

// ── Size multiplier constants ───────────────────────────────────────────────

pub const KB: usize = 1 << 10;
pub const MB: usize = 1 << 20;
pub const GB: usize = 1 << 30;

/// Maximum dictionary size accepted by LZ4 (64 KiB).
pub const LZ4_MAX_DICT_SIZE: usize = 64 * KB;

/// Maximum memory the benchmark will attempt to allocate.
///
/// On 32-bit targets this is capped at 2 GiB − 64 MiB to stay within addressable
/// space. On 64-bit targets the cap is `1 << (pointer_bits − 31)`, leaving ample
/// headroom while preventing runaway allocation.
pub const MAX_MEMORY: usize = if usize::BITS == 32 {
    (2 * GB) - (64 * MB)
} else {
    1usize << (usize::BITS - 31)
};

// ── BenchConfig struct ────────────────────────────────────────────────────────

/// Runtime parameters controlling a single benchmark session.
///
/// Construct via [`Default`] and then adjust with the builder-style setters,
/// or set fields directly. All fields are `pub` for convenient inspection.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Verbosity level: 0 = silent, 1 = errors, 2 = results+warnings (default),
    /// 3 = progress, 4 = full information.
    pub display_level: u32,

    /// Minimum benchmark duration in seconds (default: [`NBSECONDS`] = 3).
    pub nb_seconds: u32,

    /// Block size for splitting input into independent chunks.
    /// `0` means "use the full file as one block" (default: 0).
    pub block_size: usize,

    /// Auxiliary parameter that influences output formatting for machine-readable
    /// consumers (e.g. Python scripts parsing benchmark results). Default: 0.
    pub additional_param: i32,

    /// When `true`, benchmark each input file independently and emit one result
    /// line per file rather than aggregating across all inputs. Default: `false`.
    pub bench_separately: bool,

    /// When `true`, only benchmark decompression; input files must contain valid
    /// LZ4 frame data. Default: `false`.
    pub decode_only: bool,

    /// When `true`, skip checksum verification during decode-only benchmarking
    /// to isolate pure decompression throughput. Default: `false`.
    pub skip_checksums: bool,
}

impl Default for BenchConfig {
    /// Returns a `BenchConfig` with sensible defaults:
    /// - `display_level` = 2 (results + warnings)
    /// - `nb_seconds`    = 3 ([`NBSECONDS`])
    /// - `block_size`    = 0 (full file)
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
    // ── Setters ───────────────────────────────────────────────────────────────

    /// Set the verbosity level for benchmark output.
    ///
    /// `0` = silent, `1` = errors only, `2` = results + warnings (default),
    /// `3` = progress, `4` = full diagnostic output.
    pub fn set_notification_level(&mut self, level: u32) -> &mut Self {
        self.display_level = level;
        self
    }

    /// Set the auxiliary output-format parameter (see [`BenchConfig::additional_param`]).
    pub fn set_additional_param(&mut self, additional_param: i32) -> &mut Self {
        self.additional_param = additional_param;
        self
    }

    /// Set the minimum benchmark duration in seconds.
    ///
    /// Each compression and decompression phase runs for at least this many
    /// seconds before the result is recorded. Callers may log the new value
    /// at verbosity level 3 if desired.
    pub fn set_nb_seconds(&mut self, nb_seconds: u32) -> &mut Self {
        self.nb_seconds = nb_seconds;
        self
    }

    /// Set the block size used to split input data into independently compressed chunks.
    ///
    /// Pass `0` to compress each input file as a single block.
    pub fn set_block_size(&mut self, block_size: usize) -> &mut Self {
        self.block_size = block_size;
        self
    }

    /// Set whether each input file is benchmarked independently.
    ///
    /// When `true`, results are reported per-file rather than aggregated.
    pub fn set_bench_separately(&mut self, separate: bool) -> &mut Self {
        self.bench_separately = separate;
        self
    }

    /// Enable or disable decode-only mode.
    ///
    /// In decode-only mode the benchmark skips compression entirely; input
    /// files must already contain valid LZ4 frame data.
    pub fn set_decode_only(&mut self, set: bool) -> &mut Self {
        self.decode_only = set;
        self
    }

    /// Set whether content-checksum verification is skipped during decoding.
    ///
    /// Disabling checksum verification isolates raw decompression throughput from
    /// the cost of the xxHash content check. Only meaningful in decode-only mode.
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
