//! LZ4 block compression — one-shot and streaming APIs.
//!
//! Implements the core LZ4 block-format encoder, corresponding to the following
//! functions in the reference implementation (`lz4.c` v1.10.0):
//!
//! | Rust function                        | C equivalent                          |
//! |--------------------------------------|---------------------------------------|
//! | [`compress_generic_validated`]       | `LZ4_compress_generic_validated`      |
//! | [`compress_generic`]                 | `LZ4_compress_generic`                |
//! | [`compress_fast_ext_state`]          | `LZ4_compress_fast_extState`          |
//! | [`compress_fast`]                    | `LZ4_compress_fast`                   |
//! | [`compress_default`]                 | `LZ4_compress_default`                |
//! | [`compress_dest_size`]               | `LZ4_compress_destSize`               |
//!
//! The encoder uses a hash table to find back-references (matches) within a
//! sliding window of up to [`LZ4_DISTANCE_MAX`] bytes.  Each compressed
//! sequence consists of a literal run followed by a match (offset + length);
//! bytes that cannot be matched are emitted as a final literal run.
//!
//! Capacity-exceeded conditions are signalled as [`Err(Lz4Error::OutputTooSmall)`]
//! rather than returning 0, which makes error handling unambiguous at call sites.
//!
//! See the [LZ4 block format specification] for the authoritative description
//! of the on-disk layout.
//!
//! [LZ4 block format specification]: https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md
//! [`LZ4_DISTANCE_MAX`]: super::types::LZ4_DISTANCE_MAX

use core::ptr;

use super::types::{
    clear_hash, count, get_index_on_hash, get_position, get_position_on_hash, hash_position,
    prepare_table, put_index_on_hash, put_position, put_position_on_hash, read32, wild_copy8,
    write32, write_le16, DictDirective, DictIssueDirective, LimitedOutputDirective,
    StreamStateInternal, TableType, LASTLITERALS, LZ4_64KLIMIT, LZ4_DISTANCE_ABSOLUTE_MAX,
    LZ4_DISTANCE_MAX, LZ4_MIN_LENGTH, LZ4_SKIP_TRIGGER, MFLIMIT, MINMATCH, ML_BITS, ML_MASK,
    RUN_MASK,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum allowed input size (2 113 929 216 bytes).  Inputs larger than this
/// cannot be represented in an LZ4 block.
pub const LZ4_MAX_INPUT_SIZE: u32 = 0x7E00_0000;

/// Default acceleration factor (equals 1 — check every position).
pub const LZ4_ACCELERATION_DEFAULT: i32 = 1;

/// Maximum allowed acceleration factor.
pub const LZ4_ACCELERATION_MAX: i32 = 65_537;

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors returned by LZ4 block compression functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lz4Error {
    /// The output buffer is too small to hold the compressed data.
    OutputTooSmall,
    /// The input exceeds `LZ4_MAX_INPUT_SIZE`.
    InputTooLarge,
}

// ─────────────────────────────────────────────────────────────────────────────
// Utility
// ─────────────────────────────────────────────────────────────────────────────

/// Worst-case compressed size for a given input size.
///
/// Returns 0 if `input_size` exceeds `LZ4_MAX_INPUT_SIZE`.
/// Equivalent to `LZ4_compressBound` / `LZ4_COMPRESSBOUND`.
#[inline]
pub fn compress_bound(input_size: i32) -> i32 {
    if input_size < 0 || (input_size as u32) > LZ4_MAX_INPUT_SIZE {
        0
    } else {
        input_size + (input_size / 255) + 16
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core compression loop
// ─────────────────────────────────────────────────────────────────────────────

/// Inner core of LZ4 block compression.
///
/// Preconditions (caller must validate before calling):
/// - `source` is non-null.
/// - `input_size > 0`.
///
/// Equivalent to `LZ4_compress_generic_validated`.
///
/// # Safety
/// All pointers must be valid for their respective access sizes.  `cctx` must
/// be exclusively accessed for the duration of the call.
#[inline(always)]
#[allow(unused_assignments)] // dead-store inits mirror C variable declarations; vars are set before first read
pub unsafe fn compress_generic_validated(
    cctx: *mut StreamStateInternal,
    source: *const u8,
    dest: *mut u8,
    input_size: i32,
    // Only written when output_directive == FillOutput.
    input_consumed: *mut i32,
    max_output_size: i32,
    output_directive: LimitedOutputDirective,
    table_type: TableType,
    dict_directive: DictDirective,
    dict_issue: DictIssueDirective,
    acceleration: i32,
) -> Result<usize, Lz4Error> {
    let cctx_ref = &mut *cctx;

    let mut ip: *const u8 = source;

    let start_index: u32 = cctx_ref.current_offset;
    // `base` maps an absolute offset back to a source pointer:  source == base + startIndex
    let base: *const u8 = source.wrapping_sub(start_index as usize);

    let dict_ctx = cctx_ref.dict_ctx as *const StreamStateInternal;
    let dictionary: *const u8 =
        if dict_directive == DictDirective::UsingDictCtx && !dict_ctx.is_null() {
            (*dict_ctx).dictionary
        } else {
            cctx_ref.dictionary
        };
    let dict_size: u32 = if dict_directive == DictDirective::UsingDictCtx && !dict_ctx.is_null() {
        (*dict_ctx).dict_size
    } else {
        cctx_ref.dict_size
    };
    // `dictDelta` makes dictCtx offsets comparable with current-context offsets
    let dict_delta: u32 = if dict_directive == DictDirective::UsingDictCtx && !dict_ctx.is_null() {
        start_index.wrapping_sub((*dict_ctx).current_offset)
    } else {
        0
    };

    // True when the match may lie in an external dictionary.  In that case
    // the encoded back-reference offset cannot be derived by simple pointer
    // subtraction and must be tracked explicitly in `offset`.
    let maybe_ext_mem = matches!(
        dict_directive,
        DictDirective::UsingExtDict | DictDirective::UsingDictCtx
    );
    // `prefixIdxLimit` used for dictSmall: matches below this are rejected.
    let prefix_idx_limit: u32 = start_index.wrapping_sub(dict_size);
    let dict_end: *const u8 = if dictionary.is_null() {
        ptr::null()
    } else {
        dictionary.add(dict_size as usize)
    };

    let mut anchor: *const u8 = source;
    let iend: *const u8 = ip.add(input_size as usize);
    let mflimit_plus_one: *const u8 = iend.sub(MFLIMIT).add(1);
    let matchlimit: *const u8 = iend.sub(LASTLITERALS);

    // `dictBase` maps a dictCtx/extDict absolute offset to a byte pointer
    let dict_base: *const u8 = if dictionary.is_null() {
        ptr::null()
    } else if dict_directive == DictDirective::UsingDictCtx && !dict_ctx.is_null() {
        dictionary
            .add(dict_size as usize)
            .wrapping_sub((*dict_ctx).current_offset as usize)
    } else {
        dictionary
            .add(dict_size as usize)
            .wrapping_sub(start_index as usize)
    };

    let mut op: *mut u8 = dest;
    let olimit: *mut u8 = dest.add(max_output_size as usize);

    let mut offset: u32 = 0;
    let mut forward_h: u32 = 0; // initialised before use

    // Early-out: impossible to store anything
    if output_directive == LimitedOutputDirective::FillOutput && max_output_size < 1 {
        return Ok(0);
    }

    // `lowLimit`: the bottom of the range in which back-references are valid.
    // - WithPrefix64k: start of the 64 KB prefix window
    // - others: start of the current input
    let low_limit_init: *const u8 = if dict_directive == DictDirective::WithPrefix64k {
        source.wrapping_sub(dict_size as usize)
    } else {
        source
    };
    let mut low_limit: *const u8 = low_limit_init;

    // Update context state
    if dict_directive == DictDirective::UsingDictCtx {
        // After this block, the dict context is no longer used; the block we
        // just compress becomes the new "dictionary" for subsequent blocks.
        cctx_ref.dict_ctx = ptr::null_mut();
        cctx_ref.dict_size = input_size as u32;
    } else {
        cctx_ref.dict_size = cctx_ref.dict_size.wrapping_add(input_size as u32);
    }
    cctx_ref.current_offset = cctx_ref.current_offset.wrapping_add(input_size as u32);
    cctx_ref.table_type = table_type as u32;

    // ── Main compression body ────────────────────────────────────────────────
    // Breaking out of 'compress at any point skips to the trailing-literals
    // epilogue that follows, which encodes any remaining unmatched bytes.
    'compress: {
        // Input too small to compress — emit everything as literals.
        if input_size < LZ4_MIN_LENGTH as i32 {
            break 'compress;
        }

        // ── First byte ───────────────────────────────────────────────────────
        {
            let h = hash_position(ip, table_type);
            if table_type == TableType::ByPtr {
                put_position_on_hash(
                    ip,
                    h,
                    cctx_ref.hash_table.as_mut_ptr() as *mut *const u8,
                    table_type,
                );
            } else {
                put_index_on_hash(start_index, h, cctx_ref.hash_table.as_mut_ptr(), table_type);
            }
        }
        ip = ip.add(1);
        forward_h = hash_position(ip, table_type);

        // ── Main find-match / encode loop ────────────────────────────────────
        #[allow(unused_labels)]
        'main: loop {
            // Declare per-iteration working variables.
            // These are always overwritten before being read.
            let mut match_ptr: *const u8 = ptr::null();
            let mut token: *mut u8 = ptr::null_mut();
            let mut filled_ip: *const u8 = ptr::null();

            // ── Find a match ─────────────────────────────────────────────────
            if table_type == TableType::ByPtr {
                let mut forward_ip = ip;
                let mut step: i32 = 1;
                let mut search_match_nb: i32 = (acceleration) << (LZ4_SKIP_TRIGGER as i32);
                loop {
                    let h = forward_h;
                    ip = forward_ip;
                    forward_ip = forward_ip.add(step as usize);
                    step = search_match_nb >> LZ4_SKIP_TRIGGER as i32;
                    search_match_nb = search_match_nb.wrapping_add(1);

                    if forward_ip > mflimit_plus_one {
                        break 'compress; // not enough room for a match + trailing literals
                    }

                    match_ptr = get_position_on_hash(
                        h,
                        cctx_ref.hash_table.as_ptr() as *const *const u8,
                        table_type,
                    );
                    forward_h = hash_position(forward_ip, table_type);
                    put_position_on_hash(
                        ip,
                        h,
                        cctx_ref.hash_table.as_mut_ptr() as *mut *const u8,
                        table_type,
                    );

                    // Reject if too far or 4-byte prefix doesn't match
                    if match_ptr.add(LZ4_DISTANCE_MAX as usize) < ip
                        || read32(match_ptr) != read32(ip)
                    {
                        continue;
                    }
                    break; // match found
                }
            } else {
                // ByU32 / ByU16
                let mut forward_ip = ip;
                let mut step: i32 = 1;
                let mut search_match_nb: i32 = (acceleration) << (LZ4_SKIP_TRIGGER as i32);
                loop {
                    let h = forward_h;
                    let current: u32 = (forward_ip as usize - base as usize) as u32;
                    let mut match_index =
                        get_index_on_hash(h, cctx_ref.hash_table.as_ptr(), table_type);

                    ip = forward_ip;
                    forward_ip = forward_ip.add(step as usize);
                    step = search_match_nb >> LZ4_SKIP_TRIGGER as i32;
                    search_match_nb = search_match_nb.wrapping_add(1);

                    if forward_ip > mflimit_plus_one {
                        break 'compress; // not enough room for a match + trailing literals
                    }

                    // Resolve match pointer from match_index
                    if dict_directive == DictDirective::UsingDictCtx {
                        if match_index < start_index {
                            // The table points into the current context (pre-dictionary).
                            // Re-look up in dictCtx's table instead.
                            let dict_idx = if !dict_ctx.is_null() {
                                get_index_on_hash(
                                    h,
                                    (*dict_ctx).hash_table.as_ptr(),
                                    TableType::ByU32,
                                )
                            } else {
                                0
                            };
                            match_ptr = dict_base.add(dict_idx as usize);
                            match_index = dict_idx.wrapping_add(dict_delta);
                            low_limit = dictionary;
                        } else {
                            match_ptr = base.add(match_index as usize);
                            low_limit = source;
                        }
                    } else if dict_directive == DictDirective::UsingExtDict {
                        if match_index < start_index {
                            match_ptr = dict_base.add(match_index as usize);
                            low_limit = dictionary;
                        } else {
                            match_ptr = base.add(match_index as usize);
                            low_limit = source;
                        }
                    } else {
                        // NoDict / WithPrefix64k: single contiguous memory segment
                        match_ptr = base.add(match_index as usize);
                    }

                    forward_h = hash_position(forward_ip, table_type);
                    put_index_on_hash(current, h, cctx_ref.hash_table.as_mut_ptr(), table_type);

                    // Reject: match outside dictSmall valid range
                    if dict_issue == DictIssueDirective::DictSmall && match_index < prefix_idx_limit
                    {
                        continue;
                    }

                    // Reject: match too far back (only checked for byU32; byU16 offsets
                    // always fit in u16 so the distance is always ≤ 65535)
                    if (table_type != TableType::ByU16
                        || LZ4_DISTANCE_MAX < LZ4_DISTANCE_ABSOLUTE_MAX)
                        && match_index.wrapping_add(LZ4_DISTANCE_MAX) < current
                    {
                        continue; // too far
                    }

                    if read32(match_ptr) == read32(ip) {
                        if maybe_ext_mem {
                            offset = current.wrapping_sub(match_index);
                        }
                        break; // match found
                    }
                } // end find-match do-while
            }

            // ── Catch up: extend match backwards past ip/anchor ──────────────
            // `ip > anchor` is always true per C assert, but checking is safe.
            filled_ip = ip;
            if match_ptr > low_limit && ip > anchor && *ip.sub(1) == *match_ptr.sub(1) {
                loop {
                    ip = ip.sub(1);
                    match_ptr = match_ptr.sub(1);
                    if !(ip > anchor && match_ptr > low_limit && *ip.sub(1) == *match_ptr.sub(1)) {
                        break;
                    }
                }
            }

            // ── Encode literals ──────────────────────────────────────────────
            {
                let lit_length = ip as usize - anchor as usize;
                token = op;
                op = op.add(1); // reserve token byte

                // Check output budget (limitedOutput)
                if output_directive == LimitedOutputDirective::LimitedOutput
                    && op.add(lit_length + 2 + 1 + LASTLITERALS + lit_length / 255) > olimit
                {
                    return Err(Lz4Error::OutputTooSmall);
                }

                // Check output budget (fillOutput)
                if output_directive == LimitedOutputDirective::FillOutput
                    && op.add((lit_length + 240) / 255 + lit_length + 2 + 1 + MFLIMIT - MINMATCH)
                        > olimit
                {
                    op = token; // undo token reservation (op--)
                    break 'compress; // output full — emit remaining bytes as trailing literals
                }

                // Write literal length into the high nibble of the token byte
                if lit_length >= RUN_MASK as usize {
                    let mut len = lit_length - RUN_MASK as usize;
                    *token = (RUN_MASK << ML_BITS) as u8;
                    while len >= 255 {
                        *op = 255u8;
                        op = op.add(1);
                        len -= 255;
                    }
                    *op = len as u8;
                    op = op.add(1);
                } else {
                    *token = (lit_length << ML_BITS as usize) as u8;
                }

                // Copy literals (may overwrite up to 8 bytes past op + lit_length)
                wild_copy8(op, anchor, op.add(lit_length));
                op = op.add(lit_length);
            }

            // ── Encode match, then opportunistically test the next position ──
            // If the byte immediately after the current match also matches,
            // we stay in this inner loop, writing a zero-literal token and
            // re-encoding without returning to the expensive find-match scan.
            'next_match: loop {
                // fillOutput: bail if there's no room for offset + token + min trailing literals
                if output_directive == LimitedOutputDirective::FillOutput
                    && op.add(2 + 1 + MFLIMIT - MINMATCH) > olimit
                {
                    op = token; // rewind
                    break 'compress; // output full — emit remaining bytes as trailing literals
                }

                // ── Encode match offset ───────────────────────────────────────
                if maybe_ext_mem {
                    write_le16(op, offset as u16);
                } else {
                    write_le16(op, (ip as usize - match_ptr as usize) as u16);
                }
                op = op.add(2);

                // ── Encode match length ───────────────────────────────────────
                let mut match_code: u32;
                if (dict_directive == DictDirective::UsingExtDict
                    || dict_directive == DictDirective::UsingDictCtx)
                    && low_limit == dictionary
                {
                    // Match starts inside the external dictionary; count crosses boundary.
                    let dict_part = dict_end as usize - match_ptr as usize;
                    let limit_ptr = ip.add(dict_part);
                    let limit_ptr = if limit_ptr > matchlimit {
                        matchlimit
                    } else {
                        limit_ptr
                    };
                    match_code = count(ip.add(MINMATCH), match_ptr.add(MINMATCH), limit_ptr);
                    ip = ip.add(match_code as usize + MINMATCH);
                    if ip == limit_ptr {
                        // The dict match extends all the way to the source; continue counting there.
                        let more = count(limit_ptr, source, matchlimit);
                        match_code = match_code.wrapping_add(more);
                        ip = ip.add(more as usize);
                    }
                } else {
                    match_code = count(ip.add(MINMATCH), match_ptr.add(MINMATCH), matchlimit);
                    ip = ip.add(match_code as usize + MINMATCH);
                }

                // ── Check output room for match-length extension bytes ────────
                if output_directive != LimitedOutputDirective::NotLimited
                    && op.add(1 + LASTLITERALS + (match_code as usize + 240) / 255) > olimit
                {
                    if output_directive == LimitedOutputDirective::FillOutput {
                        // Shorten the match so that the extension bytes fit exactly.
                        let space = (olimit as usize).saturating_sub(op as usize);
                        // space = olimit - op;  newMatchCode = 14 + (space - 1 - LASTLITERALS)*255
                        let new_match_code: u32 = 14u32.wrapping_add(
                            (space as u32).saturating_sub(1 + LASTLITERALS as u32) * 255,
                        );
                        if new_match_code < match_code {
                            ip = ip.sub((match_code - new_match_code) as usize);
                            match_code = new_match_code;
                            // Remove stale hash entries for positions we're backing up past.
                            if (ip as usize) <= (filled_ip as usize) {
                                let mut ptr = ip;
                                while (ptr as usize) <= (filled_ip as usize) {
                                    let h = hash_position(ptr, table_type);
                                    clear_hash(h, cctx_ref.hash_table.as_mut_ptr(), table_type);
                                    ptr = ptr.add(1);
                                }
                            }
                        }
                    } else {
                        // limitedOutput
                        return Err(Lz4Error::OutputTooSmall);
                    }
                }

                // ── Write match-length tokens into output ─────────────────────
                if match_code >= ML_MASK {
                    *token = (*token).wrapping_add(ML_MASK as u8);
                    match_code -= ML_MASK;
                    // Write 4 bytes of 0xFF per 4*255 units of match length
                    write32(op, 0xFFFF_FFFFu32);
                    while match_code >= 4 * 255 {
                        op = op.add(4);
                        write32(op, 0xFFFF_FFFFu32);
                        match_code -= 4 * 255;
                    }
                    op = op.add(match_code as usize / 255);
                    *op = (match_code % 255) as u8;
                    op = op.add(1);
                } else {
                    *token = (*token).wrapping_add(match_code as u8);
                }

                anchor = ip;

                // ── Test end of input chunk ───────────────────────────────────
                if ip >= mflimit_plus_one {
                    break 'compress; // too close to end for another match — emit trailing literals
                }

                // ── Fill hash table (ip-2) ────────────────────────────────────
                {
                    let h = hash_position(ip.sub(2), table_type);
                    if table_type == TableType::ByPtr {
                        put_position_on_hash(
                            ip.sub(2),
                            h,
                            cctx_ref.hash_table.as_mut_ptr() as *mut *const u8,
                            table_type,
                        );
                    } else {
                        let idx = (ip.sub(2) as usize - base as usize) as u32;
                        put_index_on_hash(idx, h, cctx_ref.hash_table.as_mut_ptr(), table_type);
                    }
                }

                // ── Test next position: try immediate re-match ────────────────
                if table_type == TableType::ByPtr {
                    let m = get_position(
                        ip,
                        cctx_ref.hash_table.as_ptr() as *const *const u8,
                        table_type,
                    );
                    put_position(
                        ip,
                        cctx_ref.hash_table.as_mut_ptr() as *mut *const u8,
                        table_type,
                    );
                    if m.add(LZ4_DISTANCE_MAX as usize) >= ip && read32(m) == read32(ip) {
                        // Immediate match at ip: emit a 0-literal sequence
                        token = op;
                        *op = 0;
                        op = op.add(1);
                        match_ptr = m;
                        continue 'next_match;
                    }
                } else {
                    // ByU32 / ByU16
                    let h = hash_position(ip, table_type);
                    let current = (ip as usize - base as usize) as u32;
                    let mut m_index =
                        get_index_on_hash(h, cctx_ref.hash_table.as_ptr(), table_type);

                    // Resolve match pointer from m_index
                    if dict_directive == DictDirective::UsingDictCtx {
                        if m_index < start_index {
                            let dict_idx = if !dict_ctx.is_null() {
                                get_index_on_hash(
                                    h,
                                    (*dict_ctx).hash_table.as_ptr(),
                                    TableType::ByU32,
                                )
                            } else {
                                0
                            };
                            match_ptr = dict_base.add(dict_idx as usize);
                            low_limit = dictionary;
                            m_index = dict_idx.wrapping_add(dict_delta);
                        } else {
                            match_ptr = base.add(m_index as usize);
                            low_limit = source;
                        }
                    } else if dict_directive == DictDirective::UsingExtDict {
                        if m_index < start_index {
                            match_ptr = dict_base.add(m_index as usize);
                            low_limit = dictionary;
                        } else {
                            match_ptr = base.add(m_index as usize);
                            low_limit = source;
                        }
                    } else {
                        match_ptr = base.add(m_index as usize);
                    }

                    put_index_on_hash(current, h, cctx_ref.hash_table.as_mut_ptr(), table_type);

                    // Validate: dictSmall range check
                    let dict_ok = if dict_issue == DictIssueDirective::DictSmall {
                        m_index >= prefix_idx_limit
                    } else {
                        true
                    };
                    // Validate: distance check (byU16 with max-distance == absolute-max always passes)
                    let dist_ok = if table_type == TableType::ByU16
                        && LZ4_DISTANCE_MAX == LZ4_DISTANCE_ABSOLUTE_MAX
                    {
                        true
                    } else {
                        m_index.wrapping_add(LZ4_DISTANCE_MAX) >= current
                    };

                    if dict_ok && dist_ok && read32(match_ptr) == read32(ip) {
                        // Immediate match: emit a 0-literal sequence
                        token = op;
                        *op = 0;
                        op = op.add(1);
                        if maybe_ext_mem {
                            offset = current.wrapping_sub(m_index);
                        }
                        continue 'next_match;
                    }
                }

                // No immediate match: restart find-match from advanced position.
                forward_h = hash_position(ip.add(1), table_type);
                ip = ip.add(1);
                break 'next_match; // back to 'main (find-match)
            } // end 'next_match
        } // end 'main
    } // end 'compress

    // ── Trailing-literals epilogue ───────────────────────────────────────────
    // Encode all remaining bytes between `anchor` and `iend` as a literal run.
    // Every LZ4 block ends here, whether we fell out of the loop normally or
    // broke early because the output buffer was full (FillOutput mode).
    {
        let mut last_run = iend as usize - anchor as usize;

        if output_directive != LimitedOutputDirective::NotLimited
            && op.add(last_run + 1 + (last_run + 255 - RUN_MASK as usize) / 255) > olimit
        {
            if output_directive == LimitedOutputDirective::FillOutput {
                // Shrink last_run to fit the remaining output buffer.
                last_run = (olimit as usize).saturating_sub(op as usize + 1);
                last_run -= (last_run + 256 - RUN_MASK as usize) / 256;
            } else {
                // limitedOutput
                return Err(Lz4Error::OutputTooSmall);
            }
        }

        if last_run >= RUN_MASK as usize {
            let mut accumulator = last_run - RUN_MASK as usize;
            *op = (RUN_MASK << ML_BITS) as u8;
            op = op.add(1);
            while accumulator >= 255 {
                *op = 255u8;
                op = op.add(1);
                accumulator -= 255;
            }
            *op = accumulator as u8;
            op = op.add(1);
        } else {
            *op = (last_run << ML_BITS as usize) as u8;
            op = op.add(1);
        }

        ptr::copy_nonoverlapping(anchor, op, last_run);
        ip = anchor.add(last_run);
        op = op.add(last_run);
    }

    if output_directive == LimitedOutputDirective::FillOutput && !input_consumed.is_null() {
        *input_consumed = (ip as usize - source as usize) as i32;
    }

    let result = op as usize - dest as usize;
    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Null-input / zero-input dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatcher that handles `src == NULL` / `srcSize == 0` and then delegates
/// to `compress_generic_validated`.
///
/// Equivalent to `LZ4_compress_generic`.
///
/// # Safety
/// See `compress_generic_validated`.
#[inline(always)]
pub unsafe fn compress_generic(
    cctx: *mut StreamStateInternal,
    src: *const u8,
    dst: *mut u8,
    src_size: i32,
    input_consumed: *mut i32,
    dst_capacity: i32,
    output_directive: LimitedOutputDirective,
    table_type: TableType,
    dict_directive: DictDirective,
    dict_issue: DictIssueDirective,
    acceleration: i32,
) -> Result<usize, Lz4Error> {
    if src_size < 0 || (src_size as u32) > LZ4_MAX_INPUT_SIZE {
        return Ok(0); // Unsupported srcSize (mirrors C returning 0)
    }

    if src_size == 0 {
        // Empty input: emit a single token byte of 0x00.
        if output_directive != LimitedOutputDirective::NotLimited && dst_capacity <= 0 {
            return Ok(0); // No room for even an empty block
        }
        debug_assert!(!dst.is_null());
        *dst = 0;
        if output_directive == LimitedOutputDirective::FillOutput && !input_consumed.is_null() {
            *input_consumed = 0;
        }
        return Ok(1);
    }

    debug_assert!(!src.is_null());
    compress_generic_validated(
        cctx,
        src,
        dst,
        src_size,
        input_consumed,
        dst_capacity,
        output_directive,
        table_type,
        dict_directive,
        dict_issue,
        acceleration,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// One-shot public API
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `src` into `dst` using an externally-provided (caller-managed) state.
///
/// The state is reset (zero-initialized) on entry.
///
/// Equivalent to `LZ4_compress_fast_extState`.
///
/// # Safety
/// `state` must be valid and exclusively accessible for the duration of the
/// call.  `src` must be readable for `src_len` bytes; `dst` must be writable
/// for `dst_capacity` bytes.
pub unsafe fn compress_fast_ext_state(
    state: *mut StreamStateInternal,
    src: *const u8,
    src_len: i32,
    dst: *mut u8,
    dst_capacity: i32,
    acceleration: i32,
) -> Result<usize, Lz4Error> {
    // Re-initialize state (equivalent to LZ4_initStream)
    *state = StreamStateInternal::new();

    let acceleration = acceleration
        .max(LZ4_ACCELERATION_DEFAULT)
        .min(LZ4_ACCELERATION_MAX);

    if dst_capacity >= compress_bound(src_len) {
        // Unlimited output: select table type based on input size
        if (src_len as usize) < LZ4_64KLIMIT {
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                0,
                LimitedOutputDirective::NotLimited,
                TableType::ByU16,
                DictDirective::NoDict,
                DictIssueDirective::NoDictIssue,
                acceleration,
            )
        } else {
            let table_type = select_table_type_for_src(src);
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                0,
                LimitedOutputDirective::NotLimited,
                table_type,
                DictDirective::NoDict,
                DictIssueDirective::NoDictIssue,
                acceleration,
            )
        }
    } else {
        // Limited output
        if (src_len as usize) < LZ4_64KLIMIT {
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                dst_capacity,
                LimitedOutputDirective::LimitedOutput,
                TableType::ByU16,
                DictDirective::NoDict,
                DictIssueDirective::NoDictIssue,
                acceleration,
            )
        } else {
            let table_type = select_table_type_for_src(src);
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                dst_capacity,
                LimitedOutputDirective::LimitedOutput,
                table_type,
                DictDirective::NoDict,
                DictIssueDirective::NoDictIssue,
                acceleration,
            )
        }
    }
}

/// Variant of `compress_fast_ext_state` that skips the expensive state
/// initialization.  Safe only when the state is known to be already correctly
/// initialized (e.g., from a prior `reset_stream_fast` call).
///
/// Equivalent to `LZ4_compress_fast_extState_fastReset`.
///
/// # Safety
/// Same as `compress_fast_ext_state`.  Additionally, `state` must already be
/// in a valid (correctly-initialized) state.
pub unsafe fn compress_fast_ext_state_fast_reset(
    state: *mut StreamStateInternal,
    src: *const u8,
    src_len: i32,
    dst: *mut u8,
    dst_capacity: i32,
    acceleration: i32,
) -> Result<usize, Lz4Error> {
    let acceleration = acceleration
        .max(LZ4_ACCELERATION_DEFAULT)
        .min(LZ4_ACCELERATION_MAX);

    if dst_capacity >= compress_bound(src_len) {
        if (src_len as usize) < LZ4_64KLIMIT {
            let table_type = TableType::ByU16;
            prepare_table(state, src_len, table_type);
            let dict_issue = if (*state).current_offset != 0 {
                DictIssueDirective::DictSmall
            } else {
                DictIssueDirective::NoDictIssue
            };
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                0,
                LimitedOutputDirective::NotLimited,
                table_type,
                DictDirective::NoDict,
                dict_issue,
                acceleration,
            )
        } else {
            let table_type = select_table_type_for_src(src);
            prepare_table(state, src_len, table_type);
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                0,
                LimitedOutputDirective::NotLimited,
                table_type,
                DictDirective::NoDict,
                DictIssueDirective::NoDictIssue,
                acceleration,
            )
        }
    } else {
        if (src_len as usize) < LZ4_64KLIMIT {
            let table_type = TableType::ByU16;
            prepare_table(state, src_len, table_type);
            let dict_issue = if (*state).current_offset != 0 {
                DictIssueDirective::DictSmall
            } else {
                DictIssueDirective::NoDictIssue
            };
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                dst_capacity,
                LimitedOutputDirective::LimitedOutput,
                table_type,
                DictDirective::NoDict,
                dict_issue,
                acceleration,
            )
        } else {
            let table_type = select_table_type_for_src(src);
            prepare_table(state, src_len, table_type);
            compress_generic(
                state,
                src,
                dst,
                src_len,
                ptr::null_mut(),
                dst_capacity,
                LimitedOutputDirective::LimitedOutput,
                table_type,
                DictDirective::NoDict,
                DictIssueDirective::NoDictIssue,
                acceleration,
            )
        }
    }
}

/// Compress `src` into `dst` with a user-supplied `acceleration` factor.
///
/// Allocates temporary compression state on the stack.
///
/// Equivalent to `LZ4_compress_fast`.
///
/// Returns the number of bytes written to `dst`, or `Err(Lz4Error::OutputTooSmall)`.
pub fn compress_fast(src: &[u8], dst: &mut [u8], acceleration: i32) -> Result<usize, Lz4Error> {
    let src_len = src.len();
    if src_len > LZ4_MAX_INPUT_SIZE as usize {
        return Err(Lz4Error::InputTooLarge);
    }
    let mut ctx = StreamStateInternal::new();
    unsafe {
        compress_fast_ext_state(
            &mut ctx,
            src.as_ptr(),
            src_len as i32,
            dst.as_mut_ptr(),
            dst.len() as i32,
            acceleration,
        )
    }
}

/// Compress `src` into `dst` with the default acceleration factor (1).
///
/// This is the recommended entry point for one-shot LZ4 block compression.
///
/// Equivalent to `LZ4_compress_default`.
///
/// Returns the number of bytes written to `dst`, or `Err(Lz4Error::OutputTooSmall)`.
pub fn compress_default(src: &[u8], dst: &mut [u8]) -> Result<usize, Lz4Error> {
    compress_fast(src, dst, 1)
}

/// Compress as much of `src` as fits in exactly `dst_capacity` bytes.
///
/// On success returns the number of bytes consumed from `src` (via
/// `*src_size`) and the compressed length.  The caller must allocate
/// `dst` to exactly `dst_capacity` bytes.
///
/// Equivalent to `LZ4_compress_destSize`.
///
/// # Safety
/// `src_size` must be a valid pointer to a mutable `i32` that holds the
/// input length on entry and receives the number of bytes consumed on exit.
pub unsafe fn compress_dest_size_raw(
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: *mut i32,
    target_dst_size: i32,
) -> Result<usize, Lz4Error> {
    let mut ctx = StreamStateInternal::new();
    compress_dest_size_ext_state_internal(&mut ctx, src, dst, src_size_ptr, target_dst_size, 1)
}

/// Safe wrapper for `LZ4_compress_destSize`.
///
/// Fills `dst` as completely as possible, returning how many source bytes
/// were consumed and the compressed length.
///
/// Equivalent to `LZ4_compress_destSize`.
pub fn compress_dest_size(src: &[u8], dst: &mut [u8]) -> Result<(usize, usize), Lz4Error> {
    let mut src_consumed = src.len() as i32;
    let compressed = unsafe {
        compress_dest_size_raw(
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_consumed,
            dst.len() as i32,
        )?
    };
    Ok((src_consumed as usize, compressed))
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Select `ByPtr` (32-bit-only) or `ByU32` based on pointer width and address.
///
/// Matches: `((sizeof(void*)==4) && ((uptrval)source > LZ4_DISTANCE_MAX)) ? byPtr : byU32`
#[inline(always)]
fn select_table_type_for_src(src: *const u8) -> TableType {
    if cfg!(target_pointer_width = "32") && (src as usize > LZ4_DISTANCE_MAX as usize) {
        TableType::ByPtr
    } else {
        TableType::ByU32
    }
}

/// Internal destSize helper that leaves the stream in a broken state
/// (must not be used for streaming after this call without re-init).
///
/// Equivalent to `LZ4_compress_destSize_extState_internal`.
///
/// # Safety
/// All pointer preconditions of `compress_generic_validated` apply.
unsafe fn compress_dest_size_ext_state_internal(
    state: *mut StreamStateInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: *mut i32,
    target_dst_size: i32,
    acceleration: i32,
) -> Result<usize, Lz4Error> {
    // Re-initialize state
    *state = StreamStateInternal::new();

    let src_size = *src_size_ptr;
    if target_dst_size >= compress_bound(src_size) {
        // Guaranteed success — use normal compression path
        compress_fast_ext_state(
            state,
            src,
            src_size as i32,
            dst,
            target_dst_size,
            acceleration,
        )
    } else if (src_size as usize) < LZ4_64KLIMIT {
        compress_generic(
            &mut (*state),
            src,
            dst,
            src_size,
            src_size_ptr, // receives bytes consumed
            target_dst_size,
            LimitedOutputDirective::FillOutput,
            TableType::ByU16,
            DictDirective::NoDict,
            DictIssueDirective::NoDictIssue,
            acceleration,
        )
    } else {
        let addr_mode = select_table_type_for_src(src);
        compress_generic(
            &mut (*state),
            src,
            dst,
            src_size,
            src_size_ptr,
            target_dst_size,
            LimitedOutputDirective::FillOutput,
            addr_mode,
            DictDirective::NoDict,
            DictIssueDirective::NoDictIssue,
            acceleration,
        )
    }
}

/// Public variant of `compress_dest_size_ext_state_internal` that re-initializes
/// the stream on exit (leaving it in a clean state).
///
/// Equivalent to `LZ4_compress_destSize_extState`.
///
/// # Safety
/// Same as `compress_dest_size_ext_state_internal`.
pub unsafe fn compress_dest_size_ext_state(
    state: *mut StreamStateInternal,
    src: *const u8,
    dst: *mut u8,
    src_size_ptr: *mut i32,
    target_dst_size: i32,
    acceleration: i32,
) -> Result<usize, Lz4Error> {
    let r = compress_dest_size_ext_state_internal(
        state,
        src,
        dst,
        src_size_ptr,
        target_dst_size,
        acceleration,
    );
    // Clean state on exit (matches LZ4_compress_destSize_extState C behaviour)
    *state = StreamStateInternal::new();
    r
}
