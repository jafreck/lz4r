//! LZ4MID (medium compression) strategy.
//!
//! Implements the LZ4MID dual-hash algorithm; corresponds to lz4hc.c v1.10.0
//! lines 357–775.
//!
//! # Rust/C name correspondence
//!
//! | C name                        | Rust item                                         |
//! |-------------------------------|---------------------------------------------------|
//! | `LZ4HC_match_t`               | [`Match`]                                         |
//! | `LZ4HC_searchExtDict`         | [`hc_search_ext_dict`] (HC chain ext-dict search) |
//! | `LZ4MID_searchIntoDict_f`     | [`DictSearchMode`] enum (see below)               |
//! | `LZ4MID_searchHCDict`         | dispatched via `DictSearchMode::Hc`               |
//! | `LZ4MID_searchExtDict`        | dispatched via `DictSearchMode::Ext`              |
//! | `LZ4MID_addPosition`          | [`add_position`]                                  |
//! | `LZ4MID_fillHTable`           | [`fill_htable`]                                   |
//! | `select_searchDict_function`  | [`select_dict_search_mode`]                       |
//! | `LZ4MID_compress`             | [`lz4mid_compress`]                               |
//!
//! # Control-flow structure of `lz4mid_compress`
//!
//! `LZ4MID_compress` in lz4hc.c uses 8 `goto` statements.  Their Rust
//! equivalents are:
//!
//! | C label                     | Rust construct                                |
//! |-----------------------------|-----------------------------------------------|
//! | `_lz4mid_encode_sequence` (5×) | `break 'find Some((ml, md))` — labeled-block break returning a found match |
//! | `_lz4mid_dest_overflow` (1×)   | `overflow_info = Some(…); break 'compress`    |
//! | `_lz4mid_last_literals` (2×)   | fall-through to the last-literals block       |
//!
//! # DictSearchMode
//!
//! The C `LZ4MID_searchIntoDict_f` function-pointer typedef is represented as
//! [`DictSearchMode`], a plain enum whose two variants correspond to the two
//! concrete functions that may be selected at runtime.

use super::encode::{encode_sequence, Lz4HcError};
use super::types::{
    count_back, get_clevel_params, mid_hash4_ptr, mid_hash8_ptr, DictCtxDirective, HcCCtxInternal,
    HcStrategy, LZ4MID_HASHSIZE, LZ4MID_HASHTABLESIZE,
};
use crate::block::types::{
    self as bt, LimitedOutputDirective, LASTLITERALS, LZ4_DISTANCE_MAX, MFLIMIT, MINMATCH, ML_BITS,
    ML_MASK, RUN_MASK,
};

// ─────────────────────────────────────────────────────────────────────────────
// Match descriptor  (lz4hc.c:357–361)
// ─────────────────────────────────────────────────────────────────────────────

/// One match found during compression.  Mirrors `LZ4HC_match_t`.
///
/// `back` is a **negative** value indicating how many bytes the match was
/// extended backwards past the current position.
#[derive(Clone, Copy, Debug, Default)]
pub struct Match {
    /// Back-reference distance (positive; equals `ipIndex - matchIndex`).
    pub off: i32,
    /// Match length in bytes (≥ `MINMATCH` for a useful match, 0 if none).
    pub len: i32,
    /// Number of bytes the match was extended backwards (≤ 0).
    pub back: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dict search mode enum  (replaces LZ4MID_searchIntoDict_f function pointer)
// ─────────────────────────────────────────────────────────────────────────────

/// Selects which external-dictionary search function to use inside
/// `lz4mid_compress`.
///
/// Replaces the C function-pointer typedef `LZ4MID_searchIntoDict_f`.  The two
/// variants correspond to the two concrete functions that were ever stored in
/// that pointer:
///
/// | Variant | C function           |
/// |---------|----------------------|
/// | `Hc`    | `LZ4MID_searchHCDict`  |
/// | `Ext`   | `LZ4MID_searchExtDict` |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictSearchMode {
    /// HC-strategy dictionary: search using the HC chain table (2 attempts).
    Hc,
    /// MID-strategy dictionary: search using the dual MID hash tables.
    Ext,
}

// ─────────────────────────────────────────────────────────────────────────────
// HC-chain external-dict search  (lz4hc.c:363–404)
// ─────────────────────────────────────────────────────────────────────────────

/// Search for the best match for `ip` inside an external dictionary described
/// by `dict_ctx`, using the HC chain table.
///
/// Equivalent to `LZ4HC_searchExtDict`.
///
/// # Safety
/// All pointers must be valid for their respective read accesses.
pub unsafe fn hc_search_ext_dict(
    ip: *const u8,
    ip_index: u32,
    i_low_limit: *const u8,
    i_high_limit: *const u8,
    dict_ctx: *const HcCCtxInternal,
    g_dict_end_index: u32,
    current_best_ml: i32,
    mut nb_attempts: i32,
) -> Match {
    let ctx = &*dict_ctx;
    let l_dict_end_index =
        (ctx.end as usize).wrapping_sub(ctx.prefix_start as usize) + ctx.dict_limit as usize;

    // Translate the HC hash into a match index inside the dict window.
    let l_dict_match_index_init = ctx.hash_table[super::types::hash_ptr(ip) as usize];
    let mut l_dict_match_index = l_dict_match_index_init;
    let mut match_index = l_dict_match_index
        .wrapping_add(g_dict_end_index)
        .wrapping_sub(l_dict_end_index as u32);

    let mut best_ml = current_best_ml;
    let mut offset: i32 = 0;
    let mut s_back: i32 = 0;

    debug_assert!(l_dict_end_index <= bt::GB);

    while ip_index.wrapping_sub(match_index) <= LZ4_DISTANCE_MAX && nb_attempts > 0 {
        nb_attempts -= 1;

        let match_ptr = ctx
            .prefix_start
            .sub(ctx.dict_limit as usize)
            .add(l_dict_match_index as usize);

        if bt::read32(match_ptr) == bt::read32(ip) {
            let v_limit_raw =
                ip.add((l_dict_end_index as usize).wrapping_sub(l_dict_match_index as usize));
            let v_limit = if v_limit_raw > i_high_limit {
                i_high_limit
            } else {
                v_limit_raw
            };

            let mlt = bt::count(ip.add(MINMATCH), match_ptr.add(MINMATCH), v_limit) as i32
                + MINMATCH as i32;
            let back = if ip > i_low_limit {
                count_back(ip, match_ptr, i_low_limit, ctx.prefix_start)
            } else {
                0
            };
            let mlt = mlt - back;

            if mlt > best_ml {
                best_ml = mlt;
                offset = ip_index.wrapping_sub(match_index) as i32;
                s_back = back;
            }
        }

        // Follow the chain.  DELTANEXTU16(chainTable, pos) = chainTable[(pos as u16) as usize]
        let next_offset = ctx.chain_table[(l_dict_match_index as u16) as usize] as u32;
        l_dict_match_index = l_dict_match_index.wrapping_sub(next_offset);
        match_index = match_index.wrapping_sub(next_offset);
    }

    Match {
        len: best_ml,
        off: offset,
        back: s_back,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4MID dict search helpers  (lz4hc.c:410–470)
// ─────────────────────────────────────────────────────────────────────────────

/// Search for a match at `ip` in an HC-strategy dictionary using 2 chain
/// attempts.  Wraps [`hc_search_ext_dict`] with LZ4MID defaults.
///
/// Equivalent to `LZ4MID_searchHCDict`.
///
/// # Safety
/// Same requirements as [`hc_search_ext_dict`].
unsafe fn mid_search_hc_dict(
    ip: *const u8,
    ip_index: u32,
    i_high_limit: *const u8,
    dict_ctx: *const HcCCtxInternal,
    g_dict_end_index: u32,
) -> Match {
    hc_search_ext_dict(
        ip,
        ip_index,
        ip, // iLowLimit = ip (no backward extension)
        i_high_limit,
        dict_ctx,
        g_dict_end_index,
        MINMATCH as i32 - 1, // currentBestML = MINMATCH - 1
        2,                   // nbAttempts
    )
}

/// Search for a match at `ip` in a MID-strategy dictionary using the dual
/// (4-byte + 8-byte) hash tables of the dictionary context.
///
/// Equivalent to `LZ4MID_searchExtDict`.
///
/// # Safety
/// All pointers must be valid for their respective read accesses.
unsafe fn mid_search_ext_dict(
    ip: *const u8,
    ip_index: u32,
    i_high_limit: *const u8,
    dict_ctx: *const HcCCtxInternal,
    g_dict_end_index: u32,
) -> Match {
    let ctx = &*dict_ctx;
    let l_dict_end_index =
        (ctx.end as usize).wrapping_sub(ctx.prefix_start as usize) + ctx.dict_limit as usize;

    let hash4_table: *const u32 = ctx.hash_table.as_ptr();
    let hash8_table: *const u32 = hash4_table.add(LZ4MID_HASHTABLESIZE);

    debug_assert!(l_dict_end_index <= bt::GB);

    // Search long match first (8-byte hash).
    {
        let l8_dict_match_index = *hash8_table.add(mid_hash8_ptr(ip) as usize);
        let m8_index = l8_dict_match_index
            .wrapping_add(g_dict_end_index)
            .wrapping_sub(l_dict_end_index as u32);

        if ip_index.wrapping_sub(m8_index) <= LZ4_DISTANCE_MAX {
            let match_ptr = ctx
                .prefix_start
                .sub(ctx.dict_limit as usize)
                .add(l8_dict_match_index as usize);
            let dict_remaining =
                (l_dict_end_index as usize).wrapping_sub(l8_dict_match_index as usize);
            let ip_remaining = (i_high_limit as usize).wrapping_sub(ip as usize);
            let safe_len = dict_remaining.min(ip_remaining);
            let mlt = bt::count(ip, match_ptr, ip.add(safe_len)) as i32;
            if mlt >= MINMATCH as i32 {
                return Match {
                    len: mlt,
                    off: ip_index.wrapping_sub(m8_index) as i32,
                    back: 0,
                };
            }
        }
    }

    // Search short match second (4-byte hash).
    {
        let l4_dict_match_index = *hash4_table.add(mid_hash4_ptr(ip) as usize);
        let m4_index = l4_dict_match_index
            .wrapping_add(g_dict_end_index)
            .wrapping_sub(l_dict_end_index as u32);

        if ip_index.wrapping_sub(m4_index) <= LZ4_DISTANCE_MAX {
            let match_ptr = ctx
                .prefix_start
                .sub(ctx.dict_limit as usize)
                .add(l4_dict_match_index as usize);
            let dict_remaining =
                (l_dict_end_index as usize).wrapping_sub(l4_dict_match_index as usize);
            let ip_remaining = (i_high_limit as usize).wrapping_sub(ip as usize);
            let safe_len = dict_remaining.min(ip_remaining);
            let mlt = bt::count(ip, match_ptr, ip.add(safe_len)) as i32;
            if mlt >= MINMATCH as i32 {
                return Match {
                    len: mlt,
                    off: ip_index.wrapping_sub(m4_index) as i32,
                    back: 0,
                };
            }
        }
    }

    // Nothing found.
    Match {
        off: 0,
        len: 0,
        back: 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// add_position  (lz4hc.c:476–480)
// ─────────────────────────────────────────────────────────────────────────────

/// Record that position `index` in the current input block has the hash value
/// `h_value`.  Equivalent to `LZ4MID_addPosition`.
///
/// # Safety
/// `h_table` must be a valid pointer to an array of at least
/// `LZ4MID_HASHTABLESIZE` `u32` entries, and `h_value < LZ4MID_HASHTABLESIZE`.
#[inline(always)]
pub unsafe fn add_position(h_table: *mut u32, h_value: u32, index: u32) {
    *h_table.add(h_value as usize) = index;
}

// ─────────────────────────────────────────────────────────────────────────────
// fill_htable  (lz4hc.c:487–512)
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-fill the LZ4MID dual hash tables with references into a dictionary
/// buffer.
///
/// Equivalent to `LZ4MID_fillHTable`.
///
/// # Safety
/// `cctx` must be fully initialised.  `dict` must be a valid pointer to
/// `size` readable bytes and must equal `cctx.prefix_start`.
pub unsafe fn fill_htable(cctx: &mut HcCCtxInternal, dict: *const u8, size: usize) {
    let hash4_table: *mut u32 = cctx.hash_table.as_mut_ptr();
    let hash8_table: *mut u32 = hash4_table.add(LZ4MID_HASHTABLESIZE);

    let prefix_ptr: *const u8 = dict;
    let prefix_idx: u32 = cctx.dict_limit;

    if size <= LZ4MID_HASHSIZE {
        return;
    }

    // target = first position we cannot safely hash (need 8 bytes lookahead)
    let target: u32 = prefix_idx + size as u32 - LZ4MID_HASHSIZE as u32;
    let mut idx: u32 = cctx.next_to_update;

    // Coarse pass: every 3rd position, record hash4 for idx, hash8 for idx+1.
    while idx < target {
        // ADDPOS4(prefixPtr + idx - prefixIdx, idx)
        add_position(
            hash4_table,
            mid_hash4_ptr(prefix_ptr.add((idx - prefix_idx) as usize)),
            idx,
        );
        // ADDPOS8(prefixPtr + idx + 1 - prefixIdx, idx + 1)
        add_position(
            hash8_table,
            mid_hash8_ptr(prefix_ptr.add((idx + 1 - prefix_idx) as usize)),
            idx + 1,
        );
        idx = idx.wrapping_add(3);
    }

    // Fine pass: record hash8 for every position in the last 32 KB.
    idx = if size > 32 * bt::KB + LZ4MID_HASHSIZE {
        target.wrapping_sub(32 * bt::KB as u32)
    } else {
        cctx.next_to_update
    };

    while idx < target {
        add_position(
            hash8_table,
            mid_hash8_ptr(prefix_ptr.add((idx - prefix_idx) as usize)),
            idx,
        );
        idx = idx.wrapping_add(1);
    }

    cctx.next_to_update = target;
}

// ─────────────────────────────────────────────────────────────────────────────
// select_dict_search_mode  (lz4hc.c:514–520)
// ─────────────────────────────────────────────────────────────────────────────

/// Choose which dict-search function to use based on the strategy level of
/// the dictionary context.
///
/// Returns `None` when `dict_ctx` is null (no dictionary).
///
/// Equivalent to `select_searchDict_function`.
///
/// # Safety
/// If `dict_ctx` is non-null it must point to a valid, fully initialised
/// [`HcCCtxInternal`].
pub unsafe fn select_dict_search_mode(dict_ctx: *const HcCCtxInternal) -> Option<DictSearchMode> {
    if dict_ctx.is_null() {
        return None;
    }
    let ctx = &*dict_ctx;
    if get_clevel_params(ctx.compression_level as i32).strat == HcStrategy::Lz4Mid {
        Some(DictSearchMode::Ext)
    } else {
        Some(DictSearchMode::Hc)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// dispatch_dict_search  (internal helper)
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch a dict-context search to the appropriate concrete function based
/// on `mode`.
///
/// # Safety
/// `dict_ctx` must be non-null and valid; all other pointer arguments must be
/// readable for the durations of the underlying search.
#[inline(always)]
unsafe fn dispatch_dict_search(
    mode: DictSearchMode,
    ip: *const u8,
    ip_index: u32,
    i_high_limit: *const u8,
    dict_ctx: *const HcCCtxInternal,
    g_dict_end_index: u32,
) -> Match {
    match mode {
        DictSearchMode::Hc => {
            mid_search_hc_dict(ip, ip_index, i_high_limit, dict_ctx, g_dict_end_index)
        }
        DictSearchMode::Ext => {
            mid_search_ext_dict(ip, ip_index, i_high_limit, dict_ctx, g_dict_end_index)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4mid_compress  (lz4hc.c:522–773)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `src[..*src_size_ptr]` into `dst[..max_output_size]` using the
/// LZ4MID (medium-compression) dual-hash strategy.
///
/// Equivalent to `LZ4MID_compress`.
///
/// # Goto-conversion map
///
/// | C label                    | Rust equivalent                                |
/// |----------------------------|------------------------------------------------|
/// | `_lz4mid_encode_sequence`  | `break 'find Some((ml, md))`                   |
/// | `_lz4mid_dest_overflow`    | `overflow_info = Some(…); break 'compress`     |
/// | `_lz4mid_last_literals` (too-small input) | skip 'compress block entirely   |
/// | `_lz4mid_last_literals` (from overflow)   | fall-through after overflow handler |
///
/// # Returns
/// Number of bytes written to `dst`, or 0 on failure / output-too-small.
///
/// On return `*src_size_ptr` is updated to reflect how many input bytes were
/// consumed (may be less than the original value when `limit == FillOutput`).
///
/// # Safety
/// - `ctx` must be fully initialised.
/// - `src` must be readable for `*src_size_ptr` bytes.
/// - `dst` must be writable for `max_output_size` bytes.
/// - All pointer fields inside `ctx` must be valid for the duration of this call.
pub unsafe fn lz4mid_compress(
    ctx: &mut HcCCtxInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    max_output_size: i32,
    limit: LimitedOutputDirective,
    dict: DictCtxDirective,
) -> i32 {
    // ── Hash-table pointers ────────────────────────────────────────────────
    let hash4_table: *mut u32 = ctx.hash_table.as_mut_ptr();
    let hash8_table: *mut u32 = hash4_table.add(LZ4MID_HASHTABLESIZE);

    // ── Input/output cursors ───────────────────────────────────────────────
    let mut ip: *const u8 = src;
    let mut anchor: *const u8 = ip;
    let iend: *const u8 = ip.add(*src_size_ptr as usize);
    let mflimit: *const u8 = iend.sub(MFLIMIT);
    let matchlimit: *const u8 = iend.sub(LASTLITERALS);
    let ilimit: *const u8 = iend.sub(LZ4MID_HASHSIZE);
    let mut op: *mut u8 = dst;
    let mut oend: *mut u8 = op.add(max_output_size as usize);

    // ── Context-derived constants ──────────────────────────────────────────
    let prefix_ptr: *const u8 = ctx.prefix_start;
    let prefix_idx: u32 = ctx.dict_limit;
    // ilimitIdx: index of the last safely hashable position.
    let ilimit_idx: u32 =
        ((ilimit as usize).wrapping_sub(prefix_ptr as usize) as u32).wrapping_add(prefix_idx);
    let dict_start: *const u8 = ctx.dict_start;
    let dict_idx: u32 = ctx.low_limit;
    let g_dict_end_index: u32 = ctx.low_limit;

    // Select dict-search mode once (replaces the C function pointer).
    let dict_search_mode: Option<DictSearchMode> = if dict == DictCtxDirective::UsingDictCtxHc {
        select_dict_search_mode(ctx.dict_ctx)
    } else {
        None
    };

    // Mutable match state (set in 'find block, consumed in encode block).
    let mut match_length: u32 = 0;
    let mut match_distance: u32 = 0;

    // ── Input sanitisation ─────────────────────────────────────────────────
    debug_assert!(*src_size_ptr >= 0);
    if *src_size_ptr > 0 {
        debug_assert!(!src.is_null());
    }
    if max_output_size > 0 {
        debug_assert!(!dst.is_null());
    }
    if *src_size_ptr < 0 || max_output_size < 0 {
        return 0;
    }
    // LZ4_MAX_INPUT_SIZE = 0x7E000000 (forbidden by LZ4 format)
    if (*src_size_ptr as u32) > 0x7E00_0000u32 {
        return 0;
    }

    // FillOutput mode: hide the last LASTLITERALS bytes of the output window
    // so the main loop never tries to write there.
    if limit == LimitedOutputDirective::FillOutput {
        oend = oend.sub(LASTLITERALS);
    }

    // ── Goto _lz4mid_last_literals: skip compress loop for tiny inputs ─────
    // `if (*srcSizePtr < LZ4_minLength) goto _lz4mid_last_literals;`
    let do_compress = *src_size_ptr >= bt::LZ4_MIN_LENGTH as i32;

    // ── Overflow state: set when encode_sequence fails mid-loop ───────────
    let mut overflow_info: Option<(u32, u32)> = None; // (match_length, match_distance)

    // ── Main compress loop ─────────────────────────────────────────────────
    // Uses a `loop { while ip <= mflimit { … } break; }` structure so that
    // `break 'compress` exits to the overflow handler before last_literals.
    if do_compress {
        // The outer `loop` is intentional: it acts as a structured goto,
        // allowing `break 'compress` to jump past the while body to the
        // overflow handler / last-literals block.  It never iterates twice.
        #[allow(clippy::never_loop)]
        'compress: loop {
            while ip <= mflimit {
                // Compute the u32 index of the current input position in the
                // global index space.  `prefix_idx` is the base; the byte
                // difference from `prefix_ptr` gives the offset within the
                // current block.  Wrapping arithmetic is intentional: the LZ4
                // block format uses a 32-bit modular position space throughout.
                let ip_index_start: u32 = ((ip as usize).wrapping_sub(prefix_ptr as usize) as u32)
                    .wrapping_add(prefix_idx);

                // ── 'find: find a match (all 5 encode-sequence gotos) ─────
                // Returns Some((match_length, match_distance)) when a match
                // of length ≥ MINMATCH is found, None otherwise.
                let found: Option<(u32, u32)> = 'find: {
                    // ── Long-match search (8-byte hash) ───────────────────
                    {
                        let h8 = mid_hash8_ptr(ip);
                        let pos8 = *hash8_table.add(h8 as usize);
                        debug_assert!((h8 as usize) < LZ4MID_HASHTABLESIZE);
                        debug_assert!(pos8 < ip_index_start);
                        // Update hash table with current position.
                        add_position(hash8_table, h8, ip_index_start);

                        if ip_index_start.wrapping_sub(pos8) <= LZ4_DISTANCE_MAX {
                            // Match candidate found.
                            if pos8 >= prefix_idx {
                                // Match in current prefix window.
                                let match_ptr = prefix_ptr.add((pos8 - prefix_idx) as usize);
                                debug_assert!(match_ptr < ip);
                                let mlt = bt::count(ip, match_ptr, matchlimit);
                                if mlt >= MINMATCH as u32 {
                                    // goto _lz4mid_encode_sequence (1 of 5)
                                    break 'find Some((mlt, ip_index_start - pos8));
                                }
                            } else if pos8 >= dict_idx {
                                // Match in external dictionary (extDict).
                                let match_ptr = dict_start.add((pos8 - dict_idx) as usize);
                                let safe_len = ((prefix_idx - pos8) as usize)
                                    .min((matchlimit as usize).wrapping_sub(ip as usize));
                                let mlt = bt::count(ip, match_ptr, ip.add(safe_len));
                                if mlt >= MINMATCH as u32 {
                                    // goto _lz4mid_encode_sequence (2 of 5)
                                    break 'find Some((mlt, ip_index_start - pos8));
                                }
                            }
                        }
                    }

                    // ── Short-match search (4-byte hash) ──────────────────
                    {
                        let h4 = mid_hash4_ptr(ip);
                        let pos4 = *hash4_table.add(h4 as usize);
                        debug_assert!((h4 as usize) < LZ4MID_HASHTABLESIZE);
                        debug_assert!(pos4 < ip_index_start);
                        // Update hash table with current position.
                        add_position(hash4_table, h4, ip_index_start);

                        if ip_index_start.wrapping_sub(pos4) <= LZ4_DISTANCE_MAX {
                            if pos4 >= prefix_idx {
                                // Match in prefix window only.
                                let match_ptr = prefix_ptr.add((pos4 - prefix_idx) as usize);
                                debug_assert!(match_ptr < ip);
                                debug_assert!(match_ptr >= prefix_ptr);
                                let mlt = bt::count(ip, match_ptr, matchlimit);
                                if mlt >= MINMATCH as u32 {
                                    // Short match found; look one position ahead for longer.
                                    // ip advances by one for the lookahead match, but
                                    // ip_index_start is intentionally left at the original
                                    // position so that subsequent hash stores index correctly.
                                    let h8_next = mid_hash8_ptr(ip.add(1));
                                    let pos8_next = *hash8_table.add(h8_next as usize);
                                    let m2_distance = ip_index_start + 1 - pos8_next;
                                    let mut best_mlt = mlt;
                                    let mut best_dist = ip_index_start - pos4;

                                    if m2_distance <= LZ4_DISTANCE_MAX
                                        && pos8_next >= prefix_idx
                                        && ip < mflimit
                                    {
                                        let m2_ptr =
                                            prefix_ptr.add((pos8_next - prefix_idx) as usize);
                                        let ml2 = bt::count(ip.add(1), m2_ptr, matchlimit);
                                        if ml2 > best_mlt {
                                            // Prefer the longer ip+1 match.
                                            add_position(hash8_table, h8_next, ip_index_start + 1);
                                            ip = ip.add(1); // advance ip for ip+1 lookahead
                                            best_mlt = ml2;
                                            best_dist = m2_distance;
                                        }
                                    }
                                    // goto _lz4mid_encode_sequence (3 of 5)
                                    break 'find Some((best_mlt, best_dist));
                                }
                            } else if pos4 >= dict_idx {
                                // Match in external dictionary.
                                let match_ptr = dict_start.add((pos4 - dict_idx) as usize);
                                let safe_len = ((prefix_idx - pos4) as usize)
                                    .min((matchlimit as usize).wrapping_sub(ip as usize));
                                let mlt = bt::count(ip, match_ptr, ip.add(safe_len));
                                if mlt >= MINMATCH as u32 {
                                    // goto _lz4mid_encode_sequence (4 of 5)
                                    break 'find Some((mlt, ip_index_start - pos4));
                                }
                            }
                        }
                    }

                    // ── External dict-context search ───────────────────────
                    if dict == DictCtxDirective::UsingDictCtxHc {
                        // Only worth searching when dictCtx is close enough.
                        if ip_index_start.wrapping_sub(g_dict_end_index) < LZ4_DISTANCE_MAX - 8 {
                            if let Some(mode) = dict_search_mode {
                                let d_match = dispatch_dict_search(
                                    mode,
                                    ip,
                                    ip_index_start,
                                    matchlimit,
                                    ctx.dict_ctx,
                                    g_dict_end_index,
                                );
                                if d_match.len >= MINMATCH as i32 {
                                    debug_assert_eq!(d_match.back, 0);
                                    // goto _lz4mid_encode_sequence (5 of 5)
                                    break 'find Some((d_match.len as u32, d_match.off as u32));
                                }
                            }
                        }
                    }

                    // No match found this position.
                    None
                }; // end 'find

                if let Some((ml, md)) = found {
                    match_length = ml;
                    match_distance = md;

                    // ── _lz4mid_encode_sequence: catch back ────────────────
                    // Extend the match backwards while the literal side and the
                    // reference side have identical bytes before the current position.
                    // Bitwise & (not &&) evaluates both conditions without short-circuit,
                    // avoiding any undefined behaviour when ip equals anchor or
                    // the index exactly equals match_distance.
                    // Wrapping subtraction handles the u32 index space correctly
                    // near the prefix_ptr boundary.
                    while ((ip > anchor) as u8)
                        & (((ip as usize).wrapping_sub(prefix_ptr as usize) as u32 > match_distance)
                            as u8)
                        != 0
                        && *ip.sub(1) == *ip.sub(match_distance as usize + 1)
                    {
                        ip = ip.sub(1);
                        match_length += 1;
                    }

                    // ── Fill hash tables at start of match ─────────────────
                    // ip_index_start holds the loop-entry position even when ip
                    // was advanced by the ip+1 lookahead, so hash entries at
                    // indices +1 and +2 align with the correct absolute positions.
                    add_position(hash8_table, mid_hash8_ptr(ip.add(1)), ip_index_start + 1);
                    add_position(hash8_table, mid_hash8_ptr(ip.add(2)), ip_index_start + 2);
                    add_position(hash4_table, mid_hash4_ptr(ip.add(1)), ip_index_start + 1);

                    // ── Encode the sequence ────────────────────────────────
                    let saved_op = op;
                    // encode_sequence advances ip by match_length and resets anchor = ip.
                    match encode_sequence(
                        &mut ip,
                        &mut op,
                        &mut anchor,
                        match_length as i32,
                        match_distance as i32,
                        limit,
                        oend,
                    ) {
                        Ok(()) => {}
                        Err(Lz4HcError::OutputTooSmall) => {
                            // Restore op (ip and anchor were NOT modified on failure).
                            op = saved_op;
                            // goto _lz4mid_dest_overflow
                            overflow_info = Some((match_length, match_distance));
                            break 'compress;
                        }
                    }

                    // ── Fill hash tables at end of match ───────────────────
                    // ip has now advanced past the match by encode_sequence.
                    let end_match_idx: u32 = ((ip as usize).wrapping_sub(prefix_ptr as usize)
                        as u32)
                        .wrapping_add(prefix_idx);
                    let pos_m2 = end_match_idx.wrapping_sub(2);

                    if pos_m2 < ilimit_idx {
                        if (ip as usize).wrapping_sub(prefix_ptr as usize) > 5 {
                            add_position(hash8_table, mid_hash8_ptr(ip.sub(5)), end_match_idx - 5);
                        }
                        add_position(hash8_table, mid_hash8_ptr(ip.sub(3)), end_match_idx - 3);
                        add_position(hash8_table, mid_hash8_ptr(ip.sub(2)), end_match_idx - 2);
                        add_position(hash4_table, mid_hash4_ptr(ip.sub(2)), end_match_idx - 2);
                        add_position(hash4_table, mid_hash4_ptr(ip.sub(1)), end_match_idx - 1);
                    }
                } else {
                    // No match found; skip faster over incompressible data.
                    // `ip += 1 + ((ip - anchor) >> 9)`
                    let skip = 1 + ((ip as usize).wrapping_sub(anchor as usize) >> 9);
                    ip = ip.add(skip);
                }
            } // end while ip <= mflimit
            break 'compress;
        } // end 'compress loop
    } // end if do_compress

    // ── _lz4mid_dest_overflow handler ─────────────────────────────────────
    // Reached when encode_sequence failed (output too small).
    if let Some((ml, md)) = overflow_info {
        if limit != LimitedOutputDirective::FillOutput {
            // Not fillOutput → compression failed entirely.
            return 0;
        }

        // FillOutput: try to squeeze in one last (potentially truncated) sequence.
        let ll = (ip as usize).wrapping_sub(anchor as usize); // literal run length
        let ll_addbytes = (ll + 240) / 255;
        let ll_total_cost = 1 + ll_addbytes + ll;
        let max_lit_pos = oend.sub(3); // 2 bytes offset + 1 byte token

        if op.add(ll_total_cost) <= max_lit_pos {
            let bytes_left_for_ml =
                (max_lit_pos as usize).wrapping_sub(op.add(ll_total_cost) as usize);
            let max_ml_size = MINMATCH + (ML_MASK as usize - 1) + bytes_left_for_ml * 255;
            debug_assert!(max_ml_size < i32::MAX as usize);

            let adj_ml = if ml as usize > max_ml_size {
                max_ml_size as u32
            } else {
                ml
            };

            // Check whether we have room for the final match sequence.
            let check_val =
                (oend.add(LASTLITERALS) as isize) - (op.add(ll_total_cost + 2) as isize) - 1
                    + adj_ml as isize;
            if check_val >= MFLIMIT as isize {
                // Encode the partial last sequence (best-effort, ignore errors).
                let _ = encode_sequence(
                    &mut ip,
                    &mut op,
                    &mut anchor,
                    adj_ml as i32,
                    md as i32,
                    LimitedOutputDirective::NotLimited,
                    oend,
                );
            }
        }
        // Fall through to _lz4mid_last_literals (goto in C).
    }

    // ── _lz4mid_last_literals ─────────────────────────────────────────────
    // Encode the remaining bytes (from `anchor` to `iend`) as a literal run.
    {
        let last_run_size: usize = (iend as usize).wrapping_sub(anchor as usize);
        let ll_add: usize = (last_run_size + 255 - RUN_MASK as usize) / 255;
        let total_size: usize = 1 + ll_add + last_run_size;

        // Restore the real oend (FillOutput hid LASTLITERALS earlier).
        if limit == LimitedOutputDirective::FillOutput {
            oend = oend.add(LASTLITERALS);
        }

        // Handle the case where the literal run would overflow the output.
        let (last_run_size, _ll_add) =
            if limit != LimitedOutputDirective::NotLimited && op.add(total_size) > oend {
                if limit == LimitedOutputDirective::LimitedOutput {
                    // Not enough space in dst — signal failure.
                    return 0;
                }
                // FillOutput: trim the last literal run to fit.
                let lrs = (oend as usize).wrapping_sub(op as usize).saturating_sub(1);
                let la = (lrs + 256 - RUN_MASK as usize) / 256;
                (lrs - la, la)
            } else {
                (last_run_size, ll_add)
            };

        // ip reflects how many input bytes were consumed.
        ip = anchor.add(last_run_size);

        // Write the literal-length token (and optional extensions).
        if last_run_size >= RUN_MASK as usize {
            let mut accumulator = last_run_size - RUN_MASK as usize;
            *op = (RUN_MASK << ML_BITS) as u8;
            op = op.add(1);
            while accumulator >= 255 {
                *op = 255;
                op = op.add(1);
                accumulator -= 255;
            }
            *op = accumulator as u8;
            op = op.add(1);
        } else {
            *op = (last_run_size << ML_BITS as usize) as u8;
            op = op.add(1);
        }

        // Copy the literal bytes.
        debug_assert!(last_run_size <= (oend as usize).wrapping_sub(op as usize));
        core::ptr::copy_nonoverlapping(anchor, op, last_run_size);
        op = op.add(last_run_size);
    }

    // ── End ───────────────────────────────────────────────────────────────
    debug_assert!(ip >= src);
    debug_assert!(ip <= iend);
    *src_size_ptr = (ip as usize).wrapping_sub(src as usize) as i32;
    debug_assert!(op >= dst);
    debug_assert!(op <= oend);
    debug_assert!((op as usize).wrapping_sub(dst as usize) < i32::MAX as usize);

    (op as usize).wrapping_sub(dst as usize) as i32
}
