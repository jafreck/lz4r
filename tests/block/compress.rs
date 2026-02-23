// Unit tests for task-004: Block compression core and one-shot API
//
// Tests verify behavioural parity with lz4.c v1.10.0 (lines 924–1524):
//   - compress_bound() returns correct worst-case sizes
//   - compress_default() / compress_fast() compress data correctly
//   - compress_dest_size() fills the output buffer exactly
//   - Error paths return Err(Lz4Error::OutputTooSmall) / Err(Lz4Error::InputTooLarge)
//   - compress_generic() handles empty/zero-size inputs (single 0x00 token)
//   - Acceleration clamping (< DEFAULT → DEFAULT, > MAX → MAX)
//   - Constants match C counterparts exactly

use lz4::block::compress::{
    compress_bound, compress_default, compress_dest_size, compress_dest_size_ext_state,
    compress_fast, compress_fast_ext_state, compress_fast_ext_state_fast_reset, Lz4Error,
    LZ4_ACCELERATION_DEFAULT, LZ4_ACCELERATION_MAX, LZ4_MAX_INPUT_SIZE,
};
use lz4::block::types::StreamStateInternal;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_max_input_size() {
    // Matches LZ4_MAX_INPUT_SIZE == 0x7E000000
    assert_eq!(LZ4_MAX_INPUT_SIZE, 0x7E00_0000u32);
}

#[test]
fn constant_acceleration_default() {
    assert_eq!(LZ4_ACCELERATION_DEFAULT, 1i32);
}

#[test]
fn constant_acceleration_max() {
    assert_eq!(LZ4_ACCELERATION_MAX, 65_537i32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4Error — debug / copy / equality properties
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4_error_eq() {
    assert_eq!(Lz4Error::OutputTooSmall, Lz4Error::OutputTooSmall);
    assert_eq!(Lz4Error::InputTooLarge, Lz4Error::InputTooLarge);
    assert_ne!(Lz4Error::OutputTooSmall, Lz4Error::InputTooLarge);
}

#[test]
fn lz4_error_clone() {
    let e = Lz4Error::OutputTooSmall;
    assert_eq!(e, e);
}

#[test]
fn lz4_error_debug_does_not_panic() {
    let _ = format!("{:?}", Lz4Error::OutputTooSmall);
    let _ = format!("{:?}", Lz4Error::InputTooLarge);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_bound — worst-case size calculation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_bound_zero_input() {
    // input_size == 0: formula is 0 + 0/255 + 16 == 16
    assert_eq!(compress_bound(0), 16);
}

#[test]
fn compress_bound_one_byte() {
    // 1 + 0 + 16 == 17
    assert_eq!(compress_bound(1), 17);
}

#[test]
fn compress_bound_255_bytes() {
    // 255 + 1 + 16 == 272
    assert_eq!(compress_bound(255), 272);
}

#[test]
fn compress_bound_1000_bytes() {
    // 1000 + 3 + 16 == 1019
    assert_eq!(compress_bound(1000), 1019);
}

#[test]
fn compress_bound_exceeds_max_returns_zero() {
    // Input larger than LZ4_MAX_INPUT_SIZE must return 0
    assert_eq!(compress_bound(LZ4_MAX_INPUT_SIZE as i32 + 1), 0);
}

#[test]
fn compress_bound_negative_returns_zero() {
    assert_eq!(compress_bound(-1), 0);
    assert_eq!(compress_bound(i32::MIN), 0);
}

#[test]
fn compress_bound_at_max_input_size_nonzero() {
    // At exactly LZ4_MAX_INPUT_SIZE the formula should give a positive value.
    let bound = compress_bound(LZ4_MAX_INPUT_SIZE as i32);
    assert!(bound > 0, "compress_bound at max input size should be > 0");
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_default — basic happy-path compression
// ─────────────────────────────────────────────────────────────────────────────

/// Allocate a worst-case destination buffer for `src.len()` bytes.
fn make_dst(src_len: usize) -> Vec<u8> {
    let bound = compress_bound(src_len as i32).max(0) as usize;
    vec![0u8; bound]
}

#[test]
fn compress_default_single_byte() {
    let src = [0x42u8];
    let mut dst = make_dst(src.len());
    let result = compress_default(&src, &mut dst);
    assert!(result.is_ok(), "single-byte compression should succeed");
    let n = result.unwrap();
    assert!(n > 0, "compressed size should be > 0");
}

#[test]
fn compress_default_all_zeros_compresses_well() {
    // Highly compressible: 1 KB of zeros
    let src = vec![0u8; 1024];
    let mut dst = make_dst(src.len());
    let result = compress_default(&src, &mut dst);
    assert!(result.is_ok());
    let n = result.unwrap();
    // Compressed size of 1024 zeros should be much smaller than 1024
    assert!(
        n < src.len(),
        "zeros should compress to less than input size"
    );
}

#[test]
fn compress_default_highly_compressible_data() {
    // Repeating pattern — very compressible
    let src: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let mut dst = make_dst(src.len());
    let result = compress_default(&src, &mut dst);
    assert!(result.is_ok());
    let n = result.unwrap();
    assert!(n < src.len(), "repeating data should compress");
}

#[test]
fn compress_default_output_size_within_bound() {
    let src = b"hello, world! this is a test of lz4 compression";
    let mut dst = make_dst(src.len());
    let result = compress_default(src, &mut dst);
    assert!(result.is_ok());
    let n = result.unwrap();
    let bound = compress_bound(src.len() as i32) as usize;
    assert!(n <= bound, "compressed size must not exceed compress_bound");
}

#[test]
fn compress_default_small_input_below_min_length() {
    // Inputs below LZ4_MIN_LENGTH (13) are stored verbatim as literals.
    let src = b"hello";
    let mut dst = make_dst(src.len());
    let result = compress_default(src, &mut dst);
    assert!(result.is_ok(), "small inputs should succeed");
    let n = result.unwrap();
    assert!(n > 0);
}

#[test]
fn compress_default_large_input() {
    // 65 KB input — exercises the ByU32/ByPtr table type path (src >= LZ4_64KLIMIT).
    let src: Vec<u8> = (0u8..=255).cycle().take(65_600).collect();
    let mut dst = make_dst(src.len());
    let result = compress_default(&src, &mut dst);
    assert!(result.is_ok(), "65 KB compression should succeed");
    let n = result.unwrap();
    assert!(n > 0);
    assert!(n <= compress_bound(src.len() as i32) as usize);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_fast — acceleration parameter
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_acceleration_1_matches_compress_default() {
    let src = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut dst1 = make_dst(src.len());
    let mut dst2 = make_dst(src.len());
    let r1 = compress_default(src, &mut dst1).unwrap();
    let r2 = compress_fast(src, &mut dst2, 1).unwrap();
    // Same acceleration → same output
    assert_eq!(r1, r2);
    assert_eq!(&dst1[..r1], &dst2[..r2]);
}

#[test]
fn compress_fast_high_acceleration_succeeds() {
    let src: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let mut dst = make_dst(src.len());
    let result = compress_fast(&src, &mut dst, LZ4_ACCELERATION_MAX);
    assert!(result.is_ok(), "high acceleration should succeed");
}

#[test]
fn compress_fast_below_default_clamped_to_default() {
    // Acceleration < DEFAULT is clamped to DEFAULT; both calls should produce identical output.
    let src = b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz";
    let mut dst_clamped = make_dst(src.len());
    let mut dst_default = make_dst(src.len());
    let r1 = compress_fast(src, &mut dst_clamped, 0).unwrap(); // 0 < DEFAULT → clamped
    let r2 = compress_fast(src, &mut dst_default, 1).unwrap(); // == DEFAULT
    assert_eq!(r1, r2, "acceleration 0 should be clamped to 1");
    assert_eq!(&dst_clamped[..r1], &dst_default[..r2]);
}

#[test]
fn compress_fast_above_max_clamped_to_max() {
    let src: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
    let mut dst_clamped = make_dst(src.len());
    let mut dst_max = make_dst(src.len());
    let r1 = compress_fast(&src, &mut dst_clamped, i32::MAX).unwrap();
    let r2 = compress_fast(&src, &mut dst_max, LZ4_ACCELERATION_MAX).unwrap();
    assert_eq!(r1, r2, "acceleration above MAX should be clamped to MAX");
    assert_eq!(&dst_clamped[..r1], &dst_max[..r2]);
}

#[test]
fn compress_fast_input_too_large_error() {
    // Input larger than LZ4_MAX_INPUT_SIZE must return InputTooLarge.
    let huge = vec![0u8; LZ4_MAX_INPUT_SIZE as usize + 1];
    let mut dst = vec![0u8; 64];
    let result = compress_fast(&huge, &mut dst, 1);
    assert_eq!(result, Err(Lz4Error::InputTooLarge));
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_default — output-too-small error
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_default_output_too_small_returns_error() {
    // Provide a 1-byte output buffer for non-trivial input.
    let src = b"hello world, this is a longer string that cannot fit in 1 byte";
    let mut dst = [0u8; 1];
    let result = compress_default(src, &mut dst);
    assert_eq!(result, Err(Lz4Error::OutputTooSmall));
}

#[test]
fn compress_fast_output_too_small_returns_error() {
    let src = b"The quick brown fox jumps over the lazy dog.";
    let mut dst = [0u8; 4];
    let result = compress_fast(src, &mut dst, 1);
    assert_eq!(result, Err(Lz4Error::OutputTooSmall));
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_generic — empty / zero-size input behaviour
// (Validated via the safe wrappers: compress_default on an empty slice goes
//  through compress_fast which calls compress_fast_ext_state, which calls
//  compress_generic. The C source emits a single 0x00 token byte.)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_ext_state_zero_src_emits_one_zero_byte() {
    // LZ4_compress_generic with srcSize==0 emits a single 0x00 token.
    // We exercise this path via the unsafe ext_state function.
    let mut state = StreamStateInternal::new();
    let src = [0u8; 0];
    let mut dst = [0u8; 16];
    let result = unsafe {
        compress_fast_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            0i32,
            dst.as_mut_ptr(),
            dst.len() as i32,
            1,
        )
    };
    assert!(result.is_ok(), "zero-size compress should succeed");
    let n = result.unwrap();
    assert_eq!(n, 1, "empty input should produce exactly 1 byte");
    assert_eq!(dst[0], 0x00, "the single token byte must be 0x00");
}

#[test]
fn compress_fast_ext_state_zero_output_capacity_returns_zero() {
    // With dst_capacity <= 0 and output limited, the C code returns 0.
    let mut state = StreamStateInternal::new();
    let src = [0u8; 0];
    let mut dst = [0u8; 1];
    let result = unsafe {
        compress_fast_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            0i32,
            dst.as_mut_ptr(),
            0i32, // dst_capacity == 0
            1,
        )
    };
    // C returns 0 for "no room" in LimitedOutput mode
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_fast_ext_state — external state management
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_ext_state_produces_valid_compressed_output() {
    let src: Vec<u8> = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec();
    let mut state = StreamStateInternal::new();
    let mut dst = vec![0u8; compress_bound(src.len() as i32) as usize];
    let result = unsafe {
        compress_fast_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            src.len() as i32,
            dst.as_mut_ptr(),
            dst.len() as i32,
            1,
        )
    };
    assert!(result.is_ok());
    let n = result.unwrap();
    assert!(
        n > 0 && n < src.len(),
        "highly compressible data should compress"
    );
}

#[test]
fn compress_fast_ext_state_reinitializes_state_on_each_call() {
    // Calling compress_fast_ext_state twice on the same state should produce
    // identical output each time (state is reset at entry).
    let src = b"hello world! lz4 compression test.";
    let mut state = StreamStateInternal::new();
    let bound = compress_bound(src.len() as i32) as usize;
    let mut dst1 = vec![0u8; bound];
    let mut dst2 = vec![0u8; bound];

    let r1 = unsafe {
        compress_fast_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            src.len() as i32,
            dst1.as_mut_ptr(),
            dst1.len() as i32,
            1,
        )
    }
    .unwrap();

    let r2 = unsafe {
        compress_fast_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            src.len() as i32,
            dst2.as_mut_ptr(),
            dst2.len() as i32,
            1,
        )
    }
    .unwrap();

    assert_eq!(r1, r2);
    assert_eq!(&dst1[..r1], &dst2[..r2]);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_fast_ext_state_fast_reset
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_ext_state_fast_reset_after_full_init_matches() {
    // If the state was properly initialized via compress_fast_ext_state first,
    // calling fast_reset on a fresh-initialized state and then fast_reset
    // should produce identical results as a full compress.
    let src = b"The quick brown fox jumps over the lazy dog.";
    let bound = compress_bound(src.len() as i32) as usize;

    // Reference: full init
    let mut state_full = StreamStateInternal::new();
    let mut dst_full = vec![0u8; bound];
    let r_full = unsafe {
        compress_fast_ext_state(
            &mut state_full as *mut _,
            src.as_ptr(),
            src.len() as i32,
            dst_full.as_mut_ptr(),
            dst_full.len() as i32,
            1,
        )
    }
    .unwrap();

    // fast_reset on a zero-initialized state (current_offset==0 → NoDictIssue)
    let mut state_fast = StreamStateInternal::new();
    let mut dst_fast = vec![0u8; bound];
    let r_fast = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state_fast as *mut _,
            src.as_ptr(),
            src.len() as i32,
            dst_fast.as_mut_ptr(),
            dst_fast.len() as i32,
            1,
        )
    }
    .unwrap();

    assert_eq!(
        r_full, r_fast,
        "fast_reset on fresh state should match full init"
    );
    assert_eq!(&dst_full[..r_full], &dst_fast[..r_fast]);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_dest_size — fill-output mode
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_dest_size_fills_output() {
    // compress_dest_size should fill the output buffer as completely as possible.
    let src: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let dst_capacity = 256usize;
    let mut dst = vec![0u8; dst_capacity];
    let result = compress_dest_size(&src, &mut dst);
    assert!(result.is_ok(), "compress_dest_size should succeed");
    let (consumed, compressed) = result.unwrap();
    assert!(
        consumed > 0,
        "should consume at least some source bytes: consumed={consumed}"
    );
    assert!(
        compressed > 0 && compressed <= dst_capacity,
        "compressed size must be in (0, dst_capacity]: compressed={compressed}"
    );
}

#[test]
fn compress_dest_size_consumes_all_when_dst_is_large() {
    // When dst is large enough for everything, all src bytes should be consumed.
    let src: Vec<u8> = (0u8..=255).cycle().take(256).collect();
    let mut dst = make_dst(src.len());
    let result = compress_dest_size(&src, &mut dst);
    assert!(result.is_ok());
    let (consumed, compressed) = result.unwrap();
    assert_eq!(
        consumed,
        src.len(),
        "all source should be consumed when dst is large enough"
    );
    assert!(compressed > 0);
}

#[test]
fn compress_dest_size_small_dst_partial_consume() {
    // A small dst forces compress_dest_size to stop early.
    let src: Vec<u8> = vec![0u8; 4096];
    let mut dst = vec![0u8; 32];
    let result = compress_dest_size(&src, &mut dst);
    assert!(result.is_ok());
    let (consumed, compressed) = result.unwrap();
    // We can't consume 4096 bytes into 32 bytes of output
    assert!(consumed <= src.len(), "consumed must not exceed src length");
    assert!(compressed <= dst.len(), "compressed must fit in dst");
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_dest_size_ext_state — unsafe variant
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_dest_size_ext_state_reinitializes_on_exit() {
    // After the call, state should be in clean (zeroed) condition.
    let src: Vec<u8> = (0u8..=255).cycle().take(512).collect();
    let mut dst = vec![0u8; 256];
    let mut src_consumed = src.len() as i32;
    let mut state = StreamStateInternal::new();

    let result = unsafe {
        compress_dest_size_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_consumed as *mut _,
            dst.len() as i32,
            1,
        )
    };
    assert!(result.is_ok());

    // After the call, state should be zero-initialized (as per C source).
    let clean = StreamStateInternal::new();
    assert_eq!(state.hash_table, clean.hash_table);
    assert_eq!(state.current_offset, clean.current_offset);
    assert_eq!(state.table_type, clean.table_type);
    assert_eq!(state.dict_size, clean.dict_size);
}

// ─────────────────────────────────────────────────────────────────────────────
// Round-trip / output determinism
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_default_is_deterministic() {
    // Calling compress_default twice on the same input must produce identical bytes.
    let src = b"deterministic output test: repeated call must match";
    let mut dst1 = make_dst(src.len());
    let mut dst2 = make_dst(src.len());
    let r1 = compress_default(src, &mut dst1).unwrap();
    let r2 = compress_default(src, &mut dst2).unwrap();
    assert_eq!(r1, r2);
    assert_eq!(&dst1[..r1], &dst2[..r2]);
}

#[test]
fn compress_fast_is_deterministic_across_accelerations() {
    // Same src with same acceleration must always produce the same compressed bytes.
    let src = b"hello lz4: acceleration determinism check abcdefgh";
    let mut dst1 = make_dst(src.len());
    let mut dst2 = make_dst(src.len());
    let r1 = compress_fast(src, &mut dst1, 4).unwrap();
    let r2 = compress_fast(src, &mut dst2, 4).unwrap();
    assert_eq!(r1, r2);
    assert_eq!(&dst1[..r1], &dst2[..r2]);
}

#[test]
fn compress_default_non_compressible_data_succeeds_with_sufficient_output() {
    // Non-compressible (random-like) data: use compressed form larger than input.
    // compress_bound guarantees a large enough buffer always succeeds.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut src = vec![0u8; 512];
    for (i, b) in src.iter_mut().enumerate() {
        let mut h = DefaultHasher::new();
        i.hash(&mut h);
        *b = (h.finish() & 0xFF) as u8;
    }
    let mut dst = make_dst(src.len());
    let result = compress_default(&src, &mut dst);
    assert!(
        result.is_ok(),
        "should succeed with worst-case dst: {result:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4 block format spot-check
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_default_single_zero_byte_format() {
    // Compressing a single 0x00 byte:
    // The block should start with a literal-run token (1 literal) followed by
    // the literal byte itself — no match sequence.
    // Token byte: (literal_length << 4) | match_length_extra == (1 << 4) | 0 == 0x10
    // Then: 0x00 (the literal)
    // Total: 2 bytes
    let src = [0x00u8];
    let mut dst = make_dst(1);
    let n = compress_default(&src, &mut dst).unwrap();
    assert_eq!(
        n, 2,
        "single byte should compress to 2-byte block (token + literal)"
    );
    assert_eq!(dst[0], 0x10, "token should encode 1 literal (0x10)");
    assert_eq!(dst[1], 0x00, "literal value should be preserved");
}

#[test]
fn compress_default_empty_input_produces_one_zero_byte() {
    // An empty input goes through compress_generic with srcSize==0,
    // which emits a single 0x00 token byte.
    let src: &[u8] = &[];
    let mut dst = [0u8; 4];
    // compress_default calls compress_fast(src, dst, 1), which returns InputTooLarge
    // only if src.len() > LZ4_MAX_INPUT_SIZE. An empty slice is valid.
    // Then compress_fast_ext_state is called with src_len==0 → compress_generic path.
    let result = compress_default(src, &mut dst);
    assert!(result.is_ok(), "empty input must succeed");
    let n = result.unwrap();
    assert_eq!(n, 1, "empty input should produce exactly 1 byte");
    assert_eq!(dst[0], 0x00, "single 0x00 token for empty input");
}
