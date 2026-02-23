//! Strategy dispatcher and external-dictionary management for HC compression.
//!
//! This module is the central routing layer for HC (High Compression) in lz4r.
//! Given a compression level it selects one of three strategies:
//!
//! | Strategy               | Delegates to            | Character                          |
//! |------------------------|-------------------------|---------------------------------|
//! | [`HcStrategy::Lz4Mid`] | [`lz4mid_compress`]     | Fast mid-level hash-chain pass  |
//! | [`HcStrategy::Lz4Hc`]  | [`compress_hash_chain`] | Classic HC hash-chain search    |
//! | [`HcStrategy::Lz4Opt`] | [`compress_optimal`]    | Optimal parser, highest ratio   |
//!
//! It also manages the two dictionary modes supported by LZ4 HC:
//!
//! - **External dictionary** ([`set_external_dict`]): rotates the current prefix
//!   window into an ext-dict slot, retaining back-references up to 64 KB behind
//!   the new block boundary.
//! - **Dict-context** ([`compress_generic_dict_ctx`]): attaches a pre-built
//!   [`HcCCtxInternal`] as a dictionary, automatically promoting it to ext-dict
//!   mode when the position is zero and the source exceeds 4 KB.
//!
//! The public entry point for most callers is [`compress_generic`], which routes
//! to the no-dict or dict-ctx path based on `ctx.dict_ctx`.
//!
//! The algorithm here corresponds to `LZ4HC_compress_generic*` and
//! `LZ4HC_setExternalDict` in `lz4hc.c` v1.10.0.

use super::compress_hc::{compress_hash_chain, compress_optimal};
use super::lz4mid::lz4mid_compress;
use super::search::{insert, HcFavor};
use super::types::{
    get_clevel_params, DictCtxDirective, HcCCtxInternal, HcStrategy, LZ4HC_CLEVEL_MAX,
};
use crate::block::compress::LZ4_MAX_INPUT_SIZE;
use crate::block::types::LimitedOutputDirective;

/// Maximum back-reference distance at which a dict-ctx is still usable.
/// Beyond 64 KB the LZ4 format cannot encode the offset, so the dict is discarded.
const KB_64: usize = 64 * 1024;

/// Minimum source size required before promoting a dict-ctx to ext-dict mode.
/// Small inputs are not worth the overhead of a full context copy.
const KB_4: usize = 4 * 1024;

impl HcCCtxInternal {
    /// Returns `true` if `self` and `other` use the same strategy family
    /// (both lz4mid, or both non-lz4mid), meaning their internal hash tables
    /// are laid out compatibly for a dict-ctx copy.
    ///
    /// Derived from the reference condition `!(isMid1 ^ isMid2)`, which is
    /// logically equivalent to `isMid1 == isMid2`.
    #[inline]
    pub fn is_compatible(&self, other: &Self) -> bool {
        let is_mid_self =
            get_clevel_params(self.compression_level as i32).strat == HcStrategy::Lz4Mid;
        let is_mid_other =
            get_clevel_params(other.compression_level as i32).strat == HcStrategy::Lz4Mid;
        is_mid_self == is_mid_other
    }
}

/// Rotate the current prefix window into the external-dictionary slot and
/// begin a fresh prefix at `new_block`.
///
/// LZ4 HC supports one ext-dict segment at a time.  Calling this function
/// discards any previously attached dict-ctx and slides the current prefix
/// into the ext-dict slot so that the next block can reference bytes from the
/// previous one as long as the offset stays within 64 KB.
///
/// If the existing prefix is ≥ 4 bytes long and the strategy is not lz4mid,
/// the last few prefix positions are inserted into the hash/chain tables
/// before sliding — this ensures those positions remain referenceable from
/// the new block without a separate `insert` call there.
///
/// Corresponds to `LZ4HC_setExternalDict` in `lz4hc.c`.
///
/// # Safety
/// - `ctx` must be a valid, exclusively-accessible `HcCCtxInternal`.
/// - `new_block` must point to the start of the next input block and remain
///   valid for the lifetime of all subsequent operations on `ctx`.
pub unsafe fn set_external_dict(ctx: &mut HcCCtxInternal, new_block: *const u8) {
    // If the prefix has at least 4 bytes and we are not in lz4mid mode, insert
    // the last 3 prefix positions so they stay reachable after the window slides.
    if ctx.end >= ctx.prefix_start.add(4)
        && get_clevel_params(ctx.compression_level as i32).strat != HcStrategy::Lz4Mid
    {
        insert(ctx, ctx.end.sub(3));
    }

    // Slide the prefix window into the external-dictionary slot.
    // Only one extDict segment is supported; any prior one is discarded.
    ctx.low_limit = ctx.dict_limit;
    ctx.dict_start = ctx.prefix_start;
    ctx.dict_limit = ctx
        .dict_limit
        .wrapping_add((ctx.end as usize - ctx.prefix_start as usize) as u32);
    ctx.prefix_start = new_block;
    ctx.end = new_block;
    ctx.next_to_update = ctx.dict_limit; // resume match-referencing at the new dict boundary

    // An ext-dict and a dict-ctx are mutually exclusive in LZ4 HC.
    ctx.dict_ctx = core::ptr::null();
}

/// Core strategy dispatcher.
///
/// Validates inputs, advances `ctx.end` to include the new source block,
/// resolves the compression parameters for `c_level`, and delegates to the
/// appropriate compressor:
///
/// | `c_param.strat`        | Delegates to             |
/// |------------------------|---------------------------|
/// | `HcStrategy::Lz4Mid`  | [`lz4mid_compress`]      |
/// | `HcStrategy::Lz4Hc`   | [`compress_hash_chain`]  |
/// | `HcStrategy::Lz4Opt`  | [`compress_optimal`]     |
///
/// Returns the number of bytes written to `dst`, or `0` on failure.
/// On failure the context is marked dirty so callers can detect corruption.
///
/// Corresponds to `LZ4HC_compress_generic_internal` in `lz4hc.c`.
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
        HcStrategy::Lz4Mid => {
            lz4mid_compress(ctx, src, dst, src_size_ptr, dst_capacity, limit, dict)
        }
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

/// Compress without a dictionary context.
///
/// Asserts (in debug builds) that `ctx.dict_ctx` is null, then delegates to
/// [`compress_generic_internal`] with [`DictCtxDirective::NoDictCtx`].
///
/// Corresponds to `LZ4HC_compress_generic_noDictCtx` in `lz4hc.c`.
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

/// Compress with an attached dictionary context.
///
/// Selects one of three paths based on the current position within the
/// combined prefix+dict history:
///
/// 1. **position ≥ 64 KB** — the dictionary window is beyond the maximum
///    encodable offset; discard the dict-ctx and compress without it.
/// 2. **position == 0, `src_size` > 4 KB, and strategies are compatible** —
///    the context has no history yet and the source is large enough to benefit:
///    copy the dict-ctx state into `ctx`, promote it to ext-dict mode via
///    [`set_external_dict`], and compress using the standard no-dict path.
/// 3. **Otherwise** — use the attached dict-ctx directly via
///    [`DictCtxDirective::UsingDictCtxHc`].
///
/// Corresponds to `LZ4HC_compress_generic_dictCtx` in `lz4hc.c`.
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
    } else if position == 0 && *src_size_ptr > KB_4 as i32 && ctx.is_compatible(&*ctx.dict_ctx) {
        // No history yet and source is large enough to justify the promotion:
        // overwrite ctx with the full dict-ctx state (inheriting its hash tables),
        // then slide it into ext-dict position so the new block can reference it.
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

/// Top-level HC compression entry point.
///
/// Routes to [`compress_generic_no_dict_ctx`] when `ctx.dict_ctx` is null,
/// or to [`compress_generic_dict_ctx`] when a dictionary context is attached.
/// All callers that do not manage dictionary modes directly should use this
/// function rather than the lower-level variants.
///
/// Corresponds to `LZ4HC_compress_generic` in `lz4hc.c`.
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
