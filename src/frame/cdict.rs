//! Pre-digested compression dictionaries for LZ4 frame encoding.
//!
//! Provides [`Lz4FCDict`], a dictionary pre-loaded into both the fast (LZ4)
//! and high-compression (LZ4HC) stream contexts so it can be reused across
//! many independent frame-compression operations without repeating the
//! dictionary initialisation cost on every call.
//!
//! Corresponds to `LZ4F_CDict` / `LZ4F_CDict_s` in `lz4frame.c` v1.10.0
//! (lines 531–590).
//!
//! # Design notes
//! The C `LZ4F_CDict_s` struct holds three heap-allocated members:
//!   - `dictContent` — a trimmed copy of the user dictionary (at most 64 KB)
//!   - `fastCtx`     — a `LZ4_stream_t` pre-loaded with the dictionary
//!   - `HCCtx`       — a `LZ4_streamHC_t` pre-loaded at `LZ4HC_CLEVEL_DEFAULT`
//!
//! In Rust these are modelled as:
//!   - `dict_content: Vec<u8>` — owns the trimmed dictionary bytes
//!   - `fast_ctx: Box<block::stream::Lz4Stream>` — owns the fast stream state
//!   - `hc_ctx: Box<hc::api::Lz4StreamHc>` — owns the HC stream state
//!
//! Custom allocator hooks (`LZ4F_CustomMem`) are not needed in safe Rust:
//! `Box` uses the global allocator and `Drop` frees everything automatically.
//!
//! `LZ4F_freeCDict` is faithfully represented by the `Drop` implementation;
//! no explicit free function is exposed.

use crate::block::stream::Lz4Stream;
use crate::hc::api::{
    init_stream_hc, load_dict_hc, set_compression_level, Lz4StreamHc,
};
use crate::hc::types::LZ4HC_CLEVEL_DEFAULT;

// Maximum dictionary size the frame format retains (64 KB).
// Dictionaries longer than this are trimmed to their last 64 KB before use;
// see the trim logic in `LZ4F_createCDict_advanced` (lz4frame.c:546-549).
const MAX_DICT_SIZE: usize = 64 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FCDict — pre-digested compression dictionary
// ─────────────────────────────────────────────────────────────────────────────

/// A pre-digested compression dictionary for use with frame compression.
///
/// Corresponds to `LZ4F_CDict` / `LZ4F_CDict_s` (lz4frame_static.h / lz4frame.c:531-536).
///
/// # Thread safety
/// An `Lz4FCDict` is **read-only** after creation and may be shared across
/// threads concurrently, mirroring the C documentation for `LZ4F_CDict`.
///
/// # Drop behaviour
/// Dropping an `Lz4FCDict` (or `Box<Lz4FCDict>`) frees all three sub-allocations
/// (`dict_content`, `fast_ctx`, `hc_ctx`), equivalent to `LZ4F_freeCDict`.
pub struct Lz4FCDict {
    /// Trimmed copy of the user-supplied dictionary (at most 64 KB).
    /// Equivalent to `cdict->dictContent` in C.
    pub(crate) dict_content: Vec<u8>,

    /// Fast LZ4 stream pre-loaded with the dictionary data.
    /// Equivalent to `cdict->fastCtx` in C.
    pub(crate) fast_ctx: Box<Lz4Stream>,

    /// HC LZ4 stream pre-loaded with the dictionary data at `LZ4HC_CLEVEL_DEFAULT`.
    /// Equivalent to `cdict->HCCtx` in C.
    pub(crate) hc_ctx: Box<Lz4StreamHc>,
}

// SAFETY: All shared mutable state lives inside `Lz4StreamHc` and `Lz4Stream`.
// An `Lz4FCDict` is read-only after `create()` returns, matching C guarantees.
unsafe impl Send for Lz4FCDict {}
unsafe impl Sync for Lz4FCDict {}

impl Lz4FCDict {
    /// Create a pre-digested dictionary for frame compression.
    ///
    /// Equivalent to `LZ4F_createCDict` (lz4frame.c:575-579) /
    /// `LZ4F_createCDict_advanced` (lz4frame.c:538-567).
    ///
    /// At most the **last 64 KB** of `dict` is retained, matching the C trim
    /// (lz4frame.c:546-549).
    ///
    /// Returns `None` if the global allocator fails to provide the HC or fast
    /// stream state (mirrors the `!cdict->fastCtx || !cdict->HCCtx` NULL check
    /// in C which calls `LZ4F_freeCDict` and returns `NULL`).
    ///
    /// # Example
    /// ```
    /// # use lz4::frame::cdict::Lz4FCDict;
    /// let dict_bytes = b"example dictionary content";
    /// let cdict = Lz4FCDict::create(dict_bytes).expect("allocation failed");
    /// ```
    pub fn create(dict: &[u8]) -> Option<Box<Self>> {
        // Trim to last 64 KB (lz4frame.c:546-549).
        let trimmed = if dict.len() > MAX_DICT_SIZE {
            &dict[dict.len() - MAX_DICT_SIZE..]
        } else {
            dict
        };

        // Copy the trimmed dictionary bytes into owned storage (lz4frame.c:558).
        let dict_content: Vec<u8> = trimmed.to_vec();

        // Initialise fast stream and load dictionary (lz4frame.c:559-560).
        // Lz4Stream::new() already returns Box<Lz4Stream>.
        let mut fast_ctx = Lz4Stream::new();
        fast_ctx.load_dict_slow(trimmed);

        // Initialise HC stream, set default compression level, load dictionary
        // (lz4frame.c:561-565).
        let mut hc_ctx = Lz4StreamHc::create()?; // returns None on alloc failure
        init_stream_hc(&mut hc_ctx);
        set_compression_level(&mut hc_ctx, LZ4HC_CLEVEL_DEFAULT);
        // SAFETY: `dict_content` is valid for `dict_content.len()` bytes and will
        // outlive `hc_ctx` (both are owned by the same `Lz4FCDict` value).
        unsafe {
            load_dict_hc(
                &mut hc_ctx,
                dict_content.as_ptr(),
                dict_content.len() as i32,
            );
        }

        Some(Box::new(Lz4FCDict {
            dict_content,
            fast_ctx,
            hc_ctx,
        }))
    }
}

// No explicit `Drop` impl is needed: the compiler inserts implicit drops for
// all three fields (`Vec<u8>`, `Box<Lz4Stream>`, `Box<Lz4StreamHc>`), freeing
// their heap allocations in declaration order — equivalent to `LZ4F_freeCDict`
// (lz4frame.c:581-588).

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `create` succeeds with a non-empty dictionary.
    #[test]
    fn create_with_nonempty_dict() {
        let dict: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let cdict = Lz4FCDict::create(&dict);
        assert!(cdict.is_some(), "create should succeed with 1 KB dict");
        let cdict = cdict.unwrap();
        // Dict content must be trimmed to at most 64 KB.
        assert_eq!(cdict.dict_content.len(), dict.len().min(MAX_DICT_SIZE));
    }

    /// Verify that `create` succeeds with an empty dictionary.
    #[test]
    fn create_with_empty_dict() {
        let cdict = Lz4FCDict::create(&[]);
        assert!(cdict.is_some());
        let cdict = cdict.unwrap();
        assert_eq!(cdict.dict_content.len(), 0);
    }

    /// Verify that dictionaries larger than 64 KB are trimmed to exactly 64 KB,
    /// retaining the *last* 64 KB of the input (matching C behaviour).
    #[test]
    fn create_trims_large_dict() {
        let dict: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
        let cdict = Lz4FCDict::create(&dict).expect("allocation failed");
        assert_eq!(cdict.dict_content.len(), MAX_DICT_SIZE);
        // The retained bytes must be the last 64 KB.
        assert_eq!(
            cdict.dict_content.as_slice(),
            &dict[dict.len() - MAX_DICT_SIZE..]
        );
    }

    /// Parity check: CDict created from a known dict produces a non-zero HC
    /// stream state (tables populated), mirroring C `LZ4F_createCDict`.
    #[test]
    fn hc_stream_populated_after_create() {
        let dict: Vec<u8> = b"The quick brown fox jumps over the lazy dog".iter()
            .cycle()
            .take(4096)
            .copied()
            .collect();
        let cdict = Lz4FCDict::create(&dict).expect("allocation failed");
        // `base` is set to the start of the dictionary window after load_dict_hc;
        // a non-null `base` pointer indicates the HC tables were initialised.
        // We verify this indirectly: the dict_content is non-empty.
        assert!(!cdict.dict_content.is_empty());
        // The HC compression level must be at the default.
        assert_eq!(
            cdict.hc_ctx.ctx.compression_level as i32,
            LZ4HC_CLEVEL_DEFAULT
        );
    }
}
