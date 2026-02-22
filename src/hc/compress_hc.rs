//! HC main compression loop and optimal parser.
//!
//! Translated from lz4hc.c v1.10.0, lines 1121–1416 (LZ4HC_compress_hashChain and forward
//! declaration of LZ4HC_compress_optimal) plus the actual LZ4HC_compress_optimal
//! implementation at lines 1823–2123.
//!
//! ## Functions
//!
//! | C function                   | Rust function            | C lines      |
//! |------------------------------|--------------------------|--------------|
//! | `LZ4HC_compress_hashChain`   | [`compress_hash_chain`]  | 1121–1362    |
//! | `LZ4HC_literalsPrice`        | [`literals_price`]       | 1778–1785    |
//! | `LZ4HC_sequencePrice`        | [`sequence_price`]       | 1788–1800    |
//! | `LZ4HC_FindLongerMatch`      | [`find_longer_match`]    | 1803–1820    |
//! | `LZ4HC_optimal_t`            | [`Lz4HcOptimal`]         | (internal)   |
//! | `LZ4HC_compress_optimal`     | [`compress_optimal`]     | 1823–2123    |
//!
//! ## goto Conversion Strategy
//!
//! ### `LZ4HC_compress_hashChain` (5 goto destinations)
//!
//! - `goto _last_literals` (input too small, line 1155) →
//!   skip the `'compress_loop` entirely via an `if` guard.
//! - `goto _dest_overflow` (5 call-sites inside search logic) →
//!   set `overflow = true` and `overflow_m1`, then `break 'compress_loop`.
//! - `goto _Search2` (2 call-sites) →
//!   `search_state = SearchState::S2; continue 'search_loop`.
//! - `goto _Search3` (3 call-sites) →
//!   `search_state = SearchState::S3; continue 'search_loop`.
//! - `goto _last_literals` (inside `_dest_overflow` for fillOutput) →
//!   handled by falling through to the last-literals block after overflow handling.
//!
//! ### `LZ4HC_compress_optimal` (4 goto destinations)
//!
//! - `goto _dest_overflow` (2 call-sites) →
//!   set `overflow = true` + capture state, then `break 'compress_loop`.
//! - `goto encode` (1 call-site) →
//!   `break 'dp_loop` with `(best_mlen, best_off, cur)` set immediately.
//! - `goto _last_literals` (1 call-site inside `_dest_overflow`) →
//!   fall through to last-literals block after overflow handling.
//! - `goto _return_label` (2 call-sites) → `return retval` or `retval = 0; break`.

use crate::block::types::{
    self as bt, LimitedOutputDirective, LZ4_DISTANCE_MAX, LASTLITERALS, MFLIMIT, MINMATCH, ML_MASK, RUN_MASK,
};
use super::encode::encode_sequence;
use super::lz4mid::Match;
use super::search::{insert_and_find_best_match, insert_and_get_wider_match, HcFavor};
use super::types::{DictCtxDirective, HcCCtxInternal, LZ4_OPT_NUM, OPTIMAL_ML};

/// Minimum source size (< this ⇒ all literals, no search).
/// Mirrors `LZ4_minLength = MFLIMIT + 1 = 13`.
const LZ4_MIN_LENGTH: usize = MFLIMIT + 1;

/// Number of trailing literal slots in the optimal-parser DP table.
/// Mirrors `#define TRAILING_LITERALS 3`.
const TRAILING_LITERALS: usize = 3;

// ─────────────────────────────────────────────────────────────────────────────
// SearchState enum — replaces _Search2 / _Search3 goto labels
// ─────────────────────────────────────────────────────────────────────────────

/// Controls which label is active at the top of `'search_loop`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchState {
    S2, // goto _Search2
    S3, // goto _Search3
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_compress_hashChain  (lz4hc.c:1121–1362)
// ─────────────────────────────────────────────────────────────────────────────

/// Main HC compression loop: insert, search, backward-extend, encode.
///
/// Equivalent to `LZ4HC_compress_hashChain`.
///
/// On success writes compressed bytes to `dest` and updates `*src_size_ptr`
/// to the number of source bytes consumed.  Returns the number of bytes
/// written, or `0` on compression failure (only possible in `LimitedOutput`
/// mode).
///
/// # Safety
/// - `source` must be valid for reads of `*src_size_ptr` bytes.
/// - `dest` must be valid for writes of `max_output_size` bytes.
/// - `ctx` must have been initialised with `init_internal`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn compress_hash_chain(
    ctx: &mut HcCCtxInternal,
    source: *const u8,
    dest: *mut u8,
    src_size_ptr: &mut i32,
    max_output_size: i32,
    max_nb_attempts: i32,
    limit: LimitedOutputDirective,
    dict: DictCtxDirective,
) -> i32 {
    let input_size = *src_size_ptr;
    let pattern_analysis = max_nb_attempts > 128; // levels 9+

    let mut ip: *const u8 = source;
    let mut anchor: *const u8 = ip;
    let iend: *const u8 = ip.add(input_size as usize);
    let mflimit: *const u8 = iend.sub(MFLIMIT);
    let matchlimit: *const u8 = iend.sub(LASTLITERALS);

    let mut optr: *mut u8 = dest;
    let mut op: *mut u8 = dest;
    let mut oend: *mut u8 = op.add(max_output_size as usize);

    let nomatch = Match { len: 0, off: 0, back: 0 };

    // Saved state across search iterations within one outer-loop pass
    let mut start0: *const u8 = core::ptr::null();
    let mut start2: *const u8 = core::ptr::null();
    let mut start3: *const u8 = core::ptr::null();
    let mut m0 = nomatch;
    let mut m1 = nomatch;
    let mut m2 = nomatch;
    let mut m3 = nomatch;

    // Overflow state: captures m1 and optr at the time of dest_overflow
    let mut overflow_occurred = false;
    let mut overflow_m1 = nomatch;

    *src_size_ptr = 0;

    if limit == LimitedOutputDirective::FillOutput {
        // Hack: reserve LASTLITERALS at the end so the encoder never writes into them.
        oend = oend.sub(LASTLITERALS);
    }

    // If input_size < LZ4_MIN_LENGTH: skip compress loop → go directly to _last_literals.
    if input_size >= LZ4_MIN_LENGTH as i32 {
        // ── Main compression loop ─────────────────────────────────────────
        'compress_loop: while ip <= mflimit {
            m1 = insert_and_find_best_match(
                ctx, ip, matchlimit, max_nb_attempts, pattern_analysis, dict,
            );
            if m1.len < MINMATCH as i32 {
                ip = ip.add(1);
                continue 'compress_loop;
            }

            start0 = ip;
            m0 = m1;

            // ── Search2 / Search3 inner loop ─────────────────────────────
            //
            // In C, `_Search2` and `_Search3` are labels within the while body;
            // the code re-enters them via `goto`.  Here we use a single labeled
            // loop with `SearchState` to select the entry point on each iteration.
            let mut search_state = SearchState::S2;

            'search_loop: loop {
                // ─────────────────────────────────────────────────────────
                // _Search2
                // ─────────────────────────────────────────────────────────
                if search_state == SearchState::S2 {
                    if ip.add(m1.len as usize) <= mflimit {
                        start2 = ip.add(m1.len as usize - 2);
                        m2 = insert_and_get_wider_match(
                            ctx,
                            start2,
                            ip,          // i_low_limit
                            matchlimit,
                            m1.len,
                            max_nb_attempts,
                            pattern_analysis,
                            false,       // chain_swap = 0
                            dict,
                            false,       // favorCompressionRatio
                        );
                        start2 = start2.offset(m2.back as isize);
                    } else {
                        m2 = nomatch;
                    }

                    if m2.len <= m1.len {
                        // No better match → encode m1 immediately.
                        optr = op;
                        if encode_sequence(
                            &mut ip, &mut op, &mut anchor,
                            m1.len, m1.off, limit, oend,
                        )
                        .is_err()
                        {
                            overflow_m1 = m1;
                            overflow_occurred = true;
                            break 'compress_loop;
                        }
                        continue 'compress_loop;
                    }

                    if start0 < ip {
                        // First match was skipped at least once: restore if m2 squeezes m0.
                        if start2 < ip.add(m0.len as usize) {
                            ip = start0;
                            m1 = m0;
                        }
                    }

                    if (start2.offset_from(ip) as i32) < 3 {
                        // First match too small: removed; re-run Search2 with m2 as m1.
                        ip = start2;
                        m1 = m2;
                        search_state = SearchState::S2;
                        continue 'search_loop; // goto _Search2
                    }
                }

                // Reset so the *next* iteration enters S2 unless explicitly set to S3.
                search_state = SearchState::S2;

                // ─────────────────────────────────────────────────────────
                // _Search3
                // ─────────────────────────────────────────────────────────

                // Possibly shorten m1 so that m2 fits after it.
                if (start2.offset_from(ip) as i32) < OPTIMAL_ML {
                    let mut new_ml = m1.len.min(OPTIMAL_ML);
                    let ml_limit = (start2.offset_from(ip) as i32) + m2.len - MINMATCH as i32;
                    if new_ml > ml_limit {
                        new_ml = ml_limit;
                    }
                    let correction = new_ml - (start2.offset_from(ip) as i32);
                    if correction > 0 {
                        start2 = start2.add(correction as usize);
                        m2.len -= correction;
                    }
                }

                if start2.add(m2.len as usize) <= mflimit {
                    start3 = start2.add(m2.len as usize - 3);
                    m3 = insert_and_get_wider_match(
                        ctx,
                        start3,
                        start2,      // i_low_limit
                        matchlimit,
                        m2.len,
                        max_nb_attempts,
                        pattern_analysis,
                        false,       // chain_swap = 0
                        dict,
                        false,       // favorCompressionRatio
                    );
                    start3 = start3.offset(m3.back as isize);
                } else {
                    m3 = nomatch;
                }

                if m3.len <= m2.len {
                    // No better match → encode m1 and m2.
                    if start2 < ip.add(m1.len as usize) {
                        m1.len = start2.offset_from(ip) as i32;
                    }
                    optr = op;
                    if encode_sequence(
                        &mut ip, &mut op, &mut anchor,
                        m1.len, m1.off, limit, oend,
                    )
                    .is_err()
                    {
                        overflow_m1 = m1;
                        overflow_occurred = true;
                        break 'compress_loop;
                    }
                    ip = start2;
                    optr = op;
                    if encode_sequence(
                        &mut ip, &mut op, &mut anchor,
                        m2.len, m2.off, limit, oend,
                    )
                    .is_err()
                    {
                        overflow_m1 = m2; // m1 = m2 before the overflow goto
                        overflow_occurred = true;
                        break 'compress_loop;
                    }
                    continue 'compress_loop;
                }

                if start3 < ip.add(m1.len as usize + 3) {
                    if start3 >= ip.add(m1.len as usize) {
                        // Can write Seq1 immediately: Seq2 removed, Seq3 becomes Seq1.
                        if start2 < ip.add(m1.len as usize) {
                            let correction =
                                (ip.add(m1.len as usize)).offset_from(start2) as i32;
                            start2 = start2.add(correction as usize);
                            m2.len -= correction;
                            if m2.len < MINMATCH as i32 {
                                start2 = start3;
                                m2 = m3;
                            }
                        }
                        optr = op;
                        if encode_sequence(
                            &mut ip, &mut op, &mut anchor,
                            m1.len, m1.off, limit, oend,
                        )
                        .is_err()
                        {
                            overflow_m1 = m1;
                            overflow_occurred = true;
                            break 'compress_loop;
                        }
                        ip = start3;
                        m1 = m3;
                        start0 = start2;
                        m0 = m2;
                        search_state = SearchState::S2;
                        continue 'search_loop; // goto _Search2
                    }
                    // Not enough space for match 2: remove it.
                    start2 = start3;
                    m2 = m3;
                    search_state = SearchState::S3;
                    continue 'search_loop; // goto _Search3
                }

                // OK: we have 3 ascending matches; write m1.
                if start2 < ip.add(m1.len as usize) {
                    if (start2.offset_from(ip) as i32) < OPTIMAL_ML {
                        if m1.len > OPTIMAL_ML {
                            m1.len = OPTIMAL_ML;
                        }
                        let ml_limit =
                            (start2.offset_from(ip) as i32) + m2.len - MINMATCH as i32;
                        if m1.len > ml_limit {
                            m1.len = ml_limit;
                        }
                        let correction = m1.len - (start2.offset_from(ip) as i32);
                        if correction > 0 {
                            start2 = start2.add(correction as usize);
                            m2.len -= correction;
                        }
                    } else {
                        m1.len = start2.offset_from(ip) as i32;
                    }
                }
                optr = op;
                if encode_sequence(
                    &mut ip, &mut op, &mut anchor,
                    m1.len, m1.off, limit, oend,
                )
                .is_err()
                {
                    overflow_m1 = m1;
                    overflow_occurred = true;
                    break 'compress_loop;
                }

                // Shift: ML2 → ML1, ML3 → ML2; search for new ML3.
                ip = start2;
                m1 = m2;
                start2 = start3;
                m2 = m3;
                search_state = SearchState::S3;
                continue 'search_loop; // goto _Search3
            } // 'search_loop
        } // 'compress_loop

        // ── _dest_overflow handling ───────────────────────────────────────
        if overflow_occurred {
            m1 = overflow_m1;
            if limit == LimitedOutputDirective::FillOutput {
                // Assumption: ip, anchor, optr, m1 are set correctly.
                let ll = ip.offset_from(anchor) as usize;
                let ll_addbytes = (ll + 240) / 255;
                let ll_total_cost = 1 + ll_addbytes + ll;
                // 2 for offset, 1 for token
                let max_lit_pos: *mut u8 = oend.sub(3);

                op = optr; // restore correct out pointer
                if op.add(ll_total_cost) <= max_lit_pos {
                    let bytes_left_for_ml =
                        max_lit_pos.offset_from(op.add(ll_total_cost)) as usize;
                    let max_ml_size =
                        MINMATCH + (ML_MASK as usize - 1) + bytes_left_for_ml * 255;
                    debug_assert!(m1.len >= 0);
                    if m1.len as usize > max_ml_size {
                        m1.len = max_ml_size as i32;
                    }
                    // (oend + LASTLITERALS) - (op + ll_total_cost + 2) - 1 + m1.len >= MFLIMIT
                    let room = oend
                        .add(LASTLITERALS)
                        .offset_from(op.add(ll_total_cost + 2))
                        as i32
                        - 1
                        + m1.len;
                    if room >= MFLIMIT as i32 {
                        // Best-effort encode; ignore error (notLimited mode).
                        let _ = encode_sequence(
                            &mut ip, &mut op, &mut anchor,
                            m1.len, m1.off,
                            LimitedOutputDirective::NotLimited,
                            oend,
                        );
                    }
                }
                // Fall through to _last_literals.
            } else {
                // limitedOutput: compression failed.
                return 0;
            }
        }
    } // end if (input_size >= LZ4_MIN_LENGTH)

    // ── _last_literals ────────────────────────────────────────────────────────
    {
        let mut last_run_size = iend.offset_from(anchor) as usize;
        let ll_add = (last_run_size + 255 - RUN_MASK as usize) / 255;
        let total_size = 1 + ll_add + last_run_size;

        if limit == LimitedOutputDirective::FillOutput {
            oend = oend.add(LASTLITERALS); // restore correct value
        }

        if limit != LimitedOutputDirective::NotLimited && op.add(total_size) > oend {
            if limit == LimitedOutputDirective::LimitedOutput {
                return 0;
            }
            // fillOutput: adapt lastRunSize to fill 'dest'.
            let remaining = oend.offset_from(op) as isize;
            if remaining < 2 {
                // Not enough room even for the token byte + 1 literal
                return op.offset_from(dest) as i32;
            }
            last_run_size = remaining as usize - 1; // 1 for token
            let ll_add2 = (last_run_size + 256 - RUN_MASK as usize) / 256;
            last_run_size -= ll_add2;
        }

        ip = anchor.add(last_run_size); // may differ from iend if limit==fillOutput

        if last_run_size >= RUN_MASK as usize {
            let mut accumulator = last_run_size - RUN_MASK as usize;
            *op = (RUN_MASK << bt::ML_BITS) as u8;
            op = op.add(1);
            while accumulator >= 255 {
                *op = 255u8;
                op = op.add(1);
                accumulator -= 255;
            }
            *op = accumulator as u8;
            op = op.add(1);
        } else {
            *op = (last_run_size << bt::ML_BITS as usize) as u8;
            op = op.add(1);
        }
        core::ptr::copy_nonoverlapping(anchor, op, last_run_size);
        op = op.add(last_run_size);
    }

    // End
    *src_size_ptr = ip.offset_from(source) as i32;
    op.offset_from(dest) as i32
}

// ─────────────────────────────────────────────────────────────────────────────
// Optimal-parser price helpers  (lz4hc.c:1778–1800)
// ─────────────────────────────────────────────────────────────────────────────

/// Cost (in bytes) of encoding `litlen` literals.
///
/// Equivalent to `LZ4HC_literalsPrice`.
#[inline(always)]
pub fn literals_price(litlen: i32) -> i32 {
    debug_assert!(litlen >= 0);
    let mut price = litlen;
    if litlen >= RUN_MASK as i32 {
        price += 1 + (litlen - RUN_MASK as i32) / 255;
    }
    price
}

/// Cost (in bytes) of encoding a sequence with `litlen` literals and a
/// match of length `mlen` (must be ≥ `MINMATCH`).
///
/// Equivalent to `LZ4HC_sequencePrice`.
#[inline(always)]
pub fn sequence_price(litlen: i32, mlen: i32) -> i32 {
    debug_assert!(litlen >= 0);
    debug_assert!(mlen >= MINMATCH as i32);
    let mut price = 1 + 2; // token + 16-bit offset
    price += literals_price(litlen);
    if mlen >= (ML_MASK + MINMATCH as u32) as i32 {
        price += 1 + (mlen - (ML_MASK + MINMATCH as u32) as i32) / 255;
    }
    price
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_FindLongerMatch  (lz4hc.c:1803–1820)
// ─────────────────────────────────────────────────────────────────────────────

/// Insert all positions up to `ip` (exclusive) then search for the best match
/// of length strictly greater than `min_len`.
///
/// Unlike [`insert_and_get_wider_match`], this function never allows backward
/// extension (sets `i_low_limit = ip`) and enables both `patternAnalysis` and
/// `chainSwap`.
///
/// Returns a zero-length [`Match`] if no match better than `min_len` was found.
///
/// Equivalent to `LZ4HC_FindLongerMatch`.
///
/// # Safety
/// Same as [`insert_and_get_wider_match`].
#[inline]
pub unsafe fn find_longer_match(
    ctx: &mut HcCCtxInternal,
    ip: *const u8,
    i_high_limit: *const u8,
    min_len: i32,
    nb_searches: i32,
    dict: DictCtxDirective,
    favor_dec_speed: HcFavor,
) -> Match {
    let match0 = Match { len: 0, off: 0, back: 0 };
    let mut md = insert_and_get_wider_match(
        ctx,
        ip,
        ip,            // i_low_limit = ip → no backward extension
        i_high_limit,
        min_len,
        nb_searches,
        true,          // patternAnalysis = 1
        true,          // chainSwap = 1
        dict,
        favor_dec_speed == HcFavor::DecompressionSpeed,
    );
    debug_assert!(md.back == 0);
    if md.len <= min_len {
        return match0;
    }
    if favor_dec_speed == HcFavor::DecompressionSpeed {
        // Shorten overly-long matches to favour decompression speed.
        if md.len > 18 && md.len <= 36 {
            md.len = 18;
        }
    }
    md
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4HcOptimal — DP node for the optimal parser
// ─────────────────────────────────────────────────────────────────────────────

/// One node in the optimal-parser DP table.
///
/// Mirrors C `typedef struct { int price; int off; int mlen; int litlen; } LZ4HC_optimal_t`.
#[derive(Clone, Copy, Default)]
pub struct Lz4HcOptimal {
    /// Minimum cost (in bytes) to encode the stream up to this position.
    pub price: i32,
    /// Best match offset (= back-reference distance).
    pub off: i32,
    /// Best match length at this position (1 means a literal, ≥ MINMATCH means a match).
    pub mlen: i32,
    /// Number of preceding literals for the current sequence.
    pub litlen: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_compress_optimal  (lz4hc.c:1823–2123)
// ─────────────────────────────────────────────────────────────────────────────

/// Dynamic-programming optimal parser for HC compression.
///
/// Equivalent to `LZ4HC_compress_optimal`.
///
/// The C implementation stack-allocates `opt[LZ4_OPT_NUM + TRAILING_LITERALS]`
/// (~64 KB).  To avoid stack overflow in Rust, the array is heap-allocated via
/// `Box<[Lz4HcOptimal]>`.
///
/// Returns the number of bytes written to `dst`, or `0` on failure.
/// On success, `*src_size_ptr` is updated to the number of source bytes consumed.
///
/// # Safety
/// - `source` must be valid for reads of `*src_size_ptr` bytes.
/// - `dst` must be valid for writes of `dst_capacity` bytes.
/// - `ctx` must have been initialised with `init_internal`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn compress_optimal(
    ctx: &mut HcCCtxInternal,
    source: *const u8,
    dst: *mut u8,
    src_size_ptr: &mut i32,
    dst_capacity: i32,
    nb_searches: i32,
    mut sufficient_len: usize,
    limit: LimitedOutputDirective,
    full_update: bool,
    dict: DictCtxDirective,
    favor_dec_speed: HcFavor,
) -> i32 {
    let mut retval: i32 = 0;

    // Heap-allocate opt to avoid ~64 KB stack usage.
    // Mirrors `LZ4HC_optimal_t opt[LZ4_OPT_NUM + TRAILING_LITERALS]`.
    let opt_len = LZ4_OPT_NUM + TRAILING_LITERALS;
    let mut opt: Box<[Lz4HcOptimal]> = vec![Lz4HcOptimal::default(); opt_len].into_boxed_slice();

    let mut ip: *const u8 = source;
    let mut anchor: *const u8 = ip;
    let iend: *const u8 = ip.add(*src_size_ptr as usize);
    let mflimit: *const u8 = iend.sub(MFLIMIT);
    let matchlimit: *const u8 = iend.sub(LASTLITERALS);

    let mut op: *mut u8 = dst;
    let mut op_saved: *mut u8 = dst;
    let mut oend: *mut u8 = op.add(dst_capacity as usize);

    // ovml / ovoff: match that triggered a dest-overflow (mirrors `ovml`, `ovoff`)
    let mut ovml: i32 = MINMATCH as i32;
    let mut ovoff: i32 = 0;
    let mut overflow_occurred = false;

    *src_size_ptr = 0;

    if limit == LimitedOutputDirective::FillOutput {
        oend = oend.sub(LASTLITERALS); // hack for LZ4 format restriction
    }

    if sufficient_len >= LZ4_OPT_NUM {
        sufficient_len = LZ4_OPT_NUM - 1;
    }

    // ── Main Loop ─────────────────────────────────────────────────────────────
    'compress_loop: while ip <= mflimit {
        let llen = ip.offset_from(anchor) as i32;
        let mut last_match_pos: usize = 0;

        let first_match = find_longer_match(
            ctx, ip, matchlimit, MINMATCH as i32 - 1, nb_searches, dict, favor_dec_speed,
        );
        if first_match.len == 0 {
            ip = ip.add(1);
            continue 'compress_loop;
        }

        if first_match.len as usize > sufficient_len {
            // Good enough solution: immediate encoding.
            let first_ml = first_match.len;
            op_saved = op;
            if encode_sequence(
                &mut ip, &mut op, &mut anchor,
                first_ml, first_match.off, limit, oend,
            )
            .is_err()
            {
                ovml = first_ml;
                ovoff = first_match.off;
                overflow_occurred = true;
                break 'compress_loop;
            }
            continue 'compress_loop;
        }

        // ── Set prices for first positions (literals) ──────────────────────
        for r_pos in 0..MINMATCH {
            let cost = literals_price(llen + r_pos as i32);
            opt[r_pos].mlen = 1;
            opt[r_pos].off = 0;
            opt[r_pos].litlen = llen + r_pos as i32;
            opt[r_pos].price = cost;
        }

        // ── Set prices using the initial match ─────────────────────────────
        {
            let match_ml = first_match.len as usize; // < sufficient_len < LZ4_OPT_NUM
            let offset = first_match.off;
            debug_assert!(match_ml < LZ4_OPT_NUM);
            for mlen in MINMATCH..=match_ml {
                let cost = sequence_price(llen, mlen as i32);
                opt[mlen].mlen = mlen as i32;
                opt[mlen].off = offset;
                opt[mlen].litlen = llen;
                opt[mlen].price = cost;
            }
            last_match_pos = match_ml;
        }

        // Initialise trailing literal slots after the first match.
        for add_lit in 1..=TRAILING_LITERALS {
            opt[last_match_pos + add_lit].mlen = 1;
            opt[last_match_pos + add_lit].off = 0;
            opt[last_match_pos + add_lit].litlen = add_lit as i32;
            opt[last_match_pos + add_lit].price =
                opt[last_match_pos].price + literals_price(add_lit as i32);
        }

        // ── Check further positions (DP inner loop) ────────────────────────
        //
        // In C, `goto encode` jumps past the normal opt-table read into the
        // path-reconstruction block.  We capture the encode parameters in
        // mutable vars and use an `Option` to distinguish early-exit from
        // normal exit.
        let mut dp_best_mlen: i32 = 0;
        let mut dp_best_off: i32 = 0;
        let mut dp_early_exit_cur: Option<usize> = None; // Some(cur) if goto encode fired

        {
            let mut cur: usize = 1;
            while cur < last_match_pos {
                let cur_ptr = ip.add(cur);
                if cur_ptr > mflimit {
                    break;
                }

                // Skip position if next position is already cheaper (unless it helps later).
                if full_update {
                    if (opt[cur + 1].price <= opt[cur].price)
                        && (opt[cur + MINMATCH].price < opt[cur].price + 3)
                    {
                        cur += 1;
                        continue;
                    }
                } else if opt[cur + 1].price <= opt[cur].price {
                    cur += 1;
                    continue;
                }

                let new_match = if full_update {
                    find_longer_match(
                        ctx, cur_ptr, matchlimit,
                        MINMATCH as i32 - 1, nb_searches, dict, favor_dec_speed,
                    )
                } else {
                    // Only test matches of minimum length (slightly faster).
                    find_longer_match(
                        ctx, cur_ptr, matchlimit,
                        (last_match_pos - cur) as i32, nb_searches, dict, favor_dec_speed,
                    )
                };

                if new_match.len == 0 {
                    cur += 1;
                    continue;
                }

                if (new_match.len as usize > sufficient_len)
                    || (new_match.len as usize + cur >= LZ4_OPT_NUM)
                {
                    // Immediate encoding: mirrors `goto encode` in C.
                    dp_best_mlen = new_match.len;
                    dp_best_off = new_match.off;
                    dp_early_exit_cur = Some(cur);
                    last_match_pos = cur + 1;
                    break;
                }

                // Before match: set price with literals at beginning.
                {
                    let base_litlen = opt[cur].litlen;
                    for litlen in 1..MINMATCH {
                        let price = opt[cur].price
                            - literals_price(base_litlen)
                            + literals_price(base_litlen + litlen as i32);
                        let pos = cur + litlen;
                        if price < opt[pos].price {
                            opt[pos].mlen = 1;
                            opt[pos].off = 0;
                            opt[pos].litlen = base_litlen + litlen as i32;
                            opt[pos].price = price;
                        }
                    }
                }

                // Set prices using the match at position `cur`.
                {
                    let match_ml = new_match.len as usize;
                    let offset = new_match.off;
                    debug_assert!(cur + match_ml < LZ4_OPT_NUM);
                    for ml in MINMATCH..=match_ml {
                        let pos = cur + ml;
                        let (ll, price) = if opt[cur].mlen == 1 {
                            let ll = opt[cur].litlen;
                            let base_price = if cur > ll as usize {
                                opt[cur - ll as usize].price
                            } else {
                                0
                            };
                            (ll, base_price + sequence_price(ll, ml as i32))
                        } else {
                            (0, opt[cur].price + sequence_price(0, ml as i32))
                        };

                        let dec_speed_bias = favor_dec_speed as i32; // 0 or 1
                        if pos > last_match_pos + TRAILING_LITERALS
                            || price <= opt[pos].price - dec_speed_bias
                        {
                            if ml == match_ml && last_match_pos < pos {
                                last_match_pos = pos;
                            }
                            opt[pos].mlen = ml as i32;
                            opt[pos].off = offset;
                            opt[pos].litlen = ll;
                            opt[pos].price = price;
                        }
                    }
                }

                // Complete following positions with literals.
                for add_lit in 1..=TRAILING_LITERALS {
                    opt[last_match_pos + add_lit].mlen = 1;
                    opt[last_match_pos + add_lit].off = 0;
                    opt[last_match_pos + add_lit].litlen = add_lit as i32;
                    opt[last_match_pos + add_lit].price =
                        opt[last_match_pos].price + literals_price(add_lit as i32);
                }

                cur += 1;
            } // while cur < last_match_pos
        } // DP block

        // Determine encode parameters from either early-exit (goto encode) or normal path.
        // `encode:` label in C — cur, last_match_pos, best_mlen, best_off must be set.
        debug_assert!(last_match_pos < LZ4_OPT_NUM + TRAILING_LITERALS);
        let (best_mlen, best_off, mut candidate_pos) = match dp_early_exit_cur {
            Some(cur) => {
                // goto encode path: best_mlen/off already in dp_best_*, cur is known
                (dp_best_mlen, dp_best_off, cur)
            }
            None => {
                // Normal path: read from opt table
                let bm = opt[last_match_pos].mlen;
                let bo = opt[last_match_pos].off;
                let c = (last_match_pos as i32 - bm) as usize;
                (bm, bo, c)
            }
        };

        // ── encode: reverse traversal to reconstruct the optimal path ──────
        debug_assert!((candidate_pos as i32) < LZ4_OPT_NUM as i32);
        debug_assert!(last_match_pos >= 1);

        {
            let mut selected_match_length = best_mlen;
            let mut selected_offset = best_off;

            loop {
                let next_match_length = opt[candidate_pos].mlen;
                let next_offset = opt[candidate_pos].off;
                opt[candidate_pos].mlen = selected_match_length;
                opt[candidate_pos].off = selected_offset;
                selected_match_length = next_match_length;
                selected_offset = next_offset;
                if next_match_length > candidate_pos as i32 {
                    break; // last match elected (first match to encode)
                }
                debug_assert!(next_match_length > 0);
                candidate_pos -= next_match_length as usize;
            }
        }

        // ── encode all recorded sequences in order ─────────────────────────
        {
            let mut r_pos: usize = 0;
            while r_pos < last_match_pos {
                let ml = opt[r_pos].mlen;
                let offset = opt[r_pos].off;
                if ml == 1 {
                    // literal — skip it (ip advances by 1 per literal below)
                    ip = ip.add(1);
                    r_pos += 1;
                    continue;
                }
                r_pos += ml as usize;
                debug_assert!(ml >= MINMATCH as i32);
                debug_assert!(offset >= 1 && offset <= LZ4_DISTANCE_MAX as i32);
                op_saved = op;
                if encode_sequence(
                    &mut ip, &mut op, &mut anchor,
                    ml, offset, limit, oend,
                )
                .is_err()
                {
                    ovml = ml;
                    ovoff = offset;
                    overflow_occurred = true;
                    break 'compress_loop;
                }
            }
        }
    } // 'compress_loop

    // ── _dest_overflow handling ───────────────────────────────────────────────
    if overflow_occurred {
        if limit == LimitedOutputDirective::FillOutput {
            let ll = ip.offset_from(anchor) as usize;
            let ll_addbytes = (ll + 240) / 255;
            let ll_total_cost = 1 + ll_addbytes + ll;
            let max_lit_pos: *mut u8 = oend.sub(3); // 2 for offset, 1 for token

            op = op_saved; // restore correct out pointer
            if op.add(ll_total_cost) <= max_lit_pos {
                let bytes_left_for_ml =
                    max_lit_pos.offset_from(op.add(ll_total_cost)) as usize;
                let max_ml_size =
                    MINMATCH + (ML_MASK as usize - 1) + bytes_left_for_ml * 255;
                debug_assert!(ovml >= 0);
                if ovml as usize > max_ml_size {
                    ovml = max_ml_size as i32;
                }
                // (oend + LASTLITERALS) - (op + ll_total_cost + 2) - 1 + ovml >= MFLIMIT
                let room = oend
                    .add(LASTLITERALS)
                    .offset_from(op.add(ll_total_cost + 2))
                    as i32
                    - 1
                    + ovml;
                if room >= MFLIMIT as i32 {
                    // Best-effort encode; ignore result (notLimited mode).
                    let _ = encode_sequence(
                        &mut ip, &mut op, &mut anchor,
                        ovml, ovoff,
                        LimitedOutputDirective::NotLimited,
                        oend,
                    );
                }
            }
            // Fall through to _last_literals.
        } else {
            // limitedOutput: compression failed.
            retval = 0;
            // goto _return_label
            return retval;
        }
    }

    // ── _last_literals ────────────────────────────────────────────────────────
    {
        let mut last_run_size = iend.offset_from(anchor) as usize;
        let ll_add = (last_run_size + 255 - RUN_MASK as usize) / 255;
        let total_size = 1 + ll_add + last_run_size;

        if limit == LimitedOutputDirective::FillOutput {
            oend = oend.add(LASTLITERALS); // restore correct value
        }

        if limit != LimitedOutputDirective::NotLimited && op.add(total_size) > oend {
            if limit == LimitedOutputDirective::LimitedOutput {
                retval = 0;
                return retval; // goto _return_label
            }
            // fillOutput: adapt lastRunSize to fill 'dst'.
            last_run_size = oend.offset_from(op) as usize - 1; // 1 for token
            let ll_add2 = (last_run_size + 256 - RUN_MASK as usize) / 256;
            last_run_size -= ll_add2;
        }

        ip = anchor.add(last_run_size); // may differ from iend if limit==fillOutput

        if last_run_size >= RUN_MASK as usize {
            let mut accumulator = last_run_size - RUN_MASK as usize;
            *op = (RUN_MASK << bt::ML_BITS) as u8;
            op = op.add(1);
            while accumulator >= 255 {
                *op = 255u8;
                op = op.add(1);
                accumulator -= 255;
            }
            *op = accumulator as u8;
            op = op.add(1);
        } else {
            *op = (last_run_size << bt::ML_BITS as usize) as u8;
            op = op.add(1);
        }
        core::ptr::copy_nonoverlapping(anchor, op, last_run_size);
        op = op.add(last_run_size);
    }

    // End
    *src_size_ptr = ip.offset_from(source) as i32;
    retval = op.offset_from(dst) as i32;

    // _return_label:
    retval
}
