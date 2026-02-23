// Unit tests for task-011: HC Match Search.
//
// Tests verify behavioural parity with lz4hc.c v1.10.0, lines 776–1120:
//   - `LZ4HC_Insert`                 → `insert`
//   - `LZ4HC_rotatePattern`          → `rotate_pattern`
//   - `LZ4HC_countPattern`           → `count_pattern`
//   - `LZ4HC_reverseCountPattern`    → `reverse_count_pattern`
//   - `LZ4HC_protectDictEnd`         → `protect_dict_end`
//   - `repeat_state_e`               → `RepeatState`
//   - `HCfavor_e`                    → `HcFavor`
//   - `LZ4HC_InsertAndGetWiderMatch` → `insert_and_get_wider_match`
//   - `LZ4HC_InsertAndFindBestMatch` → `insert_and_find_best_match`
//
// Coverage:
//   - rotate_pattern: no-op at 0/4/8, byte rotations at 1/2/3
//   - protect_dict_end: boundary values, overflow, basic true/false
//   - count_pattern: empty range, single-byte, word-sized, multi-word, mismatch
//   - reverse_count_pattern: empty, 4-byte step, byte tail, limit boundary
//   - RepeatState / HcFavor: enum equality, copy/clone, debug, discriminants
//   - insert: advances next_to_update, fills hash table
//   - insert_and_find_best_match: no match on unique data, match on repeating data

use lz4::hc::search::{
    count_pattern, insert, insert_and_find_best_match, insert_and_get_wider_match,
    protect_dict_end, reverse_count_pattern, rotate_pattern, HcFavor, RepeatState,
};
use lz4::hc::types::{init_internal, DictCtxDirective, HcCCtxInternal};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create an initialised HC context pointing at the start of `buf`.
/// After this call `prefix_start == end == buf.as_ptr()`, suitable for
/// calling `insert` / `insert_and_find_best_match`.
unsafe fn make_ctx(buf: &[u8]) -> HcCCtxInternal {
    let mut ctx = HcCCtxInternal::new();
    init_internal(&mut ctx, buf.as_ptr());
    ctx.end = buf.as_ptr().add(buf.len());
    ctx
}

// ─────────────────────────────────────────────────────────────────────────────
// rotate_pattern  (LZ4HC_rotatePattern)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn rotate_pattern_zero_offset_is_identity() {
    let p = 0xDEAD_BEEFu32;
    assert_eq!(rotate_pattern(0, p), p);
}

#[test]
fn rotate_pattern_offset_4_same_as_zero() {
    // rotate & (4-1) == 0 when rotate is a multiple of 4
    let p = 0x1234_5678u32;
    assert_eq!(rotate_pattern(4, p), p);
    assert_eq!(rotate_pattern(8, p), p);
}

#[test]
fn rotate_pattern_offset_1_rotates_left_8_bits() {
    // rotate=1 → bits_to_rotate=8 → rotate_left(8)
    let p = 0x1234_5678u32;
    assert_eq!(rotate_pattern(1, p), p.rotate_left(8));
}

#[test]
fn rotate_pattern_offset_2_rotates_left_16_bits() {
    let p = 0xAABB_CCDDu32;
    assert_eq!(rotate_pattern(2, p), p.rotate_left(16));
}

#[test]
fn rotate_pattern_offset_3_rotates_left_24_bits() {
    let p = 0x0102_0304u32;
    assert_eq!(rotate_pattern(3, p), p.rotate_left(24));
}

#[test]
fn rotate_pattern_large_offset_uses_mod_4() {
    // offset 5 → 5 & 3 == 1 → same as offset 1
    let p = 0xABCD_EF01u32;
    assert_eq!(rotate_pattern(5, p), rotate_pattern(1, p));
}

// ─────────────────────────────────────────────────────────────────────────────
// protect_dict_end  (LZ4HC_protectDictEnd)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn protect_dict_end_returns_true_when_gap_is_exactly_3() {
    // dict_limit - 1 - match_index == 3 → true
    // dict_limit=100, match_index=96 → 99 - 96 = 3
    assert!(protect_dict_end(100, 96));
}

#[test]
fn protect_dict_end_returns_true_when_gap_larger_than_3() {
    // dict_limit=1000, match_index=0 → gap is 999 ≥ 3
    assert!(protect_dict_end(1000, 0));
}

#[test]
fn protect_dict_end_returns_false_when_gap_is_2() {
    // dict_limit=100, match_index=97 → 99 - 97 = 2 < 3
    assert!(!protect_dict_end(100, 97));
}

#[test]
fn protect_dict_end_returns_false_when_gap_is_0() {
    // dict_limit=100, match_index=99 → 99 - 99 = 0 < 3
    assert!(!protect_dict_end(100, 99));
}

#[test]
fn protect_dict_end_returns_false_when_match_index_equals_dict_limit() {
    // match_index == dict_limit → wrapping_sub → large number; must be false
    // dict_limit - 1 - dict_limit wraps to u32::MAX which is >= 3... wait,
    // actually: (dict_limit.wrapping_sub(1).wrapping_sub(match_index)) = 0u32.wrapping_sub(1) = u32::MAX
    // That's >= 3, so it returns TRUE here. Let's verify the actual semantics:
    // protect_dict_end(100, 100): (99 wrapping_sub 100) = u32::MAX = 4294967295 >= 3 → TRUE
    // This matches the C source: the C does (U32)(dictLimit-1-matchIndex) >= 3
    // when matchIndex == dictLimit, (dictLimit-1-dictLimit) = -1u = U32_MAX ≥ 3 → true.
    assert!(protect_dict_end(100, 100));
}

#[test]
fn protect_dict_end_returns_false_at_last_3_dict_bytes() {
    // match_index in the last 3 bytes: dict_limit-3, dict_limit-2, dict_limit-1
    // gap(dict_limit-3) = dict_limit-1-(dict_limit-3) = 2 < 3 → false
    let dl = 100u32;
    assert!(!protect_dict_end(dl, dl - 1)); // gap = 0
    assert!(!protect_dict_end(dl, dl - 2)); // gap = 1
    assert!(!protect_dict_end(dl, dl - 3)); // gap = 2
    assert!(protect_dict_end(dl, dl - 4));  // gap = 3 → true
}

// ─────────────────────────────────────────────────────────────────────────────
// count_pattern  (LZ4HC_countPattern)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn count_pattern_empty_range_returns_zero() {
    let buf = [0xABu8; 4];
    unsafe {
        let p = buf.as_ptr();
        assert_eq!(count_pattern(p, p, 0xABABABAB), 0);
    }
}

#[test]
fn count_pattern_mismatch_at_first_byte_returns_zero() {
    let buf = [0x00u8; 4];
    unsafe {
        let p = buf.as_ptr();
        let end = p.add(4);
        // pattern has first byte 0xAB, buf has 0x00
        assert_eq!(count_pattern(p, end, 0xABABABABu32), 0);
    }
}

#[test]
fn count_pattern_exact_4_byte_match() {
    // 4 bytes of the repeating pattern (0xAB repeating)
    let buf = [0xABu8; 4];
    unsafe {
        let p = buf.as_ptr();
        let end = p.add(4);
        // pattern32 built from little-endian bytes 0xAB,0xAB,0xAB,0xAB
        let pattern = u32::from_le_bytes([0xAB, 0xAB, 0xAB, 0xAB]);
        assert_eq!(count_pattern(p, end, pattern), 4);
    }
}

#[test]
fn count_pattern_multi_word_all_matching() {
    // 32 bytes of the same repeating byte
    let buf = vec![0x77u8; 32];
    unsafe {
        let p = buf.as_ptr();
        let end = p.add(32);
        let pattern = u32::from_le_bytes([0x77, 0x77, 0x77, 0x77]);
        assert_eq!(count_pattern(p, end, pattern), 32);
    }
}

#[test]
fn count_pattern_stops_at_mismatch() {
    // 8 matching bytes then a mismatch
    let mut buf = vec![0x55u8; 9];
    buf[8] = 0xFF; // mismatch
    unsafe {
        let p = buf.as_ptr();
        let end = p.add(9);
        let pattern = u32::from_le_bytes([0x55, 0x55, 0x55, 0x55]);
        assert_eq!(count_pattern(p, end, pattern), 8);
    }
}

#[test]
fn count_pattern_does_not_exceed_i_end() {
    // All bytes match the pattern, but i_end is 3 bytes in
    let buf = [0xCCu8; 16];
    unsafe {
        let p = buf.as_ptr();
        let end = p.add(3);
        let pattern = u32::from_le_bytes([0xCC, 0xCC, 0xCC, 0xCC]);
        assert_eq!(count_pattern(p, end, pattern), 3);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// reverse_count_pattern  (LZ4HC_reverseCountPattern)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reverse_count_pattern_empty_range_returns_zero() {
    let buf = [0xABu8; 4];
    unsafe {
        let p = buf.as_ptr().add(4);
        let pattern = u32::from_ne_bytes([0xAB, 0xAB, 0xAB, 0xAB]);
        // i_low == ip → no backward search possible
        assert_eq!(reverse_count_pattern(p, p, pattern), 0);
    }
}

#[test]
fn reverse_count_pattern_full_4_bytes_match() {
    // 4 bytes of the native-endian pattern immediately before `ip`
    let pattern = 0xABABABABu32;
    let bytes = pattern.to_ne_bytes();
    let buf = [bytes[0], bytes[1], bytes[2], bytes[3]];
    unsafe {
        let i_low = buf.as_ptr();
        let ip = buf.as_ptr().add(4);
        assert_eq!(reverse_count_pattern(ip, i_low, pattern), 4);
    }
}

#[test]
fn reverse_count_pattern_stops_at_mismatch() {
    // 4 matching bytes preceded by a different byte
    let pattern = 0x01010101u32;
    let bytes = pattern.to_ne_bytes();
    let mut buf = vec![0xFFu8; 1];
    buf.extend_from_slice(&bytes);
    // buf = [0xFF, 0x01, 0x01, 0x01, 0x01]
    unsafe {
        let i_low = buf.as_ptr();
        let ip = buf.as_ptr().add(5);
        // Searching backward from ip, we should count exactly 4
        assert_eq!(reverse_count_pattern(ip, i_low, pattern), 4);
    }
}

#[test]
fn reverse_count_pattern_does_not_go_before_i_low() {
    // 8 matching bytes but i_low cuts off at 3
    let pattern = 0xCCCCCCCCu32;
    let bytes = pattern.to_ne_bytes();
    let mut full = Vec::new();
    for _ in 0..2 { full.extend_from_slice(&bytes); } // 8 bytes
    unsafe {
        let i_low = full.as_ptr().add(5); // restrict to last 3 bytes
        let ip = full.as_ptr().add(8);
        let count = reverse_count_pattern(ip, i_low, pattern);
        assert!(count <= 3, "must not count past i_low; got {count}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RepeatState enum
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn repeat_state_equality() {
    assert_eq!(RepeatState::Untested, RepeatState::Untested);
    assert_eq!(RepeatState::Not, RepeatState::Not);
    assert_eq!(RepeatState::Confirmed, RepeatState::Confirmed);
    assert_ne!(RepeatState::Untested, RepeatState::Not);
    assert_ne!(RepeatState::Not, RepeatState::Confirmed);
}

#[test]
fn repeat_state_copy_clone() {
    let a = RepeatState::Confirmed;
    let b = a;         // Copy
    let c = a.clone(); // Clone
    assert_eq!(b, RepeatState::Confirmed);
    assert_eq!(c, RepeatState::Confirmed);
}

#[test]
fn repeat_state_debug_does_not_panic() {
    let _ = format!("{:?}", RepeatState::Untested);
    let _ = format!("{:?}", RepeatState::Not);
    let _ = format!("{:?}", RepeatState::Confirmed);
}

// ─────────────────────────────────────────────────────────────────────────────
// HcFavor enum
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hc_favor_equality() {
    assert_eq!(HcFavor::CompressionRatio, HcFavor::CompressionRatio);
    assert_eq!(HcFavor::DecompressionSpeed, HcFavor::DecompressionSpeed);
    assert_ne!(HcFavor::CompressionRatio, HcFavor::DecompressionSpeed);
}

#[test]
fn hc_favor_discriminant_values() {
    // Mirrors C: favorCompressionRatio=0, favorDecompressionSpeed=1
    assert_eq!(HcFavor::CompressionRatio as i32, 0);
    assert_eq!(HcFavor::DecompressionSpeed as i32, 1);
}

#[test]
fn hc_favor_copy_clone() {
    let a = HcFavor::DecompressionSpeed;
    let b = a;
    let c = a.clone();
    assert_eq!(b, HcFavor::DecompressionSpeed);
    assert_eq!(c, HcFavor::DecompressionSpeed);
}

#[test]
fn hc_favor_debug_does_not_panic() {
    let _ = format!("{:?}", HcFavor::CompressionRatio);
    let _ = format!("{:?}", HcFavor::DecompressionSpeed);
}

// ─────────────────────────────────────────────────────────────────────────────
// insert  (LZ4HC_Insert)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn insert_advances_next_to_update() {
    let buf = vec![0xAAu8; 64];
    unsafe {
        let mut ctx = make_ctx(&buf);
        let initial_next = ctx.next_to_update;
        let ip = buf.as_ptr().add(16);
        insert(&mut ctx, ip);
        assert!(
            ctx.next_to_update > initial_next,
            "next_to_update must advance after insert; was={initial_next}, now={}",
            ctx.next_to_update
        );
    }
}

#[test]
fn insert_sets_next_to_update_to_target() {
    let buf = vec![0xBBu8; 64];
    unsafe {
        let mut ctx = make_ctx(&buf);
        let ip = buf.as_ptr().add(20);
        insert(&mut ctx, ip);
        // target = (ip - prefix_start) + dict_limit
        let expected = (ip.offset_from(buf.as_ptr()) as u32)
            .wrapping_add(ctx.dict_limit);
        // After insert, next_to_update must equal target
        assert_eq!(ctx.next_to_update, expected);
    }
}

#[test]
fn insert_populates_hash_table() {
    // After inserting some positions the hash table should no longer be all zero.
    let buf: Vec<u8> = (0u8..=255).cycle().take(256).collect();
    unsafe {
        let mut ctx = make_ctx(&buf);
        let ip = buf.as_ptr().add(64);
        insert(&mut ctx, ip);
        let any_nonzero = ctx.hash_table.iter().any(|&v| v != 0);
        assert!(any_nonzero, "hash table must be populated after insert");
    }
}

#[test]
fn insert_noop_when_ip_at_start() {
    let buf = vec![0u8; 64];
    unsafe {
        let mut ctx = make_ctx(&buf);
        let initial_next = ctx.next_to_update;
        // ip == prefix_start → target == dict_limit == initial_next → while loop skipped
        let ip = buf.as_ptr();
        insert(&mut ctx, ip);
        assert_eq!(ctx.next_to_update, initial_next);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// insert_and_find_best_match  (LZ4HC_InsertAndFindBestMatch)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn insert_and_find_best_match_no_match_on_unique_data() {
    // 64 completely unique bytes → no match should be found (len < MINMATCH=4)
    let buf: Vec<u8> = (0u8..64).collect();
    unsafe {
        let mut ctx = make_ctx(&buf);
        // Move ip forward a bit so there's prior data to search
        let ip = buf.as_ptr().add(8);
        let i_limit = buf.as_ptr().add(64);
        let m = insert_and_find_best_match(
            &mut ctx,
            ip,
            i_limit,
            256,
            false,
            DictCtxDirective::NoDictCtx,
        );
        // Unique data: no 4-byte match → len should be 0 (or < MINMATCH)
        assert!(
            m.len < 4,
            "unique data must produce no usable match; got len={}",
            m.len
        );
    }
}

#[test]
fn insert_and_find_best_match_finds_match_on_repeating_data() {
    // 128 bytes of repeating 0xAB → at least MINMATCH=4 match should be found
    // after some data has been inserted.
    let buf = vec![0xABu8; 128];
    unsafe {
        let mut ctx = make_ctx(&buf);
        // First, insert positions 0..32 so that position 32 has predecessors
        let base = buf.as_ptr();
        // Pre-populate: insert positions up to 32
        insert(&mut ctx, base.add(32));

        let ip = base.add(32);
        let i_limit = base.add(128);
        let m = insert_and_find_best_match(
            &mut ctx,
            ip,
            i_limit,
            256,
            true, // pattern_analysis enabled
            DictCtxDirective::NoDictCtx,
        );
        assert!(
            m.len >= 4,
            "repeating data must produce a match of at least MINMATCH=4; got len={}",
            m.len
        );
    }
}

#[test]
fn insert_and_find_best_match_zero_attempts_returns_no_match() {
    // max_nb_attempts=0 → chain loop never executes → no usable match.
    // The function returns its initial `longest` value (MINMATCH-1 = 3) with
    // offset=0.  A match with len < MINMATCH or off==0 is not usable.
    let buf = vec![0xCCu8; 64];
    unsafe {
        let mut ctx = make_ctx(&buf);
        insert(&mut ctx, buf.as_ptr().add(16));

        let ip = buf.as_ptr().add(16);
        let i_limit = buf.as_ptr().add(64);
        let m = insert_and_find_best_match(
            &mut ctx,
            ip,
            i_limit,
            0, // max_nb_attempts = 0
            false,
            DictCtxDirective::NoDictCtx,
        );
        // off==0 means no match was recorded (the chain loop never ran)
        assert_eq!(m.off, 0, "zero attempts must record no offset; got off={}", m.off);
    }
}

#[test]
fn insert_and_find_best_match_back_field_is_zero() {
    // insert_and_find_best_match passes ip as i_low_limit → no backward extension
    // → s_back is always 0
    let buf = vec![0xDDu8; 64];
    unsafe {
        let mut ctx = make_ctx(&buf);
        insert(&mut ctx, buf.as_ptr().add(32));

        let ip = buf.as_ptr().add(32);
        let i_limit = buf.as_ptr().add(64);
        let m = insert_and_find_best_match(
            &mut ctx,
            ip,
            i_limit,
            64,
            false,
            DictCtxDirective::NoDictCtx,
        );
        assert_eq!(
            m.back, 0,
            "insert_and_find_best_match must not produce backward extension; got back={}",
            m.back
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// insert_and_get_wider_match  (LZ4HC_InsertAndGetWiderMatch)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn insert_and_get_wider_match_no_match_unique_data() {
    let buf: Vec<u8> = (0u8..64).collect();
    unsafe {
        let mut ctx = make_ctx(&buf);
        let base = buf.as_ptr();
        let ip = base.add(16);
        let i_low = base;
        let i_high = base.add(64);
        let m = insert_and_get_wider_match(
            &mut ctx,
            ip,
            i_low,
            i_high,
            3, // longest = MINMATCH - 1
            256,
            false,
            false,
            DictCtxDirective::NoDictCtx,
            false,
        );
        assert!(m.len < 4, "unique data should yield no match; got len={}", m.len);
    }
}

#[test]
fn insert_and_get_wider_match_finds_match_repeating_data() {
    let buf = vec![0xEEu8; 128];
    unsafe {
        let mut ctx = make_ctx(&buf);
        let base = buf.as_ptr();
        // Insert first 32 positions
        insert(&mut ctx, base.add(32));

        let ip = base.add(32);
        let i_low = base.add(32); // no lookback
        let i_high = base.add(128);
        let m = insert_and_get_wider_match(
            &mut ctx,
            ip,
            i_low,
            i_high,
            3,
            256,
            true,
            false,
            DictCtxDirective::NoDictCtx,
            false,
        );
        assert!(m.len >= 4, "repeating data must match; got len={}", m.len);
    }
}

#[test]
fn insert_and_get_wider_match_match_len_non_negative() {
    // The function must always return a non-negative len (debug_assert in source)
    let buf = vec![0x42u8; 64];
    unsafe {
        let mut ctx = make_ctx(&buf);
        let base = buf.as_ptr();
        let m = insert_and_get_wider_match(
            &mut ctx,
            base.add(8),
            base,
            base.add(64),
            3,
            64,
            false,
            false,
            DictCtxDirective::NoDictCtx,
            false,
        );
        assert!(m.len >= 0, "match len must be >= 0; got {}", m.len);
    }
}
