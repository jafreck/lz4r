//! HC main compression loop and optimal parser.
//!
//! This module implements the two high-compression (HC) encoding strategies:
//!
//! - **[`compress_hash_chain`]** — a greedy match selector that considers up to
//!   three overlapping matches at a time, choosing the best pair to emit.
//!   Used at compression levels 1–8.
//!
//! - **[`compress_optimal`]** — a dynamic-programming (DP) optimal parser that
//!   evaluates all match candidates within an [`LZ4_OPT_NUM`]-sized window and
//!   picks the encoding path with the lowest byte cost.  Used at levels 9–12
//!   (and when `favor_dec_speed` is active).
//!
//! Supporting items:
//!
//! - [`literals_price`] / [`sequence_price`] — byte-cost functions used by the
//!   DP price comparisons.
//! - [`find_longer_match`] — searches for a match strictly longer than a given
//!   minimum, used inside the DP inner loop.
//! - [`Lz4HcOptimal`] — one node in the DP table (price, offset, match length,
//!   literal count).
//!
//! Both compressors support three output-limit modes (`NotLimited`,
//! `LimitedOutput`, `FillOutput`) and optional dictionary context
//! (`DictCtxDirective`).
//!
//! See `lz4hc.c` in the LZ4 reference implementation for the authoritative
//! algorithm description.

use super::encode::encode_sequence;
use super::lz4mid::Match;
use super::search::{insert_and_find_best_match, insert_and_get_wider_match, HcFavor};
use super::types::{DictCtxDirective, HcCCtxInternal, LZ4_OPT_NUM, OPTIMAL_ML};
use crate::block::types::{
    self as bt, LimitedOutputDirective, LASTLITERALS, LZ4_DISTANCE_MAX, MFLIMIT, MINMATCH, ML_MASK,
    RUN_MASK,
};

/// Minimum source size below which no matches are searched; all bytes are
/// emitted as literals.  Equals `MFLIMIT + 1 = 13` per the LZ4 spec.
const LZ4_MIN_LENGTH: usize = MFLIMIT + 1;

/// Number of trailing literal slots in the optimal-parser DP table.
/// Extra DP table slots allocated past `last_match_pos` to hold trailing
/// literal-only positions without bounds-checking each update.
const TRAILING_LITERALS: usize = 3;

// ─────────────────────────────────────────────────────────────────────────────
// SearchState
// ─────────────────────────────────────────────────────────────────────────────

/// Selects the entry point on each iteration of `'search_loop` in
/// [`compress_hash_chain`].
///
/// `S2` re-evaluates the second candidate match; `S3` skips directly to
/// evaluating the third, reusing the second match unchanged.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchState {
    S2,
    S3,
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_hash_chain
// ─────────────────────────────────────────────────────────────────────────────

/// Greedy HC compression loop for levels 1–8 (`LZ4HC_compress_hashChain` in
/// `lz4hc.c`).
///
/// Each iteration inserts the current position into the hash chain, searches
/// for the best match, then speculatively looks one or two positions ahead to
/// decide whether a longer overlapping match produces better compression before
/// emitting any sequence.
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

    let nomatch = Match {
        len: 0,
        off: 0,
        back: 0,
    };

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
        // The LZ4 frame format requires LASTLITERALS bytes of headroom at
        // the end; shorten the effective output limit so encode_sequence
        // never writes into that reserved region.  The limit is restored
        // before writing the final literal run.
    }

    // Short inputs carry no matches; jump straight to the final literal run.
    if input_size >= LZ4_MIN_LENGTH as i32 {
        // ── Main compression loop ─────────────────────────────────────────
        'compress_loop: while ip <= mflimit {
            m1 = insert_and_find_best_match(
                ctx,
                ip,
                matchlimit,
                max_nb_attempts,
                pattern_analysis,
                dict,
            );
            if m1.len < MINMATCH as i32 {
                ip = ip.add(1);
                continue 'compress_loop;
            }

            start0 = ip;
            m0 = m1;

            // ── Lookahead loop ────────────────────────────────────────────
            //
            // Speculatively search for a second match (m2) starting near the
            // end of m1, and a third (m3) near the end of m2.  If a later
            // match is strictly better, we delay emitting the earlier one and
            // shift the window forward.  `SearchState` controls whether we
            // re-evaluate the second candidate or skip straight to the third
            // on the next iteration.
            let mut search_state = SearchState::S2;

            'search_loop: loop {
                // ── Step S2: search for a second candidate match near the end of m1.
                if search_state == SearchState::S2 {
                    if ip.add(m1.len as usize) <= mflimit {
                        start2 = ip.add(m1.len as usize - 2);
                        m2 = insert_and_get_wider_match(
                            ctx,
                            start2,
                            ip, // i_low_limit
                            matchlimit,
                            m1.len,
                            max_nb_attempts,
                            pattern_analysis,
                            false, // chain_swap = 0
                            dict,
                            false, // favorCompressionRatio
                        );
                        start2 = start2.offset(m2.back as isize);
                    } else {
                        m2 = nomatch;
                    }

                    if m2.len <= m1.len {
                        // No better match → encode m1 immediately.
                        optr = op;
                        if encode_sequence(
                            &mut ip,
                            &mut op,
                            &mut anchor,
                            m1.len,
                            m1.off,
                            limit,
                            oend,
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
                        // m1 is too short to be worth emitting on its own;
                        // promote m2 to m1 and search for a new m2.
                        ip = start2;
                        m1 = m2;
                        search_state = SearchState::S2;
                        continue 'search_loop;
                    }
                }

                // Default back to S2 for the next iteration unless S3 is selected below.
                search_state = SearchState::S2;

                // ── Step S3: optionally shorten m1 so that m2 fits before m3.
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
                        start2, // i_low_limit
                        matchlimit,
                        m2.len,
                        max_nb_attempts,
                        pattern_analysis,
                        false, // chain_swap = 0
                        dict,
                        false, // favorCompressionRatio
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
                    if encode_sequence(&mut ip, &mut op, &mut anchor, m1.len, m1.off, limit, oend)
                        .is_err()
                    {
                        overflow_m1 = m1;
                        overflow_occurred = true;
                        break 'compress_loop;
                    }
                    ip = start2;
                    optr = op;
                    if encode_sequence(&mut ip, &mut op, &mut anchor, m2.len, m2.off, limit, oend)
                        .is_err()
                    {
                        overflow_m1 = m2; // m1 was already advanced to m2 position
                        overflow_occurred = true;
                        break 'compress_loop;
                    }
                    continue 'compress_loop;
                }

                if start3 < ip.add(m1.len as usize + 3) {
                    if start3 >= ip.add(m1.len as usize) {
                        // Can write Seq1 immediately: Seq2 removed, Seq3 becomes Seq1.
                        if start2 < ip.add(m1.len as usize) {
                            let correction = (ip.add(m1.len as usize)).offset_from(start2) as i32;
                            start2 = start2.add(correction as usize);
                            m2.len -= correction;
                            if m2.len < MINMATCH as i32 {
                                start2 = start3;
                                m2 = m3;
                            }
                        }
                        optr = op;
                        if encode_sequence(
                            &mut ip,
                            &mut op,
                            &mut anchor,
                            m1.len,
                            m1.off,
                            limit,
                            oend,
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
                        continue 'search_loop;
                    }
                    // m2 does not fit before m3; skip m2 and retry with m3 as the new m2.
                    start2 = start3;
                    m2 = m3;
                    search_state = SearchState::S3;
                    continue 'search_loop;
                }

                // OK: we have 3 ascending matches; write m1.
                if start2 < ip.add(m1.len as usize) {
                    if (start2.offset_from(ip) as i32) < OPTIMAL_ML {
                        if m1.len > OPTIMAL_ML {
                            m1.len = OPTIMAL_ML;
                        }
                        let ml_limit = (start2.offset_from(ip) as i32) + m2.len - MINMATCH as i32;
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
                if encode_sequence(&mut ip, &mut op, &mut anchor, m1.len, m1.off, limit, oend)
                    .is_err()
                {
                    overflow_m1 = m1;
                    overflow_occurred = true;
                    break 'compress_loop;
                }

                // Slide the window: emit m1, then promote m2→m1 and m3→m2,
                // and search for a new m3 on the next iteration.
                ip = start2;
                m1 = m2;
                start2 = start3;
                m2 = m3;
                search_state = SearchState::S3;
                continue 'search_loop;
            } // 'search_loop
        } // 'compress_loop

        // ── Output overflow: recover partial match when filling output ─────
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
                    let bytes_left_for_ml = max_lit_pos.offset_from(op.add(ll_total_cost)) as usize;
                    let max_ml_size = MINMATCH + (ML_MASK as usize - 1) + bytes_left_for_ml * 255;
                    debug_assert!(m1.len >= 0);
                    if m1.len as usize > max_ml_size {
                        m1.len = max_ml_size as i32;
                    }
                    // (oend + LASTLITERALS) - (op + ll_total_cost + 2) - 1 + m1.len >= MFLIMIT
                    let room =
                        oend.add(LASTLITERALS)
                            .offset_from(op.add(ll_total_cost + 2)) as i32
                            - 1
                            + m1.len;
                    if room >= MFLIMIT as i32 {
                        // Best-effort encode; ignore error (notLimited mode).
                        let _ = encode_sequence(
                            &mut ip,
                            &mut op,
                            &mut anchor,
                            m1.len,
                            m1.off,
                            LimitedOutputDirective::NotLimited,
                            oend,
                        );
                    }
                }
                // Fall through to write the final literal run.
            } else {
                // LimitedOutput mode: output is full; report failure.
                return 0;
            }
        }
    } // end if (input_size >= LZ4_MIN_LENGTH)

    // ── Final literal run ─────────────────────────────────────────────────────
    {
        let mut last_run_size = iend.offset_from(anchor) as usize;
        let ll_add = (last_run_size + 255 - RUN_MASK as usize) / 255;
        let total_size = 1 + ll_add + last_run_size;

        if limit == LimitedOutputDirective::FillOutput {
            oend = oend.add(LASTLITERALS); // restore the full output boundary before writing the last run
        }

        if limit != LimitedOutputDirective::NotLimited && op.add(total_size) > oend {
            if limit == LimitedOutputDirective::LimitedOutput {
                return 0;
            }
            // FillOutput: truncate the final literal run to exactly fill remaining space.
            let remaining = oend.offset_from(op);
            if remaining < 2 {
                // Not enough room even for the token byte + 1 literal
                return op.offset_from(dest) as i32;
            }
            last_run_size = remaining as usize - 1; // 1 for token
            let ll_add2 = (last_run_size + 256 - RUN_MASK as usize) / 256;
            last_run_size -= ll_add2;
        }

        ip = anchor.add(last_run_size); // may end before `iend` in FillOutput mode

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
// Optimal-parser price helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Cost in bytes of encoding `litlen` literals in the LZ4 token+length
/// format (`LZ4HC_literalsPrice` in `lz4hc.c`).
///
/// The token byte carries the low four bits of the literal count; additional
/// 0xFF bytes are appended for every 255 literals beyond `RUN_MASK`.
#[inline(always)]
pub fn literals_price(litlen: i32) -> i32 {
    debug_assert!(litlen >= 0);
    let mut price = litlen;
    if litlen >= RUN_MASK as i32 {
        price += 1 + (litlen - RUN_MASK as i32) / 255;
    }
    price
}

/// Total cost in bytes of one LZ4 sequence: `litlen` literals followed by a
/// back-reference of length `mlen` (must be ≥ `MINMATCH`).
/// (`LZ4HC_sequencePrice` in `lz4hc.c`.)
///
/// Cost = 1 (token) + 2 (offset) + [`literals_price`]`(litlen)` + match-length
/// extension bytes.
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
// find_longer_match
// ─────────────────────────────────────────────────────────────────────────────

/// Insert all positions up to `ip` (exclusive) then search for the best match
/// of length strictly greater than `min_len`  (`LZ4HC_FindLongerMatch` in
/// `lz4hc.c`).
///
/// Unlike [`insert_and_get_wider_match`], backward extension is disabled
/// (`i_low_limit = ip`), and both pattern-analysis and chain-swap optimisations
/// are always enabled.
///
/// Returns a zero-length [`Match`] if no match better than `min_len` was found.
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
    let match0 = Match {
        len: 0,
        off: 0,
        back: 0,
    };
    let mut md = insert_and_get_wider_match(
        ctx,
        ip,
        ip, // i_low_limit = ip → no backward extension
        i_high_limit,
        min_len,
        nb_searches,
        true, // patternAnalysis = 1
        true, // chainSwap = 1
        dict,
        favor_dec_speed == HcFavor::DecompressionSpeed,
    );
    debug_assert!(md.back == 0);
    if md.len <= min_len {
        return match0;
    }
    if favor_dec_speed == HcFavor::DecompressionSpeed {
        // Decompression cost is proportional to match length; cap matches in
        // the 19–36 byte range at 18 bytes to improve decompression throughput
        // at the expense of a small compression-ratio loss.
        if md.len > 18 && md.len <= 36 {
            md.len = 18;
        }
    }
    md
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4HcOptimal
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
// compress_optimal
// ─────────────────────────────────────────────────────────────────────────────

/// Dynamic-programming optimal parser for HC compression levels 9–12
/// (`LZ4HC_compress_optimal` in `lz4hc.c`).
///
/// For each starting position the parser builds a cost table over an
/// [`LZ4_OPT_NUM`]-wide window, then back-tracks to find and emit the
/// minimum-cost sequence of literals and back-references.
///
/// The DP table (`opt`) is heap-allocated to avoid placing ~64 KB on the
/// stack; all other state is stack-local.
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

    // Heap-allocate the DP table to keep stack usage bounded.
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

    // ovml / ovoff: match length and offset captured when output overflows.
    let mut ovml: i32 = MINMATCH as i32;
    let mut ovoff: i32 = 0;
    let mut overflow_occurred = false;

    *src_size_ptr = 0;

    if limit == LimitedOutputDirective::FillOutput {
        // Reserve LASTLITERALS bytes at the end of the output buffer so that
        // encode_sequence never writes into the region that must remain
        // available for the mandatory final literal run.
        oend = oend.sub(LASTLITERALS);
    }

    if sufficient_len >= LZ4_OPT_NUM {
        sufficient_len = LZ4_OPT_NUM - 1;
    }

    // ── Main Loop ─────────────────────────────────────────────────────────────
    'compress_loop: while ip <= mflimit {
        let llen = ip.offset_from(anchor) as i32;
        let mut last_match_pos: usize = 0;

        let first_match = find_longer_match(
            ctx,
            ip,
            matchlimit,
            MINMATCH as i32 - 1,
            nb_searches,
            dict,
            favor_dec_speed,
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
                &mut ip,
                &mut op,
                &mut anchor,
                first_ml,
                first_match.off,
                limit,
                oend,
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

        // ── DP inner loop: refine prices for all candidate positions ──────
        //
        // Iterates forward through the window, inserting each position into
        // the hash chain and updating cost entries.  If a sufficiently good
        // match is found the loop terminates early and the match is encoded
        // immediately (captured via `dp_early_exit_cur`).
        let mut dp_best_mlen: i32 = 0;
        let mut dp_best_off: i32 = 0;
        let mut dp_early_exit_cur: Option<usize> = None; // Some(cur) when the loop exits early

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
                        ctx,
                        cur_ptr,
                        matchlimit,
                        MINMATCH as i32 - 1,
                        nb_searches,
                        dict,
                        favor_dec_speed,
                    )
                } else {
                    // Only test matches of minimum length (slightly faster).
                    find_longer_match(
                        ctx,
                        cur_ptr,
                        matchlimit,
                        (last_match_pos - cur) as i32,
                        nb_searches,
                        dict,
                        favor_dec_speed,
                    )
                };

                if new_match.len == 0 {
                    cur += 1;
                    continue;
                }

                if (new_match.len as usize > sufficient_len)
                    || (new_match.len as usize + cur >= LZ4_OPT_NUM)
                {
                    // Match is either past the sufficient-length threshold or
                    // would overflow the DP table; encode it immediately and
                    // skip the remaining DP iterations.
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
                        let price = opt[cur].price - literals_price(base_litlen)
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

        // Choose the best match to encode: either the early-exit match or the
        // least-cost entry in the DP table at `last_match_pos`.
        debug_assert!(last_match_pos < LZ4_OPT_NUM + TRAILING_LITERALS);
        let (best_mlen, best_off, mut candidate_pos) = match dp_early_exit_cur {
            Some(cur) => (dp_best_mlen, dp_best_off, cur),
            None => {
                let bm = opt[last_match_pos].mlen;
                let bo = opt[last_match_pos].off;
                let c = (last_match_pos as i32 - bm) as usize;
                (bm, bo, c)
            }
        };

        // ── Reverse traversal: reconstruct the optimal sequence of matches ─
        // Walk backwards through the DP table, linking each chosen match to
        // its predecessor, producing a forward-ordered chain ready for emission.
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
                    break; // reached the first match in the chain
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
                    // Literal byte: advance ip without emitting a sequence;
                    // literals accumulate between anchor and ip.
                    ip = ip.add(1);
                    r_pos += 1;
                    continue;
                }
                r_pos += ml as usize;
                debug_assert!(ml >= MINMATCH as i32);
                debug_assert!(offset >= 1 && offset <= LZ4_DISTANCE_MAX as i32);
                op_saved = op;
                if encode_sequence(&mut ip, &mut op, &mut anchor, ml, offset, limit, oend).is_err()
                {
                    ovml = ml;
                    ovoff = offset;
                    overflow_occurred = true;
                    break 'compress_loop;
                }
            }
        }
    } // 'compress_loop

    // ── Output overflow: recover partial match when filling output ─────────────
    if overflow_occurred {
        if limit == LimitedOutputDirective::FillOutput {
            let ll = ip.offset_from(anchor) as usize;
            let ll_addbytes = (ll + 240) / 255;
            let ll_total_cost = 1 + ll_addbytes + ll;
            let max_lit_pos: *mut u8 = oend.sub(3); // 2 for offset, 1 for token

            op = op_saved; // restore correct out pointer
            if op.add(ll_total_cost) <= max_lit_pos {
                let bytes_left_for_ml = max_lit_pos.offset_from(op.add(ll_total_cost)) as usize;
                let max_ml_size = MINMATCH + (ML_MASK as usize - 1) + bytes_left_for_ml * 255;
                debug_assert!(ovml >= 0);
                if ovml as usize > max_ml_size {
                    ovml = max_ml_size as i32;
                }
                // (oend + LASTLITERALS) - (op + ll_total_cost + 2) - 1 + ovml >= MFLIMIT
                let room = oend
                    .add(LASTLITERALS)
                    .offset_from(op.add(ll_total_cost + 2)) as i32
                    - 1
                    + ovml;
                if room >= MFLIMIT as i32 {
                    // Best-effort encode; ignore result (notLimited mode).
                    let _ = encode_sequence(
                        &mut ip,
                        &mut op,
                        &mut anchor,
                        ovml,
                        ovoff,
                        LimitedOutputDirective::NotLimited,
                        oend,
                    );
                }
            }
            // Fall through to write the final literal run.
        } else {
            // LimitedOutput mode: cannot fit the output; report failure.
            retval = 0;
            return retval;
        }
    }

    // ── Final literal run ─────────────────────────────────────────────────────
    {
        let mut last_run_size = iend.offset_from(anchor) as usize;
        let ll_add = (last_run_size + 255 - RUN_MASK as usize) / 255;
        let total_size = 1 + ll_add + last_run_size;

        if limit == LimitedOutputDirective::FillOutput {
            oend = oend.add(LASTLITERALS); // restore the full output boundary before writing the last run
        }

        if limit != LimitedOutputDirective::NotLimited && op.add(total_size) > oend {
            if limit == LimitedOutputDirective::LimitedOutput {
                retval = 0;
                return retval;
            }
            // FillOutput: truncate the final literal run to exactly fill remaining space.
            last_run_size = oend.offset_from(op) as usize - 1; // 1 for token
            let ll_add2 = (last_run_size + 256 - RUN_MASK as usize) / 256;
            last_run_size -= ll_add2;
        }

        ip = anchor.add(last_run_size); // may end before `iend` in FillOutput mode

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

    *src_size_ptr = ip.offset_from(source) as i32;
    retval = op.offset_from(dst) as i32;
    retval
}
