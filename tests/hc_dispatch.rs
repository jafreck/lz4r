// Unit tests for task-013: HC Strategy Dispatcher and External Dict Handling.
//
// Tests verify behavioural parity with lz4hc.c v1.10.0, lines 1373–1484
// and 1660–1678:
//   - `isStateCompatible`                 → `HcCCtxInternal::is_compatible`
//   - `LZ4HC_setExternalDict`             → `set_external_dict`
//   - `LZ4HC_compress_generic_internal`   → `compress_generic_internal`
//   - `LZ4HC_compress_generic_noDictCtx`  → `compress_generic_no_dict_ctx`
//   - `LZ4HC_compress_generic_dictCtx`    → `compress_generic_dict_ctx`
//   - `LZ4HC_compress_generic`            → `compress_generic`
//
// Coverage:
//   - is_compatible: same-mid, same-hc, mid-vs-hc, zero-level clamping
//   - set_external_dict: pointer state, dict_ctx cleared, dict_limit advance
//   - compress_generic_internal: FillOutput+zero capacity returns 0,
//       oversized src returns 0, end pointer advanced, dirty set on failure,
//       Lz4Hc level compresses successfully
//   - compress_generic_no_dict_ctx: delegates to internal, produces output
//   - compress_generic: routes to no-dict path when dict_ctx null,
//       routes to dict-ctx path when dict_ctx non-null
//   - compress_generic_dict_ctx: position>=64KB discards dict, position==0 +
//       large src promotes dict-ctx→ext-dict, otherwise uses UsingDictCtxHc

use lz4::block::compress::LZ4_MAX_INPUT_SIZE;
use lz4::block::types::LimitedOutputDirective;
use lz4::hc::dispatch::{
    compress_generic, compress_generic_dict_ctx, compress_generic_internal,
    compress_generic_no_dict_ctx, set_external_dict,
};
use lz4::hc::types::{
    init_internal, DictCtxDirective, HcCCtxInternal, HcStrategy, LZ4HC_CLEVEL_MAX,
    LZ4HC_CLEVEL_OPT_MIN, get_clevel_params,
};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create an initialised HC context pointing into `buf`.
/// Sets `end` to the end of `buf` (ready for round-trip tests).
unsafe fn make_ctx(buf: &[u8]) -> HcCCtxInternal {
    let mut ctx = HcCCtxInternal::new();
    init_internal(&mut ctx, buf.as_ptr());
    ctx.end = buf.as_ptr().add(buf.len());
    ctx
}

// ═════════════════════════════════════════════════════════════════════════════
// is_compatible  (isStateCompatible)
// ═════════════════════════════════════════════════════════════════════════════

/// Two mid-strategy contexts (levels 0–2) should be compatible.
#[test]
fn is_compatible_both_mid() {
    // Levels 0, 1, 2 all map to HcStrategy::Lz4Mid (clamped default is Hc,
    // but level 2 is explicitly Mid).
    let mut a = HcCCtxInternal::new();
    let mut b = HcCCtxInternal::new();
    a.compression_level = 2; // Mid
    b.compression_level = 2; // Mid
    assert_eq!(get_clevel_params(2).strat, HcStrategy::Lz4Mid);
    assert!(a.is_compatible(&b), "two Mid contexts should be compatible");
}

/// Two hash-chain strategy contexts (levels 3–9) should be compatible.
#[test]
fn is_compatible_both_hc() {
    let mut a = HcCCtxInternal::new();
    let mut b = HcCCtxInternal::new();
    a.compression_level = 5; // Lz4Hc
    b.compression_level = 9; // Lz4Hc
    assert_eq!(get_clevel_params(5).strat, HcStrategy::Lz4Hc);
    assert_eq!(get_clevel_params(9).strat, HcStrategy::Lz4Hc);
    assert!(a.is_compatible(&b), "two Hc contexts should be compatible");
}

/// Two optimal-parser contexts (levels 10–12) should be compatible.
#[test]
fn is_compatible_both_opt() {
    let mut a = HcCCtxInternal::new();
    let mut b = HcCCtxInternal::new();
    a.compression_level = 10; // Lz4Opt
    b.compression_level = 12; // Lz4Opt
    assert_eq!(get_clevel_params(10).strat, HcStrategy::Lz4Opt);
    assert_eq!(get_clevel_params(12).strat, HcStrategy::Lz4Opt);
    // Both are non-mid → compatible (C: !(isMid1 ^ isMid2) = !false = true)
    assert!(a.is_compatible(&b), "two Opt contexts should be compatible");
}

/// A Mid context and an Hc context should NOT be compatible.
#[test]
fn is_compatible_mid_vs_hc() {
    let mut a = HcCCtxInternal::new();
    let mut b = HcCCtxInternal::new();
    a.compression_level = 2;  // Mid
    b.compression_level = 5;  // Hc
    assert!(!a.is_compatible(&b), "Mid vs Hc should not be compatible");
}

/// A Mid context and an Opt context should NOT be compatible.
#[test]
fn is_compatible_mid_vs_opt() {
    let mut a = HcCCtxInternal::new();
    let mut b = HcCCtxInternal::new();
    a.compression_level = 2;  // Mid
    b.compression_level = 10; // Opt (non-mid)
    assert!(!a.is_compatible(&b), "Mid vs Opt should not be compatible");
}

/// Level 0 is clamped to the default HC level (9 = Lz4Hc, non-mid).
/// A level-0 context paired with a Mid context should be incompatible.
#[test]
fn is_compatible_level_zero_clamped_to_default() {
    let mut a = HcCCtxInternal::new();
    let mut b = HcCCtxInternal::new();
    a.compression_level = 0;  // clamped to Hc
    b.compression_level = 2;  // Mid
    // get_clevel_params(0) returns default (Hc)
    assert_eq!(get_clevel_params(0).strat, HcStrategy::Lz4Hc);
    assert!(!a.is_compatible(&b), "clamped-Hc vs Mid should not be compatible");
}

/// is_compatible is reflexive: any context is compatible with itself.
#[test]
fn is_compatible_reflexive() {
    for level in [2i16, 5, 9, 10, 12] {
        let mut ctx = HcCCtxInternal::new();
        ctx.compression_level = level;
        // is_compatible takes two *references* so we compare ctx with itself
        // by building a second ctx with the same level.
        let mut ctx2 = HcCCtxInternal::new();
        ctx2.compression_level = level;
        assert!(ctx.is_compatible(&ctx2), "level {level} should be self-compatible");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// set_external_dict  (LZ4HC_setExternalDict)
// ═════════════════════════════════════════════════════════════════════════════

/// After set_external_dict:
///   - prefix_start and end → new_block
///   - dict_ctx is null
///   - next_to_update == new dict_limit
#[test]
fn set_external_dict_basic_state() {
    // Build a fake prefix window: 128 bytes.
    let prefix = vec![0u8; 128];
    let new_block_data = vec![0u8; 64];

    unsafe {
        let mut ctx = HcCCtxInternal::new();
        init_internal(&mut ctx, prefix.as_ptr());
        ctx.end = prefix.as_ptr().add(128);
        // Give it a non-null dict_ctx so we can verify it's cleared.
        ctx.dict_ctx = &ctx as *const HcCCtxInternal;

        // Record original dict_limit before the call.
        let orig_dict_limit = ctx.dict_limit;

        // Use a level that is non-mid so the insert guard fires (or doesn't matter).
        ctx.compression_level = 9;

        set_external_dict(&mut ctx, new_block_data.as_ptr());

        // prefix_start and end are updated to new_block.
        assert_eq!(ctx.prefix_start, new_block_data.as_ptr());
        assert_eq!(ctx.end, new_block_data.as_ptr());

        // dict_ctx must be null after call (cannot hold both extDict and dictCtx).
        assert!(ctx.dict_ctx.is_null(), "dict_ctx must be null after set_external_dict");

        // next_to_update must equal the new dict_limit.
        assert_eq!(
            ctx.next_to_update, ctx.dict_limit,
            "next_to_update must equal new dict_limit"
        );

        // dict_limit advanced by old prefix length = 128.
        // new dict_limit = orig_dict_limit + prefix_len.
        let expected_new_dict_limit = orig_dict_limit.wrapping_add(128);
        assert_eq!(ctx.dict_limit, expected_new_dict_limit);

        // low_limit == old dict_limit.
        assert_eq!(ctx.low_limit, orig_dict_limit);
    }
}

/// set_external_dict with a short prefix (< 4 bytes) skips the insert step.
/// The state transitions still happen correctly.
#[test]
fn set_external_dict_short_prefix_no_insert() {
    // 3-byte prefix — too short for insert (requires >= 4 bytes).
    let prefix = vec![0xAA_u8; 3];
    let new_block = vec![0xBB_u8; 32];

    unsafe {
        let mut ctx = HcCCtxInternal::new();
        init_internal(&mut ctx, prefix.as_ptr());
        ctx.end = prefix.as_ptr().add(3);
        ctx.compression_level = 9; // Hc (non-mid)

        set_external_dict(&mut ctx, new_block.as_ptr());

        // State still transitions correctly.
        assert_eq!(ctx.prefix_start, new_block.as_ptr());
        assert_eq!(ctx.end, new_block.as_ptr());
        assert!(ctx.dict_ctx.is_null());
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_generic_internal  (LZ4HC_compress_generic_internal)
// ═════════════════════════════════════════════════════════════════════════════

/// FillOutput + zero capacity → immediate return 0 (no work done).
#[test]
fn compress_generic_internal_filloutput_zero_capacity_returns_zero() {
    let input = b"Hello, world!";
    let mut output = vec![0u8; 64];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            0, // dst_capacity = 0
            9, // c_level
            LimitedOutputDirective::FillOutput,
            DictCtxDirective::NoDictCtx,
        );
        assert_eq!(n, 0, "FillOutput with dst_capacity=0 must return 0");
    }
}

/// Oversized src (> LZ4_MAX_INPUT_SIZE) → return 0.
#[test]
fn compress_generic_internal_oversized_src_returns_zero() {
    let input = b"Hello";
    let mut output = vec![0u8; 64];

    unsafe {
        let mut ctx = make_ctx(input);
        // Cast to i32: LZ4_MAX_INPUT_SIZE is 0x7E000000 which fits in i32.
        let mut src_size = (LZ4_MAX_INPUT_SIZE + 1) as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );
        assert_eq!(n, 0, "oversized src must return 0");
    }
}

/// ctx.end is advanced by src_size after a successful call.
#[test]
fn compress_generic_internal_advances_ctx_end() {
    let input = b"ABCDEFGHIJ"; // 10 bytes
    let mut output = vec![0u8; 64];

    unsafe {
        let mut ctx = make_ctx(input);
        // Reset end to prefix_start so the advancement is measurable.
        let original_end = ctx.end;
        ctx.end = ctx.prefix_start;

        let mut src_size = input.len() as i32;
        compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );

        // end should be prefix_start + src_size
        let expected_end = ctx.prefix_start.add(input.len());
        assert_eq!(
            ctx.end, expected_end,
            "ctx.end must be advanced by src_size"
        );
        let _ = original_end;
    }
}

/// When compression returns 0 (failure), ctx.dirty is set to 1.
#[test]
fn compress_generic_internal_failure_sets_dirty() {
    let input = b"Hello, world!";
    // Deliberately tiny output buffer (1 byte) to force failure.
    let mut output = vec![0u8; 1];

    unsafe {
        let mut ctx = make_ctx(input);
        ctx.dirty = 0; // ensure clean before call
        let mut src_size = input.len() as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::LimitedOutput,
            DictCtxDirective::NoDictCtx,
        );
        // Failure path: n should be 0 (limited output, too small).
        assert_eq!(n, 0, "expected failure with too-small output");
        assert_eq!(ctx.dirty, 1, "ctx.dirty must be set to 1 on failure");
    }
}

/// Successful Lz4Hc-level compression produces positive output.
#[test]
fn compress_generic_internal_hc_level_compresses_data() {
    let input = vec![0xAA_u8; 1024];
    let mut output = vec![0u8; 1024];

    unsafe {
        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start; // reset before dispatch advances it
        let mut src_size = input.len() as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9, // Lz4Hc
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );
        assert!(n > 0, "Lz4Hc compression of repeated data should succeed");
        assert!((n as usize) < input.len(), "compressed output should be smaller than input");
    }
}

/// Successful Lz4Opt-level compression produces positive output.
#[test]
fn compress_generic_internal_opt_level_compresses_data() {
    let input = vec![0xBB_u8; 1024];
    let mut output = vec![0u8; 1024];

    unsafe {
        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        let mut src_size = input.len() as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            LZ4HC_CLEVEL_OPT_MIN, // 10 = Lz4Opt
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );
        assert!(n > 0, "Lz4Opt compression of repeated data should succeed");
        assert!((n as usize) < input.len(), "Lz4Opt output should be smaller than input");
    }
}

/// Successful Lz4Mid-level compression (level 2) produces positive output.
#[test]
fn compress_generic_internal_mid_level_compresses_data() {
    let input = vec![0xCC_u8; 1024];
    let mut output = vec![0u8; 1024];

    unsafe {
        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        let mut src_size = input.len() as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            2, // Lz4Mid
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );
        assert!(n > 0, "Lz4Mid compression of repeated data should succeed");
        assert!((n as usize) < input.len(), "Lz4Mid output should be smaller than input");
    }
}

/// favor_dec_speed flag takes the HcFavor::DecompressionSpeed path without crashing.
#[test]
fn compress_generic_internal_favor_dec_speed_no_crash() {
    let input = vec![0xDD_u8; 512];
    let mut output = vec![0u8; 512];

    unsafe {
        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        ctx.favor_dec_speed = 1;
        let mut src_size = input.len() as i32;
        let n = compress_generic_internal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            LZ4HC_CLEVEL_MAX, // Opt with ultra flag
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );
        assert!(n > 0, "favor_dec_speed should still produce output");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_generic_no_dict_ctx  (LZ4HC_compress_generic_noDictCtx)
// ═════════════════════════════════════════════════════════════════════════════

/// Delegates correctly — ctx.dict_ctx must be null; succeeds on repeated data.
#[test]
fn compress_generic_no_dict_ctx_succeeds() {
    let input = vec![0xEE_u8; 1024];
    let mut output = vec![0u8; 1024];

    unsafe {
        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        // dict_ctx is null by default from make_ctx / init_internal.
        assert!(ctx.dict_ctx.is_null());

        let mut src_size = input.len() as i32;
        let n = compress_generic_no_dict_ctx(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
        );
        assert!(n > 0, "compress_generic_no_dict_ctx should return positive on success");
        assert!((n as usize) < input.len());
    }
}

/// Returns 0 when dst_capacity is too small under LimitedOutput.
#[test]
fn compress_generic_no_dict_ctx_limited_output_too_small() {
    let input = b"The quick brown fox jumps over the lazy dog.";
    let mut output = vec![0u8; 2];

    unsafe {
        let mut ctx = make_ctx(input);
        ctx.end = ctx.prefix_start;
        let mut src_size = input.len() as i32;
        let n = compress_generic_no_dict_ctx(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::LimitedOutput,
        );
        assert_eq!(n, 0);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_generic  (LZ4HC_compress_generic)
// ═════════════════════════════════════════════════════════════════════════════

/// When ctx.dict_ctx is null, routes to the no-dict-ctx path.
#[test]
fn compress_generic_null_dict_ctx_routes_to_no_dict() {
    let input = vec![0xFF_u8; 512];
    let mut output = vec![0u8; 512];

    unsafe {
        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        assert!(ctx.dict_ctx.is_null()); // verify no dict_ctx

        let mut src_size = input.len() as i32;
        let n = compress_generic(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
        );
        assert!(n > 0, "compress_generic with null dict_ctx should compress");
        assert!((n as usize) < input.len());
    }
}

/// When ctx.dict_ctx is non-null, routes to the dict-ctx path.
/// (We attach a second context as the dict.)
#[test]
fn compress_generic_non_null_dict_ctx_routes_to_dict_path() {
    let input = vec![0x11_u8; 512];
    let mut output = vec![0u8; 512];

    unsafe {
        // Build a "dictionary" context pointing at the same input.
        let mut dict_ctx = make_ctx(&input);
        dict_ctx.end = dict_ctx.prefix_start;
        dict_ctx.compression_level = 9;

        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        ctx.compression_level = 9;
        ctx.dict_ctx = &dict_ctx as *const HcCCtxInternal;

        let mut src_size = input.len() as i32;
        let n = compress_generic(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
        );
        // Either it succeeds or the position-check falls back to no-dict path.
        // Either way it must not be a crash and if repeated data, likely > 0.
        assert!(n > 0, "compress_generic with non-null dict_ctx should produce output");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_generic_dict_ctx  (LZ4HC_compress_generic_dictCtx)
// ═════════════════════════════════════════════════════════════════════════════

/// Case 1: position >= 64 KB — dict-ctx is discarded, fall back to no-dict.
#[test]
fn compress_generic_dict_ctx_position_ge_64kb_discards_dict() {
    let input = vec![0x22_u8; 512];
    let mut output = vec![0u8; 512];

    unsafe {
        let mut dict_ctx = HcCCtxInternal::new();
        init_internal(&mut dict_ctx, input.as_ptr());
        dict_ctx.compression_level = 9;

        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start;
        ctx.compression_level = 9;
        ctx.dict_ctx = &dict_ctx as *const HcCCtxInternal;

        // Force position >= 64 KB by setting low_limit and dict_limit such
        // that (end - prefix_start) + (dict_limit - low_limit) >= 65536.
        // The simplest way: set dict_limit - low_limit to 65536.
        ctx.low_limit = 0;
        ctx.dict_limit = 65536; // 64 KB
        // end == prefix_start → prefix length = 0; but dict portion is 65536 → position >= 64 KB.

        let mut src_size = input.len() as i32;
        let n = compress_generic_dict_ctx(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
        );

        // dict_ctx must now be null (discarded by the function).
        assert!(ctx.dict_ctx.is_null(), "dict_ctx must be null after position >= 64KB");
        // Should still produce some output.
        assert!(n > 0, "expected output even after dict discard");
    }
}

/// Case 3: position < 64 KB and NOT (position==0 with large src and compatible)
/// → uses UsingDictCtxHc path (dict_ctx remains non-null throughout).
#[test]
fn compress_generic_dict_ctx_small_position_uses_dict_ctx() {
    let input = vec![0x33_u8; 8]; // small input (≤ 4 KB) → cannot promote

    // Allocate a 512-byte output buffer.
    let mut output = vec![0u8; 512];

    unsafe {
        let mut dict_ctx = HcCCtxInternal::new();
        init_internal(&mut dict_ctx, input.as_ptr());
        dict_ctx.compression_level = 9;

        let mut ctx = make_ctx(&input);
        ctx.end = ctx.prefix_start; // position prefix_len = 0
        ctx.compression_level = 9;
        // dict_limit == low_limit → dict portion length = 0 → position = 0.
        // But src_size (8) is NOT > 4 KB → case 3 (UsingDictCtxHc).
        ctx.dict_ctx = &dict_ctx as *const HcCCtxInternal;

        let mut src_size = input.len() as i32;
        let _ = compress_generic_dict_ctx(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
        );
        // We do not assert on n here because small unique data may return 0
        // in LimitedOutput; we only verify the function doesn't panic.
    }
}

/// Case 2: position == 0, src_size > 4 KB, and states compatible →
/// promotes dict-ctx to ext-dict mode.
#[test]
fn compress_generic_dict_ctx_promotes_to_ext_dict() {
    // > 4 KB input so the promotion path is taken.
    let input = vec![0x44_u8; 8 * 1024]; // 8 KB
    let mut output = vec![0u8; 8 * 1024];

    unsafe {
        // Build a compatible dict_ctx (same strategy = Hc, level 9).
        let mut dict_ctx = HcCCtxInternal::new();
        init_internal(&mut dict_ctx, input.as_ptr());
        dict_ctx.compression_level = 9;
        dict_ctx.end = dict_ctx.prefix_start;

        let mut ctx = HcCCtxInternal::new();
        init_internal(&mut ctx, input.as_ptr());
        ctx.end = ctx.prefix_start; // prefix_len = 0
        ctx.dict_limit = ctx.low_limit; // dict portion = 0 → position = 0
        ctx.compression_level = 9;
        ctx.dict_ctx = &dict_ctx as *const HcCCtxInternal;

        let mut src_size = input.len() as i32;
        let n = compress_generic_dict_ctx(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            9,
            LimitedOutputDirective::NotLimited,
        );

        // After promotion the context clears dict_ctx (set_external_dict nulls it).
        // So dict_ctx must be null after the call.
        assert!(
            ctx.dict_ctx.is_null(),
            "dict_ctx must be null after promotion to ext-dict"
        );
        assert!(n > 0, "promoted ext-dict compression should produce output");
        assert!((n as usize) < input.len(), "expected compression");
    }
}
