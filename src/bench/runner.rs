//! Benchmark runner: file loading, memory estimation, and compression-level sweeps.
//!
//! This module coordinates the three high-level operations the benchmark driver
//! needs before measuring throughput:
//!
//! 1. **Memory estimation** ([`find_max_mem`]): determines the largest contiguous
//!    buffer the process can safely allocate for benchmark input data.
//! 2. **File loading** ([`load_files`]): reads one or more files into a single
//!    contiguous buffer, capped by the estimated memory limit.
//! 3. **Level sweep** ([`bench_c_level`], [`bench_file_table`]): runs
//!    [`bench_mem`] for every compression level in a requested range,
//!    constructing fresh codec state per level.

use std::fs;
use std::io::{self, Read};

use super::bench_mem::bench_mem;
use super::compress_strategy::build_compression_parameters;
use super::config::{BenchConfig, MAX_MEMORY};
use super::decompress_binding::FrameDecompressor;

/// Maximum input size accepted by the LZ4 block API (`0x7E000000`).
const LZ4_MAX_INPUT_SIZE: usize = 0x7E00_0000;

// ── Memory probe ──────────────────────────────────────────────────────────────

/// Estimate the maximum usable buffer size for the benchmark.
///
/// Rounds `required_mem` up to the next 64 MiB boundary, adds two 64 MiB
/// headroom blocks, caps the result at [`MAX_MEMORY`], then subtracts two
/// further 64 MiB steps — one to account for compressor workspace and one
/// to retain headroom for the decompressor and output buffers. Returns the
/// result in bytes.
fn find_max_mem(required_mem: u64) -> usize {
    const STEP: u64 = 64 * 1024 * 1024; // 64 MiB boundary granularity

    // Round up to the next multiple of 2^26 (64 MiB).
    let mut mem = ((required_mem >> 26) + 1) << 26;

    // Add two STEP blocks of headroom before capping.
    mem = mem.saturating_add(2 * STEP);

    if mem > MAX_MEMORY as u64 {
        mem = MAX_MEMORY as u64;
    }

    // First subtraction: reserves space for the compressor's working memory,
    // which runs alongside the input buffer during measurement.
    if mem > STEP {
        mem -= STEP;
    } else {
        mem >>= 1;
    }

    // Second subtraction: retains headroom so the decompressor and output
    // buffer don't push the process into OOM territory.
    if mem > STEP {
        mem -= STEP;
    } else {
        mem >>= 1;
    }

    mem as usize
}

// ── Level iteration ───────────────────────────────────────────────────────────

/// Benchmark a single source buffer across a range of compression levels.
///
/// Extracts the basename of `display_name` (strips any leading path component,
/// checking `'\\'` first, then `'/'`) and iterates `c_level..=c_level_last`,
/// constructing a fresh [`CompressionStrategy`] and [`FrameDecompressor`] for
/// each level before delegating to [`bench_mem`]. Fresh per-level state ensures
/// that dictionary or codec internals from one level do not bleed into the next.
///
/// `file_sizes` holds per-file byte counts within `src`; an empty slice causes
/// `src` to be treated as a single logical file.
///
/// When the `realtime-priority` Cargo feature is enabled, the function
/// attempts to raise the process scheduling priority via `setpriority(2)` to
/// reduce OS-induced jitter in measurements.
pub fn bench_c_level(
    src: &[u8],
    display_name: &str,
    c_level: i32,
    c_level_last: i32,
    config: &BenchConfig,
    dict: &[u8],
    file_sizes: &[usize],
) -> io::Result<()> {
    // Strip path prefix: check '\\' first (Windows paths), then '/' (POSIX).
    // Using the last separator ensures deeply nested paths show only the filename.
    let display_name = if let Some(pos) = display_name.rfind('\\') {
        &display_name[pos + 1..]
    } else if let Some(pos) = display_name.rfind('/') {
        &display_name[pos + 1..]
    } else {
        display_name
    };

    // Raise scheduling priority to reduce OS-induced jitter in measurements.
    // Gated behind the `realtime-priority` feature to avoid surprising users
    // who run benchmarks without elevated privileges.
    #[cfg(feature = "realtime-priority")]
    {
        // SAFETY: setpriority(2) adjusts only the calling process's scheduling
        // priority; it has no memory-safety implications.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
        }
    }

    // At verbosity level 1 with no extra parameters, emit a one-line header
    // summarising the benchmark run before any per-level output appears.
    if config.display_level == 1 && config.additional_param == 0 {
        eprintln!(
            "bench {} {}: input {} bytes, {} seconds, {} KB blocks",
            crate::LZ4_VERSION_STRING,
            crate::LZ4_GIT_COMMIT_STRING,
            src.len(),
            config.nb_seconds,
            config.block_size >> 10,
        );
    }

    // Clamp: if the caller specified a last level below the first, run only the first level.
    let c_level_last = c_level_last.max(c_level);

    let mut bench_error = false;
    for l in c_level..=c_level_last {
        let mut strategy = build_compression_parameters(l, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        if let Err(e) = bench_mem(
            src,
            display_name,
            config,
            l,
            &mut *strategy,
            &mut decompressor,
            dict,
            file_sizes,
        ) {
            eprintln!("bench error at level {}: {}", l, e);
            bench_error = true;
        }
    }

    if bench_error {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "benchmark reported errors",
        ))
    } else {
        Ok(())
    }
}

// ── File loading ──────────────────────────────────────────────────────────────

/// Load multiple files into a single contiguous buffer.
///
/// Reads each path in `paths` sequentially into a buffer of up to
/// `buffer_size` bytes. When the buffer would overflow, the current file is
/// truncated to fit and loading stops — subsequent paths are not read.
/// Directories are skipped silently; a diagnostic is emitted at display
/// level ≥ 2.
///
/// Returns `(buffer, file_sizes)` where `file_sizes[i]` is the number of
/// bytes loaded for `paths[i]` (0 for skipped directories or unread paths).
///
/// # Errors
/// Returns `Err` if any file cannot be opened or read, or if the total bytes
/// loaded is zero (no data to benchmark).
pub fn load_files(
    paths: &[&str],
    buffer_size: usize,
    config: &BenchConfig,
) -> io::Result<(Vec<u8>, Vec<usize>)> {
    let mut buffer = vec![0u8; buffer_size];
    let mut file_sizes = vec![0usize; paths.len()];
    let mut pos: usize = 0;
    let mut total_size: usize = 0;
    // nb_files tracks how many paths to process; reduced when the buffer fills.
    let mut nb_files = paths.len();

    for (n, path) in paths.iter().enumerate() {
        if n >= nb_files {
            break;
        }

        // Skip directories — they carry no compressible data.
        let meta = fs::metadata(path)
            .map_err(|e| io::Error::new(e.kind(), format!("cannot stat {}: {}", path, e)))?;
        if meta.is_dir() {
            if config.display_level >= 2 {
                eprintln!("Ignoring {} directory...       ", path);
            }
            file_sizes[n] = 0;
            continue;
        }

        let file_size_on_disk = meta.len() as usize;
        if config.display_level >= 2 {
            eprint!("Loading {}...       \r", path);
        }

        // Truncate to remaining buffer capacity.
        let to_read = if file_size_on_disk > buffer_size - pos {
            // Buffer exhausted — stop processing further files after this one.
            nb_files = n;
            buffer_size - pos
        } else {
            file_size_on_disk
        };

        let mut f = fs::File::open(path).map_err(|e| {
            io::Error::new(e.kind(), format!("impossible to open file {}: {}", path, e))
        })?;

        // read_exact guarantees all requested bytes are read or returns an error,
        // avoiding silent short reads on slow or network-backed filesystems.
        f.read_exact(&mut buffer[pos..pos + to_read])
            .map_err(|e| io::Error::new(e.kind(), format!("could not read {}: {}", path, e)))?;

        pos += to_read;
        file_sizes[n] = to_read;
        total_size += to_read;
    }

    // Refuse to benchmark an empty corpus — measurements would be meaningless.
    if total_size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no data to bench",
        ));
    }

    buffer.truncate(pos);
    Ok((buffer, file_sizes))
}

// ── File table benchmark ──────────────────────────────────────────────────────

/// Benchmark a set of files across a range of compression levels.
///
/// Computes the total corpus size, derives a safe buffer limit with
/// [`find_max_mem`] (also capping at [`LZ4_MAX_INPUT_SIZE`]), loads the files
/// into that buffer with [`load_files`], then delegates to [`bench_c_level`]
/// for every level in `c_level..=c_level_last`.
pub fn bench_file_table(
    file_names: &[&str],
    c_level: i32,
    c_level_last: i32,
    dict: &[u8],
    config: &BenchConfig,
) -> io::Result<()> {
    // Sum the sizes of all non-directory paths to determine how much data to load.
    let total_size_to_load: u64 = file_names
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .filter(|m| !m.is_dir())
        .map(|m| m.len())
        .sum();

    // Request 3× the corpus size from find_max_mem (input + compressor + decompressor
    // buffers), then divide by 3 to obtain the usable input slice.
    let mut benched_size = find_max_mem(total_size_to_load.saturating_mul(3)) / 3;
    if benched_size == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "not enough memory"));
    }
    // No need to allocate more than the actual corpus.
    if benched_size as u64 > total_size_to_load {
        benched_size = total_size_to_load as usize;
    }
    if benched_size > LZ4_MAX_INPUT_SIZE {
        benched_size = LZ4_MAX_INPUT_SIZE;
        eprintln!(
            "File(s) bigger than LZ4's max input size; testing {} MB only...",
            benched_size >> 20
        );
    } else if (benched_size as u64) < total_size_to_load {
        eprintln!(
            "Not enough memory; testing {} MB only...",
            benched_size >> 20
        );
    }

    let (src_buffer, file_sizes) = load_files(file_names, benched_size, config)?;

    // Use " N files" as the display label when benchmarking multiple inputs so
    // per-level output lines remain readable without listing every filename.
    let display_name = if file_names.len() > 1 {
        format!(" {} files", file_names.len())
    } else {
        file_names[0].to_string()
    };

    bench_c_level(
        &src_buffer,
        &display_name,
        c_level,
        c_level_last,
        config,
        dict,
        &file_sizes,
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::config::BenchConfig;

    #[test]
    fn find_max_mem_sanity() {
        // Result must be > 0 and ≤ MAX_MEMORY.
        let r = find_max_mem(1024 * 1024);
        assert!(r > 0, "find_max_mem must return > 0 for non-trivial input");
        assert!(r <= MAX_MEMORY, "find_max_mem must not exceed MAX_MEMORY");
    }

    #[test]
    fn find_max_mem_large_input() {
        // Very large request should be capped at MAX_MEMORY.
        let r = find_max_mem(u64::MAX / 2);
        assert!(r <= MAX_MEMORY);
        assert!(r > 0);
    }

    #[test]
    fn find_max_mem_zero() {
        // Zero input: (((0>>26)+1)<<26) + 2*step = 3*step; minus 2 subtractions → step.
        let r = find_max_mem(0);
        assert!(r > 0);
        assert!(r <= MAX_MEMORY);
    }

    #[test]
    fn bench_c_level_three_levels() {
        // Verify that three distinct levels all complete without error.
        let src: Vec<u8> = (0u8..128).cycle().take(64 * 1024).collect();
        let mut config = BenchConfig::default();
        config.set_nb_seconds(0); // single pass — keeps the test fast
        config.set_notification_level(0); // suppress output
        let result = bench_c_level(&src, "test_input", 1, 3, &config, b"", &[]);
        assert!(
            result.is_ok(),
            "bench_c_level should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn bench_c_level_clamped_when_last_lt_first() {
        // When last < first, the range is clamped so only the first level runs.
        let src: Vec<u8> = (0u8..64).cycle().take(4096).collect();
        let mut config = BenchConfig::default();
        config.set_nb_seconds(0);
        config.set_notification_level(0);
        let result = bench_c_level(&src, "test", 3, 1, &config, b"", &[]);
        assert!(result.is_ok(), "clamped level range should succeed");
    }

    #[test]
    fn load_files_empty_returns_error() {
        // Empty file list → "no data to bench".
        let config = BenchConfig::default();
        let result = load_files(&[], 1024, &config);
        assert!(result.is_err(), "empty file list should return Err");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn load_files_reads_file_content() {
        use std::io::Write;
        // Write a temporary file, load it, verify content.
        let mut tmp = tempfile::NamedTempFile::new().expect("tmp file");
        let content = b"hello benchmark world!";
        tmp.write_all(content).unwrap();
        let path = tmp.path().to_str().unwrap().to_owned();

        let config = BenchConfig::default();
        let (buf, sizes) = load_files(&[&path], 4096, &config).expect("load should succeed");
        assert_eq!(&buf[..], content);
        assert_eq!(sizes[0], content.len());
    }

    #[test]
    fn load_files_truncates_when_buffer_small() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let content = b"abcdefghijklmnopqrstuvwxyz";
        tmp.write_all(content).unwrap();
        let path = tmp.path().to_str().unwrap().to_owned();

        let config = BenchConfig::default();
        // Buffer of 10 bytes: only first 10 bytes should be loaded.
        let (buf, sizes) =
            load_files(&[&path], 10, &config).expect("truncated load should succeed");
        assert_eq!(buf.len(), 10);
        assert_eq!(sizes[0], 10);
        assert_eq!(&buf[..], &content[..10]);
    }
}
