// Unit tests for task-010: LZ4MID (medium-compression) strategy.
//
// Tests verify behavioural parity with lz4hc.c v1.10.0, lines 357–775:
//   - `LZ4HC_match_t`           → `Match`
//   - `LZ4MID_searchIntoDict_f` → `DictSearchMode`
//   - `LZ4MID_addPosition`      → `add_position`
//   - `LZ4MID_fillHTable`       → `fill_htable`
//   - `select_searchDict_function` → `select_dict_search_mode`
//   - `LZ4MID_compress`         → `lz4mid_compress`
//
// Coverage:
//   - Match struct: default, copy, clone, debug
//   - DictSearchMode enum: equality, copy, debug
//   - add_position: writes index at hash slot
//   - fill_htable: skips tiny dicts, fills normal dicts
//   - select_dict_search_mode: null → None; Lz4Mid strategy → Ext; Lz4Hc → Hc
//   - lz4mid_compress: empty input, single-byte, incompressible data, compressible data,
//     output-too-small (LimitedOutput), src_size_ptr updated correctly

use lz4::block::types::LimitedOutputDirective;
use lz4::hc::lz4mid::{
    add_position, fill_htable, lz4mid_compress, select_dict_search_mode, DictSearchMode, Match,
};
use lz4::hc::types::DictCtxDirective;
use lz4::hc::types::{init_internal, HcCCtxInternal, LZ4MID_HASHTABLESIZE};

// ─────────────────────────────────────────────────────────────────────────────
// Match struct
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn match_default_is_zero() {
    let m: Match = Default::default();
    assert_eq!(m.off, 0);
    assert_eq!(m.len, 0);
    assert_eq!(m.back, 0);
}

#[test]
fn match_copy_clone() {
    let m = Match {
        off: 10,
        len: 20,
        back: -3,
    };
    let m2 = m; // Copy
    let m3 = m.clone(); // Clone
    assert_eq!(m2.off, 10);
    assert_eq!(m3.len, 20);
    assert_eq!(m3.back, -3);
}

#[test]
fn match_debug_does_not_panic() {
    let m = Match {
        off: 5,
        len: 8,
        back: -1,
    };
    let _ = format!("{:?}", m);
}

// ─────────────────────────────────────────────────────────────────────────────
// DictSearchMode enum
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn dict_search_mode_eq() {
    assert_eq!(DictSearchMode::Hc, DictSearchMode::Hc);
    assert_eq!(DictSearchMode::Ext, DictSearchMode::Ext);
    assert_ne!(DictSearchMode::Hc, DictSearchMode::Ext);
}

#[test]
fn dict_search_mode_copy_clone() {
    let a = DictSearchMode::Ext;
    let b = a; // Copy
    let c = a.clone(); // Clone
    assert_eq!(b, DictSearchMode::Ext);
    assert_eq!(c, DictSearchMode::Ext);
}

#[test]
fn dict_search_mode_debug() {
    let _ = format!("{:?}", DictSearchMode::Hc);
    let _ = format!("{:?}", DictSearchMode::Ext);
}

// ─────────────────────────────────────────────────────────────────────────────
// add_position
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn add_position_writes_index_at_slot() {
    let mut table = vec![0u32; LZ4MID_HASHTABLESIZE];
    let slot = 42usize;
    let index = 12345u32;
    unsafe {
        add_position(table.as_mut_ptr(), slot as u32, index);
    }
    assert_eq!(table[slot], index);
}

#[test]
fn add_position_overwrites_previous_value() {
    let mut table = vec![999u32; LZ4MID_HASHTABLESIZE];
    unsafe {
        add_position(table.as_mut_ptr(), 0, 1);
        add_position(table.as_mut_ptr(), 0, 2);
    }
    assert_eq!(table[0], 2);
}

#[test]
fn add_position_at_zero_slot() {
    let mut table = vec![0u32; LZ4MID_HASHTABLESIZE];
    unsafe {
        add_position(table.as_mut_ptr(), 0, 77);
    }
    assert_eq!(table[0], 77);
}

#[test]
fn add_position_at_last_slot() {
    let last = (LZ4MID_HASHTABLESIZE - 1) as u32;
    let mut table = vec![0u32; LZ4MID_HASHTABLESIZE];
    unsafe {
        add_position(table.as_mut_ptr(), last, 999);
    }
    assert_eq!(table[last as usize], 999);
}

// ─────────────────────────────────────────────────────────────────────────────
// fill_htable
// ─────────────────────────────────────────────────────────────────────────────

/// Build a minimal `HcCCtxInternal` pointing at `buf`, suitable for
/// fill_htable.
fn make_ctx_for_fill(buf: &[u8]) -> HcCCtxInternal {
    let mut ctx = HcCCtxInternal::new();
    unsafe {
        init_internal(&mut ctx, buf.as_ptr());
        // Simulate that end has already been extended to cover the buffer.
        ctx.end = buf.as_ptr().add(buf.len());
    }
    ctx
}

#[test]
fn fill_htable_skips_tiny_dict() {
    // dict of size <= LZ4MID_HASHSIZE (8) must be a no-op.
    let buf = vec![0xABu8; 8];
    let mut ctx = make_ctx_for_fill(&buf);
    let old_next = ctx.next_to_update;

    unsafe {
        fill_htable(&mut ctx, buf.as_ptr(), buf.len());
    }
    // next_to_update must NOT have changed for a trivially small buffer.
    assert_eq!(ctx.next_to_update, old_next);
}

#[test]
fn fill_htable_advances_next_to_update_for_normal_dict() {
    // A 1 KB dictionary should cause next_to_update to move.
    let buf = vec![0xCCu8; 1024];
    let mut ctx = make_ctx_for_fill(&buf);
    let old_next = ctx.next_to_update;

    unsafe {
        fill_htable(&mut ctx, buf.as_ptr(), buf.len());
    }
    assert!(
        ctx.next_to_update > old_next,
        "next_to_update should have advanced; was={old_next}, now={}",
        ctx.next_to_update
    );
}

#[test]
fn fill_htable_populates_hash_tables() {
    // After fill_htable on a non-trivial dict, at least one hash4/hash8 slot
    // must be non-zero (the slot is filled with the dict_limit + some offset).
    let buf: Vec<u8> = (0..512u16).map(|i| (i % 251) as u8).collect();
    let mut ctx = make_ctx_for_fill(&buf);

    unsafe {
        fill_htable(&mut ctx, buf.as_ptr(), buf.len());
    }

    // hash_table is shared for both hash4 and hash8 (hash8 starts at offset
    // LZ4MID_HASHTABLESIZE).  At least one entry should be nonzero.
    let any_nonzero = ctx.hash_table.iter().any(|&v| v != 0);
    assert!(
        any_nonzero,
        "hash tables must contain at least one entry after fill"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// select_dict_search_mode
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn select_dict_search_mode_null_returns_none() {
    let result = unsafe { select_dict_search_mode(core::ptr::null()) };
    assert_eq!(result, None);
}

#[test]
fn select_dict_search_mode_lz4mid_strategy_returns_ext() {
    // compression_level 1 → strategy Lz4Mid → DictSearchMode::Ext
    let mut dict_ctx = HcCCtxInternal::new();
    dict_ctx.compression_level = 1;

    let result = unsafe { select_dict_search_mode(&dict_ctx as *const _) };
    assert_eq!(result, Some(DictSearchMode::Ext));
}

#[test]
fn select_dict_search_mode_lz4hc_strategy_returns_hc() {
    // compression_level 9 → strategy Lz4Hc → DictSearchMode::Hc
    let mut dict_ctx = HcCCtxInternal::new();
    dict_ctx.compression_level = 9;

    let result = unsafe { select_dict_search_mode(&dict_ctx as *const _) };
    assert_eq!(result, Some(DictSearchMode::Hc));
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4mid_compress
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise a fresh context pointing at `src`.
fn make_compress_ctx(src: *const u8) -> HcCCtxInternal {
    let mut ctx = HcCCtxInternal::new();
    unsafe {
        init_internal(&mut ctx, src);
    }
    // Set a level in the lz4mid range (0–2).
    ctx.compression_level = 1;
    ctx
}

#[test]
fn lz4mid_compress_empty_input_returns_one_byte() {
    // An empty source must still produce a valid last-literals token (1 byte = 0x00).
    let src: Vec<u8> = vec![];
    let mut dst = vec![0u8; 16];
    let mut src_size = 0i32;

    let mut ctx = make_compress_ctx(src.as_ptr());
    // For empty input, src pointer is never dereferenced, but end must be valid.
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };

    // Must return 1 (the empty-run token byte).
    assert_eq!(
        ret, 1,
        "empty input must produce exactly 1 byte (last-run token)"
    );
    assert_eq!(dst[0], 0x00, "empty last-run token must be 0x00");
    assert_eq!(src_size, 0);
}

#[test]
fn lz4mid_compress_negative_max_output_size_returns_zero() {
    // Note: The migrated code has `debug_assert!(*src_size_ptr >= 0)` which
    // panics in debug builds before the explicit guard is reached for negative
    // src_size.  We test the equivalent guard via negative max_output_size,
    // which exercises the same `if *src_size_ptr < 0 || max_output_size < 0`
    // branch without triggering the debug assertion.
    let src = vec![0u8; 16];
    let mut dst = vec![0u8; 32];
    let mut src_size = 1i32;
    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            -1i32, // negative max_output_size
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };
    assert_eq!(ret, 0, "negative max_output_size must return 0");
}

#[test]
fn lz4mid_compress_oversized_src_returns_zero() {
    // src_size > LZ4_MAX_INPUT_SIZE (0x7E000000) must return 0.
    let src = vec![0u8; 16];
    let mut dst = vec![0u8; 32];
    let mut src_size = 0x7F00_0000i32; // > 0x7E000000
    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };
    assert_eq!(ret, 0, "src_size > MAX must return 0");
}

#[test]
fn lz4mid_compress_incompressible_small_input() {
    // 16 unique bytes — no matches, should still produce valid output.
    let src: Vec<u8> = (0u8..16).collect();
    let mut dst = vec![0u8; 64];
    let mut src_size = src.len() as i32;

    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };

    // Must return a positive value (at least 1 byte for the last-literal token).
    assert!(
        ret > 0,
        "must produce output for incompressible data; got {ret}"
    );
    // src_size_ptr must be set to how many bytes were consumed.
    assert_eq!(src_size as usize, 16);
}

#[test]
fn lz4mid_compress_compressible_data_smaller_than_input() {
    // 1 KB of repeating bytes — highly compressible.
    let src = vec![0xABu8; 1024];
    let mut dst = vec![0u8; 1024]; // generous output buffer
    let mut src_size = src.len() as i32;

    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };

    assert!(ret > 0, "must produce output; got {ret}");
    assert!(
        ret < src.len() as i32,
        "compressible data must compress smaller: output={ret}, input={}",
        src.len()
    );
}

#[test]
fn lz4mid_compress_limited_output_tiny_buffer_returns_zero() {
    // Output buffer too small → must return 0 (compression failure).
    let src = vec![0xCDu8; 64];
    let mut dst = vec![0u8; 4]; // way too small
    let mut src_size = src.len() as i32;

    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::LimitedOutput,
            DictCtxDirective::NoDictCtx,
        )
    };

    assert_eq!(ret, 0, "LimitedOutput with tiny buffer must return 0");
}

#[test]
fn lz4mid_compress_src_size_ptr_updated() {
    // After a successful compress, *src_size_ptr must reflect bytes consumed.
    let src = vec![0x55u8; 128];
    let mut dst = vec![0u8; 256];
    let original_size = src.len() as i32;
    let mut src_size = original_size;

    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };

    assert!(ret > 0);
    // src_size must have been updated to consumed bytes (≤ original).
    assert!(
        src_size <= original_size,
        "src_size_ptr must not exceed original size; got src_size={src_size}"
    );
    assert!(src_size >= 0);
}

#[test]
fn lz4mid_compress_not_limited_produces_valid_output() {
    // NotLimited: even with a buffer just barely large enough, should succeed.
    let src = vec![0x77u8; 32];
    // Worst-case expansion for 32 bytes ≈ 32 + 32/255 + 1 ≈ 34 bytes.
    let mut dst = vec![0u8; 64];
    let mut src_size = src.len() as i32;

    let mut ctx = make_compress_ctx(src.as_ptr());
    unsafe {
        ctx.end = src.as_ptr();
    }

    let ret = unsafe {
        lz4mid_compress(
            &mut ctx,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_size,
            dst.len() as i32,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        )
    };

    assert!(
        ret > 0,
        "NotLimited compress must succeed for 32-byte input"
    );
    assert!(ret <= dst.len() as i32);
}
