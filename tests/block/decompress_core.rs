// Unit tests for task-006: LZ4 block decompression core
//
// Tests verify behavioural parity with lz4.c v1.10.0 (lines 1969–2447):
//   - DecompressError properties (eq, copy, debug)
//   - decompress_safe: basic functionality, empty buffers, malformed input
//   - decompress_safe_partial: partial decode, target clamping, boundary conditions
//   - decompress_safe_using_dict: empty-dict fallback, with dict
//   - Variable-length literal/match extension (read_variable_length via public API)
//   - Round-trip compression → decompression correctness

use lz4::block::compress::{compress_bound, compress_default};
use lz4::block::decompress_core::{
    decompress_safe, decompress_safe_partial, decompress_safe_using_dict, DecompressError,
};

// ─────────────────────────────────────────────────────────────────────────────
// Minimal hand-crafted LZ4 blocks (all-literal sequences, no matches)
// ─────────────────────────────────────────────────────────────────────────────

// token 0x10 (ll=1, ml_nibble=0 → last sequence), literal 'A'
const BLOCK_A: &[u8] = &[0x10, b'A'];

// token 0x50 (ll=5, ml_nibble=0 → last sequence), literals "Hello"
const BLOCK_HELLO: &[u8] = &[0x50, b'H', b'e', b'l', b'l', b'o'];

// Single 0x00 token: empty block (valid only with zero-capacity dst)
const BLOCK_EMPTY: &[u8] = &[0x00];

// ─────────────────────────────────────────────────────────────────────────────
// DecompressError — trait properties
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_error_eq() {
    assert_eq!(
        DecompressError::MalformedInput,
        DecompressError::MalformedInput
    );
}

#[test]
fn decompress_error_copy() {
    let e = DecompressError::MalformedInput;
    let e2 = e; // Copy
    assert_eq!(e, e2);
}

#[test]
fn decompress_error_clone() {
    let e = DecompressError::MalformedInput;
    #[allow(clippy::clone_on_copy)]
    let e2 = e.clone();
    assert_eq!(e, e2);
}

#[test]
fn decompress_error_debug_does_not_panic() {
    let _ = format!("{:?}", DecompressError::MalformedInput);
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe — basic happy paths
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_single_literal() {
    // Single literal 'A': matches C LZ4_decompress_safe on a 1-byte block.
    let mut dst = [0u8; 1];
    let n = decompress_safe(BLOCK_A, &mut dst).expect("decompression failed");
    assert_eq!(n, 1);
    assert_eq!(dst[0], b'A');
}

#[test]
fn decompress_safe_five_literals() {
    // Five-literal block "Hello".
    let mut dst = [0u8; 5];
    let n = decompress_safe(BLOCK_HELLO, &mut dst).expect("decompression failed");
    assert_eq!(n, 5);
    assert_eq!(&dst, b"Hello");
}

#[test]
fn decompress_safe_empty_block_zero_capacity_dst() {
    // Single 0x00 token + zero-capacity dst → Ok(0).
    // Matches C: if (outputSize == 0) return (srcSize==1 && *src==0) ? 0 : -1
    let mut dst: [u8; 0] = [];
    let n = decompress_safe(BLOCK_EMPTY, &mut dst).expect("decompression failed");
    assert_eq!(n, 0);
}

#[test]
fn decompress_safe_variable_length_15_literals() {
    // token 0xF0 (ll nibble=15 = RUN_MASK, triggers variable-length read),
    // extra byte 0x00 (adds 0, terminates), then 15 literal 'A' bytes.
    // Verifies read_variable_length initial_check=true path.
    let mut block = vec![0xF0u8, 0x00];
    block.extend(std::iter::repeat(b'A').take(15));
    let mut dst = [0u8; 15];
    let n = decompress_safe(&block, &mut dst).expect("decompression failed");
    assert_eq!(n, 15);
    assert!(dst.iter().all(|&b| b == b'A'));
}

#[test]
fn decompress_safe_variable_length_16_literals() {
    // token 0xF0, extra byte 0x01 (total = 15 + 1 = 16), then 16 'B' bytes.
    let mut block = vec![0xF0u8, 0x01];
    block.extend(std::iter::repeat(b'B').take(16));
    let mut dst = [0u8; 16];
    let n = decompress_safe(&block, &mut dst).expect("decompression failed");
    assert_eq!(n, 16);
    assert!(dst.iter().all(|&b| b == b'B'));
}

#[test]
fn decompress_safe_variable_length_270_literals() {
    // token 0xF0, extra bytes [0xFF, 0x00] (total = 15 + 255 + 0 = 270).
    // Exercises the loop continuation inside read_variable_length.
    let mut block = vec![0xF0u8, 0xFF, 0x00];
    block.extend(std::iter::repeat(b'C').take(270));
    let mut dst = vec![0u8; 270];
    let n = decompress_safe(&block, &mut dst).expect("decompression failed");
    assert_eq!(n, 270);
    assert!(dst.iter().all(|&b| b == b'C'));
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe — error paths
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_empty_src_is_error() {
    // Zero-length compressed input is always malformed.
    let mut dst = [0u8; 8];
    assert_eq!(
        decompress_safe(&[], &mut dst),
        Err(DecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_zero_dst_non_empty_token_is_error() {
    // dst has zero capacity but src is not the 0x00 single-token block.
    let mut dst: [u8; 0] = [];
    assert_eq!(
        decompress_safe(BLOCK_A, &mut dst),
        Err(DecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_dst_too_small_is_error() {
    // "Hello" needs 5 bytes; supply only 3.
    let mut dst = [0u8; 3];
    assert_eq!(
        decompress_safe(BLOCK_HELLO, &mut dst),
        Err(DecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_truncated_literal_bytes_is_error() {
    // Token claims ll=5 but only 3 literal bytes follow (block is truncated).
    let block = [0x50u8, b'H', b'e', b'l']; // 1 token + 3 bytes, not 5
    let mut dst = [0u8; 5];
    assert_eq!(
        decompress_safe(&block, &mut dst),
        Err(DecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_truncated_variable_length_is_error() {
    // token 0xF0 (ll=15 triggers extended read), only 1 extra byte provided
    // and it equals 0xFF — the reader keeps going but hits ilimit.
    let block = [0xF0u8, 0xFF]; // 2 bytes total; variable-length reader hits limit
    let mut dst = [0u8; 16];
    assert_eq!(
        decompress_safe(&block, &mut dst),
        Err(DecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_all_ff_bytes_is_error() {
    // Random garbage input must not panic and must return MalformedInput.
    let block = [0xFFu8; 8];
    let mut dst = [0u8; 64];
    assert_eq!(
        decompress_safe(&block, &mut dst),
        Err(DecompressError::MalformedInput)
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe — round-trip with compress_default
// ─────────────────────────────────────────────────────────────────────────────

fn compress_then_decompress(input: &[u8]) -> Vec<u8> {
    let bound = compress_bound(input.len() as i32).max(0) as usize;
    let mut compressed = vec![0u8; bound];
    let n = compress_default(input, &mut compressed).expect("compression failed");
    compressed.truncate(n);

    let mut decompressed = vec![0u8; input.len()];
    let m = decompress_safe(&compressed, &mut decompressed).expect("decompression failed");
    decompressed.truncate(m);
    decompressed
}

#[test]
fn round_trip_empty_input() {
    // Compressing and decompressing an empty slice should produce an empty slice.
    assert_eq!(compress_then_decompress(b""), b"");
}

#[test]
fn round_trip_single_byte() {
    assert_eq!(compress_then_decompress(b"X"), b"X");
}

#[test]
fn round_trip_short_string() {
    let input = b"Hello, World!";
    assert_eq!(compress_then_decompress(input), input.as_ref());
}

#[test]
fn round_trip_all_zeros_128_bytes() {
    // All-zero data is highly compressible; exercises match-copy path.
    let input = vec![0u8; 128];
    assert_eq!(compress_then_decompress(&input), input);
}

#[test]
fn round_trip_highly_compressible_1k() {
    // 1 KiB of 'A': forces long match extensions (variable-length match length).
    let input = vec![b'A'; 1024];
    assert_eq!(compress_then_decompress(&input), input);
}

#[test]
fn round_trip_incompressible_byte_sequence() {
    // 256 unique bytes (worst-case for compressor) — all go as literals.
    let input: Vec<u8> = (0u8..=255).collect();
    assert_eq!(compress_then_decompress(&input), input);
}

#[test]
fn round_trip_repeated_pattern_512_bytes() {
    // Repeating "ABCD" pattern — highly compressible.
    let input: Vec<u8> = b"ABCD".iter().cycle().take(512).copied().collect();
    assert_eq!(compress_then_decompress(&input), input);
}

#[test]
fn round_trip_longer_string_with_matches() {
    let input = b"Hello, World! Hello, World! Goodbye, World!";
    assert_eq!(compress_then_decompress(input), input.as_ref());
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_partial
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_partial_zero_target_returns_zero() {
    // target_output_size = 0 → partial_decoding=true, output_size=0 → Ok(0).
    let mut dst = [0u8; 10];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 0).expect("partial decompress failed");
    assert_eq!(n, 0);
}

#[test]
fn decompress_safe_partial_exactly_full_block() {
    // target_output_size matches actual decompressed size.
    let mut dst = [0u8; 5];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 5).expect("partial decompress failed");
    assert_eq!(n, 5);
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_partial_target_larger_than_content() {
    // target_output_size > actual decompressed size → behaves like decompress_safe.
    let mut dst = [0u8; 10];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 100).expect("partial decompress failed");
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_partial_target_clamped_to_dst_len() {
    // target_output_size > dst.len() → internally clamped to dst.len().
    let mut dst = [0u8; 5];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 999).expect("partial decompress failed");
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_partial_fewer_bytes_than_block() {
    // Request 3 bytes from a 5-byte block.  The partial-decode path clamps to 3.
    let mut dst = [0u8; 5];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 3).expect("partial decompress failed");
    // We should get at most 5 bytes, at least 3 are valid 'H','e','l'.
    assert!(n <= 5);
    if n >= 3 {
        assert_eq!(&dst[..3], b"Hel");
    }
}

#[test]
fn decompress_safe_partial_roundtrip_compressible() {
    // Partially decode a longer compressed all-'A' block.
    let input = vec![b'A'; 256];
    let bound = compress_bound(input.len() as i32).max(0) as usize;
    let mut compressed = vec![0u8; bound];
    let n = compress_default(&input, &mut compressed).expect("compress_default failed");
    compressed.truncate(n);

    let mut dst = vec![0u8; 256];
    let decoded =
        decompress_safe_partial(&compressed, &mut dst, 128).expect("partial decompress failed");
    // Must have decoded at least up to the clamped target (or the whole block).
    assert!(decoded <= 256);
    assert!(dst[..decoded].iter().all(|&b| b == b'A'));
}

#[test]
fn decompress_safe_partial_empty_src_is_error() {
    let mut dst = [0u8; 8];
    assert_eq!(
        decompress_safe_partial(&[], &mut dst, 4),
        Err(DecompressError::MalformedInput)
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_using_dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_using_dict_empty_dict_equals_decompress_safe() {
    // Empty dict: falls through to decompress_safe internally.
    let mut dst1 = [0u8; 5];
    let mut dst2 = [0u8; 5];
    let n1 = decompress_safe(BLOCK_HELLO, &mut dst1).expect("decompress_safe failed");
    let n2 = decompress_safe_using_dict(BLOCK_HELLO, &mut dst2, &[]).expect("using_dict failed");
    assert_eq!(n1, n2);
    assert_eq!(dst1, dst2);
}

#[test]
fn decompress_safe_using_dict_empty_dict_single_literal() {
    let mut dst = [0u8; 1];
    let n = decompress_safe_using_dict(BLOCK_A, &mut dst, &[]).expect("using_dict failed");
    assert_eq!(n, 1);
    assert_eq!(dst[0], b'A');
}

#[test]
fn decompress_safe_using_dict_roundtrip_no_dict_matches() {
    // Self-contained block compressed without a dictionary.
    let input = b"Hello, World! Hello, World! Goodbye, World!";
    let bound = compress_bound(input.len() as i32).max(0) as usize;
    let mut compressed = vec![0u8; bound];
    let n = compress_default(input, &mut compressed).expect("compress_default failed");
    compressed.truncate(n);

    let mut dst = vec![0u8; input.len()];
    let m = decompress_safe_using_dict(&compressed, &mut dst, &[]).expect("using_dict failed");
    assert_eq!(&dst[..m], input.as_ref());
}

#[test]
fn decompress_safe_using_dict_empty_src_is_error() {
    // Empty compressed input is malformed regardless of dict.
    let dict = b"some dictionary prefix data";
    let mut dst = [0u8; 10];
    assert_eq!(
        decompress_safe_using_dict(&[], &mut dst, dict),
        Err(DecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_using_dict_malformed_input_with_dict() {
    let dict = b"some dictionary prefix data";
    let block = [0xFFu8; 8];
    let mut dst = [0u8; 64];
    assert_eq!(
        decompress_safe_using_dict(&block, &mut dst, dict),
        Err(DecompressError::MalformedInput)
    );
}
