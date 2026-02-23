#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Feed arbitrary bytes through the frame decompressor.
    // Err results are expected and fine; what we verify is no panics or UB.

    // Use the high-level Vec-returning helper — it exercises the full frame
    // header parsing, block decompression, and optional content-checksum
    // verification paths.
    let _ = lz4::frame::decompress_frame_to_vec(data);

    // Also exercise the lower-level streaming API to cover different code paths.
    use lz4::frame::{
        lz4f_create_decompression_context, lz4f_decompress, lz4f_free_decompression_context,
    };
    const LZ4F_VERSION: u32 = 100;

    if let Ok(mut dctx) = lz4f_create_decompression_context(LZ4F_VERSION) {
        let mut out_buf = vec![0u8; 65536];
        let mut pos = 0usize;
        loop {
            if pos >= data.len() {
                break;
            }
            match lz4f_decompress(&mut dctx, Some(&mut out_buf), &data[pos..], None) {
                Ok((consumed, _written, hint)) => {
                    if consumed == 0 || hint == 0 {
                        break;
                    }
                    pos += consumed;
                }
                Err(_) => break,
            }
        }
        lz4f_free_decompression_context(dctx);
    }
});
