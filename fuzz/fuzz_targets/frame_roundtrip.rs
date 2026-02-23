#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Compress data as a complete LZ4 frame.
    let compressed = lz4::frame::compress_frame_to_vec(data);

    // compress_frame_to_vec returns an empty Vec on internal error — skip those.
    if compressed.is_empty() && !data.is_empty() {
        return;
    }

    // Decompress the frame back.
    let recovered = match lz4::frame::decompress_frame_to_vec(&compressed) {
        Ok(v) => v,
        Err(_) => {
            // An Err here means our own compressed output is unreadable — that is a bug.
            if !data.is_empty() {
                panic!(
                    "frame round-trip: decompression of self-compressed data failed \
                     (input {} bytes, compressed {} bytes)",
                    data.len(),
                    compressed.len()
                );
            }
            return;
        }
    };

    assert_eq!(
        recovered, data,
        "frame round-trip mismatch: {} bytes in, {} bytes back",
        data.len(),
        recovered.len()
    );
});
