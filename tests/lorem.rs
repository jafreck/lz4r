// Integration tests for task-002: lorem.rs — Lorem ipsum text generator
//
// Tests verify behavioural parity with LOREM_genBuffer / LOREM_genBlock from
// lz4-1.10.0/programs/lorem.c:
//   - gen_block() returns correct byte counts
//   - gen_buffer() returns a vec of exactly `size` bytes
//   - Output is deterministic for a given seed
//   - first=true prepends the canonical "Lorem ipsum dolor sit amet..." sentence
//   - fill=true fills the entire buffer
//   - fill=false produces at most one paragraph
//   - Edge cases: empty buffer, very small buffers, large buffers

use lz4::lorem::{gen_block, gen_buffer};

// ─────────────────────────────────────────────────────────────────────────────
// gen_buffer — basic contract
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_buffer_returns_correct_size() {
    // gen_buffer must return a Vec of exactly `size` bytes
    let size = 1024;
    let buf = gen_buffer(size, 42);
    assert_eq!(buf.len(), size);
}

#[test]
fn gen_buffer_zero_size_returns_empty() {
    // Equivalent to LOREM_genBuffer with size=0
    let buf = gen_buffer(0, 0);
    assert_eq!(buf.len(), 0);
}

#[test]
fn gen_buffer_is_deterministic() {
    // Same seed must produce byte-identical output
    let a = gen_buffer(512, 12345);
    let b = gen_buffer(512, 12345);
    assert_eq!(a, b, "gen_buffer must be deterministic for a given seed");
}

#[test]
fn gen_buffer_different_seeds_produce_different_output() {
    // Different seeds must produce (almost certainly) different output
    let a = gen_buffer(512, 0);
    let b = gen_buffer(512, 1);
    assert_ne!(a, b, "different seeds should produce different output");
}

#[test]
fn gen_buffer_starts_with_lorem_ipsum() {
    // gen_buffer calls gen_block(first=true, fill=true);
    // output must start with "Lorem ipsum" (first sentence)
    let buf = gen_buffer(256, 0);
    let text = std::str::from_utf8(&buf).expect("output must be valid UTF-8");
    assert!(
        text.starts_with("Lorem ipsum"),
        "output should start with 'Lorem ipsum', got: {:?}",
        &text[..text.len().min(30)]
    );
}

#[test]
fn gen_buffer_contains_only_printable_ascii() {
    // Lorem ipsum output must consist of printable ASCII + space + newline
    let buf = gen_buffer(1024, 99);
    for &byte in &buf {
        assert!(
            byte == b'\n' || (byte >= b' ' && byte <= b'~'),
            "unexpected byte 0x{:02x} in lorem output",
            byte
        );
    }
}

#[test]
fn gen_buffer_large_size() {
    // Must handle a large buffer without panicking
    let buf = gen_buffer(64 * 1024, 7);
    assert_eq!(buf.len(), 64 * 1024);
}

// ─────────────────────────────────────────────────────────────────────────────
// gen_block — return value
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_block_returns_bytes_written_le_buf_len() {
    // LOREM_genBlock returns the number of bytes written; must be <= size
    let mut buf = vec![0u8; 1024];
    let n = gen_block(&mut buf, 0, true, true);
    assert!(n <= buf.len(), "gen_block must not write beyond the buffer");
}

#[test]
fn gen_block_fill_true_fills_entire_buffer() {
    // When fill=true, gen_block must write exactly buf.len() bytes
    let mut buf = vec![0u8; 512];
    let n = gen_block(&mut buf, 42, true, true);
    assert_eq!(n, buf.len(), "fill=true must fill the entire buffer");
}

#[test]
fn gen_block_fill_false_writes_at_most_one_paragraph() {
    // When fill=false, gen_block stops after one paragraph.
    // For a large buffer this means n < buf.len().
    let mut buf = vec![0u8; 16 * 1024];
    let n = gen_block(&mut buf, 1, true, false);
    assert!(
        n < buf.len(),
        "fill=false should not fill the entire buffer (got n={n}, buf.len()={})",
        buf.len()
    );
    assert!(n > 0, "must write at least some bytes");
}

#[test]
fn gen_block_empty_buffer_returns_zero() {
    // Empty buffer: nothing to write
    let mut buf = [];
    let n = gen_block(&mut buf, 0, true, true);
    assert_eq!(n, 0);
}

#[test]
fn gen_block_is_deterministic() {
    // Same inputs must produce identical outputs
    let mut a = vec![0u8; 512];
    let mut b = vec![0u8; 512];
    let na = gen_block(&mut a, 777, true, true);
    let nb = gen_block(&mut b, 777, true, true);
    assert_eq!(na, nb);
    assert_eq!(a, b, "gen_block must be deterministic for a given seed");
}

#[test]
fn gen_block_different_seeds_differ() {
    let mut a = vec![0u8; 256];
    let mut b = vec![0u8; 256];
    gen_block(&mut a, 0, true, true);
    gen_block(&mut b, 9999, true, true);
    assert_ne!(a, b, "different seeds should produce different output");
}

// ─────────────────────────────────────────────────────────────────────────────
// gen_block — first=true produces canonical opening
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_block_first_true_starts_with_lorem_ipsum() {
    // first=true → generateFirstSentence is called; output must start with
    // "Lorem ipsum dolor sit amet"
    let mut buf = vec![0u8; 256];
    gen_block(&mut buf, 0, true, true);
    let text = std::str::from_utf8(&buf).expect("must be valid UTF-8");
    assert!(
        text.starts_with("Lorem ipsum"),
        "first=true output should start with 'Lorem ipsum', got: {:?}",
        &text[..text.len().min(40)]
    );
}

#[test]
fn gen_block_first_false_does_not_require_lorem_start() {
    // first=false skips generateFirstSentence; for seed 0 the output will differ
    let mut a = vec![0u8; 256];
    let mut b = vec![0u8; 256];
    gen_block(&mut a, 0, true, true);
    gen_block(&mut b, 0, false, true);
    assert_ne!(
        a, b,
        "first=true and first=false must produce different output for the same seed"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// gen_block — output is valid text
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_block_output_is_valid_utf8() {
    // Lorem ipsum words are ASCII, so the entire block must be valid UTF-8
    let mut buf = vec![0u8; 1024];
    let n = gen_block(&mut buf, 0, true, true);
    assert!(
        std::str::from_utf8(&buf[..n]).is_ok(),
        "gen_block output must be valid UTF-8"
    );
}

#[test]
fn gen_block_output_contains_only_printable_ascii() {
    // Every byte in the written region must be printable ASCII, space, or newline
    let mut buf = vec![0u8; 1024];
    let n = gen_block(&mut buf, 3, true, true);
    for &byte in &buf[..n] {
        assert!(
            byte == b'\n' || (byte >= b' ' && byte <= b'~'),
            "unexpected byte 0x{:02x} in gen_block output",
            byte
        );
    }
}

#[test]
fn gen_block_fill_true_ends_with_newline_or_period() {
    // LOREM_genBlock with fill=true writes until the buffer is full;
    // writeLastCharacters ensures the final byte is '\n' (if space allows)
    let mut buf = vec![0u8; 256];
    let n = gen_block(&mut buf, 5, true, true);
    assert_eq!(n, buf.len());
    let last = buf[n - 1];
    assert!(
        last == b'\n' || last == b'.' || last == b' ',
        "last byte should be '\\n', '.', or ' ', got 0x{:02x}",
        last
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// gen_block — small buffer edge cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_block_single_byte_buffer() {
    // Must not panic with a 1-byte buffer
    let mut buf = vec![0u8; 1];
    let n = gen_block(&mut buf, 0, true, true);
    assert_eq!(n, 1);
}

#[test]
fn gen_block_two_byte_buffer() {
    let mut buf = vec![0u8; 2];
    let n = gen_block(&mut buf, 0, true, true);
    assert_eq!(n, 2);
}

#[test]
fn gen_block_small_buffer_no_panic() {
    // Verify no panic for buffers smaller than one typical word + separator
    for size in 1usize..=20 {
        let mut buf = vec![0u8; size];
        let n = gen_block(&mut buf, 0, true, true);
        assert!(n <= size, "gen_block must not exceed buffer of size {size}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Parity: gen_buffer == gen_block with first=true, fill=true
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_buffer_matches_gen_block_full_fill() {
    // gen_buffer(size, seed) is documented as equivalent to
    // gen_block(buf, seed, first=true, fill=true)
    let size = 512usize;
    let seed = 42u32;

    let buffer_result = gen_buffer(size, seed);

    let mut block_buf = vec![0u8; size];
    let n = gen_block(&mut block_buf, seed, true, true);
    assert_eq!(n, size);

    assert_eq!(
        buffer_result, block_buf,
        "gen_buffer must match gen_block(first=true, fill=true)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// PRNG / seed sensitivity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gen_block_seed_zero_produces_output() {
    // Seed 0 is a valid seed and must produce output
    let mut buf = vec![0u8; 128];
    let n = gen_block(&mut buf, 0, true, true);
    assert_eq!(n, buf.len());
}

#[test]
fn gen_block_seed_max_u32_produces_output() {
    // u32::MAX is a valid seed
    let mut buf = vec![0u8; 128];
    let n = gen_block(&mut buf, u32::MAX, true, true);
    assert_eq!(n, buf.len());
}

#[test]
fn gen_block_multiple_seeds_all_differ() {
    // A range of seeds should each produce distinct output
    let outputs: Vec<Vec<u8>> = (0u32..8)
        .map(|seed| {
            let mut buf = vec![0u8; 256];
            gen_block(&mut buf, seed, true, true);
            buf
        })
        .collect();

    // All pairs should differ
    for i in 0..outputs.len() {
        for j in (i + 1)..outputs.len() {
            assert_ne!(
                outputs[i], outputs[j],
                "seeds {i} and {j} produced identical output"
            );
        }
    }
}
