//! Benchmark entry points for lz4r.
//!
//! This module exposes [`bench_files`] as the primary public API. Callers pass
//! a list of real files, or an empty slice to run the built-in synthetic
//! lorem-ipsum benchmark. Internally, work is dispatched to:
//!
//! - [`runner::bench_c_level`] — benchmarks a single compression level for a
//!   given in-memory buffer.
//! - [`runner::bench_file_table`] — reads a set of files into memory and
//!   benchmarks them together as a single logical dataset.
//!
//! [`config::BenchConfig`] controls display verbosity, iteration count,
//! decode-only mode, and other runtime knobs.

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
/// Allocates a 10 MiB buffer filled with lorem-ipsum text (seed 0), then calls
/// [`bench_c_level`] for each compression level in `c_level..=c_level_last`.
/// This exercises the compressor on realistic but reproducible incompressible-ish
/// natural-language input without requiring an on-disk file.
fn synthetic_test(
    c_level: i32,
    c_level_last: i32,
    dict: &[u8],
    config: &BenchConfig,
) -> io::Result<()> {
    const BENCHED_SIZE: usize = 10_000_000;
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
/// When [`BenchConfig::bench_separately`] is set, this is called instead of
/// [`bench_file_table`] so that each file's throughput numbers are reported
/// independently rather than aggregated across all files.
fn bench_files_separately(
    file_names: &[&str],
    c_level: i32,
    c_level_last: i32,
    dict: &[u8],
    config: &BenchConfig,
) -> io::Result<()> {
    // Clamp both levels to the HC ceiling, then ensure the range is non-empty.
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
/// # Arguments
/// - `file_names`: paths of files to benchmark. An empty slice triggers the
///   built-in synthetic lorem-ipsum benchmark instead.
/// - `c_level` / `c_level_last`: inclusive compression-level range, both clamped
///   to [`LZ4HC_CLEVEL_MAX`]. If `c_level_last < c_level` after clamping, only
///   `c_level` is run.
/// - `dict_file`: optional path to a pre-trained dictionary. Only the last
///   [`LZ4_MAX_DICT_SIZE`] bytes of the file are loaded — LZ4 dictionaries
///   are always anchored at the tail.
/// - `config`: runtime parameters (verbosity, iteration count, decode-only, …).
///
/// # Errors
/// Returns `Err` if a required file cannot be read, the dictionary cannot be
/// loaded, or at least one benchmark pass reports a failure.
pub fn bench_files(
    file_names: &[&str],
    c_level: i32,
    c_level_last: i32,
    dict_file: Option<&str>,
    config: &BenchConfig,
) -> io::Result<()> {
    // Levels above LZ4HC_CLEVEL_MAX are undefined; clamp silently.
    let c_level = c_level.min(LZ4HC_CLEVEL_MAX);
    let mut c_level_last = c_level_last;

    // In decode-only mode there is no compression level to sweep; fix the range
    // to a single level so the loop in bench_c_level runs exactly once.
    if config.decode_only {
        if config.display_level >= 2 {
            if config.skip_checksums {
                eprintln!("Benchmark Decompression of LZ4 Frame _without_ checksum even when present ");
            } else {
                eprintln!("Benchmark Decompression of LZ4 Frame + Checksum when present ");
            }
        }
        c_level_last = c_level;
    }

    // Re-apply ceiling and non-empty-range invariant after the decode-only fixup.
    c_level_last = c_level_last.min(LZ4HC_CLEVEL_MAX).max(c_level);

    if c_level_last > c_level && config.display_level >= 2 {
        eprintln!("Benchmarking levels from {} to {}", c_level, c_level_last);
    }

    // ── Load optional dictionary ──────────────────────────────────────────────
    let dict_buf: Vec<u8> = if let Some(dict_path) = dict_file {
        // Dictionary-assisted decompression is not yet wired into the frame
        // decoder path; reject the combination early rather than silently ignoring
        // the dictionary.
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

        // LZ4 dictionaries are identified by their last LZ4_MAX_DICT_SIZE bytes;
        // if the file is larger, seek past the prefix so we only read the tail.
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

    // ── Dispatch ──────────────────────────────────────────────────────────────
    if file_names.is_empty() {
        // No files provided — fall back to the built-in synthetic benchmark.
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
