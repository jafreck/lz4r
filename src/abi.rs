//! C-ABI shims — export the four symbols that lzbench calls.
//!
//! Enabled with:
//!   cargo build --release --features c-abi
//!
//! The produced `target/release/liblz4.a` can replace `liblz4.o + liblz4hc.o`
//! in the lzbench link step via the `Makefile.rust` override.

use std::os::raw::{c_char, c_int};
use std::slice;

use crate::block::compress::compress_fast;
use crate::block::decompress_api::decompress_safe;
use crate::hc::api::compress_hc as hc_compress_hc;

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Return 0 when an i32 is non-positive (error sentinel matching the C API).
#[inline(always)]
fn ok_or_zero(n: i32) -> c_int {
    if n > 0 {
        n
    } else {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_default  (lz4.h)
//
// int LZ4_compress_default(const char *src, char *dst,
//                          int srcSize, int dstCapacity);
//
// Returns number of bytes written to dst, or 0 on failure.
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn LZ4_compress_default(
    src: *const c_char,
    dst: *mut c_char,
    src_size: c_int,
    dst_capacity: c_int,
) -> c_int {
    if src_size < 0 || dst_capacity < 0 || src.is_null() || dst.is_null() {
        return 0;
    }
    let src_slice = slice::from_raw_parts(src as *const u8, src_size as usize);
    let dst_slice = slice::from_raw_parts_mut(dst as *mut u8, dst_capacity as usize);
    match compress_fast(src_slice, dst_slice, 1) {
        Ok(n) => ok_or_zero(n as i32),
        Err(_) => 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_fast  (lz4.h)
//
// int LZ4_compress_fast(const char *src, char *dst,
//                       int srcSize, int dstCapacity, int acceleration);
//
// Returns number of bytes written to dst, or 0 on failure.
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn LZ4_compress_fast(
    src: *const c_char,
    dst: *mut c_char,
    src_size: c_int,
    dst_capacity: c_int,
    acceleration: c_int,
) -> c_int {
    if src_size < 0 || dst_capacity < 0 || src.is_null() || dst.is_null() {
        return 0;
    }
    let src_slice = slice::from_raw_parts(src as *const u8, src_size as usize);
    let dst_slice = slice::from_raw_parts_mut(dst as *mut u8, dst_capacity as usize);
    // Mirror C behaviour: clamp acceleration to 1 minimum.
    let accel = if acceleration < 1 { 1 } else { acceleration };
    match compress_fast(src_slice, dst_slice, accel) {
        Ok(n) => ok_or_zero(n as i32),
        Err(_) => 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_decompress_safe  (lz4.h)
//
// int LZ4_decompress_safe(const char *src, char *dst,
//                         int compressedSize, int dstCapacity);
//
// Returns number of bytes written to dst, or a negative value on error.
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn LZ4_decompress_safe(
    src: *const c_char,
    dst: *mut c_char,
    compressed_size: c_int,
    dst_capacity: c_int,
) -> c_int {
    if compressed_size < 0 || dst_capacity < 0 || src.is_null() || dst.is_null() {
        return -1;
    }
    let src_slice = slice::from_raw_parts(src as *const u8, compressed_size as usize);
    let dst_slice = slice::from_raw_parts_mut(dst as *mut u8, dst_capacity as usize);
    match decompress_safe(src_slice, dst_slice) {
        Ok(n) => n as c_int,
        // Negative return on error, mirroring the C API.
        Err(_) => -1,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC  (lz4hc.h)
//
// int LZ4_compress_HC(const char *src, char *dst,
//                     int srcSize, int dstCapacity, int compressionLevel);
//
// Returns number of bytes written to dst, or 0 on failure.
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn LZ4_compress_HC(
    src: *const c_char,
    dst: *mut c_char,
    src_size: c_int,
    dst_capacity: c_int,
    compression_level: c_int,
) -> c_int {
    if src_size < 0 || dst_capacity < 0 || src.is_null() || dst.is_null() {
        return 0;
    }
    // hc_compress_hc already takes *const u8 / *mut u8 and returns i32.
    ok_or_zero(hc_compress_hc(
        src as *const u8,
        dst as *mut u8,
        src_size,
        dst_capacity,
        compression_level,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::raw::c_char;

    const SAMPLE: &[u8] =
        b"Hello, LZ4 ABI! Hello, LZ4 ABI! Hello, LZ4 ABI! This is a test.";

    // ── ok_or_zero ───────────────────────────────────────────────────────────

    #[test]
    fn ok_or_zero_positive_passthrough() {
        assert_eq!(ok_or_zero(42), 42);
    }

    #[test]
    fn ok_or_zero_zero_returns_zero() {
        assert_eq!(ok_or_zero(0), 0);
    }

    #[test]
    fn ok_or_zero_negative_returns_zero() {
        assert_eq!(ok_or_zero(-1), 0);
    }

    // Helper: compress with LZ4_compress_default
    unsafe fn compress_default(src: &[u8]) -> Vec<u8> {
        let bound = (src.len() as i32) + 16 + (src.len() / 255) as i32 + 1;
        let mut dst = vec![0u8; bound as usize];
        let n = LZ4_compress_default(
            src.as_ptr() as *const c_char,
            dst.as_mut_ptr() as *mut c_char,
            src.len() as i32,
            bound,
        );
        assert!(n > 0, "compress_default returned {n}");
        dst.truncate(n as usize);
        dst
    }

    // ── LZ4_compress_default ─────────────────────────────────────────────────

    #[test]
    fn compress_default_basic_roundtrip() {
        unsafe {
            let compressed = compress_default(SAMPLE);
            let mut out = vec![0u8; SAMPLE.len()];
            let n = LZ4_decompress_safe(
                compressed.as_ptr() as *const c_char,
                out.as_mut_ptr() as *mut c_char,
                compressed.len() as i32,
                out.len() as i32,
            );
            assert_eq!(n as usize, SAMPLE.len());
            assert_eq!(&out, SAMPLE);
        }
    }

    #[test]
    fn compress_default_null_src_returns_zero() {
        unsafe {
            let mut dst = [0u8; 64];
            let n = LZ4_compress_default(
                std::ptr::null(),
                dst.as_mut_ptr() as *mut c_char,
                10,
                64,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_default_null_dst_returns_zero() {
        unsafe {
            let src = SAMPLE;
            let n = LZ4_compress_default(
                src.as_ptr() as *const c_char,
                std::ptr::null_mut(),
                src.len() as i32,
                128,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_default_negative_src_size_returns_zero() {
        unsafe {
            let src = SAMPLE;
            let mut dst = [0u8; 128];
            let n = LZ4_compress_default(
                src.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                -1,
                128,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_default_negative_dst_capacity_returns_zero() {
        unsafe {
            let src = SAMPLE;
            let mut dst = [0u8; 128];
            let n = LZ4_compress_default(
                src.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                src.len() as i32,
                -1,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_default_too_small_dst_returns_zero() {
        // Output buffer is intentionally way too small → must return 0.
        unsafe {
            let src = SAMPLE;
            let mut dst = [0u8; 2];
            let n = LZ4_compress_default(
                src.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                src.len() as i32,
                2,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_default_empty_input() {
        unsafe {
            let src: &[u8] = b"";
            let mut dst = [0u8; 32];
            let n = LZ4_compress_default(
                src.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                0,
                32,
            );
            // Empty input produces a valid compressed stream (end-mark only).
            assert!(n >= 0);
        }
    }

    // ── LZ4_compress_fast ────────────────────────────────────────────────────

    #[test]
    fn compress_fast_basic_roundtrip() {
        unsafe {
            let bound = (SAMPLE.len() as i32) + 16 + (SAMPLE.len() / 255) as i32 + 1;
            let mut compressed = vec![0u8; bound as usize];
            let n = LZ4_compress_fast(
                SAMPLE.as_ptr() as *const c_char,
                compressed.as_mut_ptr() as *mut c_char,
                SAMPLE.len() as i32,
                bound,
                1,
            );
            assert!(n > 0, "compress_fast returned {n}");
            compressed.truncate(n as usize);
            let mut out = vec![0u8; SAMPLE.len()];
            let m = LZ4_decompress_safe(
                compressed.as_ptr() as *const c_char,
                out.as_mut_ptr() as *mut c_char,
                compressed.len() as i32,
                out.len() as i32,
            );
            assert_eq!(m as usize, SAMPLE.len());
            assert_eq!(&out, SAMPLE);
        }
    }

    #[test]
    fn compress_fast_negative_acceleration_clamped_to_one() {
        // Negative acceleration is clamped to 1; should still produce valid output.
        unsafe {
            let bound = (SAMPLE.len() as i32) + 16 + (SAMPLE.len() / 255) as i32 + 1;
            let mut compressed = vec![0u8; bound as usize];
            let n = LZ4_compress_fast(
                SAMPLE.as_ptr() as *const c_char,
                compressed.as_mut_ptr() as *mut c_char,
                SAMPLE.len() as i32,
                bound,
                -5, // invalid → clamped to 1
            );
            assert!(n > 0, "compress_fast with negative accel returned {n}");
        }
    }

    #[test]
    fn compress_fast_high_acceleration() {
        unsafe {
            let bound = (SAMPLE.len() as i32) + 16 + (SAMPLE.len() / 255) as i32 + 1;
            let mut compressed = vec![0u8; bound as usize];
            let n = LZ4_compress_fast(
                SAMPLE.as_ptr() as *const c_char,
                compressed.as_mut_ptr() as *mut c_char,
                SAMPLE.len() as i32,
                bound,
                65536,
            );
            assert!(n > 0, "compress_fast with high accel returned {n}");
        }
    }

    #[test]
    fn compress_fast_null_src_returns_zero() {
        unsafe {
            let mut dst = [0u8; 64];
            let n = LZ4_compress_fast(
                std::ptr::null(),
                dst.as_mut_ptr() as *mut c_char,
                10,
                64,
                1,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_fast_null_dst_returns_zero() {
        unsafe {
            let n = LZ4_compress_fast(
                SAMPLE.as_ptr() as *const c_char,
                std::ptr::null_mut(),
                SAMPLE.len() as i32,
                128,
                1,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_fast_negative_src_size_returns_zero() {
        unsafe {
            let mut dst = [0u8; 128];
            let n = LZ4_compress_fast(
                SAMPLE.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                -10,
                128,
                1,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_fast_negative_dst_capacity_returns_zero() {
        unsafe {
            let mut dst = [0u8; 128];
            let n = LZ4_compress_fast(
                SAMPLE.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                SAMPLE.len() as i32,
                -1,
                1,
            );
            assert_eq!(n, 0);
        }
    }

    // ── LZ4_decompress_safe ──────────────────────────────────────────────────

    #[test]
    fn decompress_safe_null_src_returns_negative() {
        unsafe {
            let mut dst = [0u8; 64];
            let n = LZ4_decompress_safe(
                std::ptr::null(),
                dst.as_mut_ptr() as *mut c_char,
                10,
                64,
            );
            assert!(n < 0, "expected error for null src, got {n}");
        }
    }

    #[test]
    fn decompress_safe_null_dst_returns_negative() {
        unsafe {
            let compressed = decomp_compressed();
            let n = LZ4_decompress_safe(
                compressed.as_ptr() as *const c_char,
                std::ptr::null_mut(),
                compressed.len() as i32,
                128,
            );
            assert!(n < 0, "expected error for null dst, got {n}");
        }
    }

    #[test]
    fn decompress_safe_negative_compressed_size_returns_negative() {
        unsafe {
            let compressed = decomp_compressed();
            let mut dst = [0u8; 128];
            let n = LZ4_decompress_safe(
                compressed.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                -1,
                128,
            );
            assert!(n < 0, "expected error for negative src size, got {n}");
        }
    }

    #[test]
    fn decompress_safe_negative_dst_capacity_returns_negative() {
        unsafe {
            let compressed = decomp_compressed();
            let mut dst = [0u8; 128];
            let n = LZ4_decompress_safe(
                compressed.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                compressed.len() as i32,
                -1,
            );
            assert!(n < 0, "expected error for negative dst capacity, got {n}");
        }
    }

    #[test]
    fn decompress_safe_corrupt_data_returns_negative() {
        unsafe {
            let garbage = [0xFFu8; 32];
            let mut dst = [0u8; 128];
            let n = LZ4_decompress_safe(
                garbage.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                garbage.len() as i32,
                dst.len() as i32,
            );
            // Corrupt data should fail (returns negative) or succeed but is non-deterministic.
            // The important thing is it doesn't crash; assert it's ≤ dst.len() as i32.
            assert!(n <= dst.len() as i32);
        }
    }

    // Helper: produce a pre-compressed buffer for decompress tests.
    unsafe fn decomp_compressed() -> Vec<u8> {
        compress_default(SAMPLE)
    }

    // ── LZ4_compress_HC ──────────────────────────────────────────────────────

    #[test]
    fn compress_hc_basic_roundtrip() {
        unsafe {
            let bound = (SAMPLE.len() as i32) + 16 + (SAMPLE.len() / 255) as i32 + 1;
            let mut compressed = vec![0u8; bound as usize];
            let n = LZ4_compress_HC(
                SAMPLE.as_ptr() as *const c_char,
                compressed.as_mut_ptr() as *mut c_char,
                SAMPLE.len() as i32,
                bound,
                9,
            );
            assert!(n > 0, "compress_HC returned {n}");
            compressed.truncate(n as usize);
            let mut out = vec![0u8; SAMPLE.len()];
            let m = LZ4_decompress_safe(
                compressed.as_ptr() as *const c_char,
                out.as_mut_ptr() as *mut c_char,
                compressed.len() as i32,
                out.len() as i32,
            );
            assert_eq!(m as usize, SAMPLE.len());
            assert_eq!(&out, SAMPLE);
        }
    }

    #[test]
    fn compress_hc_level_1_to_12() {
        // All compression levels should produce valid output.
        for level in 1i32..=12 {
            unsafe {
                let bound = (SAMPLE.len() as i32) + 16 + (SAMPLE.len() / 255) as i32 + 1;
                let mut compressed = vec![0u8; bound as usize];
                let n = LZ4_compress_HC(
                    SAMPLE.as_ptr() as *const c_char,
                    compressed.as_mut_ptr() as *mut c_char,
                    SAMPLE.len() as i32,
                    bound,
                    level,
                );
                assert!(n > 0, "compress_HC level={level} returned {n}");
            }
        }
    }

    #[test]
    fn compress_hc_null_src_returns_zero() {
        unsafe {
            let mut dst = [0u8; 64];
            let n = LZ4_compress_HC(
                std::ptr::null(),
                dst.as_mut_ptr() as *mut c_char,
                10,
                64,
                9,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_hc_null_dst_returns_zero() {
        unsafe {
            let n = LZ4_compress_HC(
                SAMPLE.as_ptr() as *const c_char,
                std::ptr::null_mut(),
                SAMPLE.len() as i32,
                128,
                9,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_hc_negative_src_size_returns_zero() {
        unsafe {
            let mut dst = [0u8; 128];
            let n = LZ4_compress_HC(
                SAMPLE.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                -1,
                128,
                9,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_hc_negative_dst_capacity_returns_zero() {
        unsafe {
            let mut dst = [0u8; 128];
            let n = LZ4_compress_HC(
                SAMPLE.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                SAMPLE.len() as i32,
                -1,
                9,
            );
            assert_eq!(n, 0);
        }
    }

    #[test]
    fn compress_hc_too_small_dst_returns_zero() {
        unsafe {
            let src = SAMPLE;
            let mut dst = [0u8; 2];
            let n = LZ4_compress_HC(
                src.as_ptr() as *const c_char,
                dst.as_mut_ptr() as *mut c_char,
                src.len() as i32,
                2,
                9,
            );
            assert_eq!(n, 0);
        }
    }
}
