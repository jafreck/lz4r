//! Core benchmark timing loop for the lz4r benchmarking subsystem.
//!
//! This module provides [`bench_mem`], which drives the adaptive compression
//! and decompression timing loop used by all benchmark entry points.  The loop
//! self-calibrates: after each timing pass it adjusts the number of compression
//! and decompression iterations so that each subsequent pass takes roughly one
//! second of wall time, yielding stable throughput measurements regardless of
//! input size.
//!
//! Buffer management is block-oriented.  The source is split into fixed-size
//! chunks; each chunk owns pre-allocated compressed and result [`Vec`]s so that
//! no heap allocation occurs inside the hot timing paths.
//!
//! After every decompression pass an XXH64 round-trip checksum verifies that
//! the decompressed output is byte-for-byte identical to the original source.

use std::io;
use std::time::{Duration, Instant};

use xxhash_rust::xxh64::xxh64 as xxh64_oneshot;

use super::config::{BenchConfig, ACTIVEPERIOD_NANOSEC, COOLPERIOD_SEC, DECOMP_MULT, MB,
                    TIMELOOP_NANOSEC};
use super::compress_strategy::CompressionStrategy;
use super::decompress_binding::{decompress_frame_block, FrameDecompressor};

use crate::block::{compress_bound, decompress_safe_using_dict};

// ── LZ4 constants ─────────────────────────────────────────────────────────────

/// Maximum input size accepted by the LZ4 block API (`0x7E000000`).
const LZ4_MAX_INPUT_SIZE: usize = 0x7E00_0000;



// ── Internal block descriptor ─────────────────────────────────────────────────

/// Per-block data used by the timing loop.
///
/// Each block owns its pre-allocated storage, eliminating unsafe pointer
/// arithmetic and ensuring no heap allocation occurs inside the hot timing loops.
struct BlockParam {
    /// Byte range [src_offset .. src_offset + src_size] within `src`.
    src_offset: usize,
    src_size: usize,

    /// Pre-allocated compressed buffer.  Sized to `LZ4_compressBound(src_size)`
    /// so `compress_block` never reallocates inside the timing loop.
    c_buf: Vec<u8>,

    /// Actual compressed size after the last compression pass.
    c_size: usize,

    /// Pre-allocated decompression result buffer.  Sized to `res_capa`.
    res_buf: Vec<u8>,

    /// Actual decompressed size after the last decompression pass.
    res_size: usize,
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Source and compressed data carrier for callers that manage their own buffers.
pub struct BlockParams {
    pub src: Vec<u8>,
    pub compressed: Vec<u8>,
}

/// Result of one `bench_mem` run.
#[derive(Debug, Clone)]
pub struct BenchResult {
    /// Total bytes in the (reconstructed) source.
    pub src_size: usize,
    /// Total bytes in the compressed output.
    pub compressed_size: usize,
    /// Compression ratio (`src_size / compressed_size`).
    pub ratio: f64,
    /// Compression throughput in MB/s.
    pub compress_speed_mb_s: f64,
    /// Decompression throughput in MB/s.
    pub decompress_speed_mb_s: f64,
    /// Compression level used.
    pub c_level: i32,
}

// ── bench_mem ─────────────────────────────────────────────────────────────────

/// Core adaptive benchmark timing loop.
///
/// Compresses and decompresses `src` repeatedly, self-calibrating the number of
/// iterations after each pass to target roughly one second of wall time per
/// phase.  Returns throughput measurements once both phases have accumulated
/// enough wall time (governed by `config.nb_seconds`).
///
/// # Parameters
/// - `src`            — input data to compress and then decompress back.
/// - `display_name`   — label printed in the progress line (≤17 chars displayed).
/// - `config`         — runtime benchmark parameters.
/// - `c_level`        — compression level (for display only; `strategy` was
///                      already constructed from this level by the caller).
/// - `strategy`       — mutable compression strategy (owns any stream state).
/// - `decompressor`   — frame decompressor used in `decode_only` mode.
/// - `dict`           — optional dictionary bytes; empty slice means no dict.
/// - `file_sizes`     — per-file byte counts within `src`.  An empty slice
///                      means treat the entire `src` as a single file.
///
/// # Returns
/// `Ok(BenchResult)` on success, or `Err` if compression or checksum verification fails.
pub fn bench_mem(
    src: &[u8],
    display_name: &str,
    config: &BenchConfig,
    c_level: i32,
    strategy: &mut dyn CompressionStrategy,
    decompressor: &mut FrameDecompressor,
    dict: &[u8],
    file_sizes: &[usize],
) -> io::Result<BenchResult> {
    let src_size = src.len();

    // ── block-size selection ──────────────────────────────────────────────────
    //
    // Use the configured block size when it is ≥ 32 bytes and we are not in
    // decode-only mode; otherwise fall back to the total source size so that
    // the entire input is treated as a single block.  Add 1 when src is empty
    // to prevent division by zero in the loop-count calculation below.
    let block_size = if config.block_size >= 32 && !config.decode_only {
        config.block_size
    } else {
        src_size
    } + if src_size == 0 { 1 } else { 0 };

    // Upper bound on the number of blocks across all files.  Each file may
    // contribute a partial final block, so we add `nb_files` to account for
    // those remainders.  When file_sizes is empty, treat src as a single file.
    let nb_files = if file_sizes.is_empty() { 1 } else { file_sizes.len() };
    let max_nb_blocks = (src_size + block_size - 1) / block_size + nb_files;

    // ── multipliers for decode-only mode ──────────────────────────────────────
    let dec_multiplier: usize = if config.decode_only { 255 } else { 1 };
    let max_in_size: usize = LZ4_MAX_INPUT_SIZE / dec_multiplier;
    let max_dec_size: usize = if src_size < max_in_size {
        src_size * dec_multiplier
    } else {
        LZ4_MAX_INPUT_SIZE
    };

    // `max_dec_size` feeds into per-block `res_capa` below.  Vec allocation
    // failures propagate naturally as panics (or Err with allocator_api); no
    // explicit check is needed here.
    let _ = max_dec_size; // referenced indirectly via per-block res_capa

    // ── build block table ─────────────────────────────────────────────────────
    //
    // Iterate per-file so that the last block of each file may be smaller than
    // `block_size`.  When `file_sizes` is empty, the entire `src` is treated
    // as a single file.
    let mut block_table: Vec<BlockParam> = Vec::with_capacity(max_nb_blocks);
    {
        let single_file_sizes = [src_size];
        let effective_sizes: &[usize] = if file_sizes.is_empty() {
            &single_file_sizes
        } else {
            file_sizes
        };

        let mut src_offset = 0usize;
        for &file_size_item in effective_sizes {
            let mut remaining = file_size_item;
            let nb_blocks_for_this_file = (remaining + block_size - 1) / block_size;
            for _ in 0..nb_blocks_for_this_file {
                let this_block_size = remaining.min(block_size);

                // Worst-case compressed size; pre-allocated so the timing loop never reallocates.
                let c_room = compress_bound(this_block_size as i32) as usize;

                // Maximum decompressed output size, capped at LZ4_MAX_INPUT_SIZE.
                let res_max_size = this_block_size * dec_multiplier;
                let res_capa = if this_block_size < max_in_size {
                    res_max_size
                } else {
                    LZ4_MAX_INPUT_SIZE
                };

                // Pre-size both Vecs so timing-loop calls don't reallocate.
                let mut c_buf = vec![0u8; c_room];
                let res_buf = vec![0u8; res_capa];

                // In decode-only mode the compressed buffer is pre-filled with
                // the raw source bytes so decompression can run without a prior
                // compression pass.
                let c_size_init = if config.decode_only {
                    let copy_len = this_block_size.min(c_buf.len());
                    c_buf[..copy_len].copy_from_slice(&src[src_offset..src_offset + copy_len]);
                    copy_len
                } else {
                    0
                };

                block_table.push(BlockParam {
                    src_offset,
                    src_size: this_block_size,
                    c_buf,
                    c_size: c_size_init,
                    res_buf,
                    res_size: 0,
                });

                src_offset += this_block_size;
                remaining -= this_block_size;
            }
        }

        // Ensure block_table is never empty (guard for zero-length src).
        if block_table.is_empty() {
            let c_room = compress_bound(0) as usize;
            block_table.push(BlockParam {
                src_offset: 0,
                src_size: 0,
                c_buf: vec![0u8; c_room],
                c_size: 0,
                res_buf: vec![],
                res_size: 0,
            });
        }
    }

    // ── truncate display name to 17 chars ───────────────────────────────────
    let display_name: &str = if display_name.len() > 17 {
        &display_name[display_name.len() - 17..]
    } else {
        display_name
    };

    // ── initial warm-up: fill compressed buffers ───────────────────────────────
    // Filling with a known byte pattern before the first pass ensures cache
    // and memory state is consistent across runs.
    for block in &mut block_table {
        block.c_buf.fill(b' ');
    }

    // ── adaptive benchmark timing loop ────────────────────────────────────────
    let mut fastest_c_ns: u64 = u64::MAX;
    let mut fastest_d_ns: u64 = u64::MAX;
    let crc_orig: u64 = xxh64_oneshot(src, 0);

    let mut cool_time = Instant::now();
    let max_time_ns: u64 = config.nb_seconds as u64 * TIMELOOP_NANOSEC + 100;
    let mut nb_compression_loops: u32 = ((5 * MB) / (src_size + 1)) as u32 + 1;
    let mut nb_decode_loops: u32 = ((200 * MB) / (src_size + 1)) as u32 + 1;
    let mut total_c_time_ns: u64 = 0;
    let mut total_d_time_ns: u64 = 0;

    // In decode-only mode the compression phase is skipped entirely.
    let mut c_completed: bool = config.decode_only;
    let mut d_completed: bool = false;

    const NB_MARKS: usize = 4;
    const MARKS: [&str; NB_MARKS] = [" |", " /", " =", "\\"];
    let mut mark_nb: usize = 0;

    let mut c_size: usize = src_size; // C line 440
    let mut total_r_size: usize = src_size; // C line 441
    let mut ratio: f64 = 0.0;

    if config.display_level >= 2 {
        eprint!("\r{:79}\r", "");
    }

    // When nb_seconds is 0, run exactly one loop iteration (single-pass mode).
    if config.nb_seconds == 0 {
        nb_compression_loops = 1;
        nb_decode_loops = 1;
    }

    let mut bench_error = false;

    while !c_completed || !d_completed {
        // ── overheat protection ────────────────────────────────────────────────
        // If the active measurement period exceeds the threshold, sleep briefly
        // to allow the CPU to cool before taking the next timing sample.
        if cool_time.elapsed().as_nanos() as u64 > ACTIVEPERIOD_NANOSEC {
            if config.display_level >= 2 {
                eprint!("\rcooling down ...    \r");
            }
            std::thread::sleep(Duration::from_secs(COOLPERIOD_SEC));
            cool_time = Instant::now();
        }

        // ── compression phase ───────────────────────────────────────────────────
        if config.display_level >= 2 {
            eprint!(
                "{}-{:<17.17} :{:>10} ->\r",
                MARKS[mark_nb], display_name, total_r_size
            );
        }

        if !c_completed {
            // Fill compressed buffers with a known byte pattern before timing
            // to prevent the OS from skipping physical page mapping (demand-zero).
            for block in &mut block_table {
                block.c_buf.fill(0xE5);
                block.c_size = 0;
            }
        }

        // Brief sleep before starting the timed region.  Two back-to-back
        // 1 ms sleeps approximate a "wait for next OS scheduler tick" without
        // requiring a platform-specific tick API.
        std::thread::sleep(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(1));

        if !c_completed {
            let time_start = Instant::now();

            'compress_outer: for _ in 0..nb_compression_loops {
                // `compress_block` resets any internal stream state on each call
                // (required by the `CompressionStrategy` contract).
                for block in &mut block_table {
                    let compressed = strategy.compress_block(
                        &src[block.src_offset..block.src_offset + block.src_size],
                        &mut block.c_buf,
                    );
                    match compressed {
                        Ok(n) => {
                            block.c_size = n;
                        }
                        Err(_) => {
                            eprintln!(
                                "LZ4 compression failed on block at offset {} ",
                                block.src_offset
                            );
                            bench_error = true;
                            break 'compress_outer;
                        }
                    }
                }
            }

            let duration_ns = time_start.elapsed().as_nanos() as u64;
            if duration_ns > 0 {
                let per_loop = duration_ns / nb_compression_loops as u64;
                if per_loop < fastest_c_ns {
                    fastest_c_ns = per_loop;
                }
                // aim for ~1 second per pass
                nb_compression_loops =
                    (TIMELOOP_NANOSEC / fastest_c_ns) as u32 + 1;
            } else {
                // duration was 0; multiply to avoid an infinite spin
                assert!(nb_compression_loops < 40_000_000);
                nb_compression_loops *= 100;
            }
            total_c_time_ns += duration_ns;
            c_completed = total_c_time_ns > max_time_ns;

            // Sum compressed sizes across blocks to compute the overall ratio.
            c_size = block_table.iter().map(|b| b.c_size).sum::<usize>();
            if c_size == 0 {
                c_size = 1; // avoid div by 0
            }
            ratio = total_r_size as f64 / c_size as f64;

            mark_nb = (mark_nb + 1) % NB_MARKS;
            if config.display_level >= 2 {
                eprint!(
                    "{}-{:<17.17} :{:>10} ->{:>10} ({:5.3}),{:6.1} MB/s\r",
                    MARKS[mark_nb],
                    display_name,
                    total_r_size,
                    c_size,
                    ratio,
                    (total_r_size as f64 / fastest_c_ns as f64) * 1000.0,
                );
            }
        }

        // ── decompression phase ───────────────────────────────────────────────

        // Fill result buffers with a known byte pattern before timing to prevent
        // demand-zero pages from inflating measurement latency.
        if !d_completed {
            for block in &mut block_table {
                block.res_buf.fill(0xD6);
            }
        }

        // Sleep before the timed decompression region.  A 5 ms pause gives the
        // CPU a moment to settle; the follow-up 1 ms sleep approximates a
        // scheduler-tick boundary without a platform tick API.
        std::thread::sleep(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(1));

        if !d_completed {
            let time_start = Instant::now();

            'decode_outer: for _ in 0..nb_decode_loops {
                for block in &mut block_table {
                    let in_max_size = i32::MAX as usize / dec_multiplier;
                    let res_capa = if block.src_size < in_max_size {
                        block.src_size * dec_multiplier
                    } else {
                        i32::MAX as usize
                    };

                    if config.decode_only {
                        // LZ4 Frame decompression path.  `decompress_frame_block`
                        // appends into a caller-supplied Vec, so we use a
                        // temporary buffer and then copy into the pre-sized res_buf.
                        let mut tmp = Vec::new();
                        match decompress_frame_block(
                            decompressor,
                            &block.c_buf[..block.c_size],
                            &mut tmp,
                            res_capa,
                            config.skip_checksums,
                        ) {
                            Ok(n) => {
                                block.res_buf[..n].copy_from_slice(&tmp[..n]);
                                block.res_size = n;
                            }
                            Err(_) => {
                                eprintln!(
                                    "LZ4F_decompress() failed on block at offset {} of size {} \nIs input using LZ4 Frame format ?",
                                    block.src_offset, block.src_size
                                );
                                bench_error = true;
                                break 'decode_outer;
                            }
                        }
                    } else {
                        // LZ4 raw-block decompression with optional dictionary.
                        // SAFETY: `block.c_buf` and `block.res_buf` are distinct,
                        // valid heap allocations.  `dict` is a valid slice (possibly
                        // empty when no dictionary is used).
                        let result = unsafe {
                            decompress_safe_using_dict(
                                block.c_buf.as_ptr(),
                                block.res_buf.as_mut_ptr(),
                                block.c_size,
                                res_capa,
                                dict.as_ptr(),
                                dict.len(),
                            )
                        };
                        match result {
                            Ok(regen) => {
                                block.res_size = regen;
                            }
                            Err(_) => {
                                eprintln!(
                                    "decompress_safe_using_dict() failed on block at offset {} of size {} ",
                                    block.src_offset, block.src_size
                                );
                                bench_error = true;
                                break 'decode_outer;
                            }
                        }
                    }
                }
            }

            let duration_ns = time_start.elapsed().as_nanos() as u64;
            if duration_ns > 0 {
                let per_loop = duration_ns / nb_decode_loops as u64;
                if per_loop < fastest_d_ns {
                    fastest_d_ns = per_loop;
                }
                nb_decode_loops = (TIMELOOP_NANOSEC / fastest_d_ns) as u32 + 1;
            } else {
                assert!(nb_decode_loops < 40_000_000);
                nb_decode_loops *= 100;
            }
            total_d_time_ns += duration_ns;
            d_completed = total_d_time_ns > (DECOMP_MULT as u64 * max_time_ns);
        }

        // In decode-only mode the "source" size is the decompressed output
        // from this pass rather than the original input length.
        if config.decode_only {
            total_r_size = block_table.iter().map(|b| b.res_size).sum();
        }

        mark_nb = (mark_nb + 1) % NB_MARKS;
        ratio = if c_size > 0 {
            total_r_size as f64 / c_size as f64
        } else {
            0.0
        };

        let compress_speed = if fastest_c_ns > 0 && fastest_c_ns != u64::MAX {
            (total_r_size as f64 / fastest_c_ns as f64) * 1000.0
        } else {
            0.0
        };
        let decompress_speed = if fastest_d_ns > 0 && fastest_d_ns != u64::MAX {
            (total_r_size as f64 / fastest_d_ns as f64) * 1000.0
        } else {
            0.0
        };

        if config.display_level >= 2 {
            eprint!(
                "{}-{:<17.17} :{:>10} ->{:>10} ({:5.3}),{:6.1} MB/s, {:6.1} MB/s\r",
                MARKS[mark_nb],
                display_name,
                total_r_size,
                c_size,
                ratio,
                compress_speed,
                decompress_speed,
            );
        }

        // ── CRC checksum verification ────────────────────────────────────────
        // Compare an XXH64 digest of the round-tripped data against the digest
        // of the original source.  Any mismatch indicates a decompression bug.
        if !config.decode_only {
            let mut result_bytes: Vec<u8> =
                Vec::with_capacity(block_table.iter().map(|b| b.res_size).sum());
            for block in &block_table {
                result_bytes.extend_from_slice(&block.res_buf[..block.res_size]);
            }
            let crc_check = xxh64_oneshot(&result_bytes, 0);
            if crc_orig != crc_check {
                // Find the first mismatching byte (mirrors C lines 578–594).
                eprintln!(
                    "\n!!! WARNING !!! {:17} : Invalid Checksum : {:x} != {:x}   ",
                    display_name, crc_orig, crc_check
                );
                bench_error = true;
                // Scan for the first differing byte to aid debugging.
                for (u, (&src_b, &res_b)) in
                    src.iter().zip(result_bytes.iter()).enumerate()
                {
                    if src_b != res_b {
                        eprintln!("Decoding error at pos {} ", u);
                        break;
                    }
                    if u == src_size - 1 {
                        eprintln!("no difference detected");
                    }
                }
                break;
            }
        }
    } // while !c_completed || !d_completed

    // ── final output ──────────────────────────────────────────────────────────
    let compress_speed_mb_s = if fastest_c_ns > 0 && fastest_c_ns != u64::MAX {
        (src_size as f64 / fastest_c_ns as f64) * 1000.0
    } else {
        0.0
    };
    let decompress_speed_mb_s = if fastest_d_ns > 0 && fastest_d_ns != u64::MAX {
        (src_size as f64 / fastest_d_ns as f64) * 1000.0
    } else {
        0.0
    };

    if config.display_level >= 2 {
        eprintln!("{:2}#", c_level);
    }

    // Quiet mode: print a single summary line without a progress spinner.
    if config.display_level == 1 {
        print!(
            "-{:<3}{:>11} ({:5.3}) {:6.2} MB/s {:6.1} MB/s  {}",
            c_level, c_size, ratio, compress_speed_mb_s, decompress_speed_mb_s, display_name,
        );
        if config.additional_param != 0 {
            print!(" (param={})", config.additional_param);
        }
        println!();
    }

    if bench_error {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "benchmark reported errors (compression or checksum failure)",
        ));
    }

    Ok(BenchResult {
        src_size,
        compressed_size: c_size,
        ratio,
        compress_speed_mb_s,
        decompress_speed_mb_s,
        c_level,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::compress_strategy::build_compression_parameters;
    use crate::bench::config::BenchConfig;
    use crate::bench::decompress_binding::FrameDecompressor;

    /// Generate a 1 MiB test buffer (repeating byte pattern, easily compressible).
    fn make_1mb_buf() -> Vec<u8> {
        (0u8..=255).cycle().take(1024 * 1024).collect()
    }

    #[test]
    fn bench_mem_1mb_level1() {
        // Smoke test: bench_mem must complete without error on a 1 MB buffer
        // at level 1 and return plausible throughput values.
        let src = make_1mb_buf();
        let config = {
            let mut c = BenchConfig::default();
            c.set_nb_seconds(1); // keep the test fast
            c
        };
        let mut strategy = build_compression_parameters(1, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        let result = bench_mem(&src, "test", &config, 1, &mut *strategy, &mut decompressor, b"", &[]);
        let r = result.expect("bench_mem should succeed on a 1 MB buffer at level 1");
        assert!(r.compressed_size > 0, "compressed_size must be non-zero");
        assert!(
            r.compress_speed_mb_s > 0.0,
            "compression throughput must be positive"
        );
        assert!(
            r.decompress_speed_mb_s > 0.0,
            "decompression throughput must be positive"
        );
    }

    #[test]
    fn bench_mem_crc_passes() {
        // Checksum must match after round-trip.
        let src: Vec<u8> = b"hello world! ".iter().cycle().take(64 * 1024).cloned().collect();
        let config = {
            let mut c = BenchConfig::default();
            c.set_nb_seconds(1);
            c
        };
        let mut strategy = build_compression_parameters(1, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        let result = bench_mem(&src, "crctest", &config, 1, &mut *strategy, &mut decompressor, b"", &[]);
        assert!(result.is_ok(), "CRC check must pass: {:?}", result.err());
    }

    #[test]
    fn bench_mem_zero_seconds_single_pass() {
        // nb_seconds=0 → single pass (nbCompressionLoops=1, nbDecodeLoops=1).
        let src: Vec<u8> = (0u8..128).cycle().take(4096).collect();
        let config = {
            let mut c = BenchConfig::default();
            c.set_nb_seconds(0);
            c
        };
        let mut strategy = build_compression_parameters(1, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        let result = bench_mem(&src, "zerotest", &config, 1, &mut *strategy, &mut decompressor, b"", &[]);
        assert!(result.is_ok(), "single-pass bench_mem must succeed");
    }

    #[test]
    fn bench_mem_hc_level() {
        // HC compression (c_level=9) must also round-trip correctly.
        let src: Vec<u8> = b"aaaa".iter().cycle().take(32 * 1024).cloned().collect();
        let config = {
            let mut c = BenchConfig::default();
            c.set_nb_seconds(1);
            c
        };
        let mut strategy = build_compression_parameters(9, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        let result = bench_mem(&src, "hctest", &config, 9, &mut *strategy, &mut decompressor, b"", &[]);
        assert!(result.is_ok(), "HC bench_mem must succeed: {:?}", result.err());
    }

    #[test]
    fn bench_result_fields_plausible() {
        let src = make_1mb_buf();
        let config = {
            let mut c = BenchConfig::default();
            c.set_nb_seconds(1);
            c
        };
        let mut strategy = build_compression_parameters(1, src.len(), src.len());
        let mut decompressor = FrameDecompressor::new();
        let r = bench_mem(&src, "fields", &config, 1, &mut *strategy, &mut decompressor, b"", &[])
            .unwrap();
        assert_eq!(r.src_size, 1024 * 1024);
        assert_eq!(r.c_level, 1);
        assert!(r.ratio > 0.0);
        assert!(r.compressed_size < src.len(), "compressible input should shrink");
    }
}
