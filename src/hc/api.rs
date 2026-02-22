//! LZ4 HC public API.
//!
//! Translated from lz4hc.c v1.10.0, lines 1486–2192.
//!
//! ## Function Map
//!
//! | C function                              | Rust                                  |
//! |-----------------------------------------|---------------------------------------|
//! | `LZ4_sizeofStateHC`                     | [`sizeof_state_hc`]                   |
//! | `LZ4_compress_HC_extStateHC_fastReset`  | [`compress_hc_ext_state_fast_reset`]  |
//! | `LZ4_compress_HC_extStateHC`            | [`compress_hc_ext_state`]             |
//! | `LZ4_compress_HC`                       | [`compress_hc`]                       |
//! | `LZ4_compress_HC_destSize`              | [`compress_hc_dest_size`]             |
//! | `LZ4_createStreamHC`                    | [`Lz4StreamHc::create`]               |
//! | `LZ4_freeStreamHC`                      | (via `Drop` on `Box<Lz4StreamHc>`)    |
//! | `LZ4_initStreamHC`                      | [`init_stream_hc`]                    |
//! | `LZ4_resetStreamHC`                     | [`reset_stream_hc`]                   |
//! | `LZ4_resetStreamHC_fast`                | [`reset_stream_hc_fast`]              |
//! | `LZ4_setCompressionLevel`               | [`set_compression_level`]             |
//! | `LZ4_favorDecompressionSpeed`           | [`favor_decompression_speed`]         |
//! | `LZ4_loadDictHC`                        | [`load_dict_hc`]                      |
//! | `LZ4_attach_HC_dictionary`              | [`attach_hc_dictionary`]              |
//! | `LZ4_compress_HC_continue`              | [`compress_hc_continue`]              |
//! | `LZ4_compress_HC_continue_destSize`     | [`compress_hc_continue_dest_size`]    |
//! | `LZ4_saveDictHC`                        | [`save_dict_hc`]                      |
//!
//! ## Notes on `attach_hc_dictionary`
//!
//! The C signature takes a raw `const LZ4_streamHC_t *` for the dictionary.
//! In Rust this is modelled as `Option<*const Lz4StreamHc>`:
//!
//! - If `Some(ptr)`, the pointed-to `Lz4StreamHc` must have been prepared
//!   via [`load_dict_hc`], must remain valid and **unmodified** for the entire
//!   lifetime of the working stream's current session, and its backing
//!   dictionary buffer must likewise remain accessible.
//! - If `None`, any existing dictionary association is detached.
//!
//! Deprecated functions are **not** migrated.

use core::mem;

use crate::block::compress::compress_bound;
use crate::block::types::LimitedOutputDirective;
use super::dispatch::{compress_generic, set_external_dict};
use super::lz4mid::fill_htable;
use super::search::insert;
use super::types::{
    clear_tables, get_clevel_params, init_internal, HcCCtxInternal, HcStrategy,
    LZ4HC_CLEVEL_DEFAULT, LZ4HC_CLEVEL_MAX, LZ4HC_HASHSIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Lz4StreamHc — streaming HC state (equivalent to LZ4_streamHC_t)
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 HC streaming compression state.
///
/// Equivalent to the `LZ4_streamHC_t` union in `lz4hc.h`.  Must be
/// heap-allocated; create with [`Lz4StreamHc::create`] and free by dropping
/// the resulting `Box<Lz4StreamHc>` (equivalent to `LZ4_freeStreamHC`).
pub struct Lz4StreamHc {
    pub(crate) ctx: HcCCtxInternal,
}

// SAFETY: HC compression is single-threaded or externally synchronised.
// Raw pointers inside HcCCtxInternal refer to caller-owned data; the caller
// is responsible for ensuring those lifetimes.
unsafe impl Send for Lz4StreamHc {}

impl Lz4StreamHc {
    /// Allocate and initialise a new HC streaming state on the heap.
    ///
    /// Equivalent to `LZ4_createStreamHC`.
    /// Returns `None` only if the global allocator fails.
    pub fn create() -> Option<Box<Self>> {
        let mut stream = Box::new(Lz4StreamHc {
            ctx: HcCCtxInternal::new(),
        });
        // LZ4_createStreamHC uses ALLOC_AND_ZERO, then LZ4_setCompressionLevel.
        // HcCCtxInternal::new() already zeroes tables, but we must also call
        // set_compression_level to write the default level.
        set_compression_level(&mut stream, LZ4HC_CLEVEL_DEFAULT);
        Some(stream)
    }
}

// `LZ4_freeStreamHC` → simply `drop(box_stream)` in Rust.
// Rust's `Drop` trait handles deallocation automatically.

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_sizeofStateHC  (lz4hc.c:1486)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the size (in bytes) of the HC compression state.
///
/// Use this value when allocating an external state buffer for
/// [`compress_hc_ext_state`] or [`compress_hc_ext_state_fast_reset`].
///
/// Equivalent to `LZ4_sizeofStateHC`.
#[inline]
pub fn sizeof_state_hc() -> usize {
    mem::size_of::<HcCCtxInternal>()
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helper: full state initialisation
// (mirrors the LZ4_initStreamHC logic shared by several public functions)
// ─────────────────────────────────────────────────────────────────────────────

/// Zero and re-initialise all fields of an `Lz4StreamHc`.
///
/// Sets the compression level to `LZ4HC_CLEVEL_DEFAULT`; the caller may
/// override it afterwards with [`set_compression_level`].
///
/// Equivalent to `LZ4_initStreamHC` (which also mirrors `LZ4_resetStreamHC`).
pub fn init_stream_hc(state: &mut Lz4StreamHc) {
    let ctx = &mut state.ctx;
    clear_tables(ctx);
    ctx.end             = core::ptr::null();
    ctx.prefix_start    = core::ptr::null();
    ctx.dict_start      = core::ptr::null();
    ctx.dict_limit      = 0;
    ctx.low_limit       = 0;
    ctx.next_to_update  = 0;
    ctx.compression_level = 0;
    ctx.favor_dec_speed = 0;
    ctx.dirty           = 0;
    ctx.dict_ctx        = core::ptr::null();
    // Set default compression level after clearing
    set_compression_level(state, LZ4HC_CLEVEL_DEFAULT);
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC_extStateHC_fastReset  (lz4hc.c:1500–1510)
// ─────────────────────────────────────────────────────────────────────────────

/// HC one-shot compression using a pre-initialised external state.
///
/// The state **must** have been correctly initialised at least once prior to
/// this call (e.g., via [`init_stream_hc`] or after a prior successful
/// compression).  This variant avoids the full reset overhead; use
/// [`compress_hc_ext_state`] when the state may be in an indeterminate
/// condition.
///
/// Returns the number of bytes written to `dst`, or 0 on failure.
///
/// Equivalent to `LZ4_compress_HC_extStateHC_fastReset`.
///
/// # Safety
/// - `src` must be readable for `src_size` bytes.
/// - `dst` must be writable for `dst_capacity` bytes.
pub unsafe fn compress_hc_ext_state_fast_reset(
    state: &mut Lz4StreamHc,
    src: *const u8,
    dst: *mut u8,
    src_size: i32,
    dst_capacity: i32,
    compression_level: i32,
) -> i32 {
    reset_stream_hc_fast(state, compression_level);
    init_internal(&mut state.ctx, src);
    let mut src_size_mut = src_size;
    let limit = if dst_capacity < compress_bound(src_size) {
        LimitedOutputDirective::LimitedOutput
    } else {
        LimitedOutputDirective::NotLimited
    };
    compress_generic(
        &mut state.ctx,
        src,
        dst,
        &mut src_size_mut,
        dst_capacity,
        compression_level,
        limit,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC_extStateHC  (lz4hc.c:1512–1517)
// ─────────────────────────────────────────────────────────────────────────────

/// HC one-shot compression using an external state buffer.
///
/// The state is fully re-initialised before use.  Safe to call regardless of
/// prior state contents.
///
/// Returns the number of bytes written to `dst`, or 0 on failure.
///
/// Equivalent to `LZ4_compress_HC_extStateHC`.
///
/// # Safety
/// - `src` must be readable for `src_size` bytes.
/// - `dst` must be writable for `dst_capacity` bytes.
pub unsafe fn compress_hc_ext_state(
    state: &mut Lz4StreamHc,
    src: *const u8,
    dst: *mut u8,
    src_size: i32,
    dst_capacity: i32,
    compression_level: i32,
) -> i32 {
    // Full init (mirrors LZ4_initStreamHC in C)
    init_stream_hc(state);
    compress_hc_ext_state_fast_reset(state, src, dst, src_size, dst_capacity, compression_level)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC  (lz4hc.c:1519–1535)
// ─────────────────────────────────────────────────────────────────────────────

/// HC one-shot block compression.
///
/// Allocates a temporary state on the heap, compresses `src[..src_size]`
/// into `dst[..dst_capacity]`, then frees the state.
///
/// Returns the number of bytes written to `dst`, or 0 on failure.
///
/// Equivalent to `LZ4_compress_HC`.
///
/// # Safety
/// - `src` must be readable for `src_size` bytes.
/// - `dst` must be writable for `dst_capacity` bytes.
pub unsafe fn compress_hc(
    src: *const u8,
    dst: *mut u8,
    src_size: i32,
    dst_capacity: i32,
    compression_level: i32,
) -> i32 {
    // Equivalent to C LZ4HC_HEAPMODE==1 path: always use heap allocation.
    let Some(mut state) = Lz4StreamHc::create() else {
        return 0;
    };
    compress_hc_ext_state(&mut state, src, dst, src_size, dst_capacity, compression_level)
    // state dropped here — equivalent to `FREEMEM(statePtr)` in C
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC_destSize  (lz4hc.c:1538–1545)
// ─────────────────────────────────────────────────────────────────────────────

/// HC one-shot compression that fills the destination buffer.
///
/// Reads as much of `src` as will fit; writes at most `target_dst_size`
/// bytes to `dst`.  On success, `*src_size_ptr` is updated to the number of
/// input bytes consumed.
///
/// Returns the number of bytes written to `dst`, or 0 on failure.
///
/// Equivalent to `LZ4_compress_HC_destSize`.
///
/// # Safety
/// - `src` must be readable for `*src_size_ptr` bytes.
/// - `dst` must be writable for `target_dst_size` bytes.
pub unsafe fn compress_hc_dest_size(
    state: &mut Lz4StreamHc,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    target_dst_size: i32,
    c_level: i32,
) -> i32 {
    // Full init, then override compression level (mirrors C exactly)
    init_stream_hc(state);
    init_internal(&mut state.ctx, src);
    set_compression_level(state, c_level);
    compress_generic(
        &mut state.ctx,
        src,
        dst,
        src_size_ptr,
        target_dst_size,
        c_level,
        LimitedOutputDirective::FillOutput,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_resetStreamHC  (lz4hc.c:1589–1593)
// ─────────────────────────────────────────────────────────────────────────────

/// Full reset of an HC streaming state (equivalent to re-initialisation).
///
/// Equivalent to `LZ4_resetStreamHC`.  Prefer [`reset_stream_hc_fast`] when
/// the stream is known to be in a valid state (avoids clearing the tables).
pub fn reset_stream_hc(state: &mut Lz4StreamHc, compression_level: i32) {
    init_stream_hc(state);
    set_compression_level(state, compression_level);
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_resetStreamHC_fast  (lz4hc.c:1595–1609)
// ─────────────────────────────────────────────────────────────────────────────

/// Fast reset of an HC streaming state for starting a new stream.
///
/// If the stream is in an indeterminate ("dirty") state a full reset is
/// performed.  Otherwise the prefix window is slid forward without clearing
/// the hash/chain tables — much cheaper for repeated streaming sessions.
///
/// Equivalent to `LZ4_resetStreamHC_fast`.
pub fn reset_stream_hc_fast(state: &mut Lz4StreamHc, compression_level: i32) {
    if state.ctx.dirty != 0 {
        // Stream is in an unknown state — must do a full reset.
        init_stream_hc(state);
    } else {
        // Fast path: slide the prefix window forward.
        let ctx = &mut state.ctx;
        // Compute prefix length (safe: invariant is end >= prefix_start).
        let prefix_len: u32 = if ctx.end.is_null() || ctx.prefix_start.is_null() {
            0
        } else {
            // SAFETY: both are non-null and end >= prefix_start by invariant.
            (unsafe { ctx.end.offset_from(ctx.prefix_start) }) as u32
        };
        ctx.dict_limit   = ctx.dict_limit.wrapping_add(prefix_len);
        ctx.prefix_start = core::ptr::null();
        ctx.end          = core::ptr::null();
        ctx.dict_ctx     = core::ptr::null();
    }
    set_compression_level(state, compression_level);
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_setCompressionLevel  (lz4hc.c:1611–1617)
// ─────────────────────────────────────────────────────────────────────────────

/// Set the compression level on an HC streaming state.
///
/// Values < 1 are clamped to `LZ4HC_CLEVEL_DEFAULT`; values >
/// `LZ4HC_CLEVEL_MAX` are clamped to `LZ4HC_CLEVEL_MAX`.
///
/// Equivalent to `LZ4_setCompressionLevel`.
pub fn set_compression_level(state: &mut Lz4StreamHc, mut compression_level: i32) {
    if compression_level < 1 {
        compression_level = LZ4HC_CLEVEL_DEFAULT;
    }
    if compression_level > LZ4HC_CLEVEL_MAX {
        compression_level = LZ4HC_CLEVEL_MAX;
    }
    state.ctx.compression_level = compression_level as i16;
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_favorDecompressionSpeed  (lz4hc.c:1619–1622)
// ─────────────────────────────────────────────────────────────────────────────

/// Toggle whether the optimal parser (levels ≥ 10) favours decompression
/// speed over compression ratio.
///
/// Equivalent to `LZ4_favorDecompressionSpeed`.
pub fn favor_decompression_speed(state: &mut Lz4StreamHc, favor: bool) {
    state.ctx.favor_dec_speed = if favor { 1 } else { 0 };
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_loadDictHC  (lz4hc.c:1626–1652)
// ─────────────────────────────────────────────────────────────────────────────

/// Load a dictionary into an HC streaming state.
///
/// At most the last 64 KB of `dictionary[..dict_size]` is retained.  The
/// compression level **must** be set before calling this function (it
/// determines whether the LZ4MID or HC hash-chain tables are built).
///
/// Returns the number of bytes loaded (≤ 64 KB, ≥ 0).
///
/// Equivalent to `LZ4_loadDictHC`.
///
/// # Safety
/// `dictionary` must be readable for `dict_size` bytes and remain valid
/// (unmodified) for the lifetime of all subsequent compression calls on
/// `state`.
pub unsafe fn load_dict_hc(
    state: &mut Lz4StreamHc,
    dictionary: *const u8,
    dict_size: i32,
) -> i32 {
    debug_assert!(dict_size >= 0);

    // Trim to last 64 KB.
    let (dict, dict_size) = if dict_size as usize > 64 * 1024 {
        let trim = dict_size as usize - 64 * 1024;
        (dictionary.add(trim), 64_i32 * 1024)
    } else {
        (dictionary, dict_size)
    };

    // Save compression level; full init resets it to default.
    let c_level = state.ctx.compression_level as i32;

    // Need a full initialisation (fast-reset has bad side-effects here).
    init_stream_hc(state);
    set_compression_level(state, c_level);

    let cp = get_clevel_params(c_level);
    let ctx = &mut state.ctx;

    // Position context at the start of the dictionary.
    init_internal(ctx, dict);
    ctx.end = dict.add(dict_size as usize);

    // Build hash tables over the dictionary content.
    if cp.strat == HcStrategy::Lz4Mid {
        fill_htable(ctx, dict, dict_size as usize);
    } else if dict_size as usize >= LZ4HC_HASHSIZE {
        // Insert everything up to end-3 so the last few bytes are searchable.
        insert(ctx, ctx.end.sub(3));
    }

    dict_size
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_attach_HC_dictionary  (lz4hc.c:1654–1656)
// ─────────────────────────────────────────────────────────────────────────────

/// Attach a pre-loaded dictionary stream to a working stream (no-copy).
///
/// Pass `None` to detach any existing dictionary.
///
/// # Safety
///
/// If `dictionary_stream` is `Some(ptr)`:
/// - `ptr` must point to a valid `Lz4StreamHc` that was prepared via
///   [`load_dict_hc`].
/// - The pointed-to state and its backing dictionary buffer must remain
///   alive and **unmodified** for the entire lifetime of the working
///   stream's current session (i.e., until the next call to
///   [`reset_stream_hc`] or [`reset_stream_hc_fast`]).
///
/// Equivalent to `LZ4_attach_HC_dictionary`.
pub unsafe fn attach_hc_dictionary(
    working_stream: &mut Lz4StreamHc,
    dictionary_stream: Option<*const Lz4StreamHc>,
) {
    working_stream.ctx.dict_ctx = match dictionary_stream {
        Some(ptr) => &(*ptr).ctx as *const HcCCtxInternal,
        None      => core::ptr::null(),
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compressHC_continue_generic  (lz4hc.c:1681–1720, internal)
// ─────────────────────────────────────────────────────────────────────────────

/// Internal generic streaming HC compression.
///
/// Handles auto-init, 2 GB overflow detection, non-contiguous-block detection
/// (ext-dict switch), and overlapping input/dictionary space trimming.
///
/// Equivalent to `LZ4_compressHC_continue_generic`.
///
/// # Safety
/// - `src` must be readable for `*src_size_ptr` bytes.
/// - `dst` must be writable for `dst_capacity` bytes.
unsafe fn compress_hc_continue_generic(
    state: &mut Lz4StreamHc,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    dst_capacity: i32,
    limit: LimitedOutputDirective,
) -> i32 {
    // Auto-init: if the stream has never been used, initialise to src.
    if state.ctx.prefix_start.is_null() {
        init_internal(&mut state.ctx, src);
    }

    // Overflow check: accumulated history > 2 GB → reload last 64 KB as dict.
    // Compute values from ctx first, then drop the borrow before calling load_dict_hc.
    let overflow_reload: Option<(*const u8, i32)> = {
        let ctx = &state.ctx;
        let prefix_len: usize = if ctx.end.is_null() || ctx.prefix_start.is_null() {
            0
        } else {
            ctx.end.offset_from(ctx.prefix_start) as usize
        };
        const GB_2: usize = 2 * 1024 * 1024 * 1024;
        if prefix_len.saturating_add(ctx.dict_limit as usize) > GB_2 {
            let dict_size = prefix_len.min(64 * 1024) as i32;
            let dict_ptr = if dict_size > 0 {
                ctx.end.sub(dict_size as usize)
            } else {
                ctx.end
            };
            Some((dict_ptr, dict_size))
        } else {
            None
        }
    }; // borrow of state.ctx released here

    if let Some((dict_ptr, dict_size)) = overflow_reload {
        load_dict_hc(state, dict_ptr, dict_size);
    }

    // Check if blocks are contiguous in memory.
    // If not, rotate the current prefix into the ext-dict slot.
    if !state.ctx.end.is_null() && src != state.ctx.end {
        set_external_dict(&mut state.ctx, src);
    }

    // Check for overlapping input/dictionary space and trim accordingly.
    // Extract all values from an immutable borrow first, then apply updates via
    // a mutable borrow — this satisfies the Rust borrow checker (NLL).
    let overlap_update: Option<(u32, u32, *const u8, *const u8)> = {
        let ctx = &state.ctx;
        if !ctx.dict_start.is_null() && ctx.dict_limit > ctx.low_limit {
            let source_end = src.add(*src_size_ptr as usize);
            let dict_begin = ctx.dict_start;
            let dict_size_bytes = (ctx.dict_limit - ctx.low_limit) as usize;
            let dict_end = ctx.dict_start.add(dict_size_bytes);

            if source_end > dict_begin && src < dict_end {
                // Trim source_end to dict_end
                let eff_source_end = if source_end > dict_end { dict_end } else { source_end };
                // Both lowLimit and dictStart advance by (eff_source_end - dictStart).
                let advance = eff_source_end.offset_from(ctx.dict_start) as u32;
                let low_limit_new  = ctx.low_limit.wrapping_add(advance);
                let dict_limit     = ctx.dict_limit;
                let dict_start_new = ctx.dict_start.add(advance as usize);
                let prefix_start   = ctx.prefix_start;
                // ctx immutable borrow ends here (last use of ctx)
                Some((low_limit_new, dict_limit, dict_start_new, prefix_start))
            } else {
                None
            }
        } else {
            None
        }
        // ctx: &state.ctx goes out of scope — immutable borrow released
    };
    if let Some((low_limit_new, dict_limit, dict_start_new, prefix_start)) = overlap_update {
        let ctx = &mut state.ctx;
        ctx.low_limit  = low_limit_new;
        ctx.dict_start = dict_start_new;
        // Invalidate dictionary if it has become too small.
        if dict_limit - ctx.low_limit < LZ4HC_HASHSIZE as u32 {
            ctx.low_limit  = dict_limit;
            ctx.dict_start = prefix_start;
        }
    }

    let c_level = state.ctx.compression_level as i32;
    compress_generic(
        &mut state.ctx,
        src,
        dst,
        src_size_ptr,
        dst_capacity,
        c_level,
        limit,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC_continue  (lz4hc.c:1722–1729)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming HC block compression.
///
/// Compresses `src[..src_size]` into `dst[..dst_capacity]`, using all
/// previously compressed data (and any loaded dictionary) as history.
/// Previous input blocks must remain accessible and unmodified.
///
/// Returns the number of bytes written to `dst`, or 0 on failure (which
/// leaves `state` dirty — reset required before next use).
///
/// Equivalent to `LZ4_compress_HC_continue`.
///
/// # Safety
/// - `src` must be readable for `src_size` bytes and remain accessible for
///   the lifetime of the streaming session.
/// - `dst` must be writable for `dst_capacity` bytes.
pub unsafe fn compress_hc_continue(
    state: &mut Lz4StreamHc,
    src: *const u8,
    dst: *mut u8,
    src_size: i32,
    dst_capacity: i32,
) -> i32 {
    let mut src_size_mut = src_size;
    let limit = if dst_capacity < compress_bound(src_size) {
        LimitedOutputDirective::LimitedOutput
    } else {
        LimitedOutputDirective::NotLimited
    };
    compress_hc_continue_generic(state, src, dst, &mut src_size_mut, dst_capacity, limit)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_compress_HC_continue_destSize  (lz4hc.c:1731–1734)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming HC block compression that fills the destination buffer.
///
/// Similar to [`compress_hc_continue`] but reads as much of `src` as fits
/// within `target_dst_size` bytes.  On success `*src_size_ptr` is updated to
/// the number of source bytes consumed.
///
/// Returns the number of bytes written to `dst`, or 0 on failure.
///
/// Equivalent to `LZ4_compress_HC_continue_destSize`.
///
/// # Safety
/// - `src` must be readable for `*src_size_ptr` bytes.
/// - `dst` must be writable for `target_dst_size` bytes.
pub unsafe fn compress_hc_continue_dest_size(
    state: &mut Lz4StreamHc,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    target_dst_size: i32,
) -> i32 {
    compress_hc_continue_generic(
        state,
        src,
        dst,
        src_size_ptr,
        target_dst_size,
        LimitedOutputDirective::FillOutput,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4_saveDictHC  (lz4hc.c:1742–1764)
// ─────────────────────────────────────────────────────────────────────────────

/// Save the last up to `dict_size` bytes of the current streaming history
/// into `safe_buffer`.
///
/// After calling this, `state` references `safe_buffer` as its new prefix
/// window, so `safe_buffer` must remain alive and unmodified for subsequent
/// compression calls.  Pass `dict_size == 0` and/or `safe_buffer` as null to
/// discard history without copying.
///
/// Returns the number of bytes saved (≤ `dict_size`, ≤ 64 KB, ≥ 0).
///
/// Equivalent to `LZ4_saveDictHC`.
///
/// # Safety
/// - If `safe_buffer` is non-null it must be writable for `dict_size` bytes
///   and remain valid as long as `state` is used for subsequent compressions.
/// - If `safe_buffer` is null, `dict_size` must be 0.
pub unsafe fn save_dict_hc(
    state: &mut Lz4StreamHc,
    safe_buffer: *mut u8,
    dict_size: i32,
) -> i32 {
    let ctx = &state.ctx;
    let prefix_size = (ctx.end as usize).wrapping_sub(ctx.prefix_start as usize) as i32;
    debug_assert!(prefix_size >= 0);

    // Clamp dict_size to [0, min(64 KB, prefix_size)].
    let mut dict_size = dict_size;
    if dict_size > 64 * 1024 { dict_size = 64 * 1024; }
    if dict_size < 4          { dict_size = 0; }
    if dict_size > prefix_size { dict_size = prefix_size; }

    debug_assert!(safe_buffer != core::ptr::null_mut() || dict_size == 0);

    // Copy the tail of the prefix into the safe buffer.
    if dict_size > 0 {
        // LZ4_memmove (handles overlap)
        core::ptr::copy(
            ctx.end.sub(dict_size as usize),
            safe_buffer,
            dict_size as usize,
        );
    }

    // Compute the new end index (keeps absolute position consistent).
    let end_index = (ctx.end as usize)
        .wrapping_sub(ctx.prefix_start as usize) as u32
        + ctx.dict_limit;

    // Update context to point at the new safe buffer.
    let ctx = &mut state.ctx;
    ctx.end = if safe_buffer.is_null() {
        core::ptr::null()
    } else {
        (safe_buffer as *const u8).add(dict_size as usize)
    };
    ctx.prefix_start = safe_buffer as *const u8;
    ctx.dict_limit   = end_index.wrapping_sub(dict_size as u32);
    ctx.low_limit    = end_index.wrapping_sub(dict_size as u32);
    ctx.dict_start   = ctx.prefix_start;

    if ctx.next_to_update < ctx.dict_limit {
        ctx.next_to_update = ctx.dict_limit;
    }

    dict_size
}
