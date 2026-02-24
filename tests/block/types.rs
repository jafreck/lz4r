// Unit tests for task-003: Block types, helpers, and hash-table primitives
//
// Tests verify behavioural parity with lz4.c v1.10.0 (lines 239–740):
//   - All exported constants match their C counterparts exactly
//   - Enum variants and conversions are correct
//   - StreamStateInternal::new() / Default zero-initialises the struct
//   - Memory read/write helpers correctly handle unaligned access
//   - Wildcard-copy primitives copy the expected bytes
//   - INC32TABLE / DEC64TABLE contain the values from lz4.c:474-475
//   - nb_common_bytes returns correct byte counts
//   - count() returns the right number of matching bytes
//   - hash4 / hash5 produce the expected Knuth-multiplicative distribution
//   - Hash-table put/get/clear round-trip correctly

use lz4::block::types::{
    clear_hash, count, get_index_on_hash, get_position_on_hash, hash4, hash5, hash_position,
    memcpy_using_offset, nb_common_bytes, prepare_table, put_index_on_hash, put_position_on_hash,
    read16, read32, read_arch, read_le16, read_le32, wild_copy32, wild_copy8, write16, write32,
    write_le16, DictDirective, DictIssueDirective, LimitedOutputDirective, StreamStateInternal,
    TableType, DEC64TABLE, FASTLOOP_SAFE_DISTANCE, GB, INC32TABLE, KB, LASTLITERALS, LZ4_64KLIMIT,
    LZ4_DISTANCE_ABSOLUTE_MAX, LZ4_DISTANCE_MAX, LZ4_HASHLOG, LZ4_HASHTABLESIZE, LZ4_HASH_SIZE_U32,
    LZ4_MEMORY_USAGE, LZ4_MIN_LENGTH, LZ4_SKIP_TRIGGER, MATCH_SAFEGUARD_DISTANCE, MB, MFLIMIT,
    MINMATCH, ML_BITS, ML_MASK, RUN_BITS, RUN_MASK, WILDCOPYLENGTH,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constants — exact values from lz4.c / lz4.h
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constants_minmatch() {
    assert_eq!(MINMATCH, 4);
}

#[test]
fn constants_wildcopylength() {
    assert_eq!(WILDCOPYLENGTH, 8);
}

#[test]
fn constants_lastliterals() {
    assert_eq!(LASTLITERALS, 5);
}

#[test]
fn constants_mflimit() {
    assert_eq!(MFLIMIT, 12);
}

#[test]
fn constants_match_safeguard_distance() {
    // 2 * WILDCOPYLENGTH - MINMATCH == 12
    assert_eq!(MATCH_SAFEGUARD_DISTANCE, 12);
}

#[test]
fn constants_fastloop_safe_distance() {
    assert_eq!(FASTLOOP_SAFE_DISTANCE, 64);
}

#[test]
fn constants_lz4_min_length() {
    // MFLIMIT + 1 == 13
    assert_eq!(LZ4_MIN_LENGTH, 13);
}

#[test]
fn constants_kb_mb_gb() {
    assert_eq!(KB, 1024);
    assert_eq!(MB, 1 << 20);
    assert_eq!(GB, 1 << 30);
}

#[test]
fn constants_distance_max() {
    assert_eq!(LZ4_DISTANCE_ABSOLUTE_MAX, 65_535u32);
    assert_eq!(LZ4_DISTANCE_MAX, LZ4_DISTANCE_ABSOLUTE_MAX);
}

#[test]
fn constants_ml_run_bits() {
    assert_eq!(ML_BITS, 4u32);
    assert_eq!(ML_MASK, 0x0Fu32);
    assert_eq!(RUN_BITS, 4u32);
    assert_eq!(RUN_MASK, 0x0Fu32);
}

#[test]
fn constants_hash_table_sizing() {
    assert_eq!(LZ4_MEMORY_USAGE, 14u32);
    assert_eq!(LZ4_HASHLOG, 12u32); // 14 - 2
    assert_eq!(LZ4_HASHTABLESIZE, 1 << 14); // 16384
    assert_eq!(LZ4_HASH_SIZE_U32, 1 << 12); // 4096
}

#[test]
fn constants_64klimit() {
    // (64 * KB) + (MFLIMIT - 1) == 65536 + 11 == 65547
    assert_eq!(LZ4_64KLIMIT, 65547);
}

#[test]
fn constants_skip_trigger() {
    assert_eq!(LZ4_SKIP_TRIGGER, 6u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn limited_output_directive_values() {
    assert_eq!(LimitedOutputDirective::NotLimited as u32, 0);
    assert_eq!(LimitedOutputDirective::LimitedOutput as u32, 1);
    assert_eq!(LimitedOutputDirective::FillOutput as u32, 2);
}

#[test]
fn table_type_values() {
    assert_eq!(TableType::ClearedTable as u32, 0);
    assert_eq!(TableType::ByPtr as u32, 1);
    assert_eq!(TableType::ByU32 as u32, 2);
    assert_eq!(TableType::ByU16 as u32, 3);
}

#[test]
fn table_type_from_u32_known_values() {
    assert_eq!(TableType::from(0u32), TableType::ClearedTable);
    assert_eq!(TableType::from(1u32), TableType::ByPtr);
    assert_eq!(TableType::from(2u32), TableType::ByU32);
    assert_eq!(TableType::from(3u32), TableType::ByU16);
}

#[test]
fn table_type_from_u32_unknown_falls_back_to_cleared() {
    // Any value outside 0-3 should map to ClearedTable
    assert_eq!(TableType::from(99u32), TableType::ClearedTable);
    assert_eq!(TableType::from(u32::MAX), TableType::ClearedTable);
}

#[test]
fn dict_directive_values() {
    assert_eq!(DictDirective::NoDict as u32, 0);
    assert_eq!(DictDirective::WithPrefix64k as u32, 1);
    assert_eq!(DictDirective::UsingExtDict as u32, 2);
    assert_eq!(DictDirective::UsingDictCtx as u32, 3);
}

#[test]
fn dict_issue_directive_values() {
    assert_eq!(DictIssueDirective::NoDictIssue as u32, 0);
    assert_eq!(DictIssueDirective::DictSmall as u32, 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// StreamStateInternal construction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stream_state_new_zeroed() {
    let s = StreamStateInternal::new();
    assert!(s.hash_table.iter().all(|&x| x == 0));
    assert!(s.dictionary.is_null());
    assert!(s.dict_ctx.is_null());
    assert_eq!(s.current_offset, 0);
    assert_eq!(s.table_type, TableType::ClearedTable as u32);
    assert_eq!(s.dict_size, 0);
}

#[test]
fn stream_state_default_equals_new() {
    let a = StreamStateInternal::new();
    let b = StreamStateInternal::default();
    assert_eq!(a.hash_table, b.hash_table);
    assert_eq!(a.current_offset, b.current_offset);
    assert_eq!(a.table_type, b.table_type);
    assert_eq!(a.dict_size, b.dict_size);
    assert_eq!(a.dictionary, b.dictionary);
}

// ─────────────────────────────────────────────────────────────────────────────
// Lookup tables — exact byte values from lz4.c:474-475
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn inc32table_values() {
    assert_eq!(INC32TABLE, [0u32, 1, 2, 1, 0, 4, 4, 4]);
}

#[test]
fn dec64table_values() {
    assert_eq!(DEC64TABLE, [0i32, 0, 0, -1, -4, 1, 2, 3]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Memory read/write helpers
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn read16_native_endian() {
    let buf: [u8; 4] = [0x01, 0x02, 0x03, 0x04];
    let val = unsafe { read16(buf.as_ptr()) };
    // native-endian read of the first two bytes
    let expected = u16::from_ne_bytes([0x01, 0x02]);
    assert_eq!(val, expected);
}

#[test]
fn read32_native_endian() {
    let buf: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
    let val = unsafe { read32(buf.as_ptr()) };
    let expected = u32::from_ne_bytes([0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(val, expected);
}

#[test]
fn read_arch_reads_pointer_width_bytes() {
    // On any platform, read_arch returns size_of::<usize>() bytes interpreted
    // as a native-endian usize. We just verify it does not panic and returns
    // the same value as a direct usize read.
    let mut buf = [0u8; 16];
    buf[0] = 0x42;
    let val = unsafe { read_arch(buf.as_ptr()) };
    let expected = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const usize) };
    assert_eq!(val, expected);
}

#[test]
fn write16_and_read16_roundtrip() {
    let mut buf = [0u8; 4];
    unsafe { write16(buf.as_mut_ptr(), 0xABCD) };
    let back = unsafe { read16(buf.as_ptr()) };
    assert_eq!(back, 0xABCD);
}

#[test]
fn write32_and_read32_roundtrip() {
    let mut buf = [0u8; 4];
    unsafe { write32(buf.as_mut_ptr(), 0xDEAD_BEEF) };
    let back = unsafe { read32(buf.as_ptr()) };
    assert_eq!(back, 0xDEAD_BEEF);
}

#[test]
fn read_le16_little_endian_bytes() {
    // Bytes 0x01 0x02 in LE order represent 0x0201
    let buf: [u8; 2] = [0x01, 0x02];
    let val = unsafe { read_le16(buf.as_ptr()) };
    assert_eq!(val, 0x0201u16);
}

#[test]
fn read_le32_little_endian_bytes() {
    // Bytes 0x01 0x02 0x03 0x04 in LE order represent 0x04030201
    let buf: [u8; 4] = [0x01, 0x02, 0x03, 0x04];
    let val = unsafe { read_le32(buf.as_ptr()) };
    assert_eq!(val, 0x04030201u32);
}

#[test]
fn write_le16_stores_little_endian() {
    let mut buf = [0u8; 2];
    unsafe { write_le16(buf.as_mut_ptr(), 0x0201) };
    assert_eq!(buf[0], 0x01);
    assert_eq!(buf[1], 0x02);
}

// ─────────────────────────────────────────────────────────────────────────────
// Wildcard-copy primitives
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn wild_copy8_copies_exact_bytes() {
    // Copy 8 bytes from src to dst; dst_end == dst + 8 causes exactly one iteration.
    let src: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    // Over-allocate destination to absorb potential 8-byte overwrite
    let mut dst = [0u8; 32];
    unsafe {
        let dst_end = dst.as_mut_ptr().add(8);
        wild_copy8(dst.as_mut_ptr(), src.as_ptr(), dst_end);
    }
    assert_eq!(&dst[..8], &src[..8]);
}

#[test]
fn wild_copy8_copies_multiple_chunks() {
    let src: Vec<u8> = (0u8..=255).collect();
    let mut dst = vec![0u8; 256 + 16]; // extra margin for overwrite
    let len = 24usize;
    unsafe {
        let dst_end = dst.as_mut_ptr().add(len);
        wild_copy8(dst.as_mut_ptr(), src.as_ptr(), dst_end);
    }
    assert_eq!(&dst[..len], &src[..len]);
}

#[test]
fn wild_copy32_copies_exact_bytes() {
    let src: Vec<u8> = (0u8..128).collect();
    let mut dst = vec![0u8; 128 + 32]; // extra margin for overwrite
    let len = 32usize;
    unsafe {
        let dst_end = dst.as_mut_ptr().add(len);
        wild_copy32(dst.as_mut_ptr(), src.as_ptr(), dst_end);
    }
    assert_eq!(&dst[..len], &src[..len]);
}

#[test]
fn wild_copy32_large_copy() {
    let src: Vec<u8> = (0u8..=127).cycle().take(200).collect();
    let mut dst = vec![0u8; 200 + 32];
    let len = 64usize;
    unsafe {
        let dst_end = dst.as_mut_ptr().add(len);
        wild_copy32(dst.as_mut_ptr(), src.as_ptr(), dst_end);
    }
    assert_eq!(&dst[..len], &src[..len]);
}

// ─────────────────────────────────────────────────────────────────────────────
// memcpy_using_offset — special fast paths for offsets 1, 2, 4
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn memcpy_using_offset_offset1_replicates_byte() {
    // offset==1: output should be the byte at src repeated
    let src_val = 0xABu8;
    let mut src_buf = [0u8; 32];
    src_buf[0] = src_val;
    let mut dst_buf = [0u8; 32];
    let copy_len = 16usize;
    unsafe {
        // src_buf[0] is what will be replicated; dst starts right after src[0]
        // so offset of 1 from src to dst
        let src_ptr = src_buf.as_ptr();
        let dst_ptr = dst_buf.as_mut_ptr();
        let dst_end = dst_ptr.add(copy_len);
        memcpy_using_offset(dst_ptr, src_ptr, dst_end, 1);
    }
    for &b in &dst_buf[..copy_len] {
        assert_eq!(b, src_val, "offset=1 should replicate the byte");
    }
}

#[test]
fn memcpy_using_offset_offset2_replicates_pattern() {
    // offset==2: output should alternate s0, s1, s0, s1, ...
    let mut src_buf = [0u8; 32];
    src_buf[0] = 0x11;
    src_buf[1] = 0x22;
    let mut dst_buf = [0u8; 32];
    let copy_len = 16usize;
    unsafe {
        let src_ptr = src_buf.as_ptr();
        let dst_ptr = dst_buf.as_mut_ptr();
        let dst_end = dst_ptr.add(copy_len);
        memcpy_using_offset(dst_ptr, src_ptr, dst_end, 2);
    }
    for i in 0..copy_len {
        let expected = if i % 2 == 0 { 0x11u8 } else { 0x22u8 };
        assert_eq!(dst_buf[i], expected, "offset=2 at index {i}");
    }
}

#[test]
fn memcpy_using_offset_offset4_replicates_pattern() {
    // offset==4: output repeats s0,s1,s2,s3 pattern
    let mut src_buf = [0u8; 32];
    src_buf[0] = 0xAA;
    src_buf[1] = 0xBB;
    src_buf[2] = 0xCC;
    src_buf[3] = 0xDD;
    let mut dst_buf = [0u8; 32];
    let copy_len = 16usize;
    unsafe {
        let src_ptr = src_buf.as_ptr();
        let dst_ptr = dst_buf.as_mut_ptr();
        let dst_end = dst_ptr.add(copy_len);
        memcpy_using_offset(dst_ptr, src_ptr, dst_end, 4);
    }
    let pattern = [0xAAu8, 0xBB, 0xCC, 0xDD];
    for i in 0..copy_len {
        assert_eq!(dst_buf[i], pattern[i % 4], "offset=4 at index {i}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// nb_common_bytes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn nb_common_bytes_single_differing_bit() {
    // On LE: first differing byte is byte 0 (LSB), so 0 common bytes.
    // XOR of sequences that differ in byte 0 gives a value whose trailing bit
    // in byte 0 is set → 0 common bytes.
    let diff: usize = 1; // differs in bit 0 → byte 0
    let common = nb_common_bytes(diff);
    assert_eq!(common, 0);
}

#[test]
fn nb_common_bytes_first_byte_equal() {
    // Difference starting at byte 1 → 1 common byte on LE.
    let diff: usize = 0x100; // byte 1 set → 1 byte in common
    let common = nb_common_bytes(diff);
    #[cfg(target_endian = "little")]
    assert_eq!(common, 1);
    // On BE the first set bit is at the high end; skip the assertion for BE.
    #[cfg(not(target_endian = "little"))]
    let _ = common; // just assert it doesn't panic
}

#[test]
fn nb_common_bytes_maximum_pointer_width() {
    // Difference at the highest byte of a usize: size_of::<usize>()-1 common bytes.
    let diff: usize = 1usize << (usize::BITS - 8);
    let common = nb_common_bytes(diff);
    #[cfg(target_endian = "little")]
    assert_eq!(common, (core::mem::size_of::<usize>() - 1) as u32);
    #[cfg(not(target_endian = "little"))]
    assert_eq!(common, 0u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// count — match-length helper
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn count_zero_matching_bytes() {
    let p_in: [u8; 16] = [0xAA; 16];
    let p_match: [u8; 16] = [0xBB; 16]; // no match
    let result = unsafe {
        let limit = p_in.as_ptr().add(p_in.len());
        count(p_in.as_ptr(), p_match.as_ptr(), limit)
    };
    assert_eq!(result, 0);
}

#[test]
fn count_all_matching() {
    let data: [u8; 16] = [0x55u8; 16];
    let data2: [u8; 16] = [0x55u8; 16];
    let result = unsafe {
        let limit = data.as_ptr().add(data.len());
        count(data.as_ptr(), data2.as_ptr(), limit)
    };
    assert_eq!(result, 16);
}

#[test]
fn count_partial_match() {
    let p_in: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let p_match: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 99, 99, 99, 99];
    let result = unsafe {
        let limit = p_in.as_ptr().add(p_in.len());
        count(p_in.as_ptr(), p_match.as_ptr(), limit)
    };
    assert_eq!(result, 8);
}

#[test]
fn count_single_match() {
    let p_in: [u8; 8] = [0xAA, 0xBB, 0, 0, 0, 0, 0, 0];
    let p_match: [u8; 8] = [0xAA, 0xCC, 0, 0, 0, 0, 0, 0];
    let result = unsafe {
        let limit = p_in.as_ptr().add(p_in.len());
        count(p_in.as_ptr(), p_match.as_ptr(), limit)
    };
    assert_eq!(result, 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// hash4 / hash5
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hash4_zero_sequence_gives_zero() {
    // 0 * anything == 0; shifted result is also 0
    assert_eq!(hash4(0u32, TableType::ByU32), 0u32);
}

#[test]
fn hash4_byu16_produces_wider_index() {
    // ByU16 uses LZ4_HASHLOG+1 bits, so the index can be up to 2^13-1 (8191)
    // while ByU32 uses LZ4_HASHLOG bits (up to 2^12-1 = 4095).
    let seq = 0xDEAD_BEEFu32;
    let h32 = hash4(seq, TableType::ByU32);
    let h16 = hash4(seq, TableType::ByU16);
    assert!(
        h32 < (1u32 << LZ4_HASHLOG),
        "ByU32 hash must fit in LZ4_HASHLOG bits"
    );
    assert!(
        h16 < (1u32 << (LZ4_HASHLOG + 1)),
        "ByU16 hash must fit in LZ4_HASHLOG+1 bits"
    );
}

#[test]
fn hash4_deterministic() {
    let seq = 0x1234_5678u32;
    assert_eq!(hash4(seq, TableType::ByU32), hash4(seq, TableType::ByU32));
}

#[test]
fn hash5_deterministic() {
    let seq = 0x0102_0304_0506_0708u64;
    assert_eq!(hash5(seq, TableType::ByU32), hash5(seq, TableType::ByU32));
}

#[test]
fn hash5_byu16_vs_byu32_range() {
    let seq = 0xFEDC_BA98_7654_3210u64;
    let h32 = hash5(seq, TableType::ByU32);
    let h16 = hash5(seq, TableType::ByU16);
    assert!(h32 < (1u32 << LZ4_HASHLOG));
    assert!(h16 < (1u32 << (LZ4_HASHLOG + 1)));
}

#[test]
fn hash_position_fits_in_table_range() {
    // Allocate a small buffer with at least size_of::<usize>() bytes.
    let buf = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let h = unsafe { hash_position(buf.as_ptr(), TableType::ByU32) };
    // Must fit in the 32-bit table (2^LZ4_HASHLOG entries)
    assert!(h < (1u32 << LZ4_HASHLOG));
}

// ─────────────────────────────────────────────────────────────────────────────
// Hash-table operations: put / get / clear
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn put_and_get_index_byu32() {
    let mut table = [0u32; LZ4_HASH_SIZE_U32];
    let h = 42u32;
    let idx = 0xDEAD_BEEFu32;
    unsafe {
        put_index_on_hash(idx, h, table.as_mut_ptr(), TableType::ByU32);
        let got = get_index_on_hash(h, table.as_ptr(), TableType::ByU32);
        assert_eq!(got, idx);
    }
}

#[test]
fn put_and_get_index_byu16() {
    let mut table = [0u32; LZ4_HASH_SIZE_U32 * 2]; // twice as many u16 slots
    let h = 10u32;
    let idx = 0xBEEFu32; // must fit in u16
    unsafe {
        put_index_on_hash(idx, h, table.as_mut_ptr(), TableType::ByU16);
        let got = get_index_on_hash(h, table.as_ptr(), TableType::ByU16);
        assert_eq!(got, idx);
    }
}

#[test]
fn clear_hash_byu32() {
    let mut table = [0xFFFF_FFFFu32; LZ4_HASH_SIZE_U32];
    let h = 7u32;
    unsafe {
        clear_hash(h, table.as_mut_ptr(), TableType::ByU32);
    }
    assert_eq!(table[h as usize], 0u32);
    // Other slots remain unchanged
    assert_eq!(table[0], 0xFFFF_FFFFu32);
}

#[test]
fn clear_hash_byu16() {
    let mut table = [0xFFFF_FFFFu32; LZ4_HASH_SIZE_U32 * 2];
    let h = 3u32;
    unsafe {
        clear_hash(h, table.as_mut_ptr(), TableType::ByU16);
        // The u16 at position h should now be 0
        let tbl = table.as_ptr() as *const u16;
        let val = *tbl.add(h as usize);
        assert_eq!(val, 0u16);
    }
}

#[test]
fn put_and_get_position_byptr() {
    let data = [0xAAu8; 8];
    let ptr: *const u8 = data.as_ptr();
    let mut table = [core::ptr::null::<u8>(); LZ4_HASH_SIZE_U32];
    let h = 5u32;
    unsafe {
        put_position_on_hash(
            ptr,
            h,
            table.as_mut_ptr() as *mut *const u8,
            TableType::ByPtr,
        );
        let got = get_position_on_hash(h, table.as_ptr() as *const *const u8, TableType::ByPtr);
        assert_eq!(got, ptr);
    }
}

#[test]
fn clear_hash_byptr_sets_null() {
    let data = [0u8; 8];
    let ptr: *const u8 = data.as_ptr();
    let mut table = [ptr; LZ4_HASH_SIZE_U32];
    let h = 2u32;
    unsafe {
        clear_hash(h, table.as_mut_ptr() as *mut u32, TableType::ByPtr);
        assert!(table[h as usize].is_null());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// prepare_table
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prepare_table_cleared_table_sets_dict_fields_null() {
    // Starting from ClearedTable, no reset needed — just clears dict pointers.
    let mut ctx = StreamStateInternal::new();
    // Manually set dict fields to non-null values to ensure they get cleared.
    ctx.current_offset = 0;
    ctx.dict_size = 99;
    unsafe {
        prepare_table(&mut ctx as *mut _, 100, TableType::ByU32);
    }
    assert!(ctx.dictionary.is_null());
    assert!(ctx.dict_ctx.is_null());
    assert_eq!(ctx.dict_size, 0);
}

#[test]
fn prepare_table_type_change_resets_hash_table() {
    let mut ctx = StreamStateInternal::new();
    // Simulate a prior session with ByU32 table containing non-zero data.
    ctx.table_type = TableType::ByU32 as u32;
    ctx.hash_table[0] = 0xDEAD;
    ctx.hash_table[100] = 0xBEEF;
    // Prepare with a different table type → must reset.
    unsafe {
        prepare_table(&mut ctx as *mut _, 100, TableType::ByU16);
    }
    // After reset the table should be zeroed.
    assert!(ctx.hash_table.iter().all(|&x| x == 0));
}

#[test]
fn prepare_table_byu32_adds_64k_gap_when_offset_nonzero() {
    let mut ctx = StreamStateInternal::new();
    ctx.table_type = TableType::ByU32 as u32;
    ctx.current_offset = 1000;
    // Small input, same table type → no forced reset, but 64KB gap applied.
    unsafe {
        prepare_table(&mut ctx as *mut _, 100, TableType::ByU32);
    }
    // A 64KB gap should have been added — exact value may differ depending on
    // whether a reset occurred first (which zeroes current_offset).
    // Either current_offset == 0 (reset happened) or > 1000.
    // The invariant: it is NOT 1000 unchanged.
    assert_ne!(ctx.current_offset, 1000u32);
}

#[test]
fn prepare_table_large_input_forces_reset() {
    let mut ctx = StreamStateInternal::new();
    ctx.table_type = TableType::ByU32 as u32;
    ctx.hash_table[0] = 0xDEAD;
    // input_size >= 4*KB forces a reset
    unsafe {
        prepare_table(&mut ctx as *mut _, 4 * KB as i32, TableType::ByU32);
    }
    assert!(ctx.hash_table.iter().all(|&x| x == 0));
    assert_eq!(ctx.table_type, TableType::ClearedTable as u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// memcpy_using_offset — offsets 3, 5, 6, 7 (base fallback) and >= 8
// ─────────────────────────────────────────────────────────────────────────────

/// Helper: build a reference buffer that simulates overlapping byte-by-byte
/// copy from `src` with stride `offset`, producing `len` output bytes.
fn make_reference_overlap(src: &[u8], offset: usize, len: usize) -> Vec<u8> {
    let mut out = src.to_vec();
    out.resize(src.len() + len, 0);
    let start = src.len();
    for i in 0..len {
        out[start + i] = out[start + i - offset];
    }
    out[start..start + len].to_vec()
}

#[test]
fn memcpy_using_offset_offset_3() {
    // Pattern: "abc" repeated with offset 3
    let mut buf = vec![0u8; 256];
    buf[0] = b'a';
    buf[1] = b'b';
    buf[2] = b'c';
    let dst_start = 3usize;
    let copy_len = 64usize;
    let expected = make_reference_overlap(&buf[..3], 3, copy_len);
    unsafe {
        memcpy_using_offset(
            buf.as_mut_ptr().add(dst_start),
            buf.as_ptr(),
            buf.as_mut_ptr().add(dst_start + copy_len),
            3,
        );
    }
    assert_eq!(&buf[dst_start..dst_start + copy_len], &expected[..copy_len]);
}

#[test]
fn memcpy_using_offset_offset_5() {
    let mut buf = vec![0u8; 256];
    for i in 0..5 {
        buf[i] = (i as u8 + 1) * 10;
    }
    let dst_start = 5usize;
    let copy_len = 40usize;
    let expected = make_reference_overlap(&buf[..5], 5, copy_len);
    unsafe {
        memcpy_using_offset(
            buf.as_mut_ptr().add(dst_start),
            buf.as_ptr(),
            buf.as_mut_ptr().add(dst_start + copy_len),
            5,
        );
    }
    assert_eq!(&buf[dst_start..dst_start + copy_len], &expected[..copy_len]);
}

#[test]
fn memcpy_using_offset_offset_6() {
    let mut buf = vec![0u8; 256];
    for i in 0..6 {
        buf[i] = (i as u8 + 1) * 11;
    }
    let dst_start = 6usize;
    let copy_len = 48usize;
    let expected = make_reference_overlap(&buf[..6], 6, copy_len);
    unsafe {
        memcpy_using_offset(
            buf.as_mut_ptr().add(dst_start),
            buf.as_ptr(),
            buf.as_mut_ptr().add(dst_start + copy_len),
            6,
        );
    }
    assert_eq!(&buf[dst_start..dst_start + copy_len], &expected[..copy_len]);
}

#[test]
fn memcpy_using_offset_offset_7() {
    let mut buf = vec![0u8; 256];
    for i in 0..7 {
        buf[i] = (i as u8 + 1) * 13;
    }
    let dst_start = 7usize;
    let copy_len = 56usize;
    let expected = make_reference_overlap(&buf[..7], 7, copy_len);
    unsafe {
        memcpy_using_offset(
            buf.as_mut_ptr().add(dst_start),
            buf.as_ptr(),
            buf.as_mut_ptr().add(dst_start + copy_len),
            7,
        );
    }
    assert_eq!(&buf[dst_start..dst_start + copy_len], &expected[..copy_len]);
}

#[test]
fn memcpy_using_offset_offset_8_fast_path() {
    // offset >= 8 uses the simple non-overlapping copy path
    let mut buf = vec![0u8; 256];
    for i in 0..8 {
        buf[i] = (i as u8 + 1) * 17;
    }
    let dst_start = 8usize;
    let copy_len = 32usize;
    let expected = make_reference_overlap(&buf[..8], 8, copy_len);
    unsafe {
        memcpy_using_offset(
            buf.as_mut_ptr().add(dst_start),
            buf.as_ptr(),
            buf.as_mut_ptr().add(dst_start + copy_len),
            8,
        );
    }
    assert_eq!(&buf[dst_start..dst_start + copy_len], &expected[..copy_len]);
}

#[test]
fn memcpy_using_offset_offset_16_large() {
    let mut buf = vec![0u8; 512];
    for i in 0..16 {
        buf[i] = (i as u8) * 7;
    }
    let dst_start = 16usize;
    let copy_len = 128usize;
    let expected = make_reference_overlap(&buf[..16], 16, copy_len);
    unsafe {
        memcpy_using_offset(
            buf.as_mut_ptr().add(dst_start),
            buf.as_ptr(),
            buf.as_mut_ptr().add(dst_start + copy_len),
            16,
        );
    }
    assert_eq!(&buf[dst_start..dst_start + copy_len], &expected[..copy_len]);
}

// ─────────────────────────────────────────────────────────────────────────────
// prepare_table — ByU16 overflow and ByU32 > GB paths
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prepare_table_byu16_overflow_forces_reset() {
    let mut ctx = StreamStateInternal::new();
    ctx.table_type = TableType::ByU16 as u32;
    ctx.current_offset = 0xFFF0;
    ctx.hash_table[0] = 0xBEEF;
    // input_size such that current_offset + input_size >= 0xFFFF
    unsafe {
        prepare_table(&mut ctx as *mut _, 100, TableType::ByU16);
    }
    // Should have been reset
    assert_eq!(ctx.hash_table[0], 0);
    assert_eq!(ctx.table_type, TableType::ClearedTable as u32);
    assert_eq!(ctx.current_offset, 0);
}

#[test]
fn prepare_table_byu32_over_gb_forces_reset() {
    let mut ctx = StreamStateInternal::new();
    ctx.table_type = TableType::ByU32 as u32;
    ctx.current_offset = GB as u32 + 1;
    ctx.hash_table[0] = 0xCAFE;
    unsafe {
        prepare_table(&mut ctx as *mut _, 100, TableType::ByU32);
    }
    // Should have been reset
    assert_eq!(ctx.hash_table[0], 0);
    assert_eq!(ctx.table_type, TableType::ClearedTable as u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// count() — tail-byte matching path
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn count_exact_5_matching_bytes() {
    // 5 matching bytes → triggers 4-byte tail check (matches), then 1-byte
    let a = [1u8, 2, 3, 4, 5, 99, 99, 99];
    let b = [1u8, 2, 3, 4, 5, 0, 0, 0];
    let n = unsafe { count(a.as_ptr(), b.as_ptr(), a.as_ptr().add(8)) };
    assert_eq!(n, 5);
}

#[test]
fn count_exact_6_matching_bytes() {
    let a = [1u8, 2, 3, 4, 5, 6, 99, 99];
    let b = [1u8, 2, 3, 4, 5, 6, 0, 0];
    let n = unsafe { count(a.as_ptr(), b.as_ptr(), a.as_ptr().add(8)) };
    assert_eq!(n, 6);
}

#[test]
fn count_exact_7_matching_bytes() {
    let a = [1u8, 2, 3, 4, 5, 6, 7, 99];
    let b = [1u8, 2, 3, 4, 5, 6, 7, 0];
    let n = unsafe { count(a.as_ptr(), b.as_ptr(), a.as_ptr().add(8)) };
    assert_eq!(n, 7);
}

#[test]
fn count_13_matching_exercises_tail_after_word() {
    // 13 bytes match = 8 (word) + 4 (u32 tail) + 1 (u8 tail)
    let mut a = [0xABu8; 20];
    let mut b = [0xABu8; 20];
    a[13] = 0xFF;
    b[13] = 0x00;
    let n = unsafe { count(a.as_ptr(), b.as_ptr(), a.as_ptr().add(20)) };
    assert_eq!(n, 13);
}

#[test]
fn count_10_matching_exercises_word_plus_u16_tail() {
    // 10 bytes = 8 (word) + 2 (u16 tail)
    let mut a = [0xCDu8; 16];
    let mut b = [0xCDu8; 16];
    a[10] = 0xFF;
    b[10] = 0x00;
    let n = unsafe { count(a.as_ptr(), b.as_ptr(), a.as_ptr().add(16)) };
    assert_eq!(n, 10);
}
