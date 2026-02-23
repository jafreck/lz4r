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
    if n > 0 { n } else { 0 }
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
