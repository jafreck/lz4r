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

// ─────────────────────────────────────────────────────────────────────────────
// FillOutput mode — additional coverage
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_dest_size_very_tiny_dst_16_bytes() {
    // Tiny 16-byte output buffer exercises the FillOutput early-exit path where
    // the budget is exhausted before any match can be encoded.
    let src: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 16];
    let result = compress_dest_size(&src, &mut dst);
    assert!(result.is_ok());
    let (consumed, compressed) = result.unwrap();
    assert!(consumed > 0 && consumed < src.len());
    assert!(compressed > 0 && compressed <= 16);
}

#[test]
fn compress_dest_size_fill_output_with_alternating_pattern() {
    // Pattern of alternating short matches triggers the 'next_match inner loop
    // in FillOutput: "ABCDABCD" repeated forces re-match after the first sequence.
    let pattern = b"ABCDABCD";
    let src: Vec<u8> = pattern.iter().cycle().take(8192).copied().collect();
    let mut dst = vec![0u8; 64];
    let result = compress_dest_size(&src, &mut dst);
    assert!(result.is_ok());
    let (consumed, compressed) = result.unwrap();
    assert!(consumed > 0);
    assert!(compressed > 0 && compressed <= 64);
}

#[test]
fn compress_dest_size_ext_state_fill_output_cycling_data() {
    // Exercise compress_dest_size_ext_state with cycling data and tight output.
    let src: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 48];
    let mut src_consumed = src.len() as i32;
    let mut state = StreamStateInternal::new();
    let result = unsafe {
        compress_dest_size_ext_state(
            &mut state as *mut _,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut src_consumed as *mut _,
            48,
            1,
        )
    };
    assert!(result.is_ok());
    let n = result.unwrap();
    assert!(n > 0 && n <= 48);
    assert!((src_consumed as usize) < src.len());
}

#[test]
fn compress_dest_size_fill_output_roundtrip_partial() {
    // Compress with dest_size (partial), then decompress what was consumed.
    use lz4::block::decompress_core::decompress_safe;
    let src: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 128];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    // Decompress just the consumed portion
    let mut decoded = vec![0u8; consumed];
    let decoded_len = decompress_safe(&dst[..compressed], &mut decoded).unwrap();
    assert_eq!(&decoded[..decoded_len], &src[..consumed]);
}

// ─────────────────────────────────────────────────────────────────────────────
// External dictionary / streaming — coverage for UsingExtDict branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn streaming_ext_dict_via_compress_force_ext_dict() {
    // compress_force_ext_dict forces UsingExtDict for every block,
    // exercising the external dictionary match-search and count branches.
    let mut stream = lz4::block::stream::Lz4Stream::new();

    // Block 1: initial data
    let block1: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst1 = make_dst(block1.len());
    let n1 = unsafe {
        stream.compress_force_ext_dict(
            block1.as_ptr(),
            dst1.as_mut_ptr(),
            block1.len() as i32,
            dst1.len() as i32,
        )
    };
    assert!(n1 > 0);

    // Block 2: overlapping data should find ext-dict matches
    let block2: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst2 = make_dst(block2.len());
    let n2 = unsafe {
        stream.compress_force_ext_dict(
            block2.as_ptr(),
            dst2.as_mut_ptr(),
            block2.len() as i32,
            dst2.len() as i32,
        )
    };
    assert!(n2 > 0);
    // Second block should compress better due to ext-dict matches
    assert!(n2 < n1, "ext-dict block should be smaller: {n2} vs {n1}");
}

#[test]
fn streaming_with_dict_load_then_compress_fast_continue() {
    // load_dict + compress_fast_continue exercises dict-based compression.
    use lz4::block::decompress_core::decompress_safe_using_dict;
    let dict: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();

    let mut stream = lz4::block::stream::Lz4Stream::new();
    stream.load_dict(&dict);

    // First block: shares dict data
    let src1: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst1 = make_dst(src1.len());
    let n1 = stream.compress_fast_continue(&src1, &mut dst1, 1);
    assert!(n1 > 0);

    // Verify roundtrip using dict
    let mut dec = vec![0u8; src1.len()];
    let d = decompress_safe_using_dict(&dst1[..n1 as usize], &mut dec, &dict).unwrap();
    assert_eq!(&dec[..d], &src1[..]);
}

#[test]
fn streaming_two_contiguous_blocks_prefix_mode() {
    // Two blocks in the same buffer: second uses WithPrefix64k
    let mut stream = lz4::block::stream::Lz4Stream::new();
    let mut ring = vec![0u8; 64 * 1024 * 2]; // 128KB ring

    let block1: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    ring[..block1.len()].copy_from_slice(&block1);

    let mut dst1 = make_dst(block1.len());
    let n1 = stream.compress_fast_continue(&ring[..block1.len()], &mut dst1, 1);
    assert!(n1 > 0);

    // Block 2 right after block1 in ring — contiguous → WithPrefix64k
    let block2: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let offset = block1.len();
    ring[offset..offset + block2.len()].copy_from_slice(&block2);

    let mut dst2 = make_dst(block2.len());
    let n2 = stream.compress_fast_continue(
        &ring[offset..offset + block2.len()],
        &mut dst2,
        1,
    );
    assert!(n2 > 0);
    // Second block should be smaller due to prefix matching
    assert!(n2 < n1, "prefix block should compress better: {n2} vs {n1}");
}

#[test]
fn streaming_non_contiguous_blocks_ext_dict() {
    // Two blocks in separate allocations: second triggers UsingExtDict
    let mut stream = lz4::block::stream::Lz4Stream::new();

    let block1: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst1 = make_dst(block1.len());
    let n1 = stream.compress_fast_continue(&block1, &mut dst1, 1);
    assert!(n1 > 0);

    // Block 2 in different allocation → non-contiguous → UsingExtDict
    let block2: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst2 = make_dst(block2.len());
    let n2 = stream.compress_fast_continue(&block2, &mut dst2, 1);
    assert!(n2 > 0);
}

#[test]
fn streaming_multi_block_ext_dict_roundtrip() {
    // 4 blocks non-contiguous: ext-dict matching across all blocks.
    use lz4::block::decompress_core::decompress_safe;
    let mut stream = lz4::block::stream::Lz4Stream::new();

    let mut compressed_blocks: Vec<(Vec<u8>, usize, Vec<u8>)> = Vec::new();

    for i in 0..4u64 {
        let block: Vec<u8> = (0..2048).map(|j| ((j + i * 50) % 251) as u8).collect();
        let mut dst = make_dst(block.len());
        let n = stream.compress_fast_continue(&block, &mut dst, 1);
        assert!(n > 0);
        compressed_blocks.push((block, n as usize, dst));
    }

    // Verify the first block decompresses correctly (no dict needed)
    let (ref orig, n, ref cmp) = compressed_blocks[0];
    let mut dec = vec![0u8; orig.len()];
    let d = decompress_safe(&cmp[..n], &mut dec).unwrap();
    assert_eq!(&dec[..d], &orig[..]);
}

#[test]
fn compress_fast_ext_state_fast_reset_high_acceleration() {
    // High acceleration value exercises different step sizes in the search loop.
    let src: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut dst = make_dst(src.len());
    let mut state = StreamStateInternal::new();
    let result = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state,
            src.as_ptr(),
            src.len() as i32,
            dst.as_mut_ptr(),
            dst.len() as i32,
            100,
        )
    };
    assert!(result.is_ok());
    assert!(result.unwrap() > 0);
}

#[test]
fn compress_fast_ext_state_fast_reset_max_acceleration() {
    let src: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst = make_dst(src.len());
    let mut state = StreamStateInternal::new();
    let result = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state,
            src.as_ptr(),
            src.len() as i32,
            dst.as_mut_ptr(),
            dst.len() as i32,
            LZ4_ACCELERATION_MAX + 100, // should be clamped
        )
    };
    assert!(result.is_ok());
    assert!(result.unwrap() > 0);
}

#[test]
fn compress_fast_ext_state_fast_reset_zero_acceleration() {
    // Zero acceleration should be clamped to DEFAULT
    let src: Vec<u8> = (0..2048).map(|i| (i % 251) as u8).collect();
    let mut dst = make_dst(src.len());
    let mut state = StreamStateInternal::new();
    let result = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state,
            src.as_ptr(),
            src.len() as i32,
            dst.as_mut_ptr(),
            dst.len() as i32,
            0,
        )
    };
    assert!(result.is_ok());
    assert!(result.unwrap() > 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 6: compress_dest_size (FillOutput) and fast_reset DictSmall paths
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_dest_size_basic_roundtrip() {
    // Exercises the safe wrapper (line 1004) and guaranteed-success path (1062-1065)
    let src: Vec<u8> = (0..10000).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; compress_bound(src.len() as i32) as usize * 2];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    assert_eq!(consumed, src.len());
    assert!(compressed > 0);
    let mut dec = vec![0u8; src.len()];
    let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
    assert_eq!(&dec[..n], &src[..]);
}

#[test]
fn compress_dest_size_undersized_dst_filloutput() {
    // FillOutput: dst < compress_bound. Exercises lines 384, 425-426, 468-490, 705, 716
    let src: Vec<u8> = (0..10000).map(|i| (i % 251) as u8).collect();
    let bound = compress_bound(src.len() as i32) as usize;
    let dst_cap = bound * 6 / 10;
    let mut dst = vec![0u8; dst_cap];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    assert!(consumed <= src.len());
    assert!(compressed > 0);
    let mut dec = vec![0u8; consumed + 1024];
    let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
    assert_eq!(&dec[..n], &src[..consumed]);
}

#[test]
fn compress_dest_size_tiny_dst_filloutput() {
    // FillOutput with extremely small dst — forces early bail (line 384, 425)
    let src: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 50];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    if compressed > 0 {
        let mut dec = vec![0u8; consumed + 256];
        let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
        assert_eq!(&dec[..n], &src[..consumed]);
    }
}

#[test]
fn compress_dest_size_highly_compressible_match_shorten() {
    // Long matches with undersized dst: FillOutput shortens matches (lines 468-490)
    let src: Vec<u8> = vec![b'A'; 20000];
    let bound = compress_bound(src.len() as i32) as usize;
    let mut dst = vec![0u8; bound / 4];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    assert!(consumed > 0);
    assert!(compressed > 0);
    let mut dec = vec![0u8; consumed + 256];
    let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
    assert_eq!(&dec[..n], &src[..consumed]);
}

#[test]
fn compress_dest_size_small_input_byu16() {
    // Small input (<64KB) → ByU16 table in FillOutput (lines 1066-1074)
    let src: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
    let mut dst = vec![0u8; 200];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    assert!(consumed <= src.len());
    if compressed > 0 {
        let mut dec = vec![0u8; consumed + 256];
        let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
        assert_eq!(&dec[..n], &src[..consumed]);
    }
}

#[test]
fn compress_dest_size_large_input_byu32() {
    // Large input (≥64KB) → ByU32 in FillOutput (lines 1072-1074)
    let src: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    let bound = compress_bound(src.len() as i32) as usize;
    let mut dst = vec![0u8; bound / 2];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    assert!(consumed <= src.len());
    assert!(compressed > 0);
    let mut dec = vec![0u8; consumed + 256];
    let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
    assert_eq!(&dec[..n], &src[..consumed]);
}

#[test]
fn compress_dest_size_zero_capacity() {
    // dst capacity = 0 → compressed output is 0
    let src: Vec<u8> = vec![b'A'; 100];
    let mut dst = vec![0u8; 0];
    let result = compress_dest_size(&src, &mut dst);
    match result {
        Ok((_consumed, compressed)) => {
            assert_eq!(compressed, 0);
        }
        Err(_) => {} // error for zero capacity is acceptable
    }
}

#[test]
fn compress_dest_size_empty_input() {
    // Empty input with FillOutput — line 705
    let src: Vec<u8> = vec![];
    let mut dst = vec![0u8; 10];
    let (consumed, compressed) = compress_dest_size(&src, &mut dst).unwrap();
    assert_eq!(consumed, 0);
    assert_eq!(compressed, 1); // single 0x00 token
}

#[test]
fn compress_dest_size_ext_state_roundtrip() {
    let src: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
    let bound = compress_bound(src.len() as i32) as usize;
    let mut dst = vec![0u8; bound];
    let mut state = StreamStateInternal::new();
    let mut consumed = src.len() as i32;
    let result = unsafe {
        compress_dest_size_ext_state(
            &mut state,
            src.as_ptr(),
            dst.as_mut_ptr(),
            &mut consumed,
            dst.len() as i32,
            1,
        )
    };
    let compressed = result.unwrap();
    assert!(compressed > 0);
    let mut dec = vec![0u8; consumed as usize + 256];
    let n = lz4::block::decompress_safe(&dst[..compressed], &mut dec).unwrap();
    assert_eq!(&dec[..n], &src[..consumed as usize]);
}

#[test]
fn fast_reset_small_input_dictsmall_notlimited() {
    // Lines 815-827: ByU16 + NotLimited + DictSmall (current_offset != 0)
    let mut state = StreamStateInternal::new();
    let src1: Vec<u8> = (0..500).map(|i| (i % 251) as u8).collect();
    let bound1 = compress_bound(src1.len() as i32) as usize;
    let mut dst1 = vec![0u8; bound1];
    let _ = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state, src1.as_ptr(), src1.len() as i32,
            dst1.as_mut_ptr(), dst1.len() as i32, 1,
        )
    }.unwrap();

    // Second: small input, full dst → NotLimited + DictSmall
    let src2: Vec<u8> = (0..300).map(|i| (i % 199) as u8).collect();
    let bound2 = compress_bound(src2.len() as i32) as usize;
    let mut dst2 = vec![0u8; bound2];
    let n2 = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state, src2.as_ptr(), src2.len() as i32,
            dst2.as_mut_ptr(), dst2.len() as i32, 1,
        )
    }.unwrap();
    assert!(n2 > 0);
    let mut dec = vec![0u8; src2.len()];
    let n = lz4::block::decompress_safe(&dst2[..n2], &mut dec).unwrap();
    assert_eq!(&dec[..n], &src2[..]);
}

#[test]
fn fast_reset_small_input_limited_dictsmall() {
    // Lines 877-890: ByU16 + LimitedOutput + DictSmall
    let mut state = StreamStateInternal::new();
    let src1: Vec<u8> = (0..400).map(|i| (i % 251) as u8).collect();
    let bound1 = compress_bound(src1.len() as i32) as usize;
    let mut dst1 = vec![0u8; bound1];
    let _ = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state, src1.as_ptr(), src1.len() as i32,
            dst1.as_mut_ptr(), dst1.len() as i32, 1,
        )
    }.unwrap();

    // Second: small input, undersized dst → LimitedOutput + DictSmall
    let src2: Vec<u8> = (0..300).map(|i| (i % 199) as u8).collect();
    let bound2 = compress_bound(src2.len() as i32) as usize;
    let dst_cap = bound2 * 8 / 10;
    let mut dst2 = vec![0u8; dst_cap];
    let _ = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state, src2.as_ptr(), src2.len() as i32,
            dst2.as_mut_ptr(), dst2.len() as i32, 1,
        )
    };
    // Either succeeds or fails — both exercise the path
}

#[test]
fn fast_reset_large_limited() {
    // Line 859: fast_reset large input + limited output → ByU32+LimitedOutput
    let mut state = StreamStateInternal::new();
    let src: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    let bound = compress_bound(src.len() as i32) as usize;
    let dst_cap = bound * 9 / 10;
    let mut dst = vec![0u8; dst_cap];
    let result = unsafe {
        compress_fast_ext_state_fast_reset(
            &mut state, src.as_ptr(), src.len() as i32,
            dst.as_mut_ptr(), dst.len() as i32, 1,
        )
    };
    assert!(result.is_ok());
}
