//! Thin wrapper around the `xxhash-rust` crate providing the XXH32 API used
//! by the rest of this crate (mirrors `xxhash.c` / `xxhash.h` from LZ4 v1.10.0).
//!
//! Only XXH32 is needed: `lz4frame` uses it exclusively for content checksums.

pub use xxhash_rust::xxh32::Xxh32 as Xxh32State;

/// One-shot XXH32 hash — equivalent to the C `XXH32(data, len, seed)` function.
///
/// # Parity vectors
/// * `xxh32_oneshot(b"", 0)` == `0x02CC5D05`
/// * `xxh32_oneshot(b"lz4", 0)` == reference XXH32 output for the same input
#[inline]
pub fn xxh32_oneshot(data: &[u8], seed: u32) -> u32 {
    xxhash_rust::xxh32::xxh32(data, seed)
}
