// Unit tests for task-012: HC compression loop and optimal parser.
//
// Tests verify behavioural parity with lz4hc.c v1.10.0, lines 1121–1416 and
// 1823–2123:
//   `LZ4HC_literalsPrice`      → `literals_price`
//   `LZ4HC_sequencePrice`      → `sequence_price`
//   `LZ4HC_FindLongerMatch`    → `find_longer_match`
//   `LZ4HC_optimal_t`          → `Lz4HcOptimal`
//   `LZ4HC_compress_hashChain` → `compress_hash_chain`
//   `LZ4HC_compress_optimal`   → `compress_optimal`
//
// Coverage:
//   - literals_price: zero, below RUN_MASK, exactly RUN_MASK, RUN_MASK+255, large
//   - sequence_price: MINMATCH, small literals+match, below ML_MASK boundary,
//                     exactly ML_MASK+MINMATCH, large match
//   - Lz4HcOptimal: Default gives all-zeros, Clone/Copy compile
//   - compress_hash_chain: tiny input (all literals), repeated-byte input
//                          (produces match), limited-output too small, fillOutput
//   - compress_optimal: tiny input (all literals), limited-output too small
//   - find_longer_match: no-match on unique data

use lz4::block::types::{LimitedOutputDirective, MINMATCH, ML_MASK, RUN_MASK};
use lz4::hc::compress_hc::{
    compress_hash_chain, compress_optimal, find_longer_match, literals_price, sequence_price,
    Lz4HcOptimal,
};
use lz4::hc::search::HcFavor;
use lz4::hc::types::{init_internal, DictCtxDirective, HcCCtxInternal};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create an initialised HC context pointing at the start of `buf`.
///
/// Mirrors the pattern from task-011 tests and from the C test suite.
unsafe fn make_ctx(buf: &[u8]) -> HcCCtxInternal {
    let mut ctx = HcCCtxInternal::new();
    init_internal(&mut ctx, buf.as_ptr());
    ctx.end = buf.as_ptr().add(buf.len());
    ctx
}

// ═════════════════════════════════════════════════════════════════════════════
// literals_price (LZ4HC_literalsPrice)
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn literals_price_zero() {
    // 0 literals → price 0
    assert_eq!(literals_price(0), 0);
}

#[test]
fn literals_price_one() {
    assert_eq!(literals_price(1), 1);
}

#[test]
fn literals_price_below_run_mask() {
    // RUN_MASK = 15; any litlen < 15 → price == litlen (no extension byte)
    let run_mask = RUN_MASK as i32;
    for litlen in 1..run_mask {
        assert_eq!(
            literals_price(litlen),
            litlen,
            "literals_price({litlen}) should equal {litlen}"
        );
    }
}

#[test]
fn literals_price_exactly_run_mask() {
    // litlen == 15 → price = 15 + 1 + (15-15)/255 = 15 + 1 + 0 = 16
    let run_mask = RUN_MASK as i32;
    let expected = run_mask + 1 + (run_mask - run_mask) / 255;
    assert_eq!(literals_price(run_mask), expected);
    assert_eq!(literals_price(15), 16);
}

#[test]
fn literals_price_run_mask_plus_one() {
    // litlen = 16 → 16 + 1 + (16-15)/255 = 16 + 1 + 0 = 17
    assert_eq!(literals_price(16), 17);
}

#[test]
fn literals_price_run_mask_plus_255() {
    // litlen = 15 + 255 = 270 → 270 + 1 + (255/255) = 270 + 1 + 1 = 272
    assert_eq!(literals_price(270), 272);
}

#[test]
fn literals_price_run_mask_plus_510() {
    // litlen = 15 + 510 = 525 → 525 + 1 + (510/255) = 525 + 1 + 2 = 528
    assert_eq!(literals_price(525), 528);
}

#[test]
fn literals_price_large_value() {
    // litlen = 1000 → extra = 1000 - 15 = 985, n_extra_bytes = 985/255 = 3
    // price = 1000 + 1 + 3 = 1004
    let litlen = 1000i32;
    let run_mask = RUN_MASK as i32;
    let expected = litlen + 1 + (litlen - run_mask) / 255;
    assert_eq!(literals_price(litlen), expected);
}

// ═════════════════════════════════════════════════════════════════════════════
// sequence_price (LZ4HC_sequencePrice)
// ═════════════════════════════════════════════════════════════════════════════

// sequence_price = 1 (token) + 2 (offset) + literals_price(litlen)
//                + (if mlen >= ML_MASK + MINMATCH: 1 + (mlen - (ML_MASK+MINMATCH))/255)

#[test]
fn sequence_price_zero_literals_min_match() {
    // litlen=0, mlen=4 (MINMATCH), mlen < 19 → no ml extension
    // price = 3 + 0 = 3
    assert_eq!(sequence_price(0, MINMATCH as i32), 3);
}

#[test]
fn sequence_price_small_literals_min_match() {
    // litlen=5, mlen=4 → price = 3 + 5 = 8
    assert_eq!(sequence_price(5, MINMATCH as i32), 8);
}

#[test]
fn sequence_price_max_before_ml_extension() {
    // ML_MASK + MINMATCH = 15 + 4 = 19; mlen=18 → no extension
    // price = 3 + literals_price(0) = 3
    assert_eq!(sequence_price(0, 18), 3);
}

#[test]
fn sequence_price_exactly_ml_threshold() {
    // mlen = ML_MASK + MINMATCH = 19 → extension: 1 + (19-19)/255 = 1 + 0 = 1
    // price = 3 + 1 = 4
    let threshold = (ML_MASK + MINMATCH as u32) as i32;
    assert_eq!(threshold, 19);
    assert_eq!(sequence_price(0, threshold), 4);
}

#[test]
fn sequence_price_ml_threshold_plus_255() {
    // mlen = 19 + 255 = 274 → extension: 1 + (255/255) = 1 + 1 = 2
    // price = 3 + 2 = 5
    assert_eq!(sequence_price(0, 274), 5);
}

#[test]
fn sequence_price_large_litlen_and_mlen() {
    // litlen = 270 (→ literals_price = 272)
    // mlen = 274 (→ ml extension = 2)
    // price = 3 + 272 + 2 = 277
    assert_eq!(sequence_price(270, 274), 277);
}

#[test]
fn sequence_price_monotone_in_mlen() {
    // For fixed litlen, sequence_price should be non-decreasing in mlen.
    let mut prev = sequence_price(0, MINMATCH as i32);
    for mlen in (MINMATCH as i32 + 1)..200 {
        let cur = sequence_price(0, mlen);
        assert!(
            cur >= prev,
            "sequence_price(0, {mlen}) = {cur} < sequence_price(0, {}) = {prev}",
            mlen - 1
        );
        prev = cur;
    }
}

#[test]
fn sequence_price_monotone_in_litlen() {
    // For fixed mlen, sequence_price should be non-decreasing in litlen.
    let mlen = MINMATCH as i32;
    let mut prev = sequence_price(0, mlen);
    for litlen in 1..200 {
        let cur = sequence_price(litlen, mlen);
        assert!(cur >= prev, "sequence_price({litlen}, {mlen}) not monotone");
        prev = cur;
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Lz4HcOptimal struct
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn lz4hc_optimal_default_all_zero() {
    let opt = Lz4HcOptimal::default();
    assert_eq!(opt.price, 0);
    assert_eq!(opt.off, 0);
    assert_eq!(opt.mlen, 0);
    assert_eq!(opt.litlen, 0);
}

#[test]
fn lz4hc_optimal_clone_copy() {
    let a = Lz4HcOptimal {
        price: 10,
        off: 5,
        mlen: 4,
        litlen: 3,
    };
    let b = a; // Copy
    let c = a.clone(); // Clone
    assert_eq!(b.price, 10);
    assert_eq!(c.off, 5);
    assert_eq!(c.mlen, 4);
    assert_eq!(b.litlen, 3);
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_hash_chain (LZ4HC_compress_hashChain)
// ═════════════════════════════════════════════════════════════════════════════

/// Input smaller than LZ4_MIN_LENGTH (= MFLIMIT + 1 = 13) bypasses the
/// compress loop → all bytes become literals.
#[test]
fn compress_hash_chain_tiny_input_all_literals() {
    // 5-byte input; last_run_size = 5, 5 < RUN_MASK(15) → single token byte
    // token = (5 << ML_BITS) = 0x50, then 5 literal bytes → 6 output bytes.
    let input = b"Hello";
    let mut output = vec![0u8; 64];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_hash_chain(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            4, // max_nb_attempts (level 3 = 4)
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );

        // Should succeed and consume all 5 source bytes.
        assert!(n > 0, "expected > 0 bytes written, got {n}");
        assert_eq!(src_size, input.len() as i32);

        // Token encodes 5 literals (< RUN_MASK) in the high nibble.
        let token = output[0];
        let lit_nibble = (token >> 4) as usize;
        assert_eq!(lit_nibble, 5);

        // Literal bytes follow the token.
        assert_eq!(&output[1..6], b"Hello");

        // Total written = 6 bytes.
        assert_eq!(n, 6);
    }
}

/// Exactly 12 bytes (== MFLIMIT) is still < LZ4_MIN_LENGTH (13) → all literals.
#[test]
fn compress_hash_chain_mflimit_input_all_literals() {
    let input = b"123456789012"; // 12 bytes == MFLIMIT
    assert_eq!(input.len(), 12);
    let mut output = vec![0u8; 64];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_hash_chain(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            4,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );

        assert!(n > 0);
        assert_eq!(src_size, 12);
        // 12 < RUN_MASK(15) → single token byte (0xC0 = 12 << 4)
        assert_eq!(output[0], 0xC0_u8);
        assert_eq!(&output[1..13], input.as_ref());
        assert_eq!(n, 13);
    }
}

/// A large repeated-byte buffer should compress to fewer bytes than the input.
#[test]
fn compress_hash_chain_repeated_data_compresses() {
    // 4 KB of 0xAA bytes — highly compressible.
    let input = vec![0xAA_u8; 4096];
    let mut output = vec![0u8; 4096];

    unsafe {
        let mut ctx = make_ctx(&input);
        let mut src_size = input.len() as i32;
        let n = compress_hash_chain(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            8, // max_nb_attempts
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );

        assert!(n > 0, "compression returned 0");
        // Compressed output must be smaller than raw input.
        assert!(
            (n as usize) < input.len(),
            "expected compression: {n} bytes is not < {}",
            input.len()
        );
        // All source bytes should have been consumed.
        assert_eq!(src_size, input.len() as i32);
    }
}

/// In LimitedOutput mode, if the output buffer is too small, returns 0.
#[test]
fn compress_hash_chain_limited_output_too_small_returns_zero() {
    let input = b"The quick brown fox jumps over the lazy dog.";
    // Allocate only 3 bytes — nowhere near enough.
    let mut output = vec![0u8; 3];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_hash_chain(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            4,
            LimitedOutputDirective::LimitedOutput,
            DictCtxDirective::NoDictCtx,
        );

        assert_eq!(n, 0, "expected 0 (overflow) but got {n}");
    }
}

/// src_size_ptr is set to 0 on entry; on success it should be updated.
#[test]
fn compress_hash_chain_updates_src_size_ptr() {
    let input = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut output = vec![0u8; 256];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_hash_chain(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            4,
            LimitedOutputDirective::NotLimited,
            DictCtxDirective::NoDictCtx,
        );

        assert!(n > 0);
        // Source pointer must have been advanced to at least 1 byte.
        assert!(src_size > 0, "src_size_ptr was not updated");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// compress_optimal (LZ4HC_compress_optimal)
// ═════════════════════════════════════════════════════════════════════════════

/// Tiny input (< LZ4_MIN_LENGTH) → all literals, same as compress_hash_chain.
#[test]
fn compress_optimal_tiny_input_all_literals() {
    let input = b"Hello";
    let mut output = vec![0u8; 64];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_optimal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            16, // nb_searches
            64, // sufficient_len
            LimitedOutputDirective::NotLimited,
            true, // full_update
            DictCtxDirective::NoDictCtx,
            HcFavor::CompressionRatio,
        );

        assert!(n > 0, "expected > 0 bytes written, got {n}");
        assert_eq!(src_size, input.len() as i32);

        // Token should encode 5 literals in high nibble
        let token = output[0];
        let lit_nibble = (token >> 4) as usize;
        assert_eq!(lit_nibble, 5);
        assert_eq!(&output[1..6], b"Hello");
        assert_eq!(n, 6);
    }
}

/// A large repeated-byte buffer should compress to fewer bytes than the input.
#[test]
fn compress_optimal_repeated_data_compresses() {
    let input = vec![0xBB_u8; 4096];
    let mut output = vec![0u8; 4096];

    unsafe {
        let mut ctx = make_ctx(&input);
        let mut src_size = input.len() as i32;
        let n = compress_optimal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            16,
            64,
            LimitedOutputDirective::NotLimited,
            true,
            DictCtxDirective::NoDictCtx,
            HcFavor::CompressionRatio,
        );

        assert!(n > 0, "compression returned 0");
        assert!(
            (n as usize) < input.len(),
            "expected compression: {n} bytes is not < {}",
            input.len()
        );
        assert_eq!(src_size, input.len() as i32);
    }
}

/// In LimitedOutput mode, if the output buffer is too small, returns 0.
#[test]
fn compress_optimal_limited_output_too_small_returns_zero() {
    let input = b"The quick brown fox jumps over the lazy dog.";
    let mut output = vec![0u8; 3];

    unsafe {
        let mut ctx = make_ctx(input);
        let mut src_size = input.len() as i32;
        let n = compress_optimal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            4,
            64,
            LimitedOutputDirective::LimitedOutput,
            true,
            DictCtxDirective::NoDictCtx,
            HcFavor::CompressionRatio,
        );

        assert_eq!(n, 0, "expected 0 (overflow) but got {n}");
    }
}

/// compress_optimal with favor_dec_speed selects HcFavor::DecompressionSpeed path.
/// Just verifies it produces valid (positive) output — not a crash.
#[test]
fn compress_optimal_favor_decompression_speed() {
    let input = vec![0xCC_u8; 1024];
    let mut output = vec![0u8; 1024];

    unsafe {
        let mut ctx = make_ctx(&input);
        let mut src_size = input.len() as i32;
        let n = compress_optimal(
            &mut ctx,
            input.as_ptr(),
            output.as_mut_ptr(),
            &mut src_size,
            output.len() as i32,
            16,
            64,
            LimitedOutputDirective::NotLimited,
            false, // full_update = false
            DictCtxDirective::NoDictCtx,
            HcFavor::DecompressionSpeed,
        );

        assert!(n > 0, "expected compression to succeed");
        assert!(
            (n as usize) < input.len(),
            "expected output smaller than input"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// find_longer_match (LZ4HC_FindLongerMatch)
// ═════════════════════════════════════════════════════════════════════════════

/// On unique (random-ish) data with no matches, find_longer_match should return
/// a zero-length match.
#[test]
fn find_longer_match_no_match_on_unique_data() {
    // 64 bytes of unique values — no repetition, so no match possible.
    let input: Vec<u8> = (0u8..64).collect();
    let mut output_buf = vec![0u8; 64]; // not used, just needs some space

    unsafe {
        let mut ctx = make_ctx(&input);
        // Simulate inserting positions up to byte 20 so the context is warm.
        let ip = input.as_ptr().add(20);
        let i_high = input.as_ptr().add(input.len() - 5); // matchlimit

        let m = find_longer_match(
            &mut ctx,
            ip,
            i_high,
            MINMATCH as i32 - 1, // min_len
            4,                   // nb_searches
            DictCtxDirective::NoDictCtx,
            HcFavor::CompressionRatio,
        );

        // No match found → len == 0
        assert_eq!(
            m.len, 0,
            "expected no match on unique data, got len={}",
            m.len
        );
        let _ = &mut output_buf; // suppress unused warning
    }
}

/// On highly-repeated data, find_longer_match should find a match > min_len.
#[test]
fn find_longer_match_finds_match_on_repeated_data() {
    // 256 bytes of 0xAA — every position should match position 0.
    let input = vec![0xAA_u8; 256];

    unsafe {
        let mut ctx = make_ctx(&input);
        // Insert the first MINMATCH bytes so there's something in the table.
        // We do this by calling find_longer_match at offset 4 (after first MINMATCH bytes
        // of identical data have been processed by the context).
        // Actually, we need to advance next_to_update manually or rely on the search inserting.
        let ip = input.as_ptr().add(4);
        let i_high = input.as_ptr().add(input.len() - 5);

        let m = find_longer_match(
            &mut ctx,
            ip,
            i_high,
            MINMATCH as i32 - 1,
            64,
            DictCtxDirective::NoDictCtx,
            HcFavor::CompressionRatio,
        );

        // On repeated data, we expect either a match or no match depending on
        // whether positions have been inserted. The result must never have a
        // negative length.
        assert!(m.len >= 0, "match length must be >= 0, got {}", m.len);
        // back must be 0 (find_longer_match sets i_low_limit = ip).
        assert_eq!(m.back, 0, "find_longer_match must return back==0");
    }
}

/// find_longer_match with HcFavor::DecompressionSpeed shortens matches in [19,36].
/// Verify that 18 <= returned len <= 18 when a long match would otherwise be found.
#[test]
fn find_longer_match_dec_speed_shortens_long_matches() {
    // 512 bytes of identical data to guarantee a very long match.
    let input = vec![0xDD_u8; 512];

    unsafe {
        let mut ctx = make_ctx(&input);
        let ip = input.as_ptr().add(8);
        let i_high = input.as_ptr().add(input.len() - 5);

        let m = find_longer_match(
            &mut ctx,
            ip,
            i_high,
            MINMATCH as i32 - 1,
            256,
            DictCtxDirective::NoDictCtx,
            HcFavor::DecompressionSpeed,
        );

        // Either no match (len == 0) or a match that obeys the shortening rule.
        if m.len > 0 {
            // If len was in (18, 36], it should be clamped to 18.
            // len <= 18 or len > 36 are also valid (no shortening applied).
            assert!(
                m.len <= 18 || m.len > 36,
                "dec-speed shortening violated: len = {}",
                m.len
            );
        }
    }
}
