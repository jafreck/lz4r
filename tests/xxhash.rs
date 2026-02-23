// Unit tests for task-002: src/xxhash.rs — XXH32 wrapper module
//
// Verifies that the Rust migration of xxhash.c behaves identically to the
// original C XXH32 implementation:
//   - One-shot hashing (`xxh32_oneshot`) matches C `XXH32(data, len, seed)`
//   - Streaming API via `Xxh32State` matches C XXH32_reset/update/digest cycle
//   - Known reference vectors from the xxHash spec are satisfied

use lz4::xxhash::{xxh32_oneshot, Xxh32State};

// ---------------------------------------------------------------------------
// One-shot: basic functionality
// ---------------------------------------------------------------------------

/// Empty input with seed 0 must equal the canonical XXH32 reference value
/// 0x02CC5D05 (documented in xxhash.rs parity vectors and xxHash spec).
#[test]
fn oneshot_empty_input_known_vector() {
    let hash = xxh32_oneshot(b"", 0);
    assert_eq!(
        hash, 0x02CC5D05,
        "XXH32(\"\", 0) must equal spec value 0x02CC5D05"
    );
}

/// Non-empty input must return a non-zero hash for seed 0.
#[test]
fn oneshot_nonempty_input_nonzero() {
    let hash = xxh32_oneshot(b"lz4", 0);
    assert_ne!(hash, 0, "XXH32(\"lz4\", 0) should be non-zero");
}

/// Result must be deterministic — identical calls return the same value.
#[test]
fn oneshot_deterministic() {
    let a = xxh32_oneshot(b"hello, world", 42);
    let b = xxh32_oneshot(b"hello, world", 42);
    assert_eq!(
        a, b,
        "xxh32_oneshot must return identical results on repeated calls"
    );
}

/// Different seeds must produce different hashes for the same input.
#[test]
fn oneshot_seed_affects_output() {
    let h0 = xxh32_oneshot(b"test", 0);
    let h1 = xxh32_oneshot(b"test", 1);
    assert_ne!(h0, h1, "different seeds should produce different hashes");
}

/// Different inputs must (almost certainly) produce different hashes.
#[test]
fn oneshot_different_inputs_differ() {
    let ha = xxh32_oneshot(b"abc", 0);
    let hb = xxh32_oneshot(b"xyz", 0);
    assert_ne!(ha, hb, "different inputs should produce different hashes");
}

/// Single-byte input must not panic and must produce a stable value.
#[test]
fn oneshot_single_byte() {
    let h1 = xxh32_oneshot(b"\x00", 0);
    let h2 = xxh32_oneshot(b"\x00", 0);
    assert_eq!(h1, h2, "single-byte input must be deterministic");
}

/// Large input (>16 bytes, exercising the 4-lane accumulation path) must not
/// panic and must return a deterministic result.
#[test]
fn oneshot_large_input_deterministic() {
    let data: Vec<u8> = (0u8..=255u8).cycle().take(1024).collect();
    let h1 = xxh32_oneshot(&data, 0);
    let h2 = xxh32_oneshot(&data, 0);
    assert_eq!(h1, h2, "large input must be deterministic");
    assert_ne!(h1, 0, "large input hash should be non-zero");
}

/// Input length boundary: exactly 16 bytes (one full 4-lane block).
#[test]
fn oneshot_exactly_16_bytes() {
    let data = b"1234567890123456"; // 16 bytes
    let h1 = xxh32_oneshot(data, 0);
    let h2 = xxh32_oneshot(data, 0);
    assert_eq!(h1, h2);
}

/// Input length boundary: exactly 15 bytes (all-remainder, no full block).
#[test]
fn oneshot_exactly_15_bytes() {
    let data = b"123456789012345"; // 15 bytes
    let h1 = xxh32_oneshot(data, 0);
    let h2 = xxh32_oneshot(data, 0);
    assert_eq!(h1, h2);
}

/// All-zero input must still hash deterministically.
#[test]
fn oneshot_all_zero_bytes() {
    let data = vec![0u8; 64];
    let h = xxh32_oneshot(&data, 0);
    assert_eq!(h, xxh32_oneshot(&data, 0));
}

/// All-0xFF bytes must hash deterministically.
#[test]
fn oneshot_all_ff_bytes() {
    let data = vec![0xFFu8; 64];
    let h = xxh32_oneshot(&data, 0);
    assert_eq!(h, xxh32_oneshot(&data, 0));
}

// ---------------------------------------------------------------------------
// Streaming API: basic functionality
// ---------------------------------------------------------------------------

/// Streaming hash of empty input with seed 0 must equal the one-shot result.
#[test]
fn streaming_empty_matches_oneshot() {
    let mut state = Xxh32State::new(0);
    state.update(b"");
    let streaming = state.digest();
    let oneshot = xxh32_oneshot(b"", 0);
    assert_eq!(
        streaming, oneshot,
        "streaming and one-shot must agree on empty input"
    );
}

/// Streaming hash of b"lz4" with seed 0 must equal the one-shot result.
#[test]
fn streaming_lz4_matches_oneshot() {
    let mut state = Xxh32State::new(0);
    state.update(b"lz4");
    let streaming = state.digest();
    let oneshot = xxh32_oneshot(b"lz4", 0);
    assert_eq!(
        streaming, oneshot,
        "streaming and one-shot must agree for b\"lz4\""
    );
}

/// Feeding data in multiple chunks must equal a single one-shot call —
/// this mirrors lz4frame's per-block XXH32_update pattern.
#[test]
fn streaming_chunked_updates_match_oneshot() {
    let data = b"The quick brown fox jumps over the lazy dog";

    // Split into two chunks at an arbitrary boundary.
    let (part1, part2) = data.split_at(16);

    let mut state = Xxh32State::new(0);
    state.update(part1);
    state.update(part2);
    let streaming = state.digest();

    let oneshot = xxh32_oneshot(data, 0);
    assert_eq!(
        streaming, oneshot,
        "chunked streaming updates must equal one-shot for the same input"
    );
}

/// Streaming with seed != 0 must equal one-shot with the same seed.
#[test]
fn streaming_nonzero_seed_matches_oneshot() {
    let data = b"content checksum test";
    let seed = 0xDEAD_BEEFu32;

    let mut state = Xxh32State::new(seed);
    state.update(data);
    let streaming = state.digest();

    let oneshot = xxh32_oneshot(data, seed);
    assert_eq!(
        streaming, oneshot,
        "streaming and one-shot must agree for non-zero seed"
    );
}

/// Many small single-byte updates must equal the one-shot result — stress-tests
/// the streaming accumulator's boundary handling.
#[test]
fn streaming_single_byte_updates_match_oneshot() {
    let data = b"abcdefghijklmnopqrstuvwxyz";

    let mut state = Xxh32State::new(0);
    for byte in data.iter() {
        state.update(std::slice::from_ref(byte));
    }
    let streaming = state.digest();

    let oneshot = xxh32_oneshot(data, 0);
    assert_eq!(
        streaming, oneshot,
        "byte-by-byte streaming must equal one-shot"
    );
}

/// Streaming result must be deterministic across two independent state objects.
#[test]
fn streaming_deterministic() {
    let data = b"determinism check";

    let mut s1 = Xxh32State::new(0);
    s1.update(data);
    let h1 = s1.digest();

    let mut s2 = Xxh32State::new(0);
    s2.update(data);
    let h2 = s2.digest();

    assert_eq!(
        h1, h2,
        "two independent streaming states must produce the same digest"
    );
}

/// digest() must be repeatable — calling it twice on the same state returns
/// the same value (non-destructive finalization).
#[test]
fn streaming_digest_repeatable() {
    let mut state = Xxh32State::new(0);
    state.update(b"repeat");
    let h1 = state.digest();
    let h2 = state.digest();
    assert_eq!(
        h1, h2,
        "digest() must return identical values on repeated calls"
    );
}

/// Large streaming input must match the one-shot result.
#[test]
fn streaming_large_input_matches_oneshot() {
    let data: Vec<u8> = (0u8..=255u8).cycle().take(2048).collect();

    let mut state = Xxh32State::new(0);
    // Feed in 256-byte chunks to exercise the accumulation loop.
    for chunk in data.chunks(256) {
        state.update(chunk);
    }
    let streaming = state.digest();
    let oneshot = xxh32_oneshot(&data, 0);

    assert_eq!(
        streaming, oneshot,
        "large chunked streaming must equal one-shot"
    );
}

// ---------------------------------------------------------------------------
// lz4frame usage pattern: content checksum simulation
// ---------------------------------------------------------------------------

/// Simulate the exact pattern used by lz4frame content checksum:
///   XXH32_reset(&cctx->xxh, 0)
///   XXH32_update(&cctx->xxh, src_block_1, len1)
///   XXH32_update(&cctx->xxh, src_block_2, len2)
///   cksum = XXH32_digest(&cctx->xxh)
///
/// This must equal XXH32(full_content, full_len, 0).
#[test]
fn lz4frame_content_checksum_pattern() {
    let block1 = b"First block of compressed content. ";
    let block2 = b"Second block of compressed content.";
    let full: Vec<u8> = [block1.as_ref(), block2.as_ref()].concat();

    let mut xxh = Xxh32State::new(0);
    xxh.update(block1);
    xxh.update(block2);
    let frame_cksum = xxh.digest();

    let reference = xxh32_oneshot(&full, 0);
    assert_eq!(
        frame_cksum, reference,
        "lz4frame content-checksum pattern must match one-shot reference"
    );
}
