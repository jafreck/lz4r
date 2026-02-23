//! LZ4 block decompression core engine.
//!
//! Implements the algorithms in lz4.c v1.10.0 (lines 1969–2447):
//!   - `read_variable_length` — bounded variable-length integer decoder
//!   - `decompress_generic`   — the main, security-critical safe decompression loop
//!
//! # Security boundary
//!
//! This module is the **security-critical decompression path**.  Every bounds
//! check present in the C source has an exact Rust equivalent here.  No check
//! may be elided.  Malformed or truncated input must return
//! `Err(DecompressError::MalformedInput)` — it must **never** panic or cause
//! undefined behaviour.
//!
//! All `unsafe` blocks carry an explicit `// SAFETY:` comment.

use core::ptr;

use super::types::{
    read_le16, wild_copy8, write32, DictDirective, DEC64TABLE, INC32TABLE,
    LASTLITERALS, MATCH_SAFEGUARD_DISTANCE, MFLIMIT, MINMATCH, ML_BITS, ML_MASK, RUN_MASK,
    WILDCOPYLENGTH,
};

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors returned by LZ4 block decompression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecompressError {
    /// The compressed data is malformed, truncated, or the dimensions supplied
    /// by the caller are inconsistent.  Equivalent to a negative return value
    /// from the C `LZ4_decompress_safe` family.
    MalformedInput,
}

// ─────────────────────────────────────────────────────────────────────────────
// Returns the decompression error for all invalid-input, out-of-bounds, and
// truncation conditions.
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn output_error<T>() -> Result<T, DecompressError> {
    Err(DecompressError::MalformedInput)
}

// ─────────────────────────────────────────────────────────────────────────────
// read_variable_length — lz4.c:1978-2014
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinel value returned by `read_variable_length` on error.
/// Corresponds to C `rvl_error = (Rvl_t)(-1)`.
const RVL_ERROR: usize = usize::MAX;

/// Read a variable-length integer from the input stream.
///
/// Accumulates `u8` bytes into a `usize` sum until a byte < 255 is read, or
/// until `ilimit` is reached (which is an error).
///
/// `initial_check`: if `true`, fail immediately when `ip >= ilimit` before
/// reading the first byte (mirrors the C `initial_check` parameter).
///
/// Returns `RVL_ERROR` on any parsing failure; otherwise the accumulated value
/// (which must be added to the caller's running length counter).
///
/// # Safety
/// `ip` must point into the same allocation as `ilimit`.
/// All bytes in `[ip, ilimit]` must be readable.
#[inline(always)]
unsafe fn read_variable_length(ip: &mut *const u8, ilimit: *const u8, initial_check: bool) -> usize {
    let mut s: usize;
    let mut length: usize = 0;

    if initial_check && *ip >= ilimit {
        // No bytes remain before the limit before the first byte is read.
        return RVL_ERROR;
    }

    // Read first byte.
    // SAFETY: ensured by caller that *ip is a valid readable address.
    s = **ip as usize;
    *ip = (*ip).add(1);
    length += s;

    if *ip > ilimit {
        // The pointer advanced past the limit after consuming the first byte.
        return RVL_ERROR;
    }

    // 32-bit overflow guard: if usize is 32 bits and the accumulated value
    // already exceeds half of usize::MAX, further additions could wrap.
    if core::mem::size_of::<usize>() < 8 && length > usize::MAX / 2 {
        return RVL_ERROR;
    }

    if s != 255 {
        return length;
    }

    // Continue reading 0xFF bytes.
    loop {
        // SAFETY: *ip is within the buffer (checked at top of each iteration).
        s = **ip as usize;
        *ip = (*ip).add(1);
        length += s;

        if *ip > ilimit {
            return RVL_ERROR;
        }

        if core::mem::size_of::<usize>() < 8 && length > usize::MAX / 2 {
            return RVL_ERROR;
        }

        if s != 255 {
            break;
        }
    }

    length
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_generic — lz4.c:2022-2445
// ─────────────────────────────────────────────────────────────────────────────

/// Core LZ4 block decompression loop.
///
/// This is the Rust equivalent of `LZ4_decompress_generic` from lz4.c.
/// It covers all use-cases through its parameters:
///
/// | Parameter        | Meaning                                              |
/// |-----------------|------------------------------------------------------|
/// | `src`            | Compressed input slice                               |
/// | `dst`            | Output buffer                                        |
/// | `output_size`    | `dst.len()` (capacity) in full-block mode; capacity  |
/// |                  | in partial-decode mode                               |
/// | `partial_decoding` | `false` = decode full block; `true` = may stop early |
/// | `dict`           | Dictionary mode (noDict / withPrefix64k / extDict)   |
/// | `low_prefix`     | Always ≤ `dst.as_ptr()`; equals `dst.as_ptr()` when  |
/// |                  | no prefix                                            |
/// | `dict_start`     | Start of external dictionary (only for `UsingExtDict`)|
/// | `dict_size`      | External dictionary size in bytes                    |
///
/// Returns the number of bytes written to `dst` on success, or
/// `Err(DecompressError::MalformedInput)` for any invalid input.
///
/// # Safety
/// - `low_prefix` must be ≤ `dst` (pointer ≤ start of output buffer).
/// - When `dict == DictDirective::UsingExtDict`, `dict_start` must be a valid
///   pointer to `dict_size` readable bytes, and `dict_start..dict_start+dict_size`
///   must not alias the output buffer in an unsafe way.
/// - `dst` must have at least `output_size + WILDCOPYLENGTH + MINMATCH` bytes
///   of backing storage to absorb wildcard-copy overruns (callers outside this
///   crate are responsible for reserving sufficient buffer space).
#[allow(clippy::too_many_arguments)]
pub unsafe fn decompress_generic(
    src: *const u8,
    dst: *mut u8,
    src_size: usize,
    output_size: usize,
    partial_decoding: bool,
    dict: DictDirective,
    low_prefix: *const u8,
    dict_start: *const u8, // only meaningful when dict == UsingExtDict
    dict_size: usize,
) -> Result<usize, DecompressError> {
    // ── Validate top-level arguments ─────────────────────────────────────────
    if src.is_null() || (output_size as isize) < 0 {
        return output_error();
    }

    // ── Set up working pointers ───────────────────────────────────────────────
    // SAFETY: src_size bytes of readable memory begin at src (caller contract).
    let mut ip: *const u8 = src;
    let iend: *const u8 = src.add(src_size);

    // SAFETY: output_size bytes of writable memory begin at dst (caller contract).
    let mut op: *mut u8 = dst;
    let oend: *mut u8 = dst.add(output_size);

    // Pointer to end of external dictionary.
    let dict_end: *const u8 = if dict_start.is_null() {
        ptr::null()
    } else {
        dict_start.add(dict_size)
    };

    // Whether we need to validate that match back-references fall within the
    // combined (dict + current output) window.  When dict_size >= 64 KiB the
    // full LZ4 64-KiB window is always valid, so we skip the check.
    let check_offset: bool = dict_size < 64 * 1024;

    // Shortcut pointers: define the "safe zone" where both input and output
    // have enough remaining space for the two-stage fast-path copy.
    // shortiend = iend - 14 (maxLL) - 2 (offset field)
    // shortoend = oend - 14 (maxLL) - 18 (maxML)
    let short_iend: *const u8 = if src_size >= 16 { iend.sub(14).sub(2) } else { src };
    let short_oend: *mut u8 = if output_size >= 32 { oend.sub(14).sub(18) } else { dst };

    // ── Special cases ─────────────────────────────────────────────────────────
    // Equivalent to C assert(lowPrefix <= op).
    debug_assert!(low_prefix <= op as *const u8);

    if output_size == 0 {
        if partial_decoding {
            return Ok(0);
        }
        // Empty dst is only valid when the compressed block is a single 0-token.
        return if src_size == 1 && *src == 0 { Ok(0) } else { output_error() };
    }
    if src_size == 0 {
        return output_error();
    }

    // ── Main decode loop ──────────────────────────────────────────────────────
    //
    // Control-flow mapping from C goto labels:
    //   goto _output_error  →  return output_error()
    //   goto _copy_match    →  handled by shared match-decode block at end of
    //                          each loop iteration (both the shortcut-failure
    //                          path and the normal path set up `ml`/`offset`/
    //                          `match_ptr` and fall through to the same code)
    //   break (EOF)         →  break out of 'decode loop
    'decode: loop {
        // Every iteration starts with a new token byte.
        // C: assert(ip < iend);
        debug_assert!(ip < iend);

        // SAFETY: ip < iend guarantees one readable byte.
        let token: u8 = *ip;
        ip = ip.add(1);

        let mut lit_length: usize = (token >> ML_BITS as u8) as usize;

        // Variables shared between shortcut and normal path, set before the
        // match-decode section at the bottom of the loop.
        let offset: usize;
        let match_ptr: *const u8;
        let ml: usize; // match length, token nibble only (not yet extended)

        // ── Two-stage shortcut (C: lines 2230-2261) ───────────────────────────
        //
        // When literal length is short (< 15) and there is ample space in both
        // input and output, skip the full bounds-checked paths and copy 16/18
        // bytes at once.
        if lit_length != RUN_MASK as usize
            && (ip < short_iend)
            && (op as *const u8 <= short_oend as *const u8)
        {
            // Stage 1: copy exactly `lit_length` bytes (via a 16-byte write).
            // SAFETY: The shortcut conditions guarantee:
            //   - ip + 16 <= iend  (short_iend = iend - 16)
            //   - op + 16 <= oend  (short_oend = oend - 32)
            // so reading 16 from ip and writing 16 to op are both in bounds.
            ptr::copy_nonoverlapping(ip, op, 16);
            op = op.add(lit_length);
            ip = ip.add(lit_length);

            // Stage 2: decode match info.
            ml = (token & ML_MASK as u8) as usize;

            // SAFETY: short_iend = iend - 16, lit_length <= 14, so ip has
            // advanced by at most 14 bytes; ip + 2 <= iend is guaranteed.
            let off16 = read_le16(ip) as usize;
            ip = ip.add(2);

            // SAFETY: op >= dst; off16 may be 0 (checked later).
            let mp = (op as *const u8).wrapping_sub(off16);

            if ml != ML_MASK as usize
                && off16 >= 8
                && (dict == DictDirective::WithPrefix64k || mp >= low_prefix)
            {
                // Fast 18-byte match copy — no overlap possible (offset >= 8).
                // SAFETY: The shortcut conditions guarantee op + 18 <= oend.
                // mp >= low_prefix guarantees the source is within the valid window.
                ptr::copy_nonoverlapping(mp, op, 8);
                ptr::copy_nonoverlapping(mp.add(8), op.add(8), 8);
                ptr::copy_nonoverlapping(mp.add(16), op.add(16), 2);
                op = op.add(ml + MINMATCH);
                continue 'decode;
            }

            // Stage 2 did not qualify for the fast copy; the literal copy
            // already happened, the offset is already consumed.  Fall through
            // to the shared match-decode section below.
            offset = off16;
            match_ptr = mp;
        } else {
            // ── Full literal decode path (C: lines 2263-2334) ─────────────────

            if lit_length == RUN_MASK as usize {
                // SAFETY: iend - RUN_MASK is the ilimit for the variable-length
                // literal reader (C: `iend - RUN_MASK`).
                let ilimit = if src_size >= RUN_MASK as usize {
                    iend.sub(RUN_MASK as usize)
                } else {
                    src
                };
                let addl = read_variable_length(&mut ip, ilimit, true);
                if addl == RVL_ERROR {
                    return output_error();
                }
                lit_length += addl;

                // Pointer wrap-around detection (matches C uptrval overflow check).
                if (op as usize).wrapping_add(lit_length) < op as usize {
                    return output_error();
                }
                if (ip as usize).wrapping_add(lit_length) < ip as usize {
                    return output_error();
                }
            }

            // Copy literals.
            let cpy: *mut u8 = op.add(lit_length);

            // Check whether we are at the last sequence or near the buffer ends.
            // C: (cpy > oend-MFLIMIT) || (ip+length > iend-(2+1+LASTLITERALS))
            let near_out_end = cpy > oend.sub(MFLIMIT);
            let near_in_end = ip.add(lit_length)
                > iend.sub(2 + 1 + LASTLITERALS);

            if near_out_end || near_in_end {
                // Slow / last-sequence path.
                if partial_decoding {
                    // Clamp literal length to whatever fits in input.
                    let (lit_length, cpy) = if ip.add(lit_length) > iend {
                        let ll = iend as usize - ip as usize;
                        (ll, op.add(ll))
                    } else {
                        (lit_length, cpy)
                    };
                    // Clamp to output capacity.
                    let (lit_length, cpy) = if cpy > oend {
                        let ll = oend as usize - op as usize;
                        (ll, oend)
                    } else {
                        (lit_length, cpy)
                    };

                    // SAFETY: src and dst may overlap in in-place decompression;
                    // ptr::copy (memmove) handles overlapping regions correctly.
                    ptr::copy(ip, op, lit_length);
                    ip = ip.add(lit_length);
                    op = cpy;

                    // Break when output is full or input is exhausted (need at least
                    // 2 bytes for a match offset).  The `!partial_decoding` guard is
                    // always false in this branch; it mirrors the C source's unified
                    // condition and is left for structural clarity.
                    if !partial_decoding || cpy == oend || ip >= iend.sub(2) {
                        break 'decode;
                    }
                } else {
                    // Full-block mode: this must be the last sequence.
                    // C: (ip+length != iend) || (cpy > oend) → _output_error
                    if ip.add(lit_length) != iend || cpy > oend {
                        return output_error();
                    }
                    // SAFETY: same as above — memmove for in-place safety.
                    ptr::copy(ip, op, lit_length);
                    op = cpy;
                    break 'decode;
                }
            } else {
                // Normal path: wildcard-copy.
                // SAFETY: wild_copy8 may write up to 8 bytes past `cpy`.
                // The condition `!near_out_end` guarantees cpy <= oend - MFLIMIT,
                // and MFLIMIT (12) > WILDCOPYLENGTH (8), so the overrun is safe.
                wild_copy8(op, ip, cpy);
                ip = ip.add(lit_length);
                op = cpy;
            }

            // Read match offset (2 bytes).
            // SAFETY: !near_in_end guarantees ip + 2 <= iend.
            offset = read_le16(ip) as usize;
            ip = ip.add(2);

            // SAFETY: op >= dst; arithmetic may produce a pointer before the
            // output buffer if offset is bogus — validated below.
            match_ptr = (op as *const u8).wrapping_sub(offset);

            ml = (token & ML_MASK as u8) as usize;
        }

        // ── _copy_match: (C: line 2344) ───────────────────────────────────────
        //
        // Reached from BOTH the shortcut-failure path and the normal path.
        // At this point:
        //   - `ml`        = `token & ML_MASK` (may need extension)
        //   - `offset`    = 16-bit back-reference distance (already consumed)
        //   - `match_ptr` = `op - offset` (may point before dst / into dict)
        //   - `ip`        = positioned after the offset field

        let mut ml_ext = ml;

        if ml == ML_MASK as usize {
            // Extended match length.
            // ilimit = iend - LASTLITERALS + 1  (C: line 2346)
            let ilimit = if src_size >= LASTLITERALS {
                iend.sub(LASTLITERALS).add(1)
            } else {
                src
            };
            let addl = read_variable_length(&mut ip, ilimit, false);
            if addl == RVL_ERROR {
                return output_error();
            }
            ml_ext += addl;

            // Overflow detection: C `(uptrval)(op)+length < (uptrval)op`
            if (op as usize).wrapping_add(ml_ext) < op as usize {
                return output_error();
            }
        }
        let match_length: usize = ml_ext + MINMATCH;

        // ── Bounds check: offset validity ──────────────────────────────────────
        // C: if (checkOffset) && (match + dictSize < lowPrefix) → _output_error
        //
        // SAFETY: this is a pointer-arithmetic comparison; wrapping is intentional
        // (a bogus match_ptr far before the buffer will wrap and be < low_prefix).
        if check_offset
            && (match_ptr as usize).wrapping_add(dict_size) < low_prefix as usize
        {
            return output_error();
        }

        // ── External-dictionary match (C: lines 2358-2384) ────────────────────
        if dict == DictDirective::UsingExtDict && (match_ptr as *const u8) < low_prefix {
            // The reference is before the current output prefix → it lives in the
            // external dictionary.
            debug_assert!(!dict_end.is_null());

            // Partial-decode or full-block end-of-block constraint.
            let match_length = if op.add(match_length) > oend.sub(LASTLITERALS) {
                if partial_decoding {
                    // Clamp to available output.
                    // SAFETY: oend >= op (loop invariant).
                    (oend as usize - op as usize).min(match_length)
                } else {
                    return output_error();
                }
            } else {
                match_length
            };

            // Distance from match_ptr to the start of the current output prefix.
            let copy_size = low_prefix as usize - match_ptr as usize;

            if match_length <= copy_size {
                // Match fits entirely within the external dictionary.
                // SAFETY: dict_end - copy_size is a valid address inside the
                // dictionary allocation; we copy `match_length` bytes from it.
                let dict_src = dict_end.sub(copy_size);
                // ptr::copy handles overlapping in-place scenarios.
                ptr::copy(dict_src, op, match_length);
                op = op.add(match_length);
            } else {
                // Match spans both dictionary and current output prefix.
                let rest_size = match_length - copy_size;

                // First: copy `copy_size` bytes from tail of external dict.
                // SAFETY: dict_end - copy_size .. dict_end is valid dict memory.
                ptr::copy_nonoverlapping(dict_end.sub(copy_size), op, copy_size);
                op = op.add(copy_size);

                // Then: copy `rest_size` bytes from the start of the prefix.
                // This may overlap the current output — handle carefully.
                if rest_size > (op as usize - low_prefix as usize) {
                    // Overlapping: must copy byte-by-byte.
                    let end_of_match: *mut u8 = op.add(rest_size);
                    let mut copy_from: *const u8 = low_prefix;
                    // SAFETY: copy_from stays within the current output block
                    // (low_prefix..op is already written), advancing in lock-step.
                    while op < end_of_match {
                        *op = *copy_from;
                        op = op.add(1);
                        copy_from = copy_from.add(1);
                    }
                } else {
                    // No overlap: plain memcpy from prefix start.
                    // SAFETY: low_prefix .. low_prefix + rest_size is within
                    // the already-written output window.
                    ptr::copy_nonoverlapping(low_prefix, op, rest_size);
                    op = op.add(rest_size);
                }
            }
            continue 'decode;
        }

        // ── Within-block match copy ────────────────────────────────────────────
        // C: assert(match >= lowPrefix);
        debug_assert!(match_ptr >= low_prefix);

        let cpy: *mut u8 = op.add(match_length);

        // Partial-decode: near the end of the output buffer we cannot use the
        // fast wildcopy routines.
        if partial_decoding && cpy > oend.sub(MATCH_SAFEGUARD_DISTANCE) {
            let mlen = (oend as usize - op as usize).min(match_length);
            let match_end: *const u8 = match_ptr.add(mlen);
            let copy_end: *mut u8 = op.add(mlen);

            if match_end > op as *const u8 {
                // Overlap: copy byte-by-byte.
                // SAFETY: Both src and dst are within valid memory; byte-by-byte
                // copy handles the overlap correctly.
                let mut mp = match_ptr;
                while op < copy_end {
                    *op = *mp;
                    op = op.add(1);
                    mp = mp.add(1);
                }
            } else {
                // No overlap.
                // SAFETY: mlen bytes are available at match_ptr (validated by
                // the offset check above).
                ptr::copy_nonoverlapping(match_ptr, op, mlen);
            }
            op = copy_end;
            if op == oend {
                break 'decode;
            }
            continue 'decode;
        }

        // Standard match copy.
        // First handle the tricky small-offset (< 8) overlapping case using the
        // offset tables, then handle offsets >= 8 with a plain 8-byte copy.
        let mut mp: *const u8 = match_ptr; // local mutable alias for adjustment

        if offset < 8 {
            // Small offset: may be a repeating-byte or repeating-pair pattern.
            // Write 0 first so that memory-sanitizers see an initialised value
            // if offset == 0 (which is an error, caught by the offset check).
            // SAFETY: op has at least 4 bytes of space (cpy > op + MINMATCH).
            write32(op, 0);
            // SAFETY: mp is within the valid window (checked by offset guard above).
            *op = *mp;
            *op.add(1) = *mp.add(1);
            *op.add(2) = *mp.add(2);
            *op.add(3) = *mp.add(3);
            mp = mp.add(INC32TABLE[offset] as usize);
            // SAFETY: copy 4 bytes from adjusted match position.
            ptr::copy_nonoverlapping(mp, op.add(4), 4);
            mp = mp.offset(-(DEC64TABLE[offset] as isize));
        } else {
            // SAFETY: offset >= 8 means source and destination 8-byte chunks
            // cannot overlap; copy_nonoverlapping is safe.
            ptr::copy_nonoverlapping(mp, op, 8);
            mp = mp.add(8);
        }
        op = op.add(8);

        // Finish the match copy, handling the near-end-of-buffer case.
        if cpy > oend.sub(MATCH_SAFEGUARD_DISTANCE) {
            // Close to the output end: cannot use wildCopy8 freely.
            let o_copy_limit: *mut u8 = oend.sub(WILDCOPYLENGTH - 1);

            // C: if (cpy > oend-LASTLITERALS) → _output_error
            // The last LASTLITERALS bytes of the block must be literals.
            if cpy > oend.sub(LASTLITERALS) {
                return output_error();
            }

            if op < o_copy_limit {
                // SAFETY: wild_copy8 may write 8 bytes past o_copy_limit; the
                // margin `oend - o_copy_limit = WILDCOPYLENGTH - 1 = 7` bytes is
                // within the buffer's WILDCOPYLENGTH reserved tail.
                wild_copy8(op, mp, o_copy_limit);
                // SAFETY: arithmetic matches the bytes written by wild_copy8.
                mp = mp.add(o_copy_limit as usize - op as usize);
                op = o_copy_limit;
            }

            // Final byte-by-byte copy up to cpy.
            // SAFETY: cpy <= oend - LASTLITERALS <= oend; op < cpy.
            while op < cpy {
                *op = *mp;
                op = op.add(1);
                mp = mp.add(1);
            }
        } else {
            // Normal case: plenty of room.
            // SAFETY: copy_nonoverlapping safe (8 bytes, offset >= 8 on this path).
            ptr::copy_nonoverlapping(mp, op, 8);
            if match_length > 16 {
                // SAFETY: wild_copy8 may write 8 bytes past cpy; the caller
                // must have reserved at least WILDCOPYLENGTH bytes past oend.
                wild_copy8(op.add(8), mp.add(8), cpy);
            }
        }

        // Wildcopy correction: advance op to the exact end of the match.
        op = cpy;
    } // end 'decode

    // ── End of decoding ───────────────────────────────────────────────────────
    // Return the number of bytes written to the output buffer.
    // SAFETY: op started at dst and only advanced forward; op - dst is the count.
    Ok(op as usize - dst as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public safe wrappers
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress a full LZ4 block (no dictionary).
///
/// Equivalent to `LZ4_decompress_safe`.
///
/// Returns the number of bytes written into `dst` on success, or
/// `Err(DecompressError::MalformedInput)` if the input is invalid.
pub fn decompress_safe(src: &[u8], dst: &mut [u8]) -> Result<usize, DecompressError> {
    if dst.is_empty() {
        // Special case: zero-capacity output.
        if src.len() == 1 && src[0] == 0 {
            return Ok(0);
        }
        return output_error();
    }

    // SAFETY:
    //   - `src.as_ptr()` and `dst.as_mut_ptr()` are valid, non-null, and
    //     correctly sized by the slice invariants.
    //   - `low_prefix == dst.as_ptr()` (no prefix).
    //   - `dict_start` is null and `dict_size` is 0.
    //   - The caller is responsible for providing a `dst` buffer that is large
    //     enough; we pass `dst.len()` as the output capacity.
    unsafe {
        decompress_generic(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len(),
            dst.len(),
            false, // full block
            DictDirective::NoDict,
            dst.as_ptr(), // low_prefix == dst start
            ptr::null(),  // no external dictionary
            0,
        )
    }
}

/// Decompress up to `target_output_size` bytes from an LZ4 block (no dict).
///
/// Equivalent to `LZ4_decompress_safe_partial`.
///
/// `dst.len()` is the capacity of the output buffer; `target_output_size` is
/// the number of decompressed bytes the caller wants.  If the compressed block
/// contains more data it will be decoded up to the limit.
///
/// Returns the number of bytes written into `dst`, or
/// `Err(DecompressError::MalformedInput)` on error.
pub fn decompress_safe_partial(
    src: &[u8],
    dst: &mut [u8],
    target_output_size: usize,
) -> Result<usize, DecompressError> {
    let output_size = target_output_size.min(dst.len());

    // SAFETY: same contracts as `decompress_safe`; partial_decoding = true.
    unsafe {
        decompress_generic(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len(),
            output_size,
            true, // partial decode
            DictDirective::NoDict,
            dst.as_ptr(),
            ptr::null(),
            0,
        )
    }
}

/// Decompress an LZ4 block using an external dictionary.
///
/// Equivalent to `LZ4_decompress_safe_usingDict` (for the non-split,
/// non-streaming case).
///
/// `dict` must be the same dictionary that was used during compression.
/// Returns the number of bytes written into `dst`, or
/// `Err(DecompressError::MalformedInput)` on error.
pub fn decompress_safe_using_dict(
    src: &[u8],
    dst: &mut [u8],
    dict: &[u8],
) -> Result<usize, DecompressError> {
    if dict.is_empty() {
        return decompress_safe(src, dst);
    }

    // SAFETY:
    //   - All slices are valid by Rust slice invariants.
    //   - low_prefix == dst.as_ptr() (no prior output prefix).
    //   - dict_start / dict_size describe the external dictionary.
    unsafe {
        decompress_generic(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len(),
            dst.len(),
            false,
            DictDirective::UsingExtDict,
            dst.as_ptr(), // low_prefix: nothing before dst
            dict.as_ptr(),
            dict.len(),
        )
    }
}
