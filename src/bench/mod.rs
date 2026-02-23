/*
    bench/mod.rs — Benchmark module public API
    Migrated from lz4-1.10.0/programs/bench.c (lines 756–865) and bench.h

    Original copyright (C) Yann Collet 2012-2020 — GPL v2 License.

    Migration notes:
    - `BMK_syntheticTest` (static, lines 756–781) → `synthetic_test` (private)
    - `BMK_benchFilesSeparately` (static, lines 784–799) → `bench_files_separately` (private)
    - `BMK_benchFiles` (public, lines 802–865) → `pub fn bench_files` (public API)
    - LZ4HC_CLEVEL_MAX (12) is re-exported from `crate::hc::types`.
    - Dictionary loading mirrors C: use only the last LZ4_MAX_DICT_SIZE bytes of
      the file (bench.c lines 837–842), consistent with C's UTIL_fseek logic.
    - DISPLAYLEVEL(2, …) → `if config.display_level >= 2 { eprintln!(...) }`.
    - END_PROCESS(code, msg) → `return Err(io::Error::new(Other, msg))`.
    - `LOREM_genBuffer` → `crate::lorem::gen_buffer(size, seed)`.
*/

pub mod config;
pub mod compress_strategy;
pub mod decompress_binding;
pub mod bench_mem;
pub mod runner;

// Re-export public types so callers can use `bench::BenchConfig` directly.
pub use config::BenchConfig;

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};

use crate::hc::types::LZ4HC_CLEVEL_MAX;
use config::LZ4_MAX_DICT_SIZE;
use runner::{bench_c_level, bench_file_table};

// ── Synthetic test ────────────────────────────────────────────────────────────

/// Run a benchmark using synthetically generated lorem-ipsum data.
///
/// Migrated from `BMK_syntheticTest` (bench.c lines 756–781).
///
/// Allocates a 10 MiB buffer, fills it with lorem ipsum text (seed 0, first=true,
/// fill=true — identical to `LOREM_genBuffer(src, benchedSize, 0)`), then calls
/// [`bench_c_level`] for each level in `c_level..=c_level_last`.
fn synthetic_test(
    c_level: i32,
    c_level_last: i32,
    dict: &[u8],
    config: &BenchConfig,
) -> io::Result<()> {
    const BENCHED_SIZE: usize = 10_000_000;
    // LOREM_genBuffer(srcBuffer, benchedSize, 0) — fill with lorem ipsum, seed 0.
    let src_buffer = crate::lorem::gen_buffer(BENCHED_SIZE, 0);
    bench_c_level(
        &src_buffer,
        "Lorem ipsum",
        c_level,
        c_level_last,
        config,
        dict,
        &[BENCHED_SIZE], // single "file" spanning the whole buffer
    )
}

// ── Per-file benchmarking ─────────────────────────────────────────────────────

/// Benchmark each file in `file_names` separately, one per call to [`bench_file_table`].
///
/// Migrated from `BMK_benchFilesSeparately` (bench.c lines 784–799).
fn bench_files_separately(
    file_names: &[&str],
    c_level: i32,
    c_level_last: i32,
    dict: &[u8],
    config: &BenchConfig,
) -> io::Result<()> {
    // C: clamp both levels to LZ4HC_CLEVEL_MAX, then ensure last >= first.
    let c_level = c_level.min(LZ4HC_CLEVEL_MAX);
    let c_level_last = c_level_last.min(LZ4HC_CLEVEL_MAX).max(c_level);

    let mut bench_error = false;
    for file_name in file_names {
        if let Err(e) = bench_file_table(&[file_name], c_level, c_level_last, dict, config) {
            eprintln!("bench error for {}: {}", file_name, e);
            bench_error = true;
        }
    }
    if bench_error {
        Err(io::Error::new(io::ErrorKind::Other, "benchmark reported errors"))
    } else {
        Ok(())
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Benchmark compression and decompression across one or more files.
///
/// Migrated from `BMK_benchFiles` (bench.c lines 802–865), which is the sole
/// function declared in `bench.h`.
///
/// # Arguments
/// - `file_names`: slice of file paths to benchmark. Pass an empty slice to run
///   the synthetic lorem-ipsum test (equivalent to `nbFiles == 0` in C).
/// - `c_level` / `c_level_last`: inclusive compression-level range. If `c_level_last
///   < c_level`, only `c_level` is benchmarked (C clamps after capping to
///   `LZ4HC_CLEVEL_MAX`).
/// - `dict_file`: optional path to a dictionary file. When provided, the last
///   [`LZ4_MAX_DICT_SIZE`] bytes of the file are used as the dictionary
///   (mirrors C bench.c lines 825–851).
/// - `config`: runtime benchmark parameters.
///
/// # Errors
/// Returns `Err` if any required file cannot be read, the dictionary cannot be
/// loaded, or at least one benchmark pass fails.
pub fn bench_files(
    file_names: &[&str],
    c_level: i32,
    c_level_last: i32,
    dict_file: Option<&str>,
    config: &BenchConfig,
) -> io::Result<()> {
    // C: if (cLevel > LZ4HC_CLEVEL_MAX) cLevel = LZ4HC_CLEVEL_MAX
    let c_level = c_level.min(LZ4HC_CLEVEL_MAX);
    let mut c_level_last = c_level_last;

    // Decode-only mode adjustments (bench.c lines 811–818).
    if config.decode_only {
        if config.display_level >= 2 {
            if config.skip_checksums {
                eprintln!("Benchmark Decompression of LZ4 Frame _without_ checksum even when present ");
            } else {
                eprintln!("Benchmark Decompression of LZ4 Frame + Checksum when present ");
            }
        }
        c_level_last = c_level; // decode-only: single level
    }

    // C: cap cLevelLast, then ensure last >= first.
    c_level_last = c_level_last.min(LZ4HC_CLEVEL_MAX).max(c_level);

    if c_level_last > c_level && config.display_level >= 2 {
        eprintln!("Benchmarking levels from {} to {}", c_level, c_level_last);
    }

    // ── Load optional dictionary (bench.c lines 825–851) ────────────────────
    let dict_buf: Vec<u8> = if let Some(dict_path) = dict_file {
        // Validate decode-only incompatibility (bench.c line 830).
        if config.decode_only {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Error : LZ4 Frame decoder mode not compatible with dictionary yet",
            ));
        }

        let meta = fs::metadata(dict_path).map_err(|e| {
            io::Error::new(e.kind(), format!("Dictionary error : could not stat dictionary file: {}", e))
        })?;
        let dict_file_size = meta.len() as usize;
        if dict_file_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Dictionary error : could not stat dictionary file",
            ));
        }

        let mut f = fs::File::open(dict_path).map_err(|e| {
            io::Error::new(e.kind(), format!("Dictionary error : could not open dictionary file: {}", e))
        })?;

        // C: if (dictFileSize > LZ4_MAX_DICT_SIZE) seek to last LZ4_MAX_DICT_SIZE bytes.
        let dict_size = if dict_file_size > LZ4_MAX_DICT_SIZE {
            let offset = (dict_file_size - LZ4_MAX_DICT_SIZE) as u64;
            f.seek(SeekFrom::Start(offset)).map_err(|e| {
                io::Error::new(e.kind(), format!("Dictionary error : could not seek dictionary file: {}", e))
            })?;
            LZ4_MAX_DICT_SIZE
        } else {
            dict_file_size
        };

        let mut buf = vec![0u8; dict_size];
        f.read_exact(&mut buf).map_err(|e| {
            io::Error::new(e.kind(), format!("Dictionary error : could not read dictionary file: {}", e))
        })?;
        buf
    } else {
        Vec::new()
    };

    // ── Dispatch (bench.c lines 854–861) ────────────────────────────────────
    if file_names.is_empty() {
        // nbFiles == 0 → synthetic test
        synthetic_test(c_level, c_level_last, &dict_buf, config)
    } else if config.bench_separately {
        bench_files_separately(file_names, c_level, c_level_last, &dict_buf, config)
    } else {
        bench_file_table(file_names, c_level, c_level_last, &dict_buf, config)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_files_synthetic_ok() {
        // Acceptance criterion: bench_files(&[], 1, 1, None, &config) returns Ok(())
        let mut config = BenchConfig::default();
        config.set_nb_seconds(0);         // single pass — fast
        config.set_notification_level(0); // suppress output
        let result = bench_files(&[], 1, 1, None, &config);
        assert!(result.is_ok(), "synthetic test should return Ok: {:?}", result.err());
    }

    #[test]
    fn bench_files_missing_file_returns_err() {
        let config = BenchConfig::default();
        let result = bench_files(&["/nonexistent/file.bin"], 1, 1, None, &config);
        assert!(result.is_err(), "nonexistent file should return Err");
    }

    #[test]
    fn bench_files_with_real_file_three_levels() {
        use std::io::Write;
        // Write a temporary file and benchmark levels 1..=3.
        let mut tmp = tempfile::NamedTempFile::new().expect("tmp");
        // Write 64 KiB of repeating data so compression is interesting.
        let data: Vec<u8> = (0u8..=255).cycle().take(65536).collect();
        tmp.write_all(&data).unwrap();
        let path = tmp.path().to_str().unwrap().to_owned();

        let mut config = BenchConfig::default();
        config.set_nb_seconds(0);
        config.set_notification_level(0);

        let result = bench_files(&[&path], 1, 3, None, &config);
        assert!(result.is_ok(), "3-level file bench should succeed: {:?}", result.err());
    }

    #[test]
    fn bench_files_separately_flag() {
        use std::io::Write;
        let mut tmp1 = tempfile::NamedTempFile::new().expect("tmp1");
        let mut tmp2 = tempfile::NamedTempFile::new().expect("tmp2");
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        tmp1.write_all(&data).unwrap();
        tmp2.write_all(&data).unwrap();
        let path1 = tmp1.path().to_str().unwrap().to_owned();
        let path2 = tmp2.path().to_str().unwrap().to_owned();

        let mut config = BenchConfig::default();
        config.set_nb_seconds(0);
        config.set_notification_level(0);
        config.set_bench_separately(true);

        let result = bench_files(&[&path1, &path2], 1, 1, None, &config);
        assert!(result.is_ok(), "bench_separately should succeed: {:?}", result.err());
    }

    #[test]
    fn bench_files_clamps_level_range() {
        // When c_level_last < c_level, only c_level is benchmarked (no error).
        let mut config = BenchConfig::default();
        config.set_nb_seconds(0);
        config.set_notification_level(0);
        // c_level_last < c_level → clamped to c_level
        let result = bench_files(&[], 3, 1, None, &config);
        assert!(result.is_ok(), "clamped level range should succeed");
    }

    #[test]
    fn bench_files_missing_dict_returns_err() {
        let config = BenchConfig::default();
        let result = bench_files(&[], 1, 1, Some("/nonexistent/dict.bin"), &config);
        assert!(result.is_err(), "missing dict file should return Err");
    }
}
