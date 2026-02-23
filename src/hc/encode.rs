//! HC sequence encoder.
//!
//! Provides [`encode_sequence`], the low-level routine that serialises one
//! complete LZ4 sequence — literal run, token byte, extended-length bytes,
//! 16-bit back-reference offset, and match-length extension — into the output
//! buffer.  Called once per match during HC compression.
//!
//! Corresponds to `LZ4HC_encodeSequence` in `lz4hc.c` (v1.10.0, lines 262–355).

use crate::block::types::{
    wild_copy8, write_le16, LimitedOutputDirective, LASTLITERALS, LZ4_DISTANCE_MAX, MINMATCH,
    ML_BITS, ML_MASK, RUN_MASK,
};

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by the HC sequence encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lz4HcError {
    /// The output buffer does not have enough room for the encoded sequence.
    OutputTooSmall,
}

// ─────────────────────────────────────────────────────────────────────────────
// encode_sequence
// ─────────────────────────────────────────────────────────────────────────────

/// Encode one LZ4 sequence (literal run + match) into the output buffer.
///
/// Equivalent to `LZ4HC_encodeSequence` in lz4hc.c (lines 268–354).
///
/// # Parameters
///
/// | Rust parameter  | C macro / param | Description                             |
/// |-----------------|-----------------|-----------------------------------------|
/// | `ip`            | `*_ip`          | Current read position in the input      |
/// | `op`            | `*_op`          | Current write position in the output    |
/// | `anchor`        | `*_anchor`      | Start of the current literal run        |
/// | `match_length`  | `matchLength`   | Length of the match (≥ `MINMATCH`)      |
/// | `offset`        | `offset`        | Back-reference distance (> 0, ≤ 65535)  |
/// | `limit`         | `limit`         | Whether to enforce output-buffer bounds |
/// | `oend`          | `oend`          | One-past-end of the output buffer       |
///
/// On success the function advances `*ip` by `match_length` and resets
/// `*anchor` to the new `*ip`, ready for the next sequence.
///
/// # Safety
///
/// * `*ip` and `*anchor` must point into (or one-past-end of) the same
///   input allocation, with `*anchor <= *ip`.
/// * `*op` must point into the output allocation; `oend` must point
///   one byte past the end of that same allocation.
/// * All pointer arithmetic must remain within bounds.
///
/// # Errors
///
/// Returns `Err(Lz4HcError::OutputTooSmall)` when
/// `limit == LimitedOutputDirective::LimitedOutput` and the output buffer
/// does not have sufficient space for the encoded sequence.
#[inline(always)]
pub unsafe fn encode_sequence(
    ip: &mut *const u8,
    op: &mut *mut u8,
    anchor: &mut *const u8,
    match_length: i32,
    offset: i32,
    limit: LimitedOutputDirective,
    oend: *mut u8,
) -> Result<(), Lz4HcError> {
    // ── Literal length ────────────────────────────────────────────────────
    // Distance from the start of the literal run to the current input pos.
    let literal_length: usize = (*ip).offset_from(*anchor) as usize;

    // Reserve one byte for the token; we will fill it in below.
    let token: *mut u8 = *op;
    *op = (*op).add(1);

    // ── Output-limit check for literal run ───────────────────────────────
    // Worst-case space needed before the match data:
    //   ceil(length/255) extension bytes + length literal bytes
    //   + 2-byte offset + 1 remaining token byte + LASTLITERALS reserved.
    if limit == LimitedOutputDirective::LimitedOutput {
        let needed = literal_length / 255 + literal_length + (2 + 1 + LASTLITERALS);
        if (*op).add(needed) > oend {
            return Err(Lz4HcError::OutputTooSmall);
        }
    }

    // ── Literal-length field (token high nibble + optional extension) ─────
    if literal_length >= RUN_MASK as usize {
        // Fill the literal-length nibble with its maximum value (0xF).
        *token = (RUN_MASK << ML_BITS) as u8;
        // Write 255-byte continuation bytes for the excess.
        let mut remaining = literal_length - RUN_MASK as usize;
        while remaining >= 255 {
            **op = 255u8;
            *op = (*op).add(1);
            remaining -= 255;
        }
        // Write the final remainder byte.
        **op = remaining as u8;
        *op = (*op).add(1);
    } else {
        // Length fits in the 4-bit nibble; shift it into place.
        *token = (literal_length << ML_BITS as usize) as u8;
    }

    // ── Copy literals ─────────────────────────────────────────────────────
    wild_copy8(*op, *anchor, (*op).add(literal_length));
    *op = (*op).add(literal_length);

    // ── Encode offset as little-endian 16-bit ────────────────────────────
    debug_assert!(offset > 0);
    debug_assert!(offset <= LZ4_DISTANCE_MAX as i32);
    write_le16(*op, offset as u16);
    *op = (*op).add(2);

    // ── Match-length field (token low nibble + optional extension) ────────
    debug_assert!(match_length >= MINMATCH as i32);
    // Subtract the implicit MINMATCH that is always present in LZ4 matches.
    let mut ml_remaining = (match_length as usize) - MINMATCH;

    // ── Output-limit check for match length ──────────────────────────────
    if limit == LimitedOutputDirective::LimitedOutput {
        let needed = ml_remaining / 255 + (1 + LASTLITERALS);
        if (*op).add(needed) > oend {
            return Err(Lz4HcError::OutputTooSmall);
        }
    }

    if ml_remaining >= ML_MASK as usize {
        *token += ML_MASK as u8;
        ml_remaining -= ML_MASK as usize;
        // Unrolled: emit two 255-byte extension bytes per iteration when
        // the remaining length is ≥ 510, halving branch-and-advance overhead.
        while ml_remaining >= 510 {
            **op = 255u8;
            *(*op).add(1) = 255u8;
            *op = (*op).add(2);
            ml_remaining -= 510;
        }
        if ml_remaining >= 255 {
            **op = 255u8;
            *op = (*op).add(1);
            ml_remaining -= 255;
        }
        **op = ml_remaining as u8;
        *op = (*op).add(1);
    } else {
        *token += ml_remaining as u8;
    }

    // ── Advance state for the next sequence ───────────────────────────────
    *ip = (*ip).add(match_length as usize);
    *anchor = *ip;

    Ok(())
}
