// Unit tests for task-007: LZ4 block decompression public API
//
// Tests verify behavioural parity with lz4.c v1.10.0 (lines 2448–2760):
//   - LZ4_MAX_INPUT_SIZE constant value
//   - BlockDecompressError re-export
//   - Lz4StreamDecode: new(), default(), struct fields initialisation
//   - decompress_safe: basic, edge cases, variable-length literals, error paths
//   - decompress_safe_partial: partial decode, clamping, error paths
//   - set_stream_decode: dictionary configuration and context reset
//   - decoder_ring_buffer_size: valid inputs, minimum block size, over-limit
//   - decompress_safe_force_ext_dict: all-literal blocks, error cases
//   - decompress_safe_partial_force_ext_dict: partial external-dict decode
//   - decompress_safe_using_dict: no-dict fallback, adjacent prefix, ext-dict
//   - decompress_safe_partial_using_dict: partial variants of the above
//   - decompress_safe_continue: first call, contiguous rolling, buffer-wrap paths
//   - Round-trip tests through the API

use lz4::block::compress::{compress_bound, compress_default};
use lz4::block::decompress_api::{
    decoder_ring_buffer_size, decompress_safe, decompress_safe_continue,
    decompress_safe_force_ext_dict, decompress_safe_partial,
    decompress_safe_partial_force_ext_dict, decompress_safe_partial_using_dict,
    decompress_safe_using_dict, set_stream_decode, BlockDecompressError, Lz4StreamDecode,
    LZ4_MAX_INPUT_SIZE,
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
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn compress_input(input: &[u8]) -> Vec<u8> {
    let bound = compress_bound(input.len() as i32).max(0) as usize;
    let mut compressed = vec![0u8; bound];
    let n = compress_default(input, &mut compressed).expect("compression failed");
    compressed.truncate(n);
    compressed
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_lz4_max_input_size_value() {
    // Equivalent to C macro LZ4_MAX_INPUT_SIZE = 0x7E000000.
    assert_eq!(LZ4_MAX_INPUT_SIZE, 0x7E000000usize);
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockDecompressError re-export
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_decompress_error_eq() {
    assert_eq!(
        BlockDecompressError::MalformedInput,
        BlockDecompressError::MalformedInput
    );
}

#[test]
fn block_decompress_error_debug_does_not_panic() {
    let _ = format!("{:?}", BlockDecompressError::MalformedInput);
}

#[test]
fn block_decompress_error_copy() {
    let e = BlockDecompressError::MalformedInput;
    let e2 = e; // Copy trait
    assert_eq!(e, e2);
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4StreamDecode construction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn stream_decode_new_is_usable_for_first_block() {
    // A freshly-created context (prefix_size==0) must work on the first call
    // to decompress_safe_continue without panicking.
    let compressed = compress_input(b"Hello");
    let mut ctx = Lz4StreamDecode::new();
    let mut buf = vec![0u8; 64];
    let n = unsafe {
        decompress_safe_continue(
            &mut ctx,
            compressed.as_ptr(),
            buf.as_mut_ptr(),
            compressed.len(),
            buf.len(),
        )
    }
    .expect("first-call from new() context failed");
    assert_eq!(&buf[..n], b"Hello");
}

#[test]
fn stream_decode_default_is_usable_for_first_block() {
    // Default::default() must behave identically to new().
    let compressed = compress_input(b"World");
    let mut ctx = Lz4StreamDecode::default();
    let mut buf = vec![0u8; 64];
    let n = unsafe {
        decompress_safe_continue(
            &mut ctx,
            compressed.as_ptr(),
            buf.as_mut_ptr(),
            compressed.len(),
            buf.len(),
        )
    }
    .expect("first-call from default() context failed");
    assert_eq!(&buf[..n], b"World");
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe — basic happy paths (API-level wrappers)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_api_single_literal() {
    let mut dst = [0u8; 1];
    let n = decompress_safe(BLOCK_A, &mut dst).expect("decompression failed");
    assert_eq!(n, 1);
    assert_eq!(dst[0], b'A');
}

#[test]
fn decompress_safe_api_five_literals() {
    let mut dst = [0u8; 5];
    let n = decompress_safe(BLOCK_HELLO, &mut dst).expect("decompression failed");
    assert_eq!(n, 5);
    assert_eq!(&dst, b"Hello");
}

#[test]
fn decompress_safe_api_empty_block_zero_capacity_dst() {
    // Matches C: outputSize==0 && src==&0x00 → Ok(0).
    let mut dst: [u8; 0] = [];
    let n = decompress_safe(BLOCK_EMPTY, &mut dst).expect("decompression failed");
    assert_eq!(n, 0);
}

#[test]
fn decompress_safe_api_variable_length_270_literals() {
    // token 0xF0 + [0xFF, 0x00] → 15 + 255 + 0 = 270 literals.
    let mut block = vec![0xF0u8, 0xFF, 0x00];
    block.extend(std::iter::repeat(b'C').take(270));
    let mut dst = vec![0u8; 270];
    let n = decompress_safe(&block, &mut dst).expect("decompression failed");
    assert_eq!(n, 270);
    assert!(dst.iter().all(|&b| b == b'C'));
}

#[test]
fn decompress_safe_api_empty_src_is_error() {
    let mut dst = [0u8; 8];
    assert_eq!(
        decompress_safe(&[], &mut dst),
        Err(BlockDecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_api_dst_too_small_is_error() {
    let mut dst = [0u8; 3];
    assert_eq!(
        decompress_safe(BLOCK_HELLO, &mut dst),
        Err(BlockDecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_api_garbage_input_is_error() {
    let block = [0xFFu8; 8];
    let mut dst = [0u8; 64];
    assert_eq!(
        decompress_safe(&block, &mut dst),
        Err(BlockDecompressError::MalformedInput)
    );
}

#[test]
fn decompress_safe_api_round_trip_compressible() {
    let input = vec![b'A'; 1024];
    let compressed = compress_input(&input);
    let mut dst = vec![0u8; input.len()];
    let n = decompress_safe(&compressed, &mut dst).expect("decompression failed");
    assert_eq!(n, input.len());
    assert_eq!(&dst[..n], input.as_slice());
}

#[test]
fn decompress_safe_api_round_trip_pattern() {
    let input: Vec<u8> = b"ABCD".iter().cycle().take(512).copied().collect();
    let compressed = compress_input(&input);
    let mut dst = vec![0u8; input.len()];
    let n = decompress_safe(&compressed, &mut dst).expect("decompression failed");
    assert_eq!(&dst[..n], input.as_slice());
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_partial
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_partial_api_zero_target_returns_zero() {
    // target_output_size = 0 → Ok(0).
    let mut dst = [0u8; 10];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 0)
        .expect("partial decompress failed");
    assert_eq!(n, 0);
}

#[test]
fn decompress_safe_partial_api_exactly_full_block() {
    let mut dst = [0u8; 5];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 5)
        .expect("partial decompress failed");
    assert_eq!(n, 5);
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_partial_api_target_clamped_to_dst_len() {
    // target_output_size > dst.len() → clamped to dst.len().
    let mut dst = [0u8; 5];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 999)
        .expect("partial decompress failed");
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_partial_api_request_fewer_bytes() {
    // Request 3 of 5 bytes.
    let mut dst = [0u8; 5];
    let n = decompress_safe_partial(BLOCK_HELLO, &mut dst, 3)
        .expect("partial decompress failed");
    assert!(n <= 5);
    if n >= 3 {
        assert_eq!(&dst[..3], b"Hel");
    }
}

#[test]
fn decompress_safe_partial_api_empty_src_is_error() {
    let mut dst = [0u8; 8];
    assert_eq!(
        decompress_safe_partial(&[], &mut dst, 4),
        Err(BlockDecompressError::MalformedInput)
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// decoder_ring_buffer_size
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decoder_ring_buffer_size_standard_value() {
    // Equivalent to C LZ4_decoderRingBufferSize(maxBlockSize).
    // Formula: 65536 + 14 + max(maxBlockSize, 16).
    let size = decoder_ring_buffer_size(65536).expect("should be Some");
    assert_eq!(size, 65536 + 14 + 65536);
}

#[test]
fn decoder_ring_buffer_size_minimum_block_size() {
    // Values below 16 are clamped to 16.
    let size = decoder_ring_buffer_size(1).expect("should be Some");
    assert_eq!(size, 65536 + 14 + 16);
}

#[test]
fn decoder_ring_buffer_size_zero_block_size() {
    // Zero is also clamped to 16.
    let size = decoder_ring_buffer_size(0).expect("should be Some");
    assert_eq!(size, 65536 + 14 + 16);
}

#[test]
fn decoder_ring_buffer_size_exactly_16() {
    let size = decoder_ring_buffer_size(16).expect("should be Some");
    assert_eq!(size, 65536 + 14 + 16);
}

#[test]
fn decoder_ring_buffer_size_exceeds_max_is_none() {
    // max_block_size > LZ4_MAX_INPUT_SIZE → None (mirrors C returning 0).
    assert_eq!(decoder_ring_buffer_size(LZ4_MAX_INPUT_SIZE + 1), None);
}

#[test]
fn decoder_ring_buffer_size_exactly_max_is_some() {
    // LZ4_MAX_INPUT_SIZE itself is valid.
    assert!(decoder_ring_buffer_size(LZ4_MAX_INPUT_SIZE).is_some());
}

// ─────────────────────────────────────────────────────────────────────────────
// set_stream_decode
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn set_stream_decode_empty_dict_allows_subsequent_decode() {
    // Empty dict resets context; first decompress_safe_continue call should work.
    let mut ctx = Lz4StreamDecode::new();
    let result = unsafe { set_stream_decode(&mut ctx, &[]) };
    assert!(result, "set_stream_decode should return true");
    // Verify reset by decoding a block immediately.
    let compressed = compress_input(b"test");
    let mut buf = vec![0u8; 64];
    let n = unsafe {
        decompress_safe_continue(
            &mut ctx,
            compressed.as_ptr(),
            buf.as_mut_ptr(),
            compressed.len(),
            buf.len(),
        )
    }
    .expect("decode after set_stream_decode(empty) failed");
    assert_eq!(&buf[..n], b"test");
}

#[test]
fn set_stream_decode_non_empty_dict_returns_true() {
    // Non-empty dict: must return true. Behaviour beyond that is internal.
    let dict = b"dictionary content for streaming";
    let mut ctx = Lz4StreamDecode::new();
    let result = unsafe { set_stream_decode(&mut ctx, dict) };
    assert!(result, "set_stream_decode should return true");
}

#[test]
fn set_stream_decode_returns_true() {
    // Mirrors C returning 1 on success.
    let mut ctx = Lz4StreamDecode::new();
    assert!(unsafe { set_stream_decode(&mut ctx, b"hello") });
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_force_ext_dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_force_ext_dict_all_literal_block() {
    // All-literal block: does not reference the dict, so any dict is fine.
    let mut dst = vec![0u8; 5];
    let dict = b"some dictionary data";
    let n = unsafe {
        decompress_safe_force_ext_dict(
            BLOCK_HELLO.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_HELLO.len(),
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    }
    .expect("decompression failed");
    assert_eq!(n, 5);
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_force_ext_dict_single_literal() {
    let mut dst = vec![0u8; 1];
    let dict = [0u8; 0];
    let n = unsafe {
        decompress_safe_force_ext_dict(
            BLOCK_A.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_A.len(),
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    }
    .expect("decompression failed");
    assert_eq!(n, 1);
    assert_eq!(dst[0], b'A');
}

#[test]
fn decompress_safe_force_ext_dict_garbage_input_is_error() {
    let block = [0xFFu8; 8];
    let mut dst = vec![0u8; 64];
    let dict = b"dict";
    let result = unsafe {
        decompress_safe_force_ext_dict(
            block.as_ptr(),
            dst.as_mut_ptr(),
            block.len(),
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    };
    assert_eq!(result, Err(BlockDecompressError::MalformedInput));
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_partial_force_ext_dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_partial_force_ext_dict_clamps_to_target() {
    // Request 3 of 5 bytes from an all-literal block with an ignored dict.
    let dict = b"some dictionary";
    let mut dst = vec![0u8; 5];
    let n = unsafe {
        decompress_safe_partial_force_ext_dict(
            BLOCK_HELLO.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_HELLO.len(),
            3,
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    }
    .expect("partial ext_dict decompression failed");
    assert!(n <= 5);
    if n >= 3 {
        assert_eq!(&dst[..3], b"Hel");
    }
}

#[test]
fn decompress_safe_partial_force_ext_dict_zero_target() {
    let dict = b"dict";
    let mut dst = vec![0u8; 5];
    let n = unsafe {
        decompress_safe_partial_force_ext_dict(
            BLOCK_HELLO.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_HELLO.len(),
            0,
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    }
    .expect("zero-target partial decompress failed");
    assert_eq!(n, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_using_dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_using_dict_no_dict_matches_decompress_safe() {
    // dict_size==0 → internal fallback to decompress_safe.
    let mut dst1 = [0u8; 5];
    let mut dst2 = [0u8; 5];
    let n1 = decompress_safe(BLOCK_HELLO, &mut dst1).expect("decompress_safe failed");
    let dict = [0u8; 0];
    let n2 = unsafe {
        decompress_safe_using_dict(
            BLOCK_HELLO.as_ptr(),
            dst2.as_mut_ptr(),
            BLOCK_HELLO.len(),
            dst2.len(),
            dict.as_ptr(),
            0,
        )
    }
    .expect("using_dict (no dict) failed");
    assert_eq!(n1, n2);
    assert_eq!(dst1, dst2);
}

#[test]
fn decompress_safe_using_dict_single_literal_no_dict() {
    let mut dst = [0u8; 1];
    let dict = [0u8; 0];
    let n = unsafe {
        decompress_safe_using_dict(
            BLOCK_A.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_A.len(),
            dst.len(),
            dict.as_ptr(),
            0,
        )
    }
    .expect("using_dict failed");
    assert_eq!(n, 1);
    assert_eq!(dst[0], b'A');
}

#[test]
fn decompress_safe_using_dict_round_trip_no_dict() {
    // Compress without a dict, decompress without a dict via decompress_safe_using_dict.
    let input = b"Hello, World! Hello, World! Goodbye, World!";
    let compressed = compress_input(input);
    let mut dst = vec![0u8; input.len()];
    let dict = [0u8; 0];
    let n = unsafe {
        decompress_safe_using_dict(
            compressed.as_ptr(),
            dst.as_mut_ptr(),
            compressed.len(),
            dst.len(),
            dict.as_ptr(),
            0,
        )
    }
    .expect("using_dict round-trip failed");
    assert_eq!(&dst[..n], input.as_ref());
}

#[test]
fn decompress_safe_using_dict_adjacent_small_prefix_path() {
    // When dict_start + dict_size == dst_ptr (adjacent) and dict_size < 64KiB-1,
    // the small-prefix path is taken. Test with an all-literal block.
    // dict and dst must be adjacent: dict.as_ptr() + dict.len() == dst.as_ptr()
    // Allocate a combined buffer so the regions are guaranteed contiguous.
    let mut buf = vec![0u8; 32 + 5];
    let dict_ptr = buf.as_ptr();
    let dst_ptr = unsafe { buf.as_mut_ptr().add(32) };

    // Write BLOCK_HELLO output into the dst region by decoding.
    let n = unsafe {
        decompress_safe_using_dict(
            BLOCK_HELLO.as_ptr(),
            dst_ptr,
            BLOCK_HELLO.len(),
            5,
            dict_ptr,
            32,
        )
    }
    .expect("adjacent prefix path failed");
    assert_eq!(n, 5);
    assert_eq!(&buf[32..32 + n], b"Hello");
}

#[test]
fn decompress_safe_using_dict_garbage_input_is_error() {
    let block = [0xFFu8; 8];
    let mut dst = [0u8; 64];
    let dict = b"dict";
    let result = unsafe {
        decompress_safe_using_dict(
            block.as_ptr(),
            dst.as_mut_ptr(),
            block.len(),
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    };
    assert_eq!(result, Err(BlockDecompressError::MalformedInput));
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_partial_using_dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_partial_using_dict_zero_target_no_dict() {
    let dict = [0u8; 0];
    let mut dst = [0u8; 10];
    let n = unsafe {
        decompress_safe_partial_using_dict(
            BLOCK_HELLO.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_HELLO.len(),
            0,
            dst.len(),
            dict.as_ptr(),
            0,
        )
    }
    .expect("partial using_dict zero-target failed");
    assert_eq!(n, 0);
}

#[test]
fn decompress_safe_partial_using_dict_full_decode_no_dict() {
    let dict = [0u8; 0];
    let mut dst = [0u8; 5];
    let n = unsafe {
        decompress_safe_partial_using_dict(
            BLOCK_HELLO.as_ptr(),
            dst.as_mut_ptr(),
            BLOCK_HELLO.len(),
            5,
            dst.len(),
            dict.as_ptr(),
            0,
        )
    }
    .expect("partial using_dict full-block failed");
    assert_eq!(n, 5);
    assert_eq!(&dst[..n], b"Hello");
}

#[test]
fn decompress_safe_partial_using_dict_garbage_is_error() {
    let block = [0xFFu8; 8];
    let mut dst = [0u8; 64];
    let dict = b"dict";
    let result = unsafe {
        decompress_safe_partial_using_dict(
            block.as_ptr(),
            dst.as_mut_ptr(),
            block.len(),
            64,
            dst.len(),
            dict.as_ptr(),
            dict.len(),
        )
    };
    assert_eq!(result, Err(BlockDecompressError::MalformedInput));
}

// ─────────────────────────────────────────────────────────────────────────────
// decompress_safe_continue — streaming
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_safe_continue_first_call_no_prefix() {
    // When ctx.prefix_size == 0, the first-call path (safe decompress_safe) is used.
    let compressed = compress_input(b"Hello");
    let mut ctx = Lz4StreamDecode::new();
    let mut buf = vec![0u8; 128];
    let n = unsafe {
        decompress_safe_continue(
            &mut ctx,
            compressed.as_ptr(),
            buf.as_mut_ptr(),
            compressed.len(),
            buf.len(),
        )
    }
    .expect("decompress_safe_continue first call failed");
    assert_eq!(&buf[..n], b"Hello");
}

#[test]
fn decompress_safe_continue_contiguous_second_block() {
    // Second call where dst == ctx.prefix_end (contiguous rolling buffer).
    let input1 = b"Hello, ";
    let input2 = b"World!";
    let c1 = compress_input(input1);
    let c2 = compress_input(input2);

    // Allocate a single backing buffer large enough for both outputs.
    let mut buf = vec![0u8; 256];
    let mut ctx = Lz4StreamDecode::new();

    // First call — fills buf[0..n1].
    let n1 = unsafe {
        decompress_safe_continue(
            &mut ctx,
            c1.as_ptr(),
            buf.as_mut_ptr(),
            c1.len(),
            128,
        )
    }
    .expect("first block failed");
    assert_eq!(&buf[..n1], input1.as_ref());

    // Second call — dst = buf[n1..], which is immediately after the first output.
    let n2 = unsafe {
        decompress_safe_continue(
            &mut ctx,
            c2.as_ptr(),
            buf.as_mut_ptr().add(n1),
            c2.len(),
            128,
        )
    }
    .expect("second block failed");
    assert_eq!(&buf[n1..n1 + n2], input2.as_ref());
}

#[test]
fn decompress_safe_continue_non_contiguous_uses_ext_dict() {
    // When dst != ctx.prefix_end, the previous prefix becomes the external dict.
    let input1 = b"First block content";
    let c1 = compress_input(input1);

    let mut buf1 = vec![0u8; 128];
    let mut buf2 = vec![0u8; 128];
    let mut ctx = Lz4StreamDecode::new();

    // First call into buf1.
    let n1 = unsafe {
        decompress_safe_continue(
            &mut ctx,
            c1.as_ptr(),
            buf1.as_mut_ptr(),
            c1.len(),
            buf1.len(),
        )
    }
    .expect("first block failed");
    assert_eq!(&buf1[..n1], input1.as_ref());

    // Second call into buf2 (not contiguous with buf1 → ext-dict path).
    let input2 = b"Second block";
    let c2 = compress_input(input2);
    let n2 = unsafe {
        decompress_safe_continue(
            &mut ctx,
            c2.as_ptr(),
            buf2.as_mut_ptr(),
            c2.len(),
            buf2.len(),
        )
    }
    .expect("non-contiguous second block failed");
    assert_eq!(&buf2[..n2], input2.as_ref());
}

#[test]
fn decompress_safe_continue_first_call_malformed_is_error() {
    // Ensure errors propagate correctly from the first-call path.
    let mut ctx = Lz4StreamDecode::new();
    let mut buf = vec![0u8; 64];
    let block = [0xFFu8; 8];
    let result = unsafe {
        decompress_safe_continue(
            &mut ctx,
            block.as_ptr(),
            buf.as_mut_ptr(),
            block.len(),
            buf.len(),
        )
    };
    assert_eq!(result, Err(BlockDecompressError::MalformedInput));
}
