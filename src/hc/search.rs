//! Match-finding core for the LZ4-HC compressor.
//!
//! Performs three interleaved operations at each input position:
//!
//! 1. **Insertion** ([`insert`]) — update the hash and chain tables for every
//!    unprocessed position in `[next_to_update, ip)`.
//! 2. **Pattern utilities** ([`count_pattern`], [`reverse_count_pattern`],
//!    [`rotate_pattern`]) — fast run-length counting for the repeating-pattern
//!    optimisation in the HC chain search.
//! 3. **Match search** ([`insert_and_get_wider_match`],
//!    [`insert_and_find_best_match`]) — walk the hash chain for the longest
//!    match, with optional backward extension, chain-swap, pattern-analysis,
//!    and dictionary-context modes.
//!
//! Corresponds to `lz4hc.c` (v1.10.0, lines 776–1120):
//!   - [`insert`]                     ← `LZ4HC_Insert`
//!   - [`rotate_pattern`]             ← `LZ4HC_rotatePattern`
//!   - [`count_pattern`]              ← `LZ4HC_countPattern`
//!   - [`reverse_count_pattern`]      ← `LZ4HC_reverseCountPattern`
//!   - [`protect_dict_end`]           ← `LZ4HC_protectDictEnd`
//!   - [`RepeatState`]                ← `repeat_state_e`
//!   - [`HcFavor`]                    ← `HCfavor_e`
//!   - [`insert_and_get_wider_match`] ← `LZ4HC_InsertAndGetWiderMatch`
//!   - [`insert_and_find_best_match`] ← `LZ4HC_InsertAndFindBestMatch`

use super::lz4mid::Match;
use super::types::{count_back, hash_ptr, DictCtxDirective, HcCCtxInternal, LZ4HC_MAXD_MASK};
use crate::block::types::{self as bt, LZ4_DISTANCE_MAX, MINMATCH};

// ─────────────────────────────────────────────────────────────────────────────
// Chain delta accessor (DELTANEXTU16)
// ─────────────────────────────────────────────────────────────────────────────

/// Read the chain delta at `idx` from the chain table.
///
/// Equivalent to C macro `DELTANEXTU16(chainTable, idx)` which expands to
/// `chainTable[idx & LZ4HC_MAXD_MASK]`.
#[inline(always)]
fn delta_next(chain_table: &[u16], idx: u32) -> u32 {
    chain_table[idx as usize & LZ4HC_MAXD_MASK] as u32
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_Insert  (lz4hc.c:781–802)
// ─────────────────────────────────────────────────────────────────────────────

/// Fill hash table and chain table entries for all positions in
/// `[hc4.next_to_update, ip)`.
///
/// Equivalent to `LZ4HC_Insert`.
///
/// # Safety
/// `ip` must be within the prefix window described by `hc4`, i.e.
/// `ip >= hc4.prefix_start` and the range `[prefix_start, ip)` must be
/// readable.
#[inline]
pub unsafe fn insert(hc4: &mut HcCCtxInternal, ip: *const u8) {
    let prefix_ptr = hc4.prefix_start;
    let prefix_idx = hc4.dict_limit;
    // target = byte offset of ip from prefixStart + prefixIdx
    let target = (ip.offset_from(prefix_ptr) as u32).wrapping_add(prefix_idx);
    let mut idx = hc4.next_to_update;

    debug_assert!(ip >= prefix_ptr);
    debug_assert!(target >= prefix_idx);

    while idx < target {
        let h = hash_ptr(prefix_ptr.add((idx - prefix_idx) as usize)) as usize;
        let mut delta = idx.wrapping_sub(hc4.hash_table[h]);
        if delta > LZ4_DISTANCE_MAX {
            delta = LZ4_DISTANCE_MAX;
        }
        hc4.chain_table[idx as usize & LZ4HC_MAXD_MASK] = delta as u16;
        hc4.hash_table[h] = idx;
        idx += 1;
    }

    hc4.next_to_update = target;
}

// ─────────────────────────────────────────────────────────────────────────────
// Pattern utilities  (lz4hc.c:811–878)
// ─────────────────────────────────────────────────────────────────────────────

/// Rotate a 32-bit pattern left by `(rotate mod 4) * 8` bits.
///
/// Equivalent to `LZ4HC_rotatePattern`.
#[inline(always)]
pub fn rotate_pattern(rotate: usize, pattern: u32) -> u32 {
    let bits_to_rotate = (rotate & (core::mem::size_of::<u32>() - 1)) << 3;
    if bits_to_rotate == 0 {
        return pattern;
    }
    pattern.rotate_left(bits_to_rotate as u32)
}

/// Count how many bytes starting at `ip` match `pattern32` (a 1-, 2-, or
/// 4-byte repeating pattern), stopping before `i_end`.
///
/// Equivalent to `LZ4HC_countPattern`.
///
/// # Safety
/// `ip` must be valid for reads up to `i_end`.
#[inline]
pub unsafe fn count_pattern(mut ip: *const u8, i_end: *const u8, pattern32: u32) -> usize {
    let i_start = ip;

    // Build a native-word-sized pattern for bulk comparison.
    // On 64-bit: duplicate pattern32 into both halves of a u64.
    // On 32-bit: use pattern32 directly.
    #[cfg(target_pointer_width = "64")]
    let pattern: usize = (pattern32 as usize) | ((pattern32 as usize) << 32);
    #[cfg(not(target_pointer_width = "64"))]
    let pattern: usize = pattern32 as usize;

    let word_size = core::mem::size_of::<usize>();

    // Fast word-at-a-time loop
    while ip.add(word_size) <= i_end {
        let diff = bt::read_arch(ip) ^ pattern;
        if diff != 0 {
            ip = ip.add(bt::nb_common_bytes(diff) as usize);
            return ip.offset_from(i_start) as usize;
        }
        ip = ip.add(word_size);
    }

    // Tail: byte-by-byte, endian-aware
    #[cfg(target_endian = "little")]
    {
        let mut pattern_byte = pattern32 as u64;
        while ip < i_end && *ip == (pattern_byte as u8) {
            ip = ip.add(1);
            pattern_byte >>= 8;
            // Wrap the 4-byte cycle: pattern_byte drains 8 bits per step and
            // hits zero after 4 steps; reload for 1- and 2-byte patterns.
            if pattern_byte == 0 {
                pattern_byte = pattern32 as u64;
            }
        }
    }
    #[cfg(not(target_endian = "little"))]
    {
        let mut bit_offset = 24u32; // start from the most-significant byte
        while ip < i_end {
            let byte = (pattern32 >> bit_offset) as u8;
            if *ip != byte {
                break;
            }
            ip = ip.add(1);
            if bit_offset == 0 {
                bit_offset = 24;
            } else {
                bit_offset -= 8;
            }
        }
    }

    ip.offset_from(i_start) as usize
}

/// Count how many bytes *before* `ip` match `pattern` going backward,
/// stopping at or before `i_low`.
///
/// Returns the number of common bytes (positive).
///
/// Equivalent to `LZ4HC_reverseCountPattern`.
///
/// # Safety
/// `ip` must be within a valid readable range down to `i_low`.
#[inline]
pub unsafe fn reverse_count_pattern(mut ip: *const u8, i_low: *const u8, pattern: u32) -> usize {
    let i_start = ip;

    // 4-byte step backward
    while ip >= i_low.add(4) {
        if bt::read32(ip.sub(4)) != pattern {
            break;
        }
        ip = ip.sub(4);
    }

    // Byte-by-byte tail: `to_ne_bytes()` returns bytes in native-endian
    // memory order, so index 3 is always the byte that appeared at the
    // highest address of the 4-byte group — the first one encountered when
    // stepping backward through the stream.
    let pattern_bytes = pattern.to_ne_bytes();
    let mut byte_idx: isize = 3; // start from highest address byte of pattern
    while ip > i_low {
        if *ip.sub(1) != pattern_bytes[byte_idx as usize] {
            break;
        }
        ip = ip.sub(1);
        byte_idx -= 1;
        if byte_idx < 0 {
            byte_idx = 3;
        }
    }

    i_start.offset_from(ip) as usize
}

/// Return `true` if `match_index` is not in the last 3 bytes of the
/// dictionary (i.e. reading a 4-byte MINMATCH would not overflow).
///
/// Equivalent to `LZ4HC_protectDictEnd`.
#[inline(always)]
pub fn protect_dict_end(dict_limit: u32, match_index: u32) -> bool {
    (dict_limit.wrapping_sub(1).wrapping_sub(match_index)) >= 3
}

// ─────────────────────────────────────────────────────────────────────────────
// Enums  (lz4hc.c:880–881)
// ─────────────────────────────────────────────────────────────────────────────

/// Repeat-pattern detection state for the pattern-analysis optimisation.
///
/// Mirrors `repeat_state_e { rep_untested, rep_not, rep_confirmed }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepeatState {
    Untested,
    Not,
    Confirmed,
}

/// Whether to favour compression ratio or decompression speed when choosing
/// among equally-long matches.
///
/// Mirrors `HCfavor_e { favorCompressionRatio=0, favorDecompressionSpeed }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HcFavor {
    CompressionRatio = 0,
    DecompressionSpeed = 1,
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_InsertAndGetWiderMatch  (lz4hc.c:884–1104)
// ─────────────────────────────────────────────────────────────────────────────

/// Insert all positions up to `ip` (exclusive) into the hash/chain tables,
/// then search for the best match of at least `longest` bytes.
///
/// The search may extend the match start up to `i_low_limit` bytes backward
/// (`sBack` in the returned [`Match`]).
///
/// Equivalent to `LZ4HC_InsertAndGetWiderMatch`.
///
/// # Parameters
///
/// - `hc4`             – mutable HC context (hash + chain tables, window info)
/// - `ip`              – current input position
/// - `i_low_limit`     – lower bound for backward match extension
/// - `i_high_limit`    – upper bound for forward match counting
/// - `longest`         – best match length seen so far (minimum to improve)
/// - `max_nb_attempts` – maximum chain walk iterations
/// - `pattern_analysis`– enable repeat-pattern optimisation
/// - `chain_swap`      – enable chain-swap optimisation (forward search only)
/// - `dict`            – dictionary mode selector
/// - `favor_dec_speed` – if `true`, skip short-offset matches for speed
///
/// # Safety
/// All pointer arithmetic is over caller-guaranteed valid memory regions.
#[allow(clippy::too_many_arguments)]
pub unsafe fn insert_and_get_wider_match(
    hc4: &mut HcCCtxInternal,
    ip: *const u8,
    i_low_limit: *const u8,
    i_high_limit: *const u8,
    mut longest: i32,
    max_nb_attempts: i32,
    pattern_analysis: bool,
    chain_swap: bool,
    dict: DictCtxDirective,
    favor_dec_speed: bool,
) -> Match {
    let prefix_ptr = hc4.prefix_start;
    let prefix_idx = hc4.dict_limit;
    let ip_index = (ip.offset_from(prefix_ptr) as u32).wrapping_add(prefix_idx);
    let within_start_distance = hc4.low_limit.wrapping_add(LZ4_DISTANCE_MAX + 1) > ip_index;
    let lowest_match_index = if within_start_distance {
        hc4.low_limit
    } else {
        ip_index - LZ4_DISTANCE_MAX
    };
    let dict_start = hc4.dict_start;
    let dict_idx = hc4.low_limit;
    let dict_end = dict_start.add((prefix_idx - dict_idx) as usize);
    let look_back_length = ip.offset_from(i_low_limit) as i32;
    let mut nb_attempts = max_nb_attempts;
    let mut match_chain_pos: u32 = 0;
    let pattern = bt::read32(ip);
    let mut repeat = RepeatState::Untested;
    let mut src_pattern_length: usize = 0;
    let mut offset: i32 = 0;
    let mut s_back: i32 = 0;

    // Insert all prior positions up to ip (excluded)
    insert(hc4, ip);
    let mut match_index = hc4.hash_table[hash_ptr(ip) as usize];

    'chain_loop: while match_index >= lowest_match_index && nb_attempts > 0 {
        let mut match_length: i32 = 0;
        nb_attempts -= 1;

        debug_assert!(match_index < ip_index);

        if favor_dec_speed && (ip_index - match_index < 8) {
            // Intentionally skip matches with offset < 8 for decompression speed.
        } else if match_index >= prefix_idx {
            // ── Within current prefix ─────────────────────────────────────
            let match_ptr = prefix_ptr.add((match_index - prefix_idx) as usize);
            debug_assert!(match_ptr < ip);
            debug_assert!(longest >= 1);
            // Quick 2-byte suffix check before full comparison
            if bt::read16(i_low_limit.add((longest - 1) as usize))
                == bt::read16(
                    match_ptr
                        .sub(look_back_length as usize)
                        .add((longest - 1) as usize),
                )
                && bt::read32(match_ptr) == pattern
            {
                let back = if look_back_length != 0 {
                    count_back(ip, match_ptr, i_low_limit, prefix_ptr)
                } else {
                    0
                };
                match_length = MINMATCH as i32
                    + bt::count(ip.add(MINMATCH), match_ptr.add(MINMATCH), i_high_limit) as i32;
                match_length -= back;
                if match_length > longest {
                    longest = match_length;
                    offset = (ip_index - match_index) as i32;
                    s_back = back;
                }
            }
        } else {
            // ── Within external dict (lowestMatchIndex <= matchIndex < dictLimit) ──
            let match_ptr = dict_start.add((match_index - dict_idx) as usize);
            debug_assert!(match_index >= dict_idx);
            if match_index <= prefix_idx.wrapping_sub(4) && bt::read32(match_ptr) == pattern {
                let mut v_limit = ip.add((prefix_idx - match_index) as usize);
                if v_limit > i_high_limit {
                    v_limit = i_high_limit;
                }
                let mut mlt = bt::count(ip.add(MINMATCH), match_ptr.add(MINMATCH), v_limit) as i32
                    + MINMATCH as i32;
                if ip.add(mlt as usize) == v_limit && v_limit < i_high_limit {
                    mlt += bt::count(ip.add(mlt as usize), prefix_ptr, i_high_limit) as i32;
                }
                let back = if look_back_length != 0 {
                    count_back(ip, match_ptr, i_low_limit, dict_start)
                } else {
                    0
                };
                mlt -= back;
                if mlt > longest {
                    longest = mlt;
                    offset = (ip_index - match_index) as i32;
                    s_back = back;
                }
            }
        }

        // ── Chain-swap optimisation ───────────────────────────────────────
        if chain_swap && match_length == longest {
            debug_assert!(look_back_length == 0); // forward search only
            if match_index.wrapping_add(longest as u32) <= ip_index {
                const K_TRIGGER: i32 = 4;
                let mut distance_to_next_match: u32 = 1;
                let end = longest - MINMATCH as i32 + 1;
                let mut step: i32;
                let mut accel: i32 = 1 << K_TRIGGER;
                let mut pos: i32 = 0;
                while pos < end {
                    let candidate_dist = delta_next(&hc4.chain_table, match_index + pos as u32);
                    step = accel >> K_TRIGGER;
                    accel += 1;
                    if candidate_dist > distance_to_next_match {
                        distance_to_next_match = candidate_dist;
                        match_chain_pos = pos as u32;
                        accel = 1 << K_TRIGGER;
                    }
                    pos += step;
                }
                if distance_to_next_match > 1 {
                    if distance_to_next_match > match_index {
                        break 'chain_loop; // avoid overflow
                    }
                    match_index -= distance_to_next_match;
                    continue 'chain_loop;
                }
            }
        }

        // ── Pattern-analysis optimisation ────────────────────────────────
        {
            let dist_next_match = delta_next(&hc4.chain_table, match_index);
            if pattern_analysis && dist_next_match == 1 && match_chain_pos == 0 {
                let match_candidate_idx = match_index.wrapping_sub(1);

                // Detect if we have a repeating pattern
                if repeat == RepeatState::Untested {
                    if ((pattern & 0xFFFF) == (pattern >> 16))
                        && ((pattern & 0xFF) == (pattern >> 24))
                    {
                        repeat = RepeatState::Confirmed;
                        src_pattern_length = count_pattern(ip.add(4), i_high_limit, pattern) + 4;
                    } else {
                        repeat = RepeatState::Not;
                    }
                }

                if repeat == RepeatState::Confirmed
                    && match_candidate_idx >= lowest_match_index
                    && protect_dict_end(prefix_idx, match_candidate_idx)
                {
                    let ext_dict = match_candidate_idx < prefix_idx;
                    let match_ptr = if ext_dict {
                        dict_start.add((match_candidate_idx - dict_idx) as usize)
                    } else {
                        prefix_ptr.add((match_candidate_idx - prefix_idx) as usize)
                    };

                    if bt::read32(match_ptr) == pattern {
                        // Good candidate — compute forward and backward pattern extents
                        let i_limit = if ext_dict { dict_end } else { i_high_limit };
                        let mut forward_pattern_length =
                            count_pattern(match_ptr.add(4), i_limit, pattern) + 4;

                        if ext_dict && match_ptr.add(forward_pattern_length) == i_limit {
                            let rotated = rotate_pattern(forward_pattern_length, pattern);
                            forward_pattern_length +=
                                count_pattern(prefix_ptr, i_high_limit, rotated);
                        }

                        {
                            let lowest_match_ptr = if ext_dict { dict_start } else { prefix_ptr };
                            let mut back_length =
                                reverse_count_pattern(match_ptr, lowest_match_ptr, pattern);

                            if !ext_dict
                                && match_ptr.sub(back_length) == prefix_ptr
                                && dict_idx < prefix_idx
                            {
                                let rotated =
                                    rotate_pattern((0usize).wrapping_sub(back_length), pattern);
                                back_length += reverse_count_pattern(dict_end, dict_start, rotated);
                            }

                            // Limit backLength so we don't go before lowestMatchIndex
                            back_length = (match_candidate_idx
                                - match_candidate_idx
                                    .wrapping_sub(back_length as u32)
                                    .max(lowest_match_index))
                                as usize;
                            debug_assert!(
                                match_candidate_idx.wrapping_sub(back_length as u32)
                                    >= lowest_match_index
                            );

                            let current_segment_length = back_length + forward_pattern_length;

                            if current_segment_length >= src_pattern_length
                                && forward_pattern_length <= src_pattern_length
                            {
                                // Best position: end of pattern, full srcPatternLength
                                let new_match_index = match_candidate_idx
                                    .wrapping_add(forward_pattern_length as u32)
                                    .wrapping_sub(src_pattern_length as u32);
                                if protect_dict_end(prefix_idx, new_match_index) {
                                    match_index = new_match_index;
                                } else {
                                    // Can only happen if we started in the prefix
                                    debug_assert!(
                                        new_match_index >= prefix_idx.wrapping_sub(3)
                                            && new_match_index < prefix_idx
                                            && !ext_dict
                                    );
                                    match_index = prefix_idx;
                                }
                            } else {
                                // Farthest position in current segment
                                let new_match_index =
                                    match_candidate_idx.wrapping_sub(back_length as u32);
                                if !protect_dict_end(prefix_idx, new_match_index) {
                                    debug_assert!(
                                        new_match_index >= prefix_idx.wrapping_sub(3)
                                            && new_match_index < prefix_idx
                                            && !ext_dict
                                    );
                                    match_index = prefix_idx;
                                } else {
                                    match_index = new_match_index;
                                    if look_back_length == 0 {
                                        // No back possible — try direct length improvement
                                        let max_ml = current_segment_length.min(src_pattern_length);
                                        if (longest as usize) < max_ml {
                                            debug_assert!(
                                                prefix_ptr
                                                    .sub(prefix_idx as usize)
                                                    .add(match_index as usize)
                                                    != ip
                                            );
                                            let dist = ip.offset_from(prefix_ptr) as u32
                                                + prefix_idx
                                                - match_index;
                                            if dist > LZ4_DISTANCE_MAX {
                                                break 'chain_loop; // distance exceeded
                                            }
                                            longest = max_ml as i32;
                                            offset = (ip_index - match_index) as i32;
                                            debug_assert!(s_back == 0);
                                        }
                                        // Advance along the chain
                                        let dist_to_next =
                                            delta_next(&hc4.chain_table, match_index);
                                        if dist_to_next > match_index {
                                            break 'chain_loop; // avoid overflow
                                        }
                                        match_index -= dist_to_next;
                                    }
                                }
                            }
                        }

                        continue 'chain_loop; // goto _FindBestMatch equivalent
                    }
                }
            }
        } // end pattern-analysis block

        // ── Follow current chain ──────────────────────────────────────────
        match_index =
            match_index.wrapping_sub(delta_next(&hc4.chain_table, match_index + match_chain_pos));
    } // 'chain_loop

    // ── Dict-ctx search  (usingDictCtxHc mode) ───────────────────────────
    if dict == DictCtxDirective::UsingDictCtxHc && nb_attempts > 0 && within_start_distance {
        let dict_ctx = &*hc4.dict_ctx;
        let dict_end_offset = (dict_ctx.end.offset_from(dict_ctx.prefix_start) as usize)
            .wrapping_add(dict_ctx.dict_limit as usize);
        debug_assert!(dict_end_offset <= 1 << 30); // 1 GB
        let mut dict_match_index = dict_ctx.hash_table[hash_ptr(ip) as usize];
        let mut match_index_dc = dict_match_index
            .wrapping_add(lowest_match_index)
            .wrapping_sub(dict_end_offset as u32);

        while ip_index.wrapping_sub(match_index_dc) <= LZ4_DISTANCE_MAX && nb_attempts > 0 {
            nb_attempts -= 1;
            let match_ptr = dict_ctx
                .prefix_start
                .sub(dict_ctx.dict_limit as usize)
                .add(dict_match_index as usize);

            if bt::read32(match_ptr) == pattern {
                let mut v_limit = ip.add(dict_end_offset - dict_match_index as usize);
                if v_limit > i_high_limit {
                    v_limit = i_high_limit;
                }
                let mut mlt = bt::count(ip.add(MINMATCH), match_ptr.add(MINMATCH), v_limit) as i32
                    + MINMATCH as i32;
                let back = if look_back_length != 0 {
                    count_back(ip, match_ptr, i_low_limit, dict_ctx.prefix_start)
                } else {
                    0
                };
                mlt -= back;
                if mlt > longest {
                    longest = mlt;
                    offset = (ip_index - match_index_dc) as i32;
                    s_back = back;
                }
            }

            let next_offset = delta_next(&dict_ctx.chain_table, dict_match_index);
            dict_match_index = dict_match_index.wrapping_sub(next_offset);
            match_index_dc = match_index_dc.wrapping_sub(next_offset);
        }
    }

    debug_assert!(longest >= 0);
    Match {
        len: longest,
        off: offset,
        back: s_back,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4HC_InsertAndFindBestMatch  (lz4hc.c:1106–1118)
// ─────────────────────────────────────────────────────────────────────────────

/// Convenience wrapper around [`insert_and_get_wider_match`] that restricts
/// the backward-extension lower bound to `ip` itself (no backward extension).
///
/// Equivalent to `LZ4HC_InsertAndFindBestMatch`.
///
/// # Safety
/// Same as [`insert_and_get_wider_match`].
#[inline]
pub unsafe fn insert_and_find_best_match(
    hc4: &mut HcCCtxInternal,
    ip: *const u8,
    i_limit: *const u8,
    max_nb_attempts: i32,
    pattern_analysis: bool,
    dict: DictCtxDirective,
) -> Match {
    // Passing ip as i_low_limit means no backward extension is allowed.
    insert_and_get_wider_match(
        hc4,
        ip,
        ip, // i_low_limit == ip → no lookback
        i_limit,
        MINMATCH as i32 - 1,
        max_nb_attempts,
        pattern_analysis,
        false, // chain_swap disabled
        dict,
        false, // favor_dec_speed = favorCompressionRatio
    )
}
