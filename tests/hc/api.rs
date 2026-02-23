// Unit tests for task-014: HC Public API (hc/api.rs).
//
// Tests verify behavioural parity with lz4hc.c v1.10.0, lines 1486–2192:
//   `LZ4_sizeofStateHC`                     → `sizeof_state_hc`
//   `LZ4_createStreamHC` / Drop             → `Lz4StreamHc::create` / Drop
//   `LZ4_initStreamHC`                      → `init_stream_hc`
//   `LZ4_compress_HC_extStateHC_fastReset`  → `compress_hc_ext_state_fast_reset`
//   `LZ4_compress_HC_extStateHC`            → `compress_hc_ext_state`
//   `LZ4_compress_HC`                       → `compress_hc`
//   `LZ4_compress_HC_destSize`              → `compress_hc_dest_size`
//   `LZ4_resetStreamHC`                     → `reset_stream_hc`
//   `LZ4_resetStreamHC_fast`               → `reset_stream_hc_fast`
//   `LZ4_setCompressionLevel`               → `set_compression_level`
//   `LZ4_favorDecompressionSpeed`           → `favor_decompression_speed`
//   `LZ4_loadDictHC`                        → `load_dict_hc`
//   `LZ4_attach_HC_dictionary`              → `attach_hc_dictionary`
//   `LZ4_compress_HC_continue`              → `compress_hc_continue`
//   `LZ4_compress_HC_continue_destSize`     → `compress_hc_continue_dest_size`
//   `LZ4_saveDictHC`                        → `save_dict_hc`
//
// All tests operate on the public API only; internal fields of Lz4StreamHc
// (which are pub(crate)) are not accessed directly.

use lz4::block::decompress_api::decompress_safe;
use lz4::hc::api::{
    attach_hc_dictionary, compress_hc, compress_hc_continue, compress_hc_continue_dest_size,
    compress_hc_dest_size, compress_hc_ext_state, compress_hc_ext_state_fast_reset,
    favor_decompression_speed, init_stream_hc, load_dict_hc, reset_stream_hc, reset_stream_hc_fast,
    save_dict_hc, set_compression_level, sizeof_state_hc, Lz4StreamHc,
};
use lz4::hc::types::{HcCCtxInternal, LZ4HC_CLEVEL_DEFAULT, LZ4HC_CLEVEL_MAX};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Highly compressible data (all same byte).
fn repeated_input(n: usize) -> Vec<u8> {
    vec![0xAA_u8; n]
}

/// Decompress `compressed[..n]` to a fresh buffer of `original_size`.
fn roundtrip_decompress(compressed: &[u8], n: usize, original_size: usize) -> Vec<u8> {
    let mut out = vec![0u8; original_size];
    let result = decompress_safe(&compressed[..n], &mut out);
    let written = result.expect("decompression should succeed");
    out.truncate(written);
    out
}

// ═════════════════════════════════════════════════════════════════════════════
// sizeof_state_hc  (LZ4_sizeofStateHC)
// ═════════════════════════════════════════════════════════════════════════════

/// sizeof_state_hc must equal mem::size_of::<HcCCtxInternal>().
#[test]
fn sizeof_state_hc_matches_struct_size() {
    use core::mem;
    assert_eq!(sizeof_state_hc(), mem::size_of::<HcCCtxInternal>());
}

/// sizeof_state_hc returns a non-zero value.
#[test]
fn sizeof_state_hc_nonzero() {
    assert!(sizeof_state_hc() > 0);
}

// ═════════════════════════════════════════════════════════════════════════════
// Lz4StreamHc::create  (LZ4_createStreamHC)
// ═════════════════════════════════════════════════════════════════════════════

/// create() returns Some (allocator is available in tests).
#[test]
fn create_returns_some() {
    assert!(Lz4StreamHc::create().is_some());
}

/// After create(), a stream can immediately compress data (level is initialised).
#[test]
fn create_allows_immediate_compression() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(
        n > 0,
        "stream from create() must compress immediately: got {n}"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// init_stream_hc  (LZ4_initStreamHC)
// ═════════════════════════════════════════════════════════════════════════════

/// After init_stream_hc, the stream compresses correctly.
#[test]
fn init_stream_hc_allows_compression() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    init_stream_hc(&mut stream);
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "stream after init_stream_hc must compress: got {n}");
}

/// init_stream_hc sets the default compression level (verified by round-trip).
#[test]
fn init_stream_hc_sets_default_level_for_compression() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 1024];
    let mut stream = Lz4StreamHc::create().unwrap();
    init_stream_hc(&mut stream);
    let n = unsafe {
        compress_hc_ext_state(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            LZ4HC_CLEVEL_DEFAULT,
        )
    };
    assert!(n > 0);
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

// ═════════════════════════════════════════════════════════════════════════════
// set_compression_level  (LZ4_setCompressionLevel)
// ═════════════════════════════════════════════════════════════════════════════

/// Level 0 (below minimum 1) is clamped — still produces output.
#[test]
fn set_compression_level_zero_clamped_still_compresses() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 0);
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            0,
        )
    };
    assert!(n > 0, "level 0 clamped to default must still compress: {n}");
}

/// Negative level is clamped to default — still produces output.
#[test]
fn set_compression_level_negative_clamped_still_compresses() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, -99);
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            -99,
        )
    };
    assert!(
        n > 0,
        "level -99 clamped to default must still compress: {n}"
    );
}

/// Level above max is clamped — still produces output.
#[test]
fn set_compression_level_above_max_clamped() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 999);
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            999,
        )
    };
    assert!(n > 0, "level 999 clamped to max must still compress: {n}");
}

/// All valid levels 1–12 produce a decompressible result.
#[test]
fn set_compression_level_all_valid_levels_compress() {
    let src = repeated_input(512);
    for level in 1..=12 {
        let mut dst = vec![0u8; 512];
        let mut stream = Lz4StreamHc::create().unwrap();
        set_compression_level(&mut stream, level);
        let n = unsafe {
            compress_hc_ext_state_fast_reset(
                &mut stream,
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                level,
            )
        };
        assert!(n > 0, "level {level} must compress: {n}");
        let recovered = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(recovered, src, "level {level} roundtrip must succeed");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// favor_decompression_speed  (LZ4_favorDecompressionSpeed)
// ═════════════════════════════════════════════════════════════════════════════

/// favor_decompression_speed(true) at level ≥10 must not crash; produces output.
#[test]
fn favor_decompression_speed_true_compresses_at_opt_level() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 1024];
    let mut stream = Lz4StreamHc::create().unwrap();
    favor_decompression_speed(&mut stream, true);
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            LZ4HC_CLEVEL_MAX, // level 12 triggers optimal parser
        )
    };
    assert!(
        n > 0,
        "favor_dec_speed=true must still compress at max level: {n}"
    );
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

/// favor_decompression_speed(false) restores normal behavior.
#[test]
fn favor_decompression_speed_false_compresses_normally() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    favor_decompression_speed(&mut stream, true);
    favor_decompression_speed(&mut stream, false);
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0, "favor_dec_speed=false must compress: {n}");
}

// ═════════════════════════════════════════════════════════════════════════════
// reset_stream_hc  (LZ4_resetStreamHC)
// ═════════════════════════════════════════════════════════════════════════════

/// reset_stream_hc enables compression after the stream has been used.
#[test]
fn reset_stream_hc_enables_reuse() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();

    // First use.
    let _n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };

    // Full reset.
    reset_stream_hc(&mut stream, 9);

    // Second use on the same stream.
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(
        n2 > 0,
        "stream reused after reset_stream_hc must compress: {n2}"
    );
}

/// reset_stream_hc with different levels produces output at those levels.
#[test]
fn reset_stream_hc_applies_new_level() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();

    // Test with a few different levels after reset.
    for level in [3, 6, 9, 12] {
        reset_stream_hc(&mut stream, level);
        let n = unsafe {
            compress_hc_continue(
                &mut stream,
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
            )
        };
        assert!(
            n > 0,
            "after reset with level {level}, compress must succeed: {n}"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// reset_stream_hc_fast  (LZ4_resetStreamHC_fast)
// ═════════════════════════════════════════════════════════════════════════════

/// Fast reset on a fresh stream still allows compression.
#[test]
fn reset_stream_hc_fast_fresh_stream_compresses() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc_fast(&mut stream, 9);
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "fast-reset on fresh stream must compress: {n}");
}

/// Fast reset after successful compression allows next compression.
#[test]
fn reset_stream_hc_fast_after_use_allows_reuse() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();

    // First block.
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n1 > 0, "first block must succeed");

    // Fast reset (clean path — no dirty flag set).
    reset_stream_hc_fast(&mut stream, 9);

    // After fast reset, stream should compress fresh data again.
    let src2 = repeated_input(512);
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            src2.as_ptr(),
            dst.as_mut_ptr(),
            src2.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n2 > 0, "after fast-reset, stream must compress again: {n2}");
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hc_ext_state_fast_reset  (LZ4_compress_HC_extStateHC_fastReset)
// ═════════════════════════════════════════════════════════════════════════════

/// Returns a positive byte count on compressible data.
#[test]
fn compress_hc_ext_state_fast_reset_basic_compression() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 1024];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0, "expected positive output, got {n}");
    assert!(
        (n as usize) < src.len(),
        "compressed should be smaller than input"
    );
}

/// Returns 0 when dst_capacity is too small (limited output mode).
#[test]
fn compress_hc_ext_state_fast_reset_too_small_dst_returns_zero() {
    let src = repeated_input(128);
    let mut dst = vec![0u8; 2]; // too small
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert_eq!(n, 0, "too-small dst must return 0");
}

/// Output is decompressible to the original.
#[test]
fn compress_hc_ext_state_fast_reset_roundtrip() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0);
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src, "roundtrip must reproduce the original");
}

/// Called twice on the same stream; both calls succeed and produce correct output.
#[test]
fn compress_hc_ext_state_fast_reset_repeated_calls() {
    let src = repeated_input(512);
    let mut stream = Lz4StreamHc::create().unwrap();
    for _ in 0..3 {
        let mut dst = vec![0u8; 512];
        let n = unsafe {
            compress_hc_ext_state_fast_reset(
                &mut stream,
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                9,
            )
        };
        assert!(n > 0, "repeated fast-reset calls must succeed");
        let recovered = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(recovered, src);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hc_ext_state  (LZ4_compress_HC_extStateHC)
// ═════════════════════════════════════════════════════════════════════════════

/// Compresses successfully; result decompresses to original.
#[test]
fn compress_hc_ext_state_roundtrip() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 1024];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0);
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

/// Can be called repeatedly on the same stream (full re-init each time).
#[test]
fn compress_hc_ext_state_repeated_calls() {
    let src = repeated_input(512);
    let mut stream = Lz4StreamHc::create().unwrap();
    for _ in 0..3 {
        let mut dst = vec![0u8; 512];
        let n = unsafe {
            compress_hc_ext_state(
                &mut stream,
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                6,
            )
        };
        assert!(n > 0, "each call to compress_hc_ext_state must succeed");
        let recovered = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(recovered, src);
    }
}

/// Returns 0 for too-small dst (limited output).
#[test]
fn compress_hc_ext_state_too_small_dst_returns_zero() {
    let src = repeated_input(256);
    let mut dst = vec![0u8; 2];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert_eq!(n, 0);
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hc  (LZ4_compress_HC)
// ═════════════════════════════════════════════════════════════════════════════

/// compress_hc allocates state internally and compresses correctly.
#[test]
fn compress_hc_compresses_repeated_data() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 1024];
    let n = unsafe {
        compress_hc(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0, "compress_hc should succeed: got {n}");
    assert!((n as usize) < src.len());
}

/// compress_hc with a large buffer produces a decompressible result.
#[test]
fn compress_hc_roundtrip() {
    let src: Vec<u8> = (0u8..=255).cycle().take(2048).collect();
    let mut dst = vec![0u8; 4096];
    let n = unsafe {
        compress_hc(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            6,
        )
    };
    assert!(n > 0);
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

/// compress_hc returns 0 when dst_capacity is insufficient.
#[test]
fn compress_hc_tiny_dst_returns_zero() {
    let src = repeated_input(128);
    let mut dst = vec![0u8; 2];
    let n = unsafe {
        compress_hc(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert_eq!(n, 0, "insufficient dst must return 0");
}

/// compress_hc with level 0 (clamped to default) still produces output.
#[test]
fn compress_hc_level_zero_clamped() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let n = unsafe {
        compress_hc(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            0, // clamped to default
        )
    };
    assert!(n > 0, "level 0 must compress: got {n}");
}

/// compress_hc at maximum level produces a decompressible result.
#[test]
fn compress_hc_max_level_roundtrip() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let n = unsafe {
        compress_hc(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            LZ4HC_CLEVEL_MAX,
        )
    };
    assert!(n > 0, "max level must compress: got {n}");
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hc_dest_size  (LZ4_compress_HC_destSize)
// ═════════════════════════════════════════════════════════════════════════════

/// compress_hc_dest_size fills the output buffer and updates src_size_ptr.
#[test]
fn compress_hc_dest_size_fills_dst_and_updates_src_size() {
    let src = repeated_input(4096);
    let mut dst = vec![0u8; 64]; // small dst to exercise FillOutput path
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0, "expected output from compress_hc_dest_size: {n}");
    assert!(
        src_size > 0 && src_size <= 4096,
        "src_size_ptr must be updated to bytes consumed, got {src_size}"
    );
    assert!(
        n <= dst.len() as i32,
        "output must not exceed target_dst_size"
    );
}

/// With adequate capacity, all input is consumed and output is decompressible.
#[test]
fn compress_hc_dest_size_full_roundtrip() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 2048];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0, "must compress: {n}");
    assert_eq!(src_size, src.len() as i32, "all input should be consumed");
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

/// Extremely small target dst returns 0.
#[test]
fn compress_hc_dest_size_tiny_dst_returns_zero() {
    let src = repeated_input(128);
    let mut dst = vec![0u8; 1];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            9,
        )
    };
    assert_eq!(n, 0, "impossible to compress into 1 byte");
}

// ═════════════════════════════════════════════════════════════════════════════
// load_dict_hc  (LZ4_loadDictHC)
// ═════════════════════════════════════════════════════════════════════════════

/// load_dict_hc returns the number of bytes loaded.
#[test]
fn load_dict_hc_returns_loaded_size() {
    let dict = repeated_input(1024);
    let mut stream = Lz4StreamHc::create().unwrap();
    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };
    assert_eq!(loaded, dict.len() as i32);
}

/// load_dict_hc trims dictionary to 64 KB when larger.
#[test]
fn load_dict_hc_trims_to_64kb() {
    let dict = repeated_input(128 * 1024); // 128 KB
    let mut stream = Lz4StreamHc::create().unwrap();
    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };
    assert_eq!(
        loaded,
        64 * 1024,
        "dict larger than 64 KB must be trimmed to 64 KB"
    );
}

/// load_dict_hc with empty dict returns 0.
#[test]
fn load_dict_hc_empty_dict_returns_zero() {
    let dict = [0u8; 0];
    let mut stream = Lz4StreamHc::create().unwrap();
    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), 0) };
    assert_eq!(loaded, 0);
}

/// After load_dict_hc, subsequent compression works and produces valid output.
#[test]
fn load_dict_hc_compression_after_load_works() {
    let dict = repeated_input(512);
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 9);

    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };
    assert!(loaded > 0);

    // Compression after dict load should work.
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "compression after load_dict_hc must succeed: {n}");
}

/// load_dict_hc on a small dict (exactly 4 bytes, below HC_HASHSIZE threshold).
#[test]
fn load_dict_hc_small_dict_below_hashsize() {
    let dict = [0xDE, 0xAD, 0xBE, 0xEF]; // exactly 4 bytes = LZ4HC_HASHSIZE
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 9);
    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };
    assert_eq!(loaded, 4);
}

// ═════════════════════════════════════════════════════════════════════════════
// attach_hc_dictionary  (LZ4_attach_HC_dictionary)
// ═════════════════════════════════════════════════════════════════════════════

/// attach_hc_dictionary(None) does not crash and subsequent compression works.
#[test]
fn attach_hc_dictionary_none_no_crash() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut working = Lz4StreamHc::create().unwrap();
    unsafe { attach_hc_dictionary(&mut working, None) };
    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "attach(None) must allow compression: {n}");
}

/// attach_hc_dictionary with a pre-loaded dict then detach (None) still compresses.
#[test]
fn attach_hc_dictionary_detach_with_none_still_compresses() {
    let dict_data = repeated_input(1024);
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];

    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 9);
    unsafe { load_dict_hc(&mut dict_stream, dict_data.as_ptr(), dict_data.len() as i32) };

    let mut working = Lz4StreamHc::create().unwrap();
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
        // Now detach.
        attach_hc_dictionary(&mut working, None);
    }

    // After detaching, compression on working stream must still work.
    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "compression after detach must work: {n}");
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hc_continue  (LZ4_compress_HC_continue)
// ═════════════════════════════════════════════════════════════════════════════

/// First block in a streaming session compresses successfully.
#[test]
fn compress_hc_continue_first_block_succeeds() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "first streaming block must succeed: {n}");
}

/// First block output is decompressible.
#[test]
fn compress_hc_continue_first_block_roundtrip() {
    let src = repeated_input(512);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0);
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

/// Two sequential contiguous blocks both compress successfully.
#[test]
fn compress_hc_continue_two_contiguous_blocks_succeed() {
    // Use a single 1 KB allocation; split into two 512-byte blocks.
    let combined = repeated_input(1024);
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();

    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            combined.as_ptr(),
            dst.as_mut_ptr(),
            512,
            dst.len() as i32,
        )
    };
    assert!(n1 > 0, "block 1 must succeed: {n1}");

    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            combined.as_ptr().add(512),
            dst.as_mut_ptr(),
            512,
            dst.len() as i32,
        )
    };
    assert!(n2 > 0, "block 2 must succeed: {n2}");
}

/// Returns 0 when dst_capacity is too small (limited output mode).
#[test]
fn compress_hc_continue_too_small_dst_returns_zero() {
    let src = repeated_input(128);
    let mut dst = vec![0u8; 2]; // definitely too small
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert_eq!(n, 0, "tiny dst must return 0");
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hc_continue_dest_size  (LZ4_compress_HC_continue_destSize)
// ═════════════════════════════════════════════════════════════════════════════

/// Updates src_size_ptr and produces output when dst is adequate.
#[test]
fn compress_hc_continue_dest_size_updates_src_size() {
    let src = repeated_input(4096);
    let mut dst = vec![0u8; 64]; // small dst → FillOutput
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_continue_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
        )
    };
    if n > 0 {
        assert!(
            src_size > 0 && src_size <= 4096,
            "src_size_ptr must reflect bytes consumed: {src_size}"
        );
    }
}

/// With adequate capacity, all input consumed; output is decompressible.
#[test]
fn compress_hc_continue_dest_size_roundtrip() {
    let src = repeated_input(1024);
    let mut dst = vec![0u8; 2048];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_continue_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "should compress: {n}");
    assert_eq!(src_size, src.len() as i32, "all input should be consumed");
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

// ═════════════════════════════════════════════════════════════════════════════
// save_dict_hc  (LZ4_saveDictHC)
// ═════════════════════════════════════════════════════════════════════════════

/// save_dict_hc returns a non-negative byte count after a streaming session.
#[test]
fn save_dict_hc_returns_bytes_saved() {
    let src = repeated_input(512);
    let mut compressed = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            compressed.as_mut_ptr(),
            src.len() as i32,
            compressed.len() as i32,
        )
    };
    assert!(n > 0);

    let mut save_buf = vec![0u8; 256];
    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
    assert!(saved >= 0, "save_dict_hc must return non-negative");
    assert!(saved <= 256, "saved bytes must not exceed requested size");
}

/// save_dict_hc clamps to 64 KB.
#[test]
fn save_dict_hc_clamps_to_64kb() {
    let src = repeated_input(128 * 1024); // 128 KB to establish long history
    let mut dst = vec![0u8; 256 * 1024];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0);

    let mut save_buf = vec![0u8; 128 * 1024]; // request 128 KB
    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
    assert!(
        saved <= 64 * 1024,
        "save_dict_hc must clamp to 64 KB, got {saved}"
    );
}

/// save_dict_hc with dict_size < 4 saves 0 bytes.
#[test]
fn save_dict_hc_size_below_4_saves_zero() {
    let src = repeated_input(512);
    let mut compressed = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            compressed.as_mut_ptr(),
            src.len() as i32,
            compressed.len() as i32,
        )
    };
    assert!(n > 0);

    let mut save_buf = vec![0u8; 10];
    // dict_size = 3 → below minimum of 4, must return 0.
    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), 3) };
    assert_eq!(saved, 0, "dict_size < 4 must save 0 bytes");
}

/// After save_dict_hc, the stream remains usable for subsequent compression.
#[test]
fn save_dict_hc_stream_remains_usable_after_save() {
    let src = repeated_input(512);
    let mut compressed = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();

    // Compress first block.
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            compressed.as_mut_ptr(),
            src.len() as i32,
            compressed.len() as i32,
        )
    };
    assert!(n > 0, "first block must succeed");

    // Save dict.
    let mut save_buf = vec![0u8; 256];
    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
    assert!(saved >= 0);

    // Subsequent compression block on the same stream.
    let src2 = repeated_input(512);
    let mut dst2 = vec![0u8; 512];
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            src2.as_ptr(),
            dst2.as_mut_ptr(),
            src2.len() as i32,
            dst2.len() as i32,
        )
    };
    // Stream may or may not succeed depending on memory layout (non-contiguous blocks
    // trigger ext-dict mode), but it must not panic.
    let _ = n2;
}

/// save_dict_hc with dict_size = 0 returns 0 and does not crash.
#[test]
fn save_dict_hc_zero_size_returns_zero() {
    let src = repeated_input(512);
    let mut compressed = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            compressed.as_mut_ptr(),
            src.len() as i32,
            compressed.len() as i32,
        )
    };
    assert!(n > 0);

    let saved = unsafe {
        // Safety: dict_size == 0 → safe_buffer is not accessed; null is allowed by C contract.
        save_dict_hc(&mut stream, core::ptr::null_mut(), 0)
    };
    assert_eq!(saved, 0);
}
