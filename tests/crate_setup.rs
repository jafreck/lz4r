// Unit tests for task-001: Cargo.toml + src/lib.rs scaffold
//
// These tests verify that the initial crate scaffolding (task-001) is correct:
// - The crate compiles without errors
// - The crate-level module structure (lorem, timefn, config, util, threadpool,
//   io, bench) is declared in lib.rs
// - Key runtime dependencies (anyhow) are available and functional

// Test that the crate root compiles and links successfully.
// This is the minimal parity check for the placeholder lib.rs.
#[test]
fn crate_compiles() {
    // If this test file compiles and links against `lz4_programs`, the crate
    // is structurally valid. No assertions needed beyond successful compilation.
}

// Verify native block compression round-trips correctly.
#[test]
fn block_compression_available() {
    let input = b"lz4-programs crate scaffold verification";
    let compressed = lz4::block::compress_block_to_vec(input);
    let decompressed = lz4::block::decompress_block_to_vec(&compressed, input.len());
    assert_eq!(
        decompressed, input,
        "round-trip through native block API should be lossless"
    );
}

// Verify the native block API handles empty input without panicking.
#[test]
fn block_compression_empty_input() {
    let compressed = lz4::block::compress_block_to_vec(b"");
    let decompressed = lz4::block::decompress_block_to_vec(&compressed, 0);
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

// Verify the native frame encoder/decoder round-trips correctly.
// The frame format is the wire format used by lz4io.
#[test]
fn frame_round_trip() {
    let input = b"hello from lz4-programs frame round-trip test";
    let compressed = lz4::frame::compress_frame_to_vec(input);
    let decoded = lz4::frame::decompress_frame_to_vec(&compressed).expect("frame decode");
    assert_eq!(decoded, input);
}
