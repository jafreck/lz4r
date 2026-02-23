#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Feed arbitrary bytes through the block decompressor.
    // Err results are expected and fine; what we verify is no panics or UB.

    // Zero-length output buffer.
    {
        let mut dst = vec![0u8; 0];
        let _ = lz4::lz4_decompress_safe(data, &mut dst);
    }

    // 4 KiB output buffer — covers most real block sizes.
    {
        let mut dst = vec![0u8; 4096];
        let _ = lz4::lz4_decompress_safe(data, &mut dst);
    }

    // Output buffer as large as data itself (a common heuristic).
    if !data.is_empty() {
        let mut dst = vec![0u8; data.len()];
        let _ = lz4::lz4_decompress_safe(data, &mut dst);
    }

    // Large output buffer to stress the length-limit path.
    {
        // Cap at 1 MiB so the fuzzer doesn't OOM on tiny inputs that claim huge output.
        let large = (data.len().saturating_mul(255)).min(1 << 20);
        let mut dst = vec![0u8; large];
        let _ = lz4::lz4_decompress_safe(data, &mut dst);
    }
});
