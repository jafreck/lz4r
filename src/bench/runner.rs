/*
    bench/runner.rs — File loading, memory probe, and level iteration
    Migrated from lz4-1.10.0/programs/bench.c (lines 622–753)

    Original copyright (C) Yann Collet 2012-2020 — GPL v2 License.

    Migration notes:
    - `BMK_findMaxMem` (lines 622–643): The C implementation probes available
      memory by repeatedly attempting malloc at decreasing sizes. In Rust,
      Vec::with_capacity provides natural OOM handling; the probing loop is
      replaced with a purely arithmetic computation that mirrors C exactly for
      the common case (ample memory): the probe loop subtracts `step` once,
      then the "keep some space available" block subtracts `step` a second time.
      Both subtractions are performed explicitly in `find_max_mem`.
    - `BMK_benchCLevel` (lines 646–672): Preserves the basename-extraction
      logic (strrchr on '\\' then '/') and the level-iteration loop. A fresh
      CompressionStrategy and FrameDecompressor are constructed for each level,
      matching C's per-level re-initialisation. `SET_REALTIME_PRIORITY` is
      implemented via `libc::setpriority` and gated behind the
      `realtime-priority` Cargo feature.
    - `BMK_loadFiles` (lines 678–708): Translated to idiomatic Rust using
      `std::fs::File` + `read_exact`. Skips directories via `fs::metadata`.
      Returns `Err` when no data was loaded (mirrors END_PROCESS(12, …)).
    - `BMK_benchFileTable` (lines 710–753): Uses the simplified `find_max_mem`
      helper. The LZ4_MAX_INPUT_SIZE cap and "not enough memory" truncation
      messages are preserved. Returns `io::Error` instead of calling the C
      END_PROCESS() macro that would have terminated the process.
*/

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
/// Migrated from `BMK_findMaxMem` (bench.c lines 622–643).
///
/// The C implementation probes available memory via repeated `malloc` attempts.
/// In Rust, `Vec` allocation fails naturally on OOM, so the probe loop is
/// replaced with a purely arithmetic computation that matches C's common-case
/// result (when the first malloc succeeds): round `required_mem` up to the next
/// 64 MiB boundary, add two 64 MiB headroom blocks, cap at [`MAX_MEMORY`],
/// subtract one 64 MiB step (simulating the probe loop's single iteration), then
/// subtract another 64 MiB step ("keep some space available", bench.c lines 638–640).
fn find_max_mem(required_mem: u64) -> usize {
    const STEP: u64 = 64 * 1024 * 1024; // 64 MB — matches bench.c line 624

    // C: requiredMem = (((requiredMem >> 26) + 1) << 26)
    // Rounds up to the next multiple of 2^26 (64 MiB).
    let mut mem = ((required_mem >> 26) + 1) << 26;

    // C: requiredMem += 2*step
    mem = mem.saturating_add(2 * STEP);

    // C: if (requiredMem > maxMemory) requiredMem = maxMemory
    if mem > MAX_MEMORY as u64 {
        mem = MAX_MEMORY as u64;
    }

    // C: probe loop (bench.c lines 631–635): in the common case (ample memory),
    // the loop executes exactly once, subtracting `step` before a successful malloc.
    if mem > STEP {
        mem -= STEP;
    } else {
        mem >>= 1;
    }

    // C: keep some space available (bench.c lines 638–640)
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
/// Migrated from `BMK_benchCLevel` (bench.c lines 646–672).
///
/// Extracts the basename of `display_name` (strips any leading path component
/// using the exact C `strrchr` logic — checking `'\\'` first, then `'/'` only
/// if no `'\\'` is found), then iterates
/// `c_level..=c_level_last`, constructing a fresh [`CompressionStrategy`] and
/// [`FrameDecompressor`] for each level and delegating to [`bench_mem`].
///
/// `file_sizes` — per-file byte counts within `src` (C `fileSizes`/`nbFiles`);
/// empty slice means treat `src` as a single file.
///
/// When the `realtime-priority` Cargo feature is enabled, the function
/// attempts to raise the process scheduling priority via `setpriority(2)`
/// (equivalent to `SET_REALTIME_PRIORITY` in platform.h).
pub fn bench_c_level(
    src: &[u8],
    display_name: &str,
    c_level: i32,
    c_level_last: i32,
    config: &BenchConfig,
    dict: &[u8],
    file_sizes: &[usize],
) -> io::Result<()> {
    // Strip path prefix — mirrors C strrchr logic (bench.c lines 653–655).
    // C checks '\\'first; only if not found does it check '/'.
    let display_name = if let Some(pos) = display_name.rfind('\\') {
        &display_name[pos + 1..]
    } else if let Some(pos) = display_name.rfind('/') {
        &display_name[pos + 1..]
    } else {
        display_name
    };

    // SET_REALTIME_PRIORITY (bench.c line 657); feature-gated.
    #[cfg(feature = "realtime-priority")]
    {
        // SAFETY: setpriority only modifies scheduling priority for this
        // process; no memory safety implications.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
        }
    }

    // Mirrors C: if (g_displayLevel == 1 && !g_additionalParam) DISPLAY(...)
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

    // C: if (cLevelLast < cLevel) cLevelLast = cLevel;
    let c_level_last = c_level_last.max(c_level);

    let mut bench_error = false;
    for l in c_level..=c_level_last {
        let mut strategy = build_compression_parameters(l, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        if let Err(e) = bench_mem(src, display_name, config, l, &mut *strategy, &mut decompressor, dict, file_sizes) {
            eprintln!("bench error at level {}: {}", l, e);
            bench_error = true;
        }
    }

    if bench_error {
        Err(io::Error::new(io::ErrorKind::Other, "benchmark reported errors"))
    } else {
        Ok(())
    }
}

// ── File loading ──────────────────────────────────────────────────────────────

/// Load multiple files into a single contiguous buffer.
///
/// Migrated from `BMK_loadFiles` (bench.c lines 678–708).
///
/// Reads each path in `paths` into a buffer of up to `buffer_size` bytes.
/// Loading stops early (after the current file is truncated) when the buffer
/// would overflow, mirroring the C behaviour where `nbFiles` is reduced.
/// Directories are silently skipped with a message at display level ≥ 2.
///
/// Returns `(buffer, file_sizes)` where `file_sizes[i]` is the number of
/// bytes read for `paths[i]` (0 for skipped directories or unread files).
///
/// # Errors
/// Returns `Err` if any file cannot be opened or read, or if `total_size == 0`
/// (mirrors `END_PROCESS(12, "no data to bench")`).
pub fn load_files(
    paths: &[&str],
    buffer_size: usize,
    config: &BenchConfig,
) -> io::Result<(Vec<u8>, Vec<usize>)> {
    let mut buffer = vec![0u8; buffer_size];
    let mut file_sizes = vec![0usize; paths.len()];
    let mut pos: usize = 0;
    let mut total_size: usize = 0;
    // nb_files may be reduced when the buffer is full (mirrors C nbFiles=n).
    let mut nb_files = paths.len();

    for (n, path) in paths.iter().enumerate() {
        if n >= nb_files {
            break;
        }

        // Skip directories (mirrors UTIL_isDirectory check, bench.c 687–690).
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

        // Truncate to remaining buffer capacity (mirrors C lines 695–698).
        let to_read = if file_size_on_disk > buffer_size - pos {
            // Buffer too small — stop after this file (C: nbFiles=n).
            nb_files = n;
            buffer_size - pos
        } else {
            file_size_on_disk
        };

        let mut f = fs::File::open(path)
            .map_err(|e| io::Error::new(e.kind(), format!("impossible to open file {}: {}", path, e)))?;

        // Use read_exact to match C's fread-with-error-check semantics.
        f.read_exact(&mut buffer[pos..pos + to_read])
            .map_err(|e| io::Error::new(e.kind(), format!("could not read {}: {}", path, e)))?;

        pos += to_read;
        file_sizes[n] = to_read;
        total_size += to_read;
    }

    // C: if (totalSize == 0) END_PROCESS(12, "no data to bench")
    if total_size == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no data to bench"));
    }

    buffer.truncate(pos);
    Ok((buffer, file_sizes))
}

// ── File table benchmark ──────────────────────────────────────────────────────

/// Benchmark a set of files across a range of compression levels.
///
/// Migrated from `BMK_benchFileTable` (bench.c lines 710–753).
///
/// Loads all files into a single contiguous buffer (capped by the memory
/// estimate from [`find_max_mem`] and by [`LZ4_MAX_INPUT_SIZE`]), then calls
/// [`bench_c_level`] for each level in `c_level..=c_level_last`.
pub fn bench_file_table(
    file_names: &[&str],
    c_level: i32,
    c_level_last: i32,
    dict: &[u8],
    config: &BenchConfig,
) -> io::Result<()> {
    // Compute total file size (equivalent to UTIL_getTotalFileSize).
    let total_size_to_load: u64 = file_names
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .filter(|m| !m.is_dir())
        .map(|m| m.len())
        .sum();

    // Memory allocation & restrictions (bench.c lines 723–735).
    // C: benchedSize = BMK_findMaxMem(totalSizeToLoad * 3) / 3
    let mut benched_size = find_max_mem(total_size_to_load.saturating_mul(3)) / 3;
    if benched_size == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "not enough memory"));
    }
    // C: if ((U64)benchedSize > totalSizeToLoad) benchedSize = (size_t)totalSizeToLoad
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

    // Load input buffer (bench.c lines 737–738).
    let (src_buffer, file_sizes) = load_files(file_names, benched_size, config)?;

    // Display name: " N files" for multiple files, else the single filename
    // (bench.c lines 741–742: snprintf(mfName, ..., " %u files")).
    let display_name = if file_names.len() > 1 {
        format!(" {} files", file_names.len())
    } else {
        file_names[0].to_string()
    };

    // Benchmark (bench.c lines 743–747).
    bench_c_level(&src_buffer, &display_name, c_level, c_level_last, config, dict, &file_sizes)
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
        // Verification criterion from migration plan:
        // "bench_c_level(src, 1, 3, config) calls bench_mem for levels 1, 2, and 3"
        let src: Vec<u8> = (0u8..128).cycle().take(64 * 1024).collect();
        let mut config = BenchConfig::default();
        config.set_nb_seconds(0); // single pass — keeps the test fast
        config.set_notification_level(0); // suppress output
        let result = bench_c_level(&src, "test_input", 1, 3, &config, b"", &[]);
        assert!(result.is_ok(), "bench_c_level should succeed: {:?}", result.err());
    }

    #[test]
    fn bench_c_level_clamped_when_last_lt_first() {
        // C: if (cLevelLast < cLevel) cLevelLast = cLevel — single level run.
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
        let (buf, sizes) = load_files(&[&path], 10, &config).expect("truncated load should succeed");
        assert_eq!(buf.len(), 10);
        assert_eq!(sizes[0], 10);
        assert_eq!(&buf[..], &content[..10]);
    }
}
