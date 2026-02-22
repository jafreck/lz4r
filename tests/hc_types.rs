// Unit tests for task-008: HC compression types, level table, hash functions,
// context initialisation.
//
// Tests verify behavioural parity with lz4hc.c / lz4hc.h v1.10.0 (lines 71–260):
//   - Compression-level constants (LZ4HC_CLEVEL_MIN, DEFAULT, OPT_MIN, MAX)
//   - Hash-table sizing constants (DICTIONARY_LOGSIZE, MAXD, HASH_LOG, HASHTABLESIZE, …)
//   - LZ4MID sizing constants (HASHSIZE, HASHLOG, HASHTABLESIZE)
//   - Other constants (OPTIMAL_ML, LZ4_OPT_NUM)
//   - DictCtxDirective and HcStrategy enum variants
//   - K_CL_TABLE: all 13 entries match the C source exactly
//   - get_clevel_params: clamping, boundary values, per-level lookups
//   - read64 / read_le64: correct unaligned reads
//   - hash_ptr: Knuth-multiplicative hash over 4 bytes
//   - mid_hash4 / mid_hash4_ptr: 4-byte LZ4MID hash
//   - mid_hash7 / mid_hash8_ptr: 7-byte (LE) LZ4MID hash
//   - nb_common_bytes32: trailing/leading zeros >> 3
//   - hc_count: delegates to block::types::count
//   - count_back: backward match extension
//   - HcCCtxInternal: new(), Default, field values
//   - clear_tables: zeroes hash_table, fills chain_table with 0xFFFF
//   - init_internal: offset computation, 1 GB threshold, 64 KB guard

use lz4::hc::types::{
    clear_tables, count_back, get_clevel_params, hc_count, hash_ptr, init_internal, mid_hash4,
    mid_hash4_ptr, mid_hash7, mid_hash8_ptr, nb_common_bytes32, read64, read_le64, CParams,
    DictCtxDirective, HcCCtxInternal, HcStrategy, K_CL_TABLE, LZ4HC_CLEVEL_DEFAULT,
    LZ4HC_CLEVEL_MAX, LZ4HC_CLEVEL_MIN, LZ4HC_CLEVEL_OPT_MIN, LZ4HC_DICTIONARY_LOGSIZE,
    LZ4HC_HASH_LOG, LZ4HC_HASH_MASK, LZ4HC_HASHTABLESIZE, LZ4HC_HASHSIZE, LZ4HC_MAXD,
    LZ4HC_MAXD_MASK, LZ4MID_HASHLOG, LZ4MID_HASHTABLESIZE, LZ4MID_HASHSIZE, LZ4_OPT_NUM,
    OPTIMAL_ML,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constants — compression level
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_clevel_min() {
    // lz4hc.h:47
    assert_eq!(LZ4HC_CLEVEL_MIN, 2);
}

#[test]
fn constant_clevel_default() {
    // lz4hc.h:48
    assert_eq!(LZ4HC_CLEVEL_DEFAULT, 9);
}

#[test]
fn constant_clevel_opt_min() {
    // lz4hc.h:49
    assert_eq!(LZ4HC_CLEVEL_OPT_MIN, 10);
}

#[test]
fn constant_clevel_max() {
    // lz4hc.h:50
    assert_eq!(LZ4HC_CLEVEL_MAX, 12);
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants — hash-table sizing
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_dictionary_logsize() {
    assert_eq!(LZ4HC_DICTIONARY_LOGSIZE, 16);
}

#[test]
fn constant_maxd() {
    // 1 << 16 == 65536
    assert_eq!(LZ4HC_MAXD, 65536);
}

#[test]
fn constant_maxd_mask() {
    // LZ4HC_MAXD - 1 == 65535
    assert_eq!(LZ4HC_MAXD_MASK, 65535);
}

#[test]
fn constant_hash_log() {
    assert_eq!(LZ4HC_HASH_LOG, 15);
}

#[test]
fn constant_hashtablesize() {
    // 1 << 15 == 32768
    assert_eq!(LZ4HC_HASHTABLESIZE, 32768);
}

#[test]
fn constant_hash_mask() {
    // LZ4HC_HASHTABLESIZE - 1 == 32767
    assert_eq!(LZ4HC_HASH_MASK, 32767u32);
}

#[test]
fn constant_hashsize() {
    assert_eq!(LZ4HC_HASHSIZE, 4);
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants — LZ4MID sizing
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_mid_hashsize() {
    assert_eq!(LZ4MID_HASHSIZE, 8);
}

#[test]
fn constant_mid_hashlog() {
    // LZ4HC_HASH_LOG - 1 == 14
    assert_eq!(LZ4MID_HASHLOG, 14);
}

#[test]
fn constant_mid_hashtablesize() {
    // 1 << 14 == 16384
    assert_eq!(LZ4MID_HASHTABLESIZE, 16384);
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants — other
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_optimal_ml() {
    // (ML_MASK - 1) + MINMATCH == (15 - 1) + 4 == 18
    assert_eq!(OPTIMAL_ML, 18);
}

#[test]
fn constant_lz4_opt_num() {
    // 1 << 12 == 4096
    assert_eq!(LZ4_OPT_NUM, 4096);
}

// ─────────────────────────────────────────────────────────────────────────────
// DictCtxDirective enum
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn dict_ctx_directive_variants_are_distinct() {
    assert_ne!(DictCtxDirective::NoDictCtx, DictCtxDirective::UsingDictCtxHc);
}

#[test]
fn dict_ctx_directive_eq_and_copy() {
    let a = DictCtxDirective::NoDictCtx;
    let b = a; // Copy
    assert_eq!(a, b);
    let c = DictCtxDirective::UsingDictCtxHc;
    let d = c;
    assert_eq!(c, d);
}

#[test]
fn dict_ctx_directive_debug_does_not_panic() {
    let _ = format!("{:?}", DictCtxDirective::NoDictCtx);
    let _ = format!("{:?}", DictCtxDirective::UsingDictCtxHc);
}

// ─────────────────────────────────────────────────────────────────────────────
// HcStrategy enum
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hc_strategy_discriminants() {
    // lz4hc.c:86 — lz4mid=0, lz4hc=1, lz4opt=2
    assert_eq!(HcStrategy::Lz4Mid as u32, 0);
    assert_eq!(HcStrategy::Lz4Hc as u32, 1);
    assert_eq!(HcStrategy::Lz4Opt as u32, 2);
}

#[test]
fn hc_strategy_eq_and_copy() {
    let s = HcStrategy::Lz4Hc;
    let t = s; // Copy
    assert_eq!(s, t);
}

#[test]
fn hc_strategy_all_variants_distinct() {
    assert_ne!(HcStrategy::Lz4Mid, HcStrategy::Lz4Hc);
    assert_ne!(HcStrategy::Lz4Hc, HcStrategy::Lz4Opt);
    assert_ne!(HcStrategy::Lz4Mid, HcStrategy::Lz4Opt);
}

#[test]
fn hc_strategy_debug_does_not_panic() {
    let _ = format!("{:?}", HcStrategy::Lz4Opt);
}

// ─────────────────────────────────────────────────────────────────────────────
// K_CL_TABLE — all 13 entries from lz4hc.c:92-106
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn k_cl_table_length() {
    // LZ4HC_CLEVEL_MAX + 1 == 13
    assert_eq!(K_CL_TABLE.len(), 13);
}

#[test]
fn k_cl_table_level0_unused() {
    let p = K_CL_TABLE[0];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Mid as u32);
    assert_eq!(p.nb_searches, 2);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level1_unused() {
    let p = K_CL_TABLE[1];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Mid as u32);
    assert_eq!(p.nb_searches, 2);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level2_lz4mid() {
    // LZ4HC_CLEVEL_MIN == 2 → Lz4Mid strategy
    let p = K_CL_TABLE[2];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Mid as u32);
    assert_eq!(p.nb_searches, 2);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level3_lz4hc_starts() {
    let p = K_CL_TABLE[3];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 4);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level4() {
    let p = K_CL_TABLE[4];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 8);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level5() {
    let p = K_CL_TABLE[5];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 16);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level6() {
    let p = K_CL_TABLE[6];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 32);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level7() {
    let p = K_CL_TABLE[7];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 64);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level8() {
    let p = K_CL_TABLE[8];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 128);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level9_default() {
    // LZ4HC_CLEVEL_DEFAULT == 9
    let p = K_CL_TABLE[9];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, 256);
    assert_eq!(p.target_length, 16);
}

#[test]
fn k_cl_table_level10_opt_min() {
    // LZ4HC_CLEVEL_OPT_MIN == 10
    let p = K_CL_TABLE[10];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Opt as u32);
    assert_eq!(p.nb_searches, 96);
    assert_eq!(p.target_length, 64);
}

#[test]
fn k_cl_table_level11() {
    let p = K_CL_TABLE[11];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Opt as u32);
    assert_eq!(p.nb_searches, 512);
    assert_eq!(p.target_length, 128);
}

#[test]
fn k_cl_table_level12_max() {
    // LZ4HC_CLEVEL_MAX == 12; target_length == LZ4_OPT_NUM == 4096
    let p = K_CL_TABLE[12];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Opt as u32);
    assert_eq!(p.nb_searches, 16384);
    assert_eq!(p.target_length, LZ4_OPT_NUM as u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// get_clevel_params — clamping and level lookup
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn get_clevel_params_zero_clamps_to_default() {
    // Level < 1 → LZ4HC_CLEVEL_DEFAULT (9)
    let p = get_clevel_params(0);
    let expected = K_CL_TABLE[LZ4HC_CLEVEL_DEFAULT as usize];
    assert_eq!(p.strat as u32, expected.strat as u32);
    assert_eq!(p.nb_searches, expected.nb_searches);
    assert_eq!(p.target_length, expected.target_length);
}

#[test]
fn get_clevel_params_negative_clamps_to_default() {
    let p = get_clevel_params(-5);
    let expected = K_CL_TABLE[LZ4HC_CLEVEL_DEFAULT as usize];
    assert_eq!(p.nb_searches, expected.nb_searches);
    assert_eq!(p.target_length, expected.target_length);
}

#[test]
fn get_clevel_params_level1_uses_table_index1() {
    // Level 1 is >= 1, not clamped below; clamped to min(1, 12) = 1
    let p = get_clevel_params(1);
    let expected = K_CL_TABLE[1];
    assert_eq!(p.nb_searches, expected.nb_searches);
    assert_eq!(p.target_length, expected.target_length);
}

#[test]
fn get_clevel_params_level9_default() {
    let p = get_clevel_params(9);
    let expected = K_CL_TABLE[9];
    assert_eq!(p.strat as u32, HcStrategy::Lz4Hc as u32);
    assert_eq!(p.nb_searches, expected.nb_searches);
    assert_eq!(p.target_length, expected.target_length);
}

#[test]
fn get_clevel_params_level12_max_boundary() {
    let p = get_clevel_params(12);
    let expected = K_CL_TABLE[12];
    assert_eq!(p.nb_searches, expected.nb_searches);
    assert_eq!(p.target_length, expected.target_length);
}

#[test]
fn get_clevel_params_above_max_clamps_to_12() {
    // Levels > 12 → clamped to 12
    let p_high = get_clevel_params(100);
    let p_12 = get_clevel_params(12);
    assert_eq!(p_high.strat as u32, p_12.strat as u32);
    assert_eq!(p_high.nb_searches, p_12.nb_searches);
    assert_eq!(p_high.target_length, p_12.target_length);
}

#[test]
fn get_clevel_params_level2_min_valid() {
    let p = get_clevel_params(2);
    assert_eq!(p.strat as u32, HcStrategy::Lz4Mid as u32);
    assert_eq!(p.nb_searches, 2);
    assert_eq!(p.target_length, 16);
}

#[test]
fn get_clevel_params_level10_opt_strategy() {
    let p = get_clevel_params(10);
    assert_eq!(p.strat as u32, HcStrategy::Lz4Opt as u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// read64 / read_le64
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn read64_reads_native_endian_u64() {
    let val: u64 = 0xDEAD_BEEF_1234_5678u64;
    let bytes = val.to_ne_bytes();
    let got = unsafe { read64(bytes.as_ptr()) };
    assert_eq!(got, val);
}

#[test]
fn read64_unaligned_access() {
    // Ensure no panic/UB on unaligned read (8 bytes in middle of 16-byte array).
    let mut buf = [0u8; 16];
    let val: u64 = 0x0102_0304_0506_0708u64;
    buf[1..9].copy_from_slice(&val.to_ne_bytes());
    let got = unsafe { read64(buf.as_ptr().add(1)) };
    assert_eq!(got, val);
}

#[test]
fn read_le64_little_endian_bytes() {
    // Bytes 01 02 03 04 05 06 07 08 in LE order represent 0x0807060504030201
    let buf: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let got = unsafe { read_le64(buf.as_ptr()) };
    assert_eq!(got, 0x0807_0605_0403_0201u64);
}

#[test]
fn read_le64_all_zeros() {
    let buf = [0u8; 8];
    let got = unsafe { read_le64(buf.as_ptr()) };
    assert_eq!(got, 0);
}

#[test]
fn read_le64_all_ones() {
    let buf = [0xFFu8; 8];
    let got = unsafe { read_le64(buf.as_ptr()) };
    assert_eq!(got, u64::MAX);
}

// ─────────────────────────────────────────────────────────────────────────────
// hash_ptr — 4-byte Knuth-multiplicative HC hash
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hash_ptr_result_fits_in_hash_log_bits() {
    // Result must be < LZ4HC_HASHTABLESIZE (2^15 == 32768)
    let buf = [0xDE, 0xAD, 0xBE, 0xEFu8];
    let h = unsafe { hash_ptr(buf.as_ptr()) };
    assert!(h < LZ4HC_HASHTABLESIZE as u32, "hash_ptr result out of range: {h}");
}

#[test]
fn hash_ptr_zero_input() {
    let buf = [0u8; 4];
    let h = unsafe { hash_ptr(buf.as_ptr()) };
    assert_eq!(h, 0);
}

#[test]
fn hash_ptr_deterministic() {
    let buf = [0x11, 0x22, 0x33, 0x44u8];
    let h1 = unsafe { hash_ptr(buf.as_ptr()) };
    let h2 = unsafe { hash_ptr(buf.as_ptr()) };
    assert_eq!(h1, h2);
}

#[test]
fn hash_ptr_different_inputs_usually_differ() {
    let buf1 = [0x01, 0x02, 0x03, 0x04u8];
    let buf2 = [0x11, 0x22, 0x33, 0x44u8];
    let h1 = unsafe { hash_ptr(buf1.as_ptr()) };
    let h2 = unsafe { hash_ptr(buf2.as_ptr()) };
    // Not a strict requirement (hash collisions exist), but for these specific
    // values the Knuth hash should produce distinct results.
    assert_ne!(h1, h2);
}

// ─────────────────────────────────────────────────────────────────────────────
// mid_hash4 / mid_hash4_ptr
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn mid_hash4_zero_is_zero() {
    assert_eq!(mid_hash4(0), 0);
}

#[test]
fn mid_hash4_result_fits_in_mid_hashlog_bits() {
    // Must be < LZ4MID_HASHTABLESIZE (2^14 == 16384)
    let h = mid_hash4(0xDEAD_BEEF);
    assert!(h < LZ4MID_HASHTABLESIZE as u32, "mid_hash4 out of range: {h}");
}

#[test]
fn mid_hash4_deterministic() {
    assert_eq!(mid_hash4(0x1234_5678), mid_hash4(0x1234_5678));
}

#[test]
fn mid_hash4_ptr_matches_mid_hash4() {
    // mid_hash4_ptr reads 4 bytes with read32, then calls mid_hash4.
    let buf = [0xAA, 0xBB, 0xCC, 0xDDu8];
    let v = u32::from_ne_bytes(buf);
    let expected = mid_hash4(v);
    let got = unsafe { mid_hash4_ptr(buf.as_ptr()) };
    assert_eq!(got, expected);
}

#[test]
fn mid_hash4_ptr_fits_in_range() {
    let buf = [0x01, 0x02, 0x03, 0x04u8];
    let h = unsafe { mid_hash4_ptr(buf.as_ptr()) };
    assert!(h < LZ4MID_HASHTABLESIZE as u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// mid_hash7 / mid_hash8_ptr
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn mid_hash7_zero_is_zero() {
    assert_eq!(mid_hash7(0), 0);
}

#[test]
fn mid_hash7_result_fits_in_mid_hashlog_bits() {
    let h = mid_hash7(0xDEAD_BEEF_1234_5678u64);
    assert!(h < LZ4MID_HASHTABLESIZE as u32, "mid_hash7 out of range: {h}");
}

#[test]
fn mid_hash7_deterministic() {
    let v = 0xABCD_EF01_2345_6789u64;
    assert_eq!(mid_hash7(v), mid_hash7(v));
}

#[test]
fn mid_hash8_ptr_fits_in_range() {
    let buf = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08u8];
    let h = unsafe { mid_hash8_ptr(buf.as_ptr()) };
    assert!(h < LZ4MID_HASHTABLESIZE as u32);
}

#[test]
fn mid_hash8_ptr_matches_mid_hash7_of_read_le64() {
    let buf = [0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34, 0x56, 0x78u8];
    let le_val = unsafe { read_le64(buf.as_ptr()) };
    let expected = mid_hash7(le_val);
    let got = unsafe { mid_hash8_ptr(buf.as_ptr()) };
    assert_eq!(got, expected);
}

// ─────────────────────────────────────────────────────────────────────────────
// nb_common_bytes32
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn nb_common_bytes32_differs_in_first_byte() {
    // On LE: difference in byte 0 → trailing_zeros() = 0..7 → >> 3 == 0
    // On BE: leading_zeros() of 0x01 (if bit 0 set) → 31 >> 3 == 3; but for 1 it's 0 common
    // We just test the LE path since that's the typical host.
    let val: u32 = 1; // bit 0 set → byte 0 differs on LE
    let common = nb_common_bytes32(val);
    #[cfg(target_endian = "little")]
    assert_eq!(common, 0);
    #[cfg(not(target_endian = "little"))]
    let _ = common;
}

#[test]
fn nb_common_bytes32_differs_in_second_byte() {
    // On LE: difference starts at byte 1 → trailing_zeros() = 8..15 → >> 3 == 1
    let val: u32 = 0x0000_0100; // byte 1 bit 0 set
    let common = nb_common_bytes32(val);
    #[cfg(target_endian = "little")]
    assert_eq!(common, 1);
    #[cfg(not(target_endian = "little"))]
    let _ = common;
}

#[test]
fn nb_common_bytes32_differs_in_third_byte() {
    // On LE: trailing_zeros() for 0x00010000 == 16 → >> 3 == 2
    let val: u32 = 0x0001_0000;
    let common = nb_common_bytes32(val);
    #[cfg(target_endian = "little")]
    assert_eq!(common, 2);
    #[cfg(not(target_endian = "little"))]
    let _ = common;
}

#[test]
fn nb_common_bytes32_differs_in_fourth_byte() {
    // On LE: trailing_zeros() for 0x01000000 == 24 → >> 3 == 3
    let val: u32 = 0x0100_0000;
    let common = nb_common_bytes32(val);
    #[cfg(target_endian = "little")]
    assert_eq!(common, 3);
    #[cfg(not(target_endian = "little"))]
    let _ = common;
}

// ─────────────────────────────────────────────────────────────────────────────
// hc_count — delegates to block::types::count
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hc_count_all_matching() {
    let data: [u8; 16] = [0x55u8; 16];
    let result = unsafe {
        let limit = data.as_ptr().add(data.len());
        hc_count(data.as_ptr(), data.as_ptr(), limit)
    };
    assert_eq!(result, 16);
}

#[test]
fn hc_count_zero_matching() {
    let p_in:    [u8; 8] = [0xAAu8; 8];
    let p_match: [u8; 8] = [0xBBu8; 8];
    let result = unsafe {
        let limit = p_in.as_ptr().add(p_in.len());
        hc_count(p_in.as_ptr(), p_match.as_ptr(), limit)
    };
    assert_eq!(result, 0);
}

#[test]
fn hc_count_partial_match() {
    let p_in:    [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    let p_match: [u8; 8] = [1, 2, 3, 4, 9, 9, 9, 9];
    let result = unsafe {
        let limit = p_in.as_ptr().add(p_in.len());
        hc_count(p_in.as_ptr(), p_match.as_ptr(), limit)
    };
    assert_eq!(result, 4);
}

// ─────────────────────────────────────────────────────────────────────────────
// count_back — backward match extension
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn count_back_no_common_bytes() {
    // ip[-1] != match[-1] → 0 common bytes backward
    let ip_buf:    [u8; 4] = [0xAA, 0xBB, 0xCC, 0xDD];
    let match_buf: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
    let result = unsafe {
        let ip    = ip_buf.as_ptr().add(4);
        let m     = match_buf.as_ptr().add(4);
        let i_min = ip_buf.as_ptr();
        let m_min = match_buf.as_ptr();
        count_back(ip, m, i_min, m_min)
    };
    assert_eq!(result, 0);
}

#[test]
fn count_back_one_common_byte() {
    // ip[-1] == match[-1] but ip[-2] != match[-2]
    let ip_buf:    [u8; 4] = [0x00, 0x00, 0x00, 0xAA];
    let match_buf: [u8; 4] = [0x00, 0x00, 0x11, 0xAA];
    let result = unsafe {
        let ip    = ip_buf.as_ptr().add(4);
        let m     = match_buf.as_ptr().add(4);
        let i_min = ip_buf.as_ptr();
        let m_min = match_buf.as_ptr();
        count_back(ip, m, i_min, m_min)
    };
    assert_eq!(result, -1);
}

#[test]
fn count_back_all_common() {
    // All 4 bytes match backward
    let data: [u8; 4] = [0x55u8; 4];
    let result = unsafe {
        let ip    = data.as_ptr().add(4);
        let m     = data.as_ptr().add(4);
        let i_min = data.as_ptr();
        let m_min = data.as_ptr();
        count_back(ip, m, i_min, m_min)
    };
    assert_eq!(result, -4);
}

#[test]
fn count_back_limited_by_i_min() {
    // ip can step back 2 bytes, match can step back 4 — i_min is the binding limit
    let ip_buf:    [u8; 4] = [0x55, 0x55, 0x55, 0x55];
    let match_buf: [u8; 8] = [0x55; 8];
    let result = unsafe {
        let ip    = ip_buf.as_ptr().add(4);
        let m     = match_buf.as_ptr().add(8);
        // i_min only lets us go back 2 bytes from ip
        let i_min = ip_buf.as_ptr().add(2);
        let m_min = match_buf.as_ptr();
        count_back(ip, m, i_min, m_min)
    };
    // Maximum backward extension constrained by i_min: -2
    assert_eq!(result, -2);
}

// ─────────────────────────────────────────────────────────────────────────────
// HcCCtxInternal — construction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hc_ctx_new_hash_table_zeroed() {
    let ctx = HcCCtxInternal::new();
    assert!(ctx.hash_table.iter().all(|&x| x == 0));
}

#[test]
fn hc_ctx_new_chain_table_zeroed() {
    // Note: chain_table starts at 0 from const fn; clear_tables sets it to 0xFFFF.
    // new() does NOT call clear_tables, so chain_table should be 0.
    let ctx = HcCCtxInternal::new();
    assert!(ctx.chain_table.iter().all(|&x| x == 0));
}

#[test]
fn hc_ctx_new_pointers_are_null() {
    let ctx = HcCCtxInternal::new();
    assert!(ctx.end.is_null());
    assert!(ctx.prefix_start.is_null());
    assert!(ctx.dict_start.is_null());
    assert!(ctx.dict_ctx.is_null());
}

#[test]
fn hc_ctx_new_numeric_fields_zeroed() {
    let ctx = HcCCtxInternal::new();
    assert_eq!(ctx.dict_limit, 0);
    assert_eq!(ctx.low_limit, 0);
    assert_eq!(ctx.next_to_update, 0);
    assert_eq!(ctx.compression_level, 0);
    assert_eq!(ctx.favor_dec_speed, 0);
    assert_eq!(ctx.dirty, 0);
}

#[test]
fn hc_ctx_default_equals_new() {
    let a = HcCCtxInternal::new();
    let b = HcCCtxInternal::default();
    assert_eq!(a.hash_table, b.hash_table);
    assert_eq!(a.chain_table, b.chain_table);
    assert_eq!(a.dict_limit, b.dict_limit);
    assert_eq!(a.low_limit, b.low_limit);
    assert_eq!(a.next_to_update, b.next_to_update);
    assert_eq!(a.compression_level, b.compression_level);
    assert_eq!(a.favor_dec_speed, b.favor_dec_speed);
    assert_eq!(a.dirty, b.dirty);
    assert_eq!(a.end, b.end);
    assert_eq!(a.prefix_start, b.prefix_start);
    assert_eq!(a.dict_start, b.dict_start);
    assert_eq!(a.dict_ctx, b.dict_ctx);
}

#[test]
fn hc_ctx_table_sizes_match_constants() {
    let ctx = HcCCtxInternal::new();
    assert_eq!(ctx.hash_table.len(), LZ4HC_HASHTABLESIZE);
    assert_eq!(ctx.chain_table.len(), LZ4HC_MAXD);
}

// ─────────────────────────────────────────────────────────────────────────────
// clear_tables
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn clear_tables_zeroes_hash_table() {
    let mut ctx = HcCCtxInternal::new();
    // Dirty it first
    ctx.hash_table[0]     = 0xDEAD_BEEF;
    ctx.hash_table[100]   = 0x1234_5678;
    ctx.hash_table[32767] = 0xFFFF_FFFF;
    clear_tables(&mut ctx);
    assert!(ctx.hash_table.iter().all(|&x| x == 0));
}

#[test]
fn clear_tables_fills_chain_table_with_0xffff() {
    let mut ctx = HcCCtxInternal::new();
    clear_tables(&mut ctx);
    assert!(ctx.chain_table.iter().all(|&x| x == 0xFFFFu16));
}

#[test]
fn clear_tables_idempotent() {
    let mut ctx = HcCCtxInternal::new();
    clear_tables(&mut ctx);
    clear_tables(&mut ctx);
    assert!(ctx.hash_table.iter().all(|&x| x == 0));
    assert!(ctx.chain_table.iter().all(|&x| x == 0xFFFFu16));
}

// ─────────────────────────────────────────────────────────────────────────────
// init_internal
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn init_internal_fresh_context_sets_64kb_guard() {
    // Fresh context: end == null, prefix_start == null → buffer_size == 0,
    // dict_limit == 0, new_starting_offset == 0 + 64*1024 == 65536.
    let mut ctx = HcCCtxInternal::new();
    let buf = [0u8; 16];
    unsafe { init_internal(&mut ctx, buf.as_ptr()) };
    assert_eq!(ctx.next_to_update, 65536);
    assert_eq!(ctx.dict_limit, 65536);
    assert_eq!(ctx.low_limit, 65536);
}

#[test]
fn init_internal_sets_prefix_start_and_end_to_start() {
    let mut ctx = HcCCtxInternal::new();
    let buf = [0u8; 16];
    let start = buf.as_ptr();
    unsafe { init_internal(&mut ctx, start) };
    assert_eq!(ctx.prefix_start, start);
    assert_eq!(ctx.end, start);
    assert_eq!(ctx.dict_start, start);
}

#[test]
fn init_internal_over_1gb_clears_tables_and_resets() {
    // Simulate a context whose computed offset would exceed 1 GB.
    let mut ctx = HcCCtxInternal::new();

    // Allocate two real heap buffers so the pointer arithmetic is valid.
    // buffer_size + dict_limit needs to exceed 1 GB.
    // We can't actually allocate 1 GB in a unit test, so we set dict_limit
    // to 0x40000001 (just over 1 GB) with a zero-length prefix window.
    //
    // buffer_size = 0 (end == prefix_start)
    // dict_limit = 0x40000001 → new_starting_offset = 0x40000001 > 1<<30
    ctx.dict_limit = (1usize << 30) as u32 + 1;

    // Dirty the tables so we can confirm they get cleared.
    ctx.hash_table[0] = 0xDEAD_BEEF;
    ctx.chain_table[0] = 0x1234;

    let buf = [0u8; 16];
    unsafe { init_internal(&mut ctx, buf.as_ptr()) };

    // After the 1-GB threshold, tables cleared and offset reset to 0 + 64KB.
    assert_eq!(ctx.next_to_update, 65536);
    assert_eq!(ctx.dict_limit, 65536);
    assert_eq!(ctx.low_limit, 65536);
    // hash_table cleared to 0
    assert!(ctx.hash_table.iter().all(|&x| x == 0));
    // chain_table filled with 0xFFFF
    assert!(ctx.chain_table.iter().all(|&x| x == 0xFFFFu16));
}

#[test]
fn init_internal_accumulates_offset_below_1gb() {
    // Simulate a context with a small buffer already in place.
    // buffer_size = 1000, dict_limit = 2000 → new_starting_offset = 3000,
    // which is << 1 GB, so no clear. Final offset = 3000 + 65536.
    let mut ctx = HcCCtxInternal::new();
    ctx.dict_limit = 2000;

    // Create a 1000-byte window: prefix_start + 1000 == end.
    let buf = vec![0u8; 2000];
    let prefix_start = buf.as_ptr();
    let end = unsafe { buf.as_ptr().add(1000) };
    ctx.prefix_start = prefix_start;
    ctx.end = end;

    let new_start = buf.as_ptr(); // arbitrary new start pointer
    unsafe { init_internal(&mut ctx, new_start) };

    assert_eq!(ctx.next_to_update, 3000 + 65536);
    assert_eq!(ctx.dict_limit, 3000 + 65536);
    assert_eq!(ctx.low_limit, 3000 + 65536);
}
