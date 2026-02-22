//! HC strategy dispatcher and external dictionary handling.
//!
//! Translated from lz4hc.c v1.10.0:
//!   - Lines 1373–1415: `LZ4HC_compress_generic_internal`
//!   - Lines 1419–1432: `LZ4HC_compress_generic_noDictCtx`
//!   - Lines 1434–1439: `isStateCompatible`
//!   - Lines 1441–1465: `LZ4HC_compress_generic_dictCtx`
//!   - Lines 1467–1483: `LZ4HC_compress_generic`
//!   - Lines 1660–1678: `LZ4HC_setExternalDict`
//!
//! ## Function Map
//!
//! | C function                          | Rust                                  |
//! |-------------------------------------|---------------------------------------|
//! | `LZ4HC_compress_generic_internal`   | [`compress_generic_internal`]         |
//! | `LZ4HC_compress_generic_noDictCtx`  | [`compress_generic_no_dict_ctx`]      |
//! | `isStateCompatible`                 | [`HcCCtxInternal::is_compatible`]     |
//! | `LZ4HC_compress_generic_dictCtx`    | [`compress_generic_dict_ctx`]         |
//! | `LZ4HC_compress_generic`            | [`compress_generic`]                  |
//! | `LZ4HC_setExternalDict`             | [`set_external_dict`]                 |
//!
//! ## Strategy Dispatch
//!
//! The C `if/else` chain on `cParam.strat` is replaced by a Rust
//! `match c_param.strat { HcStrategy::Lz4Mid => …, Lz4Hc => …, Lz4Opt => … }`.

use crate::block::compress::LZ4_MAX_INPUT_SIZE;
use crate::block::types::LimitedOutputDirective;
use super::compress_hc::{compress_hash_chain, compress_optimal};
use super::lz4mid::lz4mid_compress;
use super::search::{insert, HcFavor};
use super::types::{
    DictCtxDirective, HcCCtxInternal, HcStrategy, LZ4HC_CLEVEL_MAX, get_clevel_params,
};

/// 64 KB boundary used in dict-ctx position check.
const KB_64: usize = 64 * 1024;

/// Minimum source size (> 4 KB) before promoting dict-ctx → ext-dict.
const KB_4: usize = 4 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// isStateCompatible  (lz4hc.c:1434–1439)
// ─────────────────────────────────────────────────────────────────────────────

impl HcCCtxInternal {
    /// Returns `true` if `self` and `other` belong to the same strategy
    /// category (both lz4mid, or both non-lz4mid).
    ///
    /// Equivalent to `isStateCompatible`.  The C expression
    /// `!(isMid1 ^ isMid2)` is equivalent to `isMid1 == isMid2`.
    #[inline]
    pub fn is_compatible(&self, other: &Self) -> bool {
        let is_mid_self  = get_clevel_params(self.compression_level as i32).strat == HcStrategy::Lz4Mid;
        let is_mid_other = get_clevel_params(other.compression_level as i32).strat == HcStrategy::Lz4Mid;
        is_mid_self == is_mid_other
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_setExternalDict  (lz4hc.c:1660–1678)
// ─────────────────────────────────────────────────────────────────────────────

/// Rotate the current prefix window into the external-dictionary slot and
/// begin a fresh prefix at `new_block`.
///
/// If the prefix is long enough (≥ 4 bytes) and the strategy is not lz4mid,
/// the last few prefix positions are inserted into the hash/chain tables so
/// they remain referenceable from the new block.
///
/// Equivalent to `LZ4HC_setExternalDict`.
///
/// # Safety
/// - `ctx` must be a valid, exclusively-accessible `HcCCtxInternal`.
/// - `new_block` must point to the start of the next input block and remain
///   valid for the lifetime of all subsequent operations on `ctx`.
pub unsafe fn set_external_dict(ctx: &mut HcCCtxInternal, new_block: *const u8) {
    // Reference the last few prefix bytes into the hash table (if applicable).
    if ctx.end >= ctx.prefix_start.add(4)
        && get_clevel_params(ctx.compression_level as i32).strat != HcStrategy::Lz4Mid
    {
        // `LZ4HC_Insert(ctxPtr, ctxPtr->end - 3)`
        insert(ctx, ctx.end.sub(3));
    }

    // Slide the prefix window into the external-dictionary slot.
    // Only one extDict segment is supported; any prior one is discarded.
    ctx.low_limit  = ctx.dict_limit;
    ctx.dict_start = ctx.prefix_start;
    ctx.dict_limit = ctx.dict_limit.wrapping_add(
        (ctx.end as usize - ctx.prefix_start as usize) as u32
    );
    ctx.prefix_start    = new_block;
    ctx.end             = new_block;
    ctx.next_to_update  = ctx.dict_limit; // match referencing resumes here

    // Cannot hold both an ext-dict and a dict-ctx simultaneously.
    ctx.dict_ctx = core::ptr::null();
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_compress_generic_internal  (lz4hc.c:1373–1415)
// ─────────────────────────────────────────────────────────────────────────────

/// Core strategy dispatcher.
///
/// Validates the inputs, advances `ctx.end`, selects the compression strategy
/// from the level table, and delegates to the appropriate compressor:
///
/// | `cParam.strat`      | Delegates to             |
/// |---------------------|--------------------------|
/// | `HcStrategy::Lz4Mid`  | [`lz4mid_compress`]    |
/// | `HcStrategy::Lz4Hc`   | [`compress_hash_chain`]|
/// | `HcStrategy::Lz4Opt`  | [`compress_optimal`]   |
///
/// Returns the number of bytes written to `dst`, or `0` on failure.
///
/// Equivalent to `LZ4HC_compress_generic_internal`.
///
/// # Safety
/// - `src` must be readable for `*src_size_ptr` bytes.
/// - `dst` must be writable for `dst_capacity` bytes.
/// - `ctx` must be a valid, exclusively-accessible context.
#[inline(always)]
pub unsafe fn compress_generic_internal(
    ctx: &mut HcCCtxInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    dst_capacity: i32,
    c_level: i32,
    limit: LimitedOutputDirective,
    dict: DictCtxDirective,
) -> i32 {
    // Impossible to store anything in fillOutput mode with insufficient capacity.
    // LASTLITERALS bytes are reserved internally, so we need at least that + 1.
    if limit == LimitedOutputDirective::FillOutput && dst_capacity < 1 {
        return 0;
    }
    if limit != LimitedOutputDirective::NotLimited && dst_capacity < 5 {
        // Not enough room even for a single token + minimal literal run.
        *src_size_ptr = 0;
        ctx.dirty = 1;
        return 0;
    }
    // Reject oversized or negative inputs.
    // Mirrors C: `(U32)*srcSizePtr > (U32)LZ4_MAX_INPUT_SIZE`.
    if *src_size_ptr as u32 > LZ4_MAX_INPUT_SIZE {
        return 0;
    }

    // Advance ctx.end by srcSize so match offsets are relative to the new end.
    ctx.end = ctx.end.add(*src_size_ptr as usize);

    let c_param = get_clevel_params(c_level);
    let favor = if ctx.favor_dec_speed != 0 {
        HcFavor::DecompressionSpeed
    } else {
        HcFavor::CompressionRatio
    };

    let result = match c_param.strat {
        HcStrategy::Lz4Mid => lz4mid_compress(
            ctx,
            src,
            dst,
            src_size_ptr,
            dst_capacity,
            limit,
            dict,
        ),
        HcStrategy::Lz4Hc => compress_hash_chain(
            ctx,
            src,
            dst,
            src_size_ptr,
            dst_capacity,
            c_param.nb_searches as i32,
            limit,
            dict,
        ),
        HcStrategy::Lz4Opt => compress_optimal(
            ctx,
            src,
            dst,
            src_size_ptr,
            dst_capacity,
            c_param.nb_searches as i32,
            c_param.target_length as usize,
            limit,
            c_level >= LZ4HC_CLEVEL_MAX, // full_update = "ultra" mode
            dict,
            favor,
        ),
    };

    if result <= 0 {
        ctx.dirty = 1;
    }
    result
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_compress_generic_noDictCtx  (lz4hc.c:1419–1432)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress without a dictionary context.
///
/// Asserts that `ctx.dict_ctx` is null, then delegates to
/// [`compress_generic_internal`] with `DictCtxDirective::NoDictCtx`.
///
/// Equivalent to `LZ4HC_compress_generic_noDictCtx`.
///
/// # Safety
/// Same as [`compress_generic_internal`].  `ctx.dict_ctx` must be null.
pub unsafe fn compress_generic_no_dict_ctx(
    ctx: &mut HcCCtxInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    dst_capacity: i32,
    c_level: i32,
    limit: LimitedOutputDirective,
) -> i32 {
    debug_assert!(
        ctx.dict_ctx.is_null(),
        "compress_generic_no_dict_ctx: dict_ctx must be null"
    );
    compress_generic_internal(
        ctx,
        src,
        dst,
        src_size_ptr,
        dst_capacity,
        c_level,
        limit,
        DictCtxDirective::NoDictCtx,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_compress_generic_dictCtx  (lz4hc.c:1441–1465)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress with an attached dictionary context.
///
/// Three cases (mirroring C exactly):
///
/// 1. **position ≥ 64 KB** — the dictionary window is too far back; discard
///    the dict-ctx and compress without it.
/// 2. **position == 0 and srcSize > 4 KB and states are compatible** — no
///    history yet and the source is large: copy the dict-ctx into `ctx`,
///    call [`set_external_dict`] to switch to ext-dict mode, and compress.
/// 3. **Otherwise** — use the attached dict-ctx directly via
///    `DictCtxDirective::UsingDictCtxHc`.
///
/// Equivalent to `LZ4HC_compress_generic_dictCtx`.
///
/// # Safety
/// Same as [`compress_generic_internal`].  `ctx.dict_ctx` must be non-null.
pub unsafe fn compress_generic_dict_ctx(
    ctx: &mut HcCCtxInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    dst_capacity: i32,
    c_level: i32,
    limit: LimitedOutputDirective,
) -> i32 {
    debug_assert!(
        !ctx.dict_ctx.is_null(),
        "compress_generic_dict_ctx: dict_ctx must be non-null"
    );

    // Current position within the overall history (prefix bytes + dict bytes).
    let position = (ctx.end as usize).wrapping_sub(ctx.prefix_start as usize)
        + (ctx.dict_limit - ctx.low_limit) as usize;

    if position >= KB_64 {
        // Dictionary is too far behind; discard and fall back to no-dict path.
        ctx.dict_ctx = core::ptr::null();
        compress_generic_no_dict_ctx(ctx, src, dst, src_size_ptr, dst_capacity, c_level, limit)
    } else if position == 0
        && *src_size_ptr > KB_4 as i32
        && ctx.is_compatible(&*ctx.dict_ctx)
    {
        // No history yet and source large enough: promote dict-ctx → ext-dict.
        // C: `LZ4_memcpy(ctx, ctx->dictCtx, sizeof(LZ4HC_CCtx_internal))`
        let dict_ctx_ptr = ctx.dict_ctx;
        core::ptr::copy_nonoverlapping(dict_ctx_ptr, ctx as *mut HcCCtxInternal, 1);
        set_external_dict(ctx, src);
        ctx.compression_level = c_level as i16;
        compress_generic_no_dict_ctx(ctx, src, dst, src_size_ptr, dst_capacity, c_level, limit)
    } else {
        // Use the attached dict-ctx directly.
        compress_generic_internal(
            ctx,
            src,
            dst,
            src_size_ptr,
            dst_capacity,
            c_level,
            limit,
            DictCtxDirective::UsingDictCtxHc,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_compress_generic  (lz4hc.c:1467–1483)
// ─────────────────────────────────────────────────────────────────────────────

/// Top-level HC compression entry point.
///
/// Routes to [`compress_generic_no_dict_ctx`] or [`compress_generic_dict_ctx`]
/// depending on whether `ctx.dict_ctx` is set.
///
/// Equivalent to `LZ4HC_compress_generic`.
///
/// # Safety
/// Same as [`compress_generic_internal`].
pub unsafe fn compress_generic(
    ctx: &mut HcCCtxInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    dst_capacity: i32,
    c_level: i32,
    limit: LimitedOutputDirective,
) -> i32 {
    if ctx.dict_ctx.is_null() {
        compress_generic_no_dict_ctx(ctx, src, dst, src_size_ptr, dst_capacity, c_level, limit)
    } else {
        compress_generic_dict_ctx(ctx, src, dst, src_size_ptr, dst_capacity, c_level, limit)
    }
}
