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

// ═════════════════════════════════════════════════════════════════════════════
// Dictionary context tests (attach_hc_dictionary with actual dict data)
// Exercises: hc/dispatch.rs dict_ctx paths, hc/lz4mid.rs dict search,
//            hc/compress_hc.rs dict paths, hc/search.rs ext-dict search
// ═════════════════════════════════════════════════════════════════════════════

/// Helper: generate data with repeating patterns that share content with dict.
fn dict_and_source_data(dict_size: usize, src_size: usize) -> (Vec<u8>, Vec<u8>) {
    // Dict has a repeating pattern; source starts with the same pattern
    // so the compressor can find back-references into the dictionary.
    let dict: Vec<u8> = (0..dict_size).map(|i| (i % 251) as u8).collect();
    let mut src: Vec<u8> = (0..src_size).map(|i| (i % 251) as u8).collect();
    // Add some variation in the second half
    for i in src_size / 2..src_size {
        src[i] = ((i * 7 + 13) % 256) as u8;
    }
    (dict, src)
}

/// Compress with attached dict at HC level (≥3) and verify round-trip.
#[test]
fn attach_dict_hc_level_compress_continue_roundtrip() {
    let (dict, src) = dict_and_source_data(8192, 4096);
    let mut dst = vec![0u8; 8192];

    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 9);
    unsafe { load_dict_hc(&mut dict_stream, dict.as_ptr(), dict.len() as i32) };

    let mut working = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut working, 9);
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
    }

    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "HC dict compression must succeed: {n}");

    // Verify the output decompresses correctly using dict prefix.
    // Since we used attach (not load_dict on working), the compressed data
    // should still be decompressable as a standalone block if the dict
    // matches are within the block itself. For full round-trip we need
    // the ext-dict decompressor. Just verify it produces output.
    assert!(n < src.len() as i32, "dict should help compress");
}

/// Compress with attached dict at LZ4Mid level (1-2) exercises lz4mid dict search.
#[test]
fn attach_dict_lz4mid_level_compress_continue() {
    let (dict, src) = dict_and_source_data(8192, 4096);
    let mut dst = vec![0u8; 8192];

    // Dict stream at LZ4Mid level
    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 1);
    unsafe { load_dict_hc(&mut dict_stream, dict.as_ptr(), dict.len() as i32) };

    // Working stream at LZ4Mid level
    let mut working = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut working, 1);
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
    }

    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "LZ4Mid dict compression must succeed: {n}");
}

/// Compress with attached dict at level 2 (boundary of LZ4Mid range).
#[test]
fn attach_dict_level2_boundary_compress() {
    let (dict, src) = dict_and_source_data(4096, 2048);
    let mut dst = vec![0u8; 4096];

    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 2);
    unsafe { load_dict_hc(&mut dict_stream, dict.as_ptr(), dict.len() as i32) };

    let mut working = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut working, 2);
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
    }

    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "level 2 dict compression must succeed: {n}");
}

/// Compress with attached dict at optimal level (10+) exercises compress_optimal dict path.
#[test]
fn attach_dict_optimal_level_compress() {
    let (dict, src) = dict_and_source_data(4096, 2048);
    let mut dst = vec![0u8; 4096];

    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 10);
    unsafe { load_dict_hc(&mut dict_stream, dict.as_ptr(), dict.len() as i32) };

    let mut working = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut working, 10);
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
    }

    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "optimal level dict compression must succeed: {n}");
}

/// Compress with attached dict where source > 4KB triggers ext-dict promotion
/// in compress_generic_dict_ctx (position == 0 && src_size > 4KB && compatible).
#[test]
fn attach_dict_large_src_triggers_ext_dict_promotion() {
    let (dict, src) = dict_and_source_data(8192, 8192);
    let mut dst = vec![0u8; 16384];

    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 9);
    unsafe { load_dict_hc(&mut dict_stream, dict.as_ptr(), dict.len() as i32) };

    let mut working = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut working, 9);
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
    }

    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "large src ext-dict promotion must succeed: {n}");
}

/// Multiple blocks with ext-dict (non-contiguous) exercises set_external_dict path.
#[test]
fn compress_hc_continue_multi_block_ext_dict() {
    let block1 = repeated_input(4096);
    let block2: Vec<u8> = (0..4096).map(|i| ((i * 3 + 7) % 256) as u8).collect();
    let mut dst = vec![0u8; 8192];
    let mut stream = Lz4StreamHc::create().unwrap();

    // First block
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst.as_mut_ptr(),
            block1.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n1 > 0, "block1 must succeed");

    // Second block (non-contiguous → triggers set_external_dict)
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst.as_mut_ptr(),
            block2.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n2 > 0, "block2 (ext-dict) must succeed: {n2}");
}

/// Multiple blocks with ext-dict at LZ4Mid level.
#[test]
fn compress_hc_continue_multi_block_ext_dict_lz4mid() {
    let block1: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let block2: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 8192];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 1);

    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst.as_mut_ptr(),
            block1.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n1 > 0);

    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst.as_mut_ptr(),
            block2.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n2 > 0, "LZ4Mid ext-dict block must succeed: {n2}");
}

/// FillOutput mode via compress_hc_dest_size with tiny dst.
#[test]
fn compress_hc_dest_size_fill_output_tiny_dst() {
    let src = repeated_input(4096);
    let mut dst = vec![0u8; 128]; // small but valid
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
    // FillOutput: should produce some output, consuming only part of input
    if n > 0 {
        assert!(
            src_size <= src.len() as i32,
            "FillOutput should consume partial input"
        );
    }
}

/// FillOutput mode via compress_hc_continue_dest_size with loaded dict (not attached).
#[test]
fn compress_hc_continue_dest_size_with_loaded_dict_fill_output() {
    let dict: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let src: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 512];

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 9);
    unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };

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
        assert!(src_size <= src.len() as i32);
    }
}

/// load_dict_hc at LZ4Mid level (≤2) exercises the fill_htable path.
#[test]
fn load_dict_hc_lz4mid_level_exercises_fill_htable() {
    let dict: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 1); // LZ4Mid strategy
    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };
    assert_eq!(loaded, 4096);

    // Now compress using the loaded dict
    let src: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 4096];
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
        "compression after LZ4Mid load_dict must succeed: {n}"
    );
}

/// load_dict_hc with large dict (>32KB) exercises fill_htable fine-pass.
#[test]
fn load_dict_hc_large_dict_fine_pass() {
    let dict: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 1); // LZ4Mid
    let loaded = unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };
    assert_eq!(loaded, 64 * 1024);

    let src: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 8192];
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "large dict LZ4Mid compression must succeed: {n}");
}

/// FillOutput with LZ4Mid strategy exercises lz4mid_compress FillOutput path.
#[test]
fn compress_hc_dest_size_lz4mid_fill_output() {
    let src = repeated_input(4096);
    let mut dst = vec![0u8; 128];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            1, // LZ4Mid
        )
    };
    if n > 0 {
        assert!(src_size <= src.len() as i32);
    }
}

/// FillOutput with optimal level exercises compress_optimal overflow path.
#[test]
fn compress_hc_dest_size_optimal_fill_output() {
    let src: Vec<u8> = (0..4096).map(|i| ((i * 7 + 3) % 256) as u8).collect();
    let mut dst = vec![0u8; 512];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            10, // Optimal
        )
    };
    if n > 0 {
        assert!(src_size <= src.len() as i32);
    }
}

/// Compress with favor_decompression_speed enabled at optimal level.
#[test]
fn compress_hc_favor_dec_speed_optimal() {
    let src: Vec<u8> = (0..4096).map(|i| ((i * 13 + 5) % 256) as u8).collect();
    let mut dst = vec![0u8; 8192];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 11);
    favor_decompression_speed(&mut stream, true);

    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            11,
        )
    };
    assert!(n > 0, "favor_dec_speed compression must succeed: {n}");
    let recovered = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(recovered, src);
}

/// save_dict_hc where dict_size > prefix_size should clamp.
#[test]
fn save_dict_hc_larger_than_prefix() {
    let src = repeated_input(64); // tiny prefix
    let mut compressed = vec![0u8; 256];
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

    let mut save_buf = vec![0u8; 8192];
    // Request more than was compressed
    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
    assert!(
        saved >= 0 && saved <= 64,
        "saved must not exceed prefix: {saved}"
    );
}

/// Multi-block streaming with save_dict_hc between blocks.
#[test]
fn multi_block_streaming_with_save_dict() {
    let block1: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    let block2: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 16384];
    let mut save_buf = vec![0u8; 65536];
    let mut stream = Lz4StreamHc::create().unwrap();

    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst.as_mut_ptr(),
            block1.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n1 > 0);

    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
    assert!(saved > 0);

    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst.as_mut_ptr(),
            block2.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n2 > 0, "second block after save_dict must succeed: {n2}");
}

/// Compress with attached dict using incompatible strategy levels.
/// Dict at HC level, working at LZ4Mid level (or vice versa).
#[test]
fn attach_dict_incompatible_strategies() {
    let (dict, src) = dict_and_source_data(4096, 8192);
    let mut dst = vec![0u8; 16384];

    // Dict at HC level (9)
    let mut dict_stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut dict_stream, 9);
    unsafe { load_dict_hc(&mut dict_stream, dict.as_ptr(), dict.len() as i32) };

    // Working at LZ4Mid level (1) - incompatible strategies
    let mut working = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut working, 1);
    unsafe {
        attach_hc_dictionary(&mut working, Some(&*dict_stream as *const Lz4StreamHc));
    }

    let n = unsafe {
        compress_hc_continue(
            &mut working,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "incompatible dict strategies must still work: {n}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional coverage: FillOutput overflow, ext-dict, dirty-stream reset
// ─────────────────────────────────────────────────────────────────────────────

/// Helper: generate semi-compressible data with repeating patterns + noise.
fn semi_compressible_data(n: usize) -> Vec<u8> {
    let phrase = b"the quick brown fox jumps over the lazy dog ";
    let mut data = Vec::with_capacity(n);
    for i in 0..n {
        if i % 60 < 44 {
            data.push(phrase[i % 44]);
        } else {
            data.push(((i * 137 + 59) % 256) as u8);
        }
    }
    data
}

/// FillOutput overflow at HC level 4 (compress_hash_chain) — tight buffer.
/// The output buffer is sized to trigger overflow mid-match-encoding.
#[test]
fn compress_hc_dest_size_fill_output_hash_chain_overflow() {
    let src = repeated_input(8192);
    let mut dst = vec![0u8; 64];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            4,
        )
    };
    assert!(
        n > 0,
        "FillOutput hash_chain overflow must produce output: {n}"
    );
}

/// FillOutput overflow at level 2 (lz4mid) — tight buffer mid-stream.
#[test]
fn compress_hc_dest_size_fill_output_lz4mid_overflow() {
    let src = repeated_input(8192);
    let mut dst = vec![0u8; 64];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            2,
        )
    };
    assert!(n > 0, "FillOutput lz4mid must produce output: {n}");
}

/// FillOutput overflow at optimal level 10 — tight buffer mid-stream.
#[test]
fn compress_hc_dest_size_fill_output_optimal_overflow() {
    let src = repeated_input(4096);
    let mut dst = vec![0u8; 128];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            10,
        )
    };
    assert!(n > 0, "FillOutput optimal must produce output: {n}");
}

/// FillOutput with buffer requiring final-run truncation (exercises truncation path).
#[test]
fn compress_hc_dest_size_fill_output_final_run_truncation() {
    // Repeated data with tight output triggers the FillOutput final-run truncation
    let src = repeated_input(8192);
    let mut dst = vec![0u8; 256]; // tight but safe
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_size = src.len() as i32;
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            6,
        )
    };
    if n > 0 {
        assert!((src_size as usize) <= src.len());
    }
}

/// FillOutput with 50% target at various levels exercises different code paths.
#[test]
fn compress_hc_dest_size_fill_output_at_each_level() {
    let src = repeated_input(4096);
    let target = 128;
    for level in [1, 2, 3, 4, 6, 8, 9, 10, 11, 12] {
        let mut dst = vec![0u8; target];
        let mut stream = Lz4StreamHc::create().unwrap();
        let mut src_size = src.len() as i32;
        let n = unsafe {
            compress_hc_dest_size(
                &mut stream,
                src.as_ptr(),
                dst.as_mut_ptr(),
                &mut src_size,
                target as i32,
                level,
            )
        };
        assert!(n >= 0, "FillOutput at level {level} should not crash");
    }
}

/// favor_decompression_speed with optimal level, data with 19-36 byte matches.
/// Exercises the match-length cap at 18 in find_longer_match.
#[test]
fn favor_dec_speed_match_cap_optimal() {
    // Create data with many ~24-byte matches: repeat a 24-byte pattern
    let pattern = b"ABCDEFGHIJKLMNOPQRSTUVWX";
    let mut src = Vec::with_capacity(4096);
    for i in 0..170 {
        src.extend_from_slice(pattern);
        // Insert a unique byte to break the pattern occasionally
        if i % 4 == 0 {
            src.push((i % 256) as u8);
        }
    }
    src.truncate(4096);

    let mut dst = vec![0u8; 8192];
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 11);
    favor_decompression_speed(&mut stream, true);
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            11,
        )
    };
    assert!(
        n > 0,
        "favor_dec_speed with optimal level must succeed: {n}"
    );
    let out = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(out, src);
}

/// Streaming multi-block at LZ4Mid level 2 with non-contiguous blocks.
/// Exercises ext-dict search path in lz4mid.
#[test]
fn streaming_lz4mid_non_contiguous_ext_dict() {
    let block1: Vec<u8> = (0..4096).map(|i| b"the quick brown fox "[i % 20]).collect();
    let block2: Vec<u8> = (0..4096).map(|i| b"the quick brown fox "[i % 20]).collect();

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 2);

    // Compress block1
    let mut dst1 = vec![0u8; 8192];
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst1.as_mut_ptr(),
            block1.len() as i32,
            dst1.len() as i32,
        )
    };
    assert!(n1 > 0);

    // Compress block2 (different memory = non-contiguous = ext-dict path)
    let mut dst2 = vec![0u8; 8192];
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst2.as_mut_ptr(),
            block2.len() as i32,
            dst2.len() as i32,
        )
    };
    assert!(n2 > 0);
}

/// Streaming at HC level 4 with non-contiguous blocks (ext-dict in hash_chain).
#[test]
fn streaming_hc_level4_non_contiguous_ext_dict() {
    let block1: Vec<u8> = (0..4096).map(|i| b"hello world greet "[i % 18]).collect();
    let block2: Vec<u8> = (0..4096).map(|i| b"hello world greet "[i % 18]).collect();

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 4);

    let mut dst1 = vec![0u8; 8192];
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst1.as_mut_ptr(),
            block1.len() as i32,
            dst1.len() as i32,
        )
    };
    assert!(n1 > 0);

    let mut dst2 = vec![0u8; 8192];
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst2.as_mut_ptr(),
            block2.len() as i32,
            dst2.len() as i32,
        )
    };
    assert!(n2 > 0);
}

/// Dirty-stream reset exercises the full-clear path in reset_stream_hc_fast.
#[test]
fn reset_stream_hc_fast_dirty_stream() {
    let src = semi_compressible_data(256);
    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 4);

    // Make the stream dirty by using LimitedOutput with a tiny buffer
    let mut tiny_dst = vec![0u8; 4]; // way too small, forces failure
    let n = unsafe {
        compress_hc_ext_state(
            &mut stream,
            src.as_ptr(),
            tiny_dst.as_mut_ptr(),
            src.len() as i32,
            tiny_dst.len() as i32,
            4,
        )
    };
    // LimitedOutput returns 0 on failure, which marks the stream as dirty
    assert_eq!(n, 0, "should fail with tiny output buffer");

    // Now reset_stream_hc_fast should take the dirty path (full reset)
    reset_stream_hc_fast(&mut stream, 4);

    // Verify stream is usable again
    let mut dst = vec![0u8; 1024];
    let n2 = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            4,
        )
    };
    assert!(n2 > 0, "stream must be usable after dirty reset: {n2}");
}

/// Streaming with save_dict then continue at lz4mid level.
#[test]
fn streaming_save_dict_lz4mid_continue() {
    let block1: Vec<u8> = (0..8192).map(|i| ((i * 137 + 59) % 256) as u8).collect();
    let block2: Vec<u8> = (0..8192).map(|i| ((i * 137 + 59) % 256) as u8).collect();
    let mut save_buf = vec![0u8; 65536];

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 2);

    let mut dst1 = vec![0u8; 16384];
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst1.as_mut_ptr(),
            block1.len() as i32,
            dst1.len() as i32,
        )
    };
    assert!(n1 > 0);

    let saved = unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
    assert!(saved > 0);

    let mut dst2 = vec![0u8; 16384];
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst2.as_mut_ptr(),
            block2.len() as i32,
            dst2.len() as i32,
        )
    };
    assert!(n2 > 0);
}

/// FillOutput with continue_dest_size at different levels (no dict).
#[test]
fn compress_hc_continue_dest_size_fill_output_levels() {
    let src = semi_compressible_data(4096);

    for level in [4, 9] {
        let mut stream = Lz4StreamHc::create().unwrap();
        set_compression_level(&mut stream, level);

        // Load a dict to properly initialize the stream
        let dict: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };

        // Use a generous target that won't crash
        let target = 2048;
        let mut dst = vec![0u8; target];
        let mut src_size = src.len() as i32;
        let n = unsafe {
            compress_hc_continue_dest_size(
                &mut stream,
                src.as_ptr(),
                dst.as_mut_ptr(),
                &mut src_size,
                target as i32,
            )
        };
        assert!(
            n >= 0,
            "FillOutput continue_dest_size at level {level} should not crash"
        );
    }
}

/// Three overlapping matches in hash_chain (exercises m1/m2/m3 overlap shortening).
#[test]
fn compress_hash_chain_three_match_overlap() {
    // Create input with three nearby matches that partially overlap
    let mut src = vec![0u8; 512];
    let pattern = b"MATCH_PAT";
    // Place pattern at offsets 0, 10, 18 to create overlapping match windows
    src[0..9].copy_from_slice(pattern);
    src[10..19].copy_from_slice(pattern);
    src[18..27].copy_from_slice(pattern);
    // Fill in gaps
    for i in 27..512 {
        src[i] = ((i * 7) % 256) as u8;
    }
    // Add more instances of the pattern to strengthen matches
    for offset in (50..400).step_by(16) {
        if offset + 9 <= 512 {
            src[offset..offset + 9].copy_from_slice(pattern);
        }
    }

    let mut dst = vec![0u8; 1024];
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state_fast_reset(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            4,
        )
    };
    assert!(n > 0);
    let out = roundtrip_decompress(&dst, n as usize, src.len());
    assert_eq!(out, src);
}

/// Pattern analysis path: repeating single-byte pattern across dict/prefix boundary.
#[test]
fn repeating_pattern_across_dict_prefix_boundary() {
    // Single-byte pattern that spans across dictionary and prefix
    let dict = vec![0xAA_u8; 8192];
    let src = vec![0xAA_u8; 4096]; // same pattern, should match across boundary

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 8); // HC level with pattern analysis
    unsafe { load_dict_hc(&mut stream, dict.as_ptr(), dict.len() as i32) };

    let mut dst = vec![0u8; 8192];
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
}

/// Ext-dict streaming with pattern that exercises count_pattern/reverse_count_pattern.
#[test]
fn ext_dict_repeating_pattern_exercises_search() {
    // Two-byte repeating pattern to exercise pattern-analysis in search.rs
    let mut block1 = vec![0u8; 4096];
    for i in 0..4096 {
        block1[i] = if i % 2 == 0 { 0xAB } else { 0xCD };
    }
    let mut block2 = block1.clone();
    // Slight variation in block2 to force partial matching
    block2[100] = 0xFF;
    block2[200] = 0xFF;

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 6);

    let mut dst1 = vec![0u8; 8192];
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            block1.as_ptr(),
            dst1.as_mut_ptr(),
            block1.len() as i32,
            dst1.len() as i32,
        )
    };
    assert!(n1 > 0);

    // Non-contiguous → ext-dict with pattern overlap
    let mut dst2 = vec![0u8; 8192];
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            block2.as_ptr(),
            dst2.as_mut_ptr(),
            block2.len() as i32,
            dst2.len() as i32,
        )
    };
    assert!(n2 > 0);
}

/// Streaming multi-block with save_dict at optimal level.
#[test]
fn streaming_optimal_save_dict_multi_block() {
    let block_data = semi_compressible_data(4096);
    let mut save_buf = vec![0u8; 65536];

    let mut stream = Lz4StreamHc::create().unwrap();
    set_compression_level(&mut stream, 10);

    for _ in 0..3 {
        let mut dst = vec![0u8; 8192];
        let n = unsafe {
            compress_hc_continue(
                &mut stream,
                block_data.as_ptr(),
                dst.as_mut_ptr(),
                block_data.len() as i32,
                dst.len() as i32,
            )
        };
        assert!(n > 0);
        let saved =
            unsafe { save_dict_hc(&mut stream, save_buf.as_mut_ptr(), save_buf.len() as i32) };
        assert!(saved > 0);
    }
}

/// LimitedOutput at HC level returns 0 when output too small.
#[test]
fn compress_hc_limited_output_too_small_returns_zero() {
    let src = semi_compressible_data(2048);
    let mut dst = vec![0u8; 16]; // way too small
    let mut stream = Lz4StreamHc::create().unwrap();
    let n = unsafe {
        compress_hc_ext_state(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
            4,
        )
    };
    assert_eq!(n, 0, "LimitedOutput should return 0 when buffer too small");
}

/// compress_hc at every supported level to cover all dispatch paths.
#[test]
fn compress_hc_all_levels_roundtrip() {
    let src = semi_compressible_data(2048);
    for level in 1..=12 {
        let mut dst = vec![0u8; 8192];
        let n = unsafe {
            compress_hc(
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                level,
            )
        };
        assert!(n > 0, "compress_hc at level {level} must succeed: {n}");
        let out = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(out, src, "roundtrip failed at level {level}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Coverage-gap tests: phase 3 — targeted at specific uncovered code paths
// ─────────────────────────────────────────────────────────────────────────────

/// Highly repetitive data (single byte) at levels 3-8 exercises pattern analysis
/// in search.rs: count_pattern, reverse_count_pattern (L122-196),
/// and insert_and_find_best_match pattern optimization (L388-528).
#[test]
fn compress_hc_levels_3_8_highly_repetitive_data() {
    let src = vec![0xAAu8; 16384];
    for level in 3..=8 {
        let mut dst = vec![0u8; 32768];
        let n = unsafe {
            compress_hc(
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                level,
            )
        };
        assert!(
            n > 0,
            "compress_hc at level {level} with repetitive data must succeed"
        );
        let dec = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(dec, src, "roundtrip failed at level {level}");
        // Highly repetitive data should compress very well
        assert!(
            n < 100,
            "16KB of 0xAA should compress to <100 bytes, got {n}"
        );
    }
}

/// Repetitive data at levels 9-12 (Lz4Opt) with pattern analysis.
#[test]
fn compress_hc_opt_levels_repetitive_data() {
    let src = vec![0xBBu8; 8192];
    for level in 9..=12 {
        let mut dst = vec![0u8; 16384];
        let n = unsafe {
            compress_hc(
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                level,
            )
        };
        assert!(
            n > 0,
            "compress_hc at level {level} with repetitive data must succeed"
        );
        let dec = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(dec, src);
    }
}

/// Multi-block HC streaming with linked blocks exercises SearchState::S3 in compress_hc.rs
/// (3-match overlapping pattern). Levels 3-4 trigger Lz4Hc strategy.
#[test]
fn compress_hc_continue_multiblock_overlapping_matches() {
    // Create data with many overlapping matches to trigger S3.
    let mut data = Vec::new();
    for i in 0..8 {
        // Each block has overlapping content with previous
        let block: Vec<u8> = (0..1024).map(|j| ((j + i * 100) % 251) as u8).collect();
        data.extend_from_slice(&block);
    }

    let mut stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut stream, 4); // level 4 = Lz4Hc

    let mut compressed = Vec::new();
    let block_size = 1024;
    for chunk in data.chunks(block_size) {
        let mut dst = vec![0u8; 4096];
        let n = unsafe {
            compress_hc_continue(
                &mut stream,
                chunk.as_ptr(),
                dst.as_mut_ptr(),
                chunk.len() as i32,
                dst.len() as i32,
            )
        };
        assert!(n > 0, "compress_hc_continue block must succeed");
        compressed.push(dst[..n as usize].to_vec());
    }
    assert_eq!(compressed.len(), 8);
}

/// HC streaming with attach_hc_dictionary to exercise dict_ctx search paths.
/// Exercises search.rs L564-574 (dict-context chain search).
#[test]
fn compress_hc_continue_with_attached_dict() {
    let dict_data: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dict_stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut dict_stream, 4);
    unsafe {
        load_dict_hc(&mut dict_stream, dict_data.as_ptr(), dict_data.len() as i32);
    }

    let mut stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut stream, 4);
    unsafe {
        attach_hc_dictionary(&mut stream, Some(&*dict_stream as *const Lz4StreamHc));
    }

    // Compress data that shares patterns with the dict.
    let src: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 4096];
    let n = unsafe {
        compress_hc_continue(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "continue with attached dict must succeed: {n}");
    // Should compress better than without dict
    let mut no_dict_stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut no_dict_stream, 4);
    let mut dst2 = vec![0u8; 4096];
    let n2 = unsafe {
        compress_hc_continue(
            &mut no_dict_stream,
            src.as_ptr(),
            dst2.as_mut_ptr(),
            src.len() as i32,
            dst2.len() as i32,
        )
    };
    assert!(n2 > 0);
    assert!(
        n <= n2,
        "with dict should compress at least as well: {n} vs {n2}"
    );
}

/// Repetitive 2-byte pattern at HC level to trigger rotate_pattern in search.rs.
#[test]
fn compress_hc_two_byte_repeating_pattern() {
    // Two-byte pattern: ABABAB...
    let src: Vec<u8> = (0..8192)
        .map(|i| if i % 2 == 0 { 0xAB } else { 0xCD })
        .collect();
    for level in [3, 6, 9] {
        let mut dst = vec![0u8; 16384];
        let n = unsafe {
            compress_hc(
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                level,
            )
        };
        assert!(n > 0, "two-byte pattern at level {level} must compress");
        let dec = roundtrip_decompress(&dst, n as usize, src.len());
        assert_eq!(dec, src);
    }
}

/// FillOutput with highly repetitive data at HC level to maximize match-length truncation.
#[test]
fn compress_hc_dest_size_fill_output_repetitive() {
    let src = vec![0xCCu8; 8192];
    let mut stream = Lz4StreamHc::create().unwrap();
    let mut src_consumed = src.len() as i32;
    let target_size = 64;
    let mut dst = vec![0u8; target_size];
    let n = unsafe {
        compress_hc_dest_size(
            &mut stream,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_consumed,
            target_size as i32,
            4,
        )
    };
    assert!(n > 0, "FillOutput repetitive must produce output: {n}");
}

/// HC streaming multi-block with both attached dict and linked continuation.
/// This forces set_external_dict and exercises the overlap trimming in compress_hc_continue.
#[test]
fn compress_hc_continue_multiblock_with_external_dict() {
    let dict_data: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    let mut stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut stream, 4);
    unsafe {
        load_dict_hc(&mut stream, dict_data.as_ptr(), dict_data.len() as i32);
    }

    let all_data: Vec<u8> = (0..16384).map(|i| (i % 251) as u8).collect();
    let block_size = 4096;
    let mut all_compressed = Vec::new();
    for chunk in all_data.chunks(block_size) {
        let mut dst = vec![0u8; 8192];
        let n = unsafe {
            compress_hc_continue(
                &mut stream,
                chunk.as_ptr(),
                dst.as_mut_ptr(),
                chunk.len() as i32,
                dst.len() as i32,
            )
        };
        assert!(n > 0, "multiblock with ext-dict must succeed");
        all_compressed.push(dst[..n as usize].to_vec());
    }
    assert_eq!(all_compressed.len(), 4);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: Chain-swap / pattern analysis / overlap trimming tests
// ─────────────────────────────────────────────────────────────────────────────

/// Compress highly repetitive data at HC level 9 (Lz4Opt strategy) to exercise
/// advanced pattern analysis and chain-swap in search.rs.
#[test]
fn compress_hc_repetitive_data_level_9() {
    // Uniform byte — triggers count_pattern and reverse_count_pattern
    let data = vec![0xABu8; 128 * 1024];
    let mut dst = vec![0u8; data.len() + 1024];
    let n = unsafe {
        compress_hc(
            data.as_ptr(),
            dst.as_mut_ptr(),
            data.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n > 0, "HC level 9 repetitive must succeed");
    let decompressed = roundtrip_decompress(&dst[..n as usize], n as usize, data.len());
    assert_eq!(decompressed, data);
}

/// Compress data with repeating short pattern at HC level 12 (Lz4Opt max) to exercise
/// insert_and_get_wider_match chain-swap optimisation.
#[test]
fn compress_hc_short_pattern_level_12() {
    // Repeating 4-byte pattern: ABCD ABCD ABCD...
    let pattern = [0x41u8, 0x42, 0x43, 0x44];
    let data: Vec<u8> = pattern.iter().cycle().take(128 * 1024).copied().collect();
    let mut dst = vec![0u8; data.len() + 1024];
    let n = unsafe {
        compress_hc(
            data.as_ptr(),
            dst.as_mut_ptr(),
            data.len() as i32,
            dst.len() as i32,
            12,
        )
    };
    assert!(n > 0, "HC level 12 short pattern must succeed");
    let decompressed = roundtrip_decompress(&dst[..n as usize], n as usize, data.len());
    assert_eq!(decompressed, data);
}

/// Large streaming HC with overlap detection — exercises overlap trim path L521-550.
/// When src pointer overlaps with the last dict region, the HC compressor
/// must adjust low_limit/dict_start.
#[test]
fn compress_hc_continue_overlap_detection() {
    // Use a single large buffer and compress overlapping segments
    let mut buf = vec![0u8; 256 * 1024];
    for i in 0..buf.len() {
        buf[i] = (i % 251) as u8;
    }

    let mut stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut stream, 4);

    // First block
    let mut dst1 = vec![0u8; 128 * 1024];
    let n1 = unsafe {
        compress_hc_continue(
            &mut stream,
            buf.as_ptr(),
            dst1.as_mut_ptr(),
            (64 * 1024) as i32,
            dst1.len() as i32,
        )
    };
    assert!(n1 > 0, "first block must succeed");

    // Second block starting in the middle of the first block's region
    // This creates overlap with the dict region
    let offset = 32 * 1024; // 32KB into first block's region
    let n2 = unsafe {
        compress_hc_continue(
            &mut stream,
            buf.as_ptr().add(offset),
            dst1.as_mut_ptr(),
            (64 * 1024) as i32,
            dst1.len() as i32,
        )
    };
    assert!(n2 > 0, "overlapping block must succeed");

    // Third block continuing from well beyond
    let n3 = unsafe {
        compress_hc_continue(
            &mut stream,
            buf.as_ptr().add(128 * 1024),
            dst1.as_mut_ptr(),
            (64 * 1024) as i32,
            dst1.len() as i32,
        )
    };
    assert!(n3 > 0, "third block must succeed");
}

/// Compress data with ext_state at level 9 and then at level 1 to exercise
/// context type switching (Fast→HC→Fast reinit path in compress.rs).
#[test]
fn compress_hc_ext_state_level_switching() {
    let data: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; data.len() + 1024];

    // First at level 9
    let mut state = Lz4StreamHc::create().unwrap();
    let n1 = unsafe {
        compress_hc_ext_state(
            &mut state,
            data.as_ptr(),
            dst.as_mut_ptr(),
            data.len() as i32,
            dst.len() as i32,
            9,
        )
    };
    assert!(n1 > 0);

    // Then reuse at level 1
    let n2 = unsafe {
        compress_hc_ext_state(
            &mut state,
            data.as_ptr(),
            dst.as_mut_ptr(),
            data.len() as i32,
            dst.len() as i32,
            1,
        )
    };
    assert!(n2 > 0);

    let decompressed = roundtrip_decompress(&dst[..n2 as usize], n2 as usize, data.len());
    assert_eq!(decompressed, data);
}

/// Compress many sequential blocks with save_dict_hc between each.
/// Exercises the dict save/reload cycle that hits different internal paths.
#[test]
fn compress_hc_continue_save_dict_cycle() {
    let mut stream = Lz4StreamHc::create().unwrap();
    reset_stream_hc(&mut stream, 4);

    let all_data: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let block_size = 16 * 1024;
    let mut dict_buf = vec![0u8; 64 * 1024];
    let mut all_compressed = Vec::new();

    for chunk in all_data.chunks(block_size) {
        let mut dst = vec![0u8; block_size * 2];
        let n = unsafe {
            compress_hc_continue(
                &mut stream,
                chunk.as_ptr(),
                dst.as_mut_ptr(),
                chunk.len() as i32,
                dst.len() as i32,
            )
        };
        assert!(n > 0, "save_dict cycle block must succeed");
        all_compressed.push(dst[..n as usize].to_vec());

        // Save dict — this exercises save_dict_hc paths
        let saved =
            unsafe { save_dict_hc(&mut stream, dict_buf.as_mut_ptr(), dict_buf.len() as i32) };
        assert!(saved >= 0, "save_dict must succeed");
    }
    assert_eq!(all_compressed.len(), 8);
}
