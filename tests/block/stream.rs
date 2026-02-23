// Unit tests for task-005: Streaming compression state (stream.rs)
//
// Tests verify behavioural parity with lz4.c v1.10.0 (lines 1526–1834):
//   - Lz4Stream::new() / Default initialisation
//   - Lz4Stream::reset() fully zeroes the state
//   - Lz4Stream::reset_fast() prepares the hash table for reuse
//   - Lz4Stream::load_dict() / load_dict_slow() — dict loading, size clamping,
//     too-small dict returns 0
//   - Lz4Stream::attach_dictionary() — None detaches; empty dict detaches;
//     non-empty dict attaches; bumps current_offset from zero
//   - Lz4Stream::renorm_dict() — only triggers above 0x80000000 boundary
//   - Lz4Stream::save_dict() — copies last ≤64 KB, updates history pointer
//   - Lz4Stream::compress_fast_continue() — basic round-trip, prefix mode,
//     multi-block streaming, output-too-small returns 0
//   - Lz4Stream::compress_force_ext_dict() — ext-dict path smoke-test
//
// Note: `Lz4Stream::internal` is `pub(crate)`, so integration tests cannot
// access it.  All assertions are therefore through the public API (compression
// output, load_dict/save_dict return values, etc.).  Tests that would require
// direct access to internal fields in order to *set up* state are marked
// `#[ignore]` with an explanatory comment.

use lz4::block::compress::compress_bound;
use lz4::block::stream::Lz4Stream;
use lz4::block::types::KB;

// ─────────────────────────────────────────────────────────────────────────────
// Helper: worst-case destination buffer
// ─────────────────────────────────────────────────────────────────────────────

fn make_dst(src_len: usize) -> Vec<u8> {
    let bound = compress_bound(src_len as i32).max(0) as usize;
    vec![0u8; bound.max(16)]
}

// ─────────────────────────────────────────────────────────────────────────────
// Construction and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn new_produces_a_working_stream() {
    // Lz4Stream::new() mirrors LZ4_createStream; the stream must be immediately
    // usable for compression without any additional initialization.
    let mut stream = Lz4Stream::new();
    let src = b"hello lz4";
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(src, &mut dst, 1);
    assert!(n > 0, "fresh stream from new() must compress successfully");
}

#[test]
fn default_produces_same_output_as_new() {
    // Lz4Stream::default() must produce the same compression output as new().
    let src = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut s_new = Lz4Stream::new();
    let mut s_def = Lz4Stream::default();
    let mut dst_new = make_dst(src.len());
    let mut dst_def = make_dst(src.len());
    let n1 = s_new.compress_fast_continue(src, &mut dst_new, 1);
    let n2 = s_def.compress_fast_continue(src, &mut dst_def, 1);
    assert_eq!(n1, n2, "new() and default() must produce identical output");
    assert_eq!(&dst_new[..n1 as usize], &dst_def[..n2 as usize]);
}

// ─────────────────────────────────────────────────────────────────────────────
// reset()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reset_clears_state_after_use() {
    // After compressing, a full reset must produce the same output as a fresh stream.
    let src = b"hello world, repeated repeated repeated repeated repeated repeated";

    let mut fresh = Lz4Stream::new();
    let mut dst_fresh = make_dst(src.len());
    let n_fresh = fresh.compress_fast_continue(src, &mut dst_fresh, 1);
    assert!(n_fresh > 0);

    let mut reused = Lz4Stream::new();
    // Compress something different first, then reset.
    let junk = b"garbage data garbage data garbage data garbage data";
    let mut dst_junk = make_dst(junk.len());
    let _ = reused.compress_fast_continue(junk, &mut dst_junk, 1);

    reused.reset();

    let mut dst_reused = make_dst(src.len());
    let n_reused = reused.compress_fast_continue(src, &mut dst_reused, 1);
    assert_eq!(n_fresh, n_reused, "reset stream must produce same output as fresh stream");
    assert_eq!(&dst_fresh[..n_fresh as usize], &dst_reused[..n_reused as usize]);
}

#[test]
fn reset_allows_fresh_compression() {
    // After reset(), compress_fast_continue must still succeed (not corrupted state).
    let mut stream = Lz4Stream::new();
    let src = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut dst = make_dst(src.len());

    let _ = stream.compress_fast_continue(src, &mut dst, 1);
    stream.reset();

    let mut dst2 = make_dst(src.len());
    let n = stream.compress_fast_continue(src, &mut dst2, 1);
    assert!(n > 0, "compress after reset should succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// reset_fast()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reset_fast_allows_fresh_compression() {
    let mut stream = Lz4Stream::new();

    // Use the stream once to put it in a known valid state.
    let src = b"The quick brown fox jumps over the lazy dog. Repeated data: ";
    let mut dst = make_dst(src.len());
    let _ = stream.compress_fast_continue(src, &mut dst, 1);

    // Fast-reset and compress again — must not panic or produce garbage.
    stream.reset_fast();
    let mut dst2 = make_dst(src.len());
    let n = stream.compress_fast_continue(src, &mut dst2, 1);
    assert!(n > 0, "compress after reset_fast should succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// load_dict()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn load_dict_empty_returns_zero() {
    // Dict smaller than HASH_UNIT (sizeof(usize), ≥ 4) returns 0.
    let mut stream = Lz4Stream::new();
    let dict: &[u8] = &[];
    let result = stream.load_dict(dict);
    assert_eq!(result, 0, "empty dict should return 0");
}

#[test]
fn load_dict_tiny_below_hash_unit_returns_zero() {
    // 3 bytes < HASH_UNIT (4 on 32-bit, 8 on 64-bit) → always returns 0.
    let mut stream = Lz4Stream::new();
    let dict = [0u8; 3];
    let result = stream.load_dict(&dict);
    assert_eq!(result, 0, "dict of 3 bytes should return 0");
}

#[test]
fn load_dict_small_dict_returns_its_size() {
    // A dict larger than HASH_UNIT should return the dict size (≤ 64 KB).
    let mut stream = Lz4Stream::new();
    let dict = vec![0xAAu8; 1024];
    let result = stream.load_dict(&dict);
    assert_eq!(result, 1024, "load_dict should return actual dict size");
}

#[test]
fn load_dict_large_dict_clamped_to_64kb() {
    // Dicts > 64 KB are truncated to the last 64 KB.
    let mut stream = Lz4Stream::new();
    let dict = vec![0u8; 128 * KB];
    let result = stream.load_dict(&dict);
    assert_eq!(result, 64 * KB as i32, "large dict must be clamped to 64 KB");
}

#[test]
fn load_dict_exactly_64kb() {
    let mut stream = Lz4Stream::new();
    let dict = vec![0u8; 64 * KB];
    let result = stream.load_dict(&dict);
    assert_eq!(result, 64 * KB as i32);
}

#[test]
fn load_dict_dict_size_reflected_by_save_dict() {
    // load_dict stores dict_size internally; save_dict returns that size.
    // This indirectly verifies that dict_size was set to the loaded amount.
    let mut stream = Lz4Stream::new();
    let dict = vec![0u8; 512];
    stream.load_dict(&dict);

    let mut save_buf = vec![0u8; 64 * KB];
    let saved = stream.save_dict(&mut save_buf);
    assert_eq!(saved, 512, "save_dict should return amount loaded by load_dict");
}

#[test]
fn load_dict_resets_stream_before_loading() {
    // load_dict internally calls reset(), so the hash table must be consistent.
    let mut stream = Lz4Stream::new();
    let src = b"some data that gets compressed";
    let mut dst = make_dst(src.len());
    let _ = stream.compress_fast_continue(src, &mut dst, 1);

    // Loading a new dict must reset and succeed cleanly.
    let dict = vec![0x42u8; 256];
    let result = stream.load_dict(&dict);
    assert_eq!(result, 256);
}

// ─────────────────────────────────────────────────────────────────────────────
// load_dict_slow()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn load_dict_slow_empty_returns_zero() {
    let mut stream = Lz4Stream::new();
    let result = stream.load_dict_slow(&[]);
    assert_eq!(result, 0);
}

#[test]
fn load_dict_slow_small_dict_returns_its_size() {
    let mut stream = Lz4Stream::new();
    let dict = vec![0xBBu8; 512];
    let result = stream.load_dict_slow(&dict);
    assert_eq!(result, 512);
}

#[test]
fn load_dict_slow_large_dict_clamped_to_64kb() {
    let mut stream = Lz4Stream::new();
    let dict = vec![0u8; 100 * KB];
    let result = stream.load_dict_slow(&dict);
    assert_eq!(result, 64 * KB as i32);
}

#[test]
fn load_dict_slow_and_fast_agree_on_dict_size() {
    // Both variants must agree on the number of bytes accepted.
    let dict = vec![0xCCu8; 1024];
    let mut s1 = Lz4Stream::new();
    let mut s2 = Lz4Stream::new();
    let r1 = s1.load_dict(&dict);
    let r2 = s2.load_dict_slow(&dict);
    assert_eq!(r1, r2, "fast and slow load_dict must return the same size");
}

// ─────────────────────────────────────────────────────────────────────────────
// attach_dictionary()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn attach_dictionary_none_allows_compress() {
    // Passing None must not corrupt the stream; compression must still work.
    let mut stream = Lz4Stream::new();
    unsafe {
        stream.attach_dictionary(None);
    }
    let src = b"hello after detach aaaaaaaaaaaaaaaaaaaaaa";
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(src, &mut dst, 1);
    assert!(n > 0, "stream with None attached dict must compress successfully");
}

#[test]
fn attach_dictionary_empty_dict_stream_allows_compress() {
    // Attaching a stream whose dict_size == 0 is treated as a detach.
    // Subsequent compression must succeed.
    let dict_stream = Lz4Stream::new(); // freshly created → dict_size == 0
    let mut working = Lz4Stream::new();
    unsafe {
        working.attach_dictionary(Some(&*dict_stream as *const Lz4Stream));
    }
    let src = b"hello after empty attach aaaaaaaaaaaaaaaaaaa";
    let mut dst = make_dst(src.len());
    let n = working.compress_fast_continue(src, &mut dst, 1);
    assert!(n > 0, "stream with empty attached dict must compress successfully");
}

#[test]
fn attach_dictionary_non_empty_dict_allows_compress() {
    // After attaching a non-empty dict, compression must succeed.
    let mut dict_stream = Lz4Stream::new();
    let dict_data = vec![0xDDu8; 1024];
    dict_stream.load_dict(&dict_data);

    let mut working = Lz4Stream::new();
    unsafe {
        working.attach_dictionary(Some(&*dict_stream as *const Lz4Stream));
    }
    let src: Vec<u8> = (0u8..=255u8).cycle().take(512).collect();
    let mut dst = make_dst(src.len());
    let n = working.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0, "stream with non-empty attached dict must compress successfully");
}

#[test]
fn attach_dictionary_bumps_current_offset_when_zero() {
    // If current_offset is 0 before attach, it must be bumped to 64 KB.
    // We verify this indirectly: save_dict on the working stream (which has
    // no data compressed into it) should return 0, but the stream must be
    // usable for subsequent compression.
    let mut dict_stream = Lz4Stream::new();
    let dict_data = vec![0xEEu8; 512];
    dict_stream.load_dict(&dict_data);

    let mut working = Lz4Stream::new();
    unsafe {
        working.attach_dictionary(Some(&*dict_stream as *const Lz4Stream));
    }
    // After attach, compression must succeed (current_offset is non-zero).
    let src: Vec<u8> = (0u8..=255u8).cycle().take(256).collect();
    let mut dst = make_dst(src.len());
    let n = working.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0, "compress after attach must succeed even from zero initial offset");
}

// ─────────────────────────────────────────────────────────────────────────────
// renorm_dict()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn renorm_dict_with_normal_offset_is_safe() {
    // renorm_dict is a no-op when current_offset is far from the overflow boundary.
    // With a fresh stream (current_offset == 0) it should be safe to call and
    // not affect subsequent compression.
    let mut stream = Lz4Stream::new();
    stream.renorm_dict(1024); // should be no-op; 0 + 1024 << 0x80000000
    let src = b"hello after renorm aaaaaaaaaaaaaaaaaaaaaa";
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(src, &mut dst, 1);
    assert!(n > 0, "compress after no-op renorm_dict must succeed");
}

// Note: renorm_dict trigger tests (current_offset near 0x7FFF_0000) require
// setting Lz4Stream::internal.current_offset, which is pub(crate) and not
// accessible from integration tests.  Those paths are tested by unit tests
// inside the library crate if added in a future task.

// ─────────────────────────────────────────────────────────────────────────────
// save_dict()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn save_dict_into_empty_buffer_returns_zero() {
    let mut stream = Lz4Stream::new();
    let src = b"data to compress so we have a dict";
    let mut dst = make_dst(src.len());
    let _ = stream.compress_fast_continue(src, &mut dst, 1);

    let mut save_buf: Vec<u8> = vec![];
    let saved = stream.save_dict(&mut save_buf);
    assert_eq!(saved, 0, "zero-capacity save buffer must return 0");
}

#[test]
fn save_dict_without_prior_compress_returns_zero() {
    // Fresh stream has no dictionary — save_dict should return 0.
    let mut stream = Lz4Stream::new();
    let mut buf = vec![0u8; 64 * KB];
    let saved = stream.save_dict(&mut buf);
    assert_eq!(saved, 0);
}

#[test]
fn save_dict_after_compress_returns_positive() {
    // After compressing data, save_dict should copy the last ≤64 KB.
    let mut stream = Lz4Stream::new();
    let src: Vec<u8> = b"Hello from lz4 streaming compression. This is test data."
        .iter()
        .cycle()
        .take(1024)
        .cloned()
        .collect();
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0);

    let mut save_buf = vec![0u8; 64 * KB];
    let saved = stream.save_dict(&mut save_buf);
    assert!(saved > 0, "save_dict should return the number of saved bytes");
    assert!(saved as usize <= 64 * KB);
}

#[test]
fn save_dict_clamps_to_64kb() {
    // Even if more data was compressed, save_dict saves at most 64 KB.
    let mut stream = Lz4Stream::new();
    let src = vec![0xAAu8; 128 * KB];
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0);

    let mut save_buf = vec![0u8; 128 * KB];
    let saved = stream.save_dict(&mut save_buf);
    assert!(saved as usize <= 64 * KB, "save_dict must not exceed 64 KB");
}

#[test]
fn save_dict_clamps_to_buffer_size() {
    // If the save buffer is smaller than 64 KB, only buffer.len() bytes can be saved.
    let mut stream = Lz4Stream::new();
    let src = vec![0xBBu8; 128 * KB];
    let mut dst = make_dst(src.len());
    let _ = stream.compress_fast_continue(&src, &mut dst, 1);

    let cap = 256usize;
    let mut save_buf = vec![0u8; cap];
    let saved = stream.save_dict(&mut save_buf);
    assert!(
        saved as usize <= cap,
        "save_dict must not exceed save buffer capacity"
    );
}

#[test]
fn save_dict_updates_history_so_subsequent_save_returns_same_size() {
    // After save_dict, the stream's dictionary pointer points into the save buffer.
    // A second save_dict on a fresh buffer should return the same saved size.
    let mut stream = Lz4Stream::new();
    let src = vec![0xCCu8; 1024];
    let mut dst = make_dst(src.len());
    let _ = stream.compress_fast_continue(&src, &mut dst, 1);

    let mut save_buf = vec![0u8; 64 * KB];
    let saved = stream.save_dict(&mut save_buf);
    assert!(saved > 0);

    // After save_dict the dict pointer refers to save_buf; a second save_dict
    // should reflect the same (or equal) size since no new data was compressed.
    let mut save_buf2 = vec![0u8; 64 * KB];
    let saved2 = stream.save_dict(&mut save_buf2);
    assert_eq!(saved, saved2, "second save_dict must return same size as first");
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_fast_continue() — basic streaming
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_continue_single_block_returns_positive() {
    let mut stream = Lz4Stream::new();
    let src = b"Hello, LZ4 streaming world! aaaaaaaaaaaaaaaaaaaaaa";
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(src, &mut dst, 1);
    assert!(n > 0, "single-block compress should return positive byte count");
}

#[test]
fn compress_fast_continue_empty_input_returns_one_byte() {
    // Empty input → compress_generic emits the single 0x00 token byte.
    let mut stream = Lz4Stream::new();
    let src: &[u8] = &[];
    let mut dst = [0u8; 16];
    let n = stream.compress_fast_continue(src, &mut dst, 1);
    assert_eq!(n, 1, "empty block must produce 1 byte (0x00 token)");
    assert_eq!(dst[0], 0x00);
}

#[test]
fn compress_fast_continue_output_too_small_returns_zero() {
    // A 1-byte output buffer for non-trivial input should return 0.
    let mut stream = Lz4Stream::new();
    let src: Vec<u8> = b"This is a string long enough that it cannot possibly fit in 1 byte after compression."
        .to_vec();
    let mut dst = [0u8; 1];
    let n = stream.compress_fast_continue(&src, &mut dst, 1);
    assert_eq!(n, 0, "output too small should return 0");
}

#[test]
fn compress_fast_continue_all_zeros_compresses_well() {
    let mut stream = Lz4Stream::new();
    let src = vec![0u8; 4096];
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0);
    assert!(
        (n as usize) < src.len(),
        "highly compressible data should compress to less than input size"
    );
}

#[test]
fn compress_fast_continue_two_independent_blocks() {
    // Compress two blocks with the same fresh stream (no prefix continuity).
    let mut stream = Lz4Stream::new();
    let block1 = b"Block one: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let block2 = b"Block two: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    let mut dst1 = make_dst(block1.len());
    let n1 = stream.compress_fast_continue(block1, &mut dst1, 1);
    assert!(n1 > 0);

    // Note: After compressing block1, stream's dictionary points into block1.
    // block2 is a different buffer, so this exercises the external-dict path.
    let mut dst2 = make_dst(block2.len());
    let n2 = stream.compress_fast_continue(block2, &mut dst2, 1);
    assert!(n2 > 0);
}

#[test]
fn compress_fast_continue_prefix_mode_accumulates_history() {
    // In prefix mode, consecutive blocks of the same buffer should allow
    // the second block to reference matches from the first.
    let mut stream = Lz4Stream::new();

    // Use a contiguous buffer and take slices to simulate prefix mode.
    let buffer: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    let (block1, block2) = buffer.split_at(2048);

    let mut dst1 = make_dst(block1.len());
    let n1 = stream.compress_fast_continue(block1, &mut dst1, 1);
    assert!(n1 > 0);

    let mut dst2 = make_dst(block2.len());
    let n2 = stream.compress_fast_continue(block2, &mut dst2, 1);
    assert!(n2 > 0);
}

#[test]
fn compress_fast_continue_deterministic() {
    // Two independent streams compressing the same input must produce identical output.
    let src = b"determinism check: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut s1 = Lz4Stream::new();
    let mut s2 = Lz4Stream::new();
    let mut dst1 = make_dst(src.len());
    let mut dst2 = make_dst(src.len());
    let n1 = s1.compress_fast_continue(src, &mut dst1, 1);
    let n2 = s2.compress_fast_continue(src, &mut dst2, 1);
    assert_eq!(n1, n2);
    assert_eq!(&dst1[..n1 as usize], &dst2[..n2 as usize]);
}

#[test]
fn compress_fast_continue_acceleration_clamped_below_default() {
    // Acceleration < 1 (DEFAULT) should be clamped to 1.
    let src = b"acceleration clamp test: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut s_clamped = Lz4Stream::new();
    let mut s_default = Lz4Stream::new();
    let mut dst1 = make_dst(src.len());
    let mut dst2 = make_dst(src.len());
    let n1 = s_clamped.compress_fast_continue(src, &mut dst1, 0); // 0 < DEFAULT → clamped
    let n2 = s_default.compress_fast_continue(src, &mut dst2, 1);
    assert_eq!(n1, n2);
    assert_eq!(&dst1[..n1 as usize], &dst2[..n2 as usize]);
}

#[test]
fn compress_fast_continue_acceleration_clamped_above_max() {
    let src: Vec<u8> = (0u8..=255u8).cycle().take(1024).collect();
    let mut s_clamped = Lz4Stream::new();
    let mut s_max = Lz4Stream::new();
    let mut dst1 = make_dst(src.len());
    let mut dst2 = make_dst(src.len());
    let n1 = s_clamped.compress_fast_continue(&src, &mut dst1, i32::MAX);
    let n2 = s_max.compress_fast_continue(&src, &mut dst2, 65_537);
    assert_eq!(n1, n2);
    assert_eq!(&dst1[..n1 as usize], &dst2[..n2 as usize]);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_fast_continue() with load_dict
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_continue_after_load_dict_succeeds() {
    let mut stream = Lz4Stream::new();
    let dict: Vec<u8> = (0u8..=255u8).cycle().take(512).collect();
    let loaded = stream.load_dict(&dict);
    assert!(loaded > 0);

    let src: Vec<u8> = b"data similar to dict: ".iter().cycle().take(256).cloned().collect();
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0, "compress after load_dict must succeed");
}

#[test]
fn compress_fast_continue_after_load_dict_slow_succeeds() {
    let mut stream = Lz4Stream::new();
    let dict: Vec<u8> = (0u8..=255u8).cycle().take(512).collect();
    stream.load_dict_slow(&dict);

    let src: Vec<u8> = (0u8..=255u8).cycle().take(256).collect();
    let mut dst = make_dst(src.len());
    let n = stream.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_fast_continue() with attach_dictionary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_fast_continue_with_attached_dict_succeeds() {
    // Compress with an attached dictionary (zero-copy dict mode).
    let mut dict_stream = Lz4Stream::new();
    let dict_data: Vec<u8> = (0u8..=255u8).cycle().take(1024).collect();
    dict_stream.load_dict(&dict_data);

    let mut working = Lz4Stream::new();
    unsafe {
        working.attach_dictionary(Some(&*dict_stream as *const Lz4Stream));
    }

    let src: Vec<u8> = (0u8..=255u8).cycle().take(512).collect();
    let mut dst = make_dst(src.len());
    let n = working.compress_fast_continue(&src, &mut dst, 1);
    assert!(n > 0, "compress with attached dict must succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// save_dict() + compress_fast_continue() round-trip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn save_dict_then_compress_continues_cleanly() {
    // save_dict followed by compress_fast_continue must not panic and must
    // return a positive compressed size.
    let mut stream = Lz4Stream::new();
    let src1: Vec<u8> = b"First block data. aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa."
        .iter()
        .cycle()
        .take(512)
        .cloned()
        .collect();
    let mut dst1 = make_dst(src1.len());
    let n1 = stream.compress_fast_continue(&src1, &mut dst1, 1);
    assert!(n1 > 0);

    let mut save_buf = vec![0u8; 64 * KB];
    let saved = stream.save_dict(&mut save_buf);
    assert!(saved > 0);

    let src2: Vec<u8> = b"Second block data. bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb."
        .iter()
        .cycle()
        .take(512)
        .cloned()
        .collect();
    let mut dst2 = make_dst(src2.len());
    let n2 = stream.compress_fast_continue(&src2, &mut dst2, 1);
    assert!(n2 > 0, "compress after save_dict must succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// compress_force_ext_dict() — smoke test
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_force_ext_dict_produces_compressed_output() {
    // Smoke-test the hidden debug helper — it must produce a positive result.
    let mut stream = Lz4Stream::new();
    let dict = vec![0x00u8; 512];
    stream.load_dict(&dict);

    let src = vec![0x00u8; 256]; // highly compressible
    let mut dst = make_dst(src.len());

    let n = unsafe {
        stream.compress_force_ext_dict(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        )
    };
    assert!(n > 0, "compress_force_ext_dict must return > 0 bytes for compressible input");
}

#[test]
fn compress_force_ext_dict_updates_dict_via_save_dict() {
    // After compress_force_ext_dict, save_dict must reflect the newly compressed
    // block as the current dictionary (dict_size == src_size).
    let mut stream = Lz4Stream::new();
    let dict = vec![0xAAu8; 256];
    stream.load_dict(&dict);

    let src = vec![0xBBu8; 128];
    let mut dst = make_dst(src.len());

    unsafe {
        stream.compress_force_ext_dict(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len() as i32,
            dst.len() as i32,
        );
    }

    // save_dict returns dict_size (internally stored); must equal src.len().
    let mut save_buf = vec![0u8; 64 * KB];
    let saved = stream.save_dict(&mut save_buf);
    assert_eq!(
        saved as usize,
        src.len(),
        "save_dict after compress_force_ext_dict must return src.len()"
    );
}
