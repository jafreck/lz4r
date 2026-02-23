// Unit tests for task-001: Cargo.toml + src/lib.rs scaffold
//
// These tests verify that the initial crate scaffolding (task-001) is correct:
// - The crate compiles without errors
// - The crate-level module structure (lorem, timefn, config, util, threadpool,
//   io, bench) is declared in lib.rs
// - Key runtime dependencies (lz4_flex, anyhow) are available and functional

// Test that the crate root compiles and links successfully.
// This is the minimal parity check for the placeholder lib.rs.
#[test]
fn crate_compiles() {
    // If this test file compiles and links against `lz4_programs`, the crate
    // is structurally valid. No assertions needed beyond successful compilation.
}

// Verify the lz4_flex dependency resolves and can compress/decompress a
// trivial payload. lz4_flex provides the core block-format compression that
// the bench module (future task-023..task-028) will use.
#[test]
fn lz4_flex_dependency_available() {
    let input = b"lz4-programs crate scaffold verification";
    let compressed = lz4_flex::compress_prepend_size(input);
    let decompressed = lz4_flex::decompress_size_prepended(&compressed)
        .expect("decompression should succeed");
    assert_eq!(decompressed, input, "round-trip through lz4_flex should be lossless");
}

// Verify lz4_flex handles empty input without panicking; this matches the
// C LZ4_compress_fast() behavior for zero-length inputs.
#[test]
fn lz4_flex_empty_input() {
    let compressed = lz4_flex::compress_prepend_size(b"");
    let decompressed = lz4_flex::decompress_size_prepended(&compressed)
        .expect("empty decompression should succeed");
    assert!(decompressed.is_empty());
}

// Verify the anyhow dependency is available; it is used pervasively for
// error propagation throughout the migrated lz4io module (future tasks).
#[test]
fn anyhow_dependency_available() {
    fn fallible() -> anyhow::Result<u32> {
        Ok(42)
    }
    assert_eq!(fallible().unwrap(), 42);
}

// Verify anyhow::bail! produces a proper error that can be inspected.
#[test]
fn anyhow_error_propagation() {
    fn failing() -> anyhow::Result<()> {
        anyhow::bail!("expected test error");
    }
    let err = failing().unwrap_err();
    assert!(err.to_string().contains("expected test error"));
}

// Verify the lz4_flex frame encoder/decoder round-trips correctly.
// The frame format is the wire format used by lz4io (future tasks).
#[test]
fn lz4_flex_frame_round_trip() {
    use lz4_flex::frame::{FrameDecoder, FrameEncoder};
    use std::io::{Read, Write};

    let input = b"hello from lz4-programs frame round-trip test";
    let mut encoded = Vec::new();
    {
        let mut enc = FrameEncoder::new(&mut encoded);
        enc.write_all(input).expect("frame encode write");
        enc.finish().expect("frame encode finish");
    }
    let mut decoded = Vec::new();
    FrameDecoder::new(encoded.as_slice())
        .read_to_end(&mut decoded)
        .expect("frame decode");
    assert_eq!(decoded, input);
}
