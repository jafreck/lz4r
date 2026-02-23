#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Compress with the Vec-returning convenience helper (handles bound calculation)
    let compressed = lz4::block::compress_block_to_vec(data);

    // A fully-incompressible input may be larger than LZ4_MAX_INPUT_SIZE; in that
    // case the codec returns an empty Vec — just skip.
    if compressed.is_empty() && !data.is_empty() {
        return;
    }

    // Decompress back, supplying the exact original length.
    let recovered = lz4::block::decompress_block_to_vec(&compressed, data.len());

    // An empty recovered Vec means decompression returned an error — that should
    // never happen for a validly-compressed buffer, so treat it as a failure.
    if !data.is_empty() {
        assert_eq!(
            recovered, data,
            "block round-trip mismatch: compressed {} bytes back to {} bytes (expected {})",
            compressed.len(),
            recovered.len(),
            data.len()
        );
    }
});
