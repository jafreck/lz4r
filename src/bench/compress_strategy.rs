//! Compression strategy selection for the benchmark subsystem.
//!
//! Defines the [`CompressionStrategy`] trait and four concrete implementations
//! covering every combination of dictionary / no-dictionary and fast / HC modes:
//!
//! | Type              | Dict | Algorithm |
//! |-------------------|------|-----------|
//! | [`NoStreamFast`]  | no   | fast      |
//! | [`NoStreamHC`]    | no   | HC        |
//! | [`StreamFast`]    | yes  | fast      |
//! | [`StreamHC`]      | yes  | HC        |
//!
//! All compression is performed through `crate::block` and `crate::hc` — no
//! FFI or third-party crate dependencies.  A zero return from any block
//! compression function is treated as an error.
//!
//! Use [`build_compression_parameters`] (no dict) or
//! [`build_compression_parameters_with_dict`] (with dict) to obtain a boxed
//! strategy.  The threshold [`LZ4HC_CLEVEL_MIN`]` = 2` determines whether the
//! fast or HC path is selected.

use std::io;

use crate::block::{compress_bound, compress_fast, Lz4Stream};
use crate::hc::{
    attach_hc_dictionary, compress_hc, compress_hc_continue, load_dict_hc, reset_stream_hc_fast,
    Lz4StreamHc,
};

/// Minimum compression level that activates HC (high-compression) mode.
/// Levels strictly below this threshold use the fast compression path.
const LZ4HC_CLEVEL_MIN: i32 = 2;

// ── CompressionStrategy trait ─────────────────────────────────────────────────

/// A single compression strategy used by the benchmark runner.
///
/// Each implementation owns its stream state, initialised in `new` and
/// released on `Drop`.  The per-block context reset is performed inside
/// [`compress_block`](CompressionStrategy::compress_block), so callers
/// require no per-block setup beyond passing source and destination buffers.
pub trait CompressionStrategy: Send + Sync {
    /// Compress `src` into `dst`.
    ///
    /// Before writing, `dst` is resized to hold at least
    /// `compress_bound(src.len())` bytes.  Returns the number of compressed
    /// bytes written into `dst`.
    fn compress_block(&mut self, src: &[u8], dst: &mut Vec<u8>) -> io::Result<usize>;
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Ensure `dst` is large enough to hold the LZ4 worst-case output for `src_len` bytes.
#[inline]
fn ensure_dst_capacity(src_len: usize, dst: &mut Vec<u8>) {
    let bound = compress_bound(src_len as i32) as usize;
    if dst.len() < bound {
        dst.resize(bound, 0u8);
    }
}

// ── Strategy 1: NoStreamFast ──────────────────────────────────────────────────

/// Stateless fast compression with no dictionary.
///
/// Each [`compress_block`](CompressionStrategy::compress_block) call is
/// fully independent — there is no cross-block history.
///
/// The acceleration factor is derived from the compression level:
/// `acceleration = if c_level < 0 { -c_level + 1 } else { 1 }`.
/// Negative levels trade compression ratio for higher throughput.
pub struct NoStreamFast {
    acceleration: i32,
}

impl NoStreamFast {
    pub fn new(c_level: i32) -> Self {
        let acceleration = if c_level < 0 { -c_level + 1 } else { 1 };
        NoStreamFast { acceleration }
    }
}

unsafe impl Send for NoStreamFast {}
unsafe impl Sync for NoStreamFast {}

impl CompressionStrategy for NoStreamFast {
    fn compress_block(&mut self, src: &[u8], dst: &mut Vec<u8>) -> io::Result<usize> {
        ensure_dst_capacity(src.len(), dst);
        // Block API: compress_fast returns Result<usize, Lz4Error>.
        compress_fast(src, dst, self.acceleration).map_err(|e| {
            io::Error::other(format!("compress_fast failed: {e:?}"))
        })
    }
}

// ── Strategy 2: NoStreamHC ────────────────────────────────────────────────────

/// Stateless high-compression (HC) compression with no dictionary.
///
/// Each block is compressed independently using the HC search algorithm.
/// Intended for levels ≥ [`LZ4HC_CLEVEL_MIN`].
pub struct NoStreamHC {
    c_level: i32,
}

impl NoStreamHC {
    pub fn new(c_level: i32) -> Self {
        NoStreamHC { c_level }
    }
}

unsafe impl Send for NoStreamHC {}
unsafe impl Sync for NoStreamHC {}

impl CompressionStrategy for NoStreamHC {
    fn compress_block(&mut self, src: &[u8], dst: &mut Vec<u8>) -> io::Result<usize> {
        ensure_dst_capacity(src.len(), dst);
        let written = unsafe {
            compress_hc(
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
                self.c_level,
            )
        };
        if written == 0 {
            Err(io::Error::other(
                "compress_hc returned 0",
            ))
        } else {
            Ok(written as usize)
        }
    }
}

// ── Strategy 3: StreamFast ────────────────────────────────────────────────────

/// Fast stream compression with an optional pre-loaded dictionary.
///
/// The dictionary is loaded into a dedicated `dict_stream` via
/// `Lz4Stream::load_dict_slow` at construction time.  On each block,
/// `attach_dictionary` links the dict stream before calling
/// `compress_fast_continue`, providing dictionary context without
/// accumulating cross-block history in the primary stream.
pub struct StreamFast {
    c_level: i32,
    stream: Box<Lz4Stream>,
    dict_stream: Box<Lz4Stream>,
    /// Owns the dict bytes so the pointer passed to lz4 remains valid.
    _dict: Vec<u8>,
}

impl StreamFast {
    /// Create a `StreamFast` strategy, pre-loading `dict` into a dedicated stream.
    ///
    /// Pass an empty slice for `dict` to compress without a dictionary.
    pub fn new(c_level: i32, dict: &[u8]) -> io::Result<Self> {
        let stream = Lz4Stream::new();
        let mut dict_stream = Lz4Stream::new();
        // Keep a private copy so the slice pointer passed to load_dict_slow
        // remains valid for the entire lifetime of this struct.
        let dict_copy: Vec<u8> = dict.to_vec();
        if !dict_copy.is_empty() {
            dict_stream.load_dict_slow(&dict_copy);
        }
        Ok(StreamFast {
            c_level,
            stream,
            dict_stream,
            _dict: dict_copy,
        })
    }
}

unsafe impl Send for StreamFast {}
unsafe impl Sync for StreamFast {}

impl CompressionStrategy for StreamFast {
    fn compress_block(&mut self, src: &[u8], dst: &mut Vec<u8>) -> io::Result<usize> {
        ensure_dst_capacity(src.len(), dst);
        // Negative levels trade compression ratio for speed via a higher acceleration value.
        let acceleration = if self.c_level < 0 {
            -self.c_level + 1
        } else {
            1
        };
        // Reset the compression context before each block to prevent unintended
        // cross-block history from a previous compress_block call.
        self.stream.reset_fast();
        // When no dictionary was provided, pass None to clear any prior attachment.
        let dict_ptr = if self._dict.is_empty() {
            None
        } else {
            Some(&*self.dict_stream as *const Lz4Stream)
        };
        unsafe {
            self.stream.attach_dictionary(dict_ptr);
        }

        let written = self.stream.compress_fast_continue(src, dst, acceleration);
        if written == 0 {
            Err(io::Error::other(
                "compress_fast_continue returned 0",
            ))
        } else {
            Ok(written as usize)
        }
    }
}

// ── Strategy 4: StreamHC ──────────────────────────────────────────────────────

/// HC stream compression with an optional pre-loaded dictionary.
///
/// Uses a dedicated `dict_stream_hc` to hold the dictionary context.
/// Before each block, `reset_stream_hc_fast` reinitialises the primary HC
/// stream and `attach_hc_dictionary` links the dict context, providing
/// dictionary-aware compression without persisting cross-block history.
pub struct StreamHC {
    c_level: i32,
    stream_hc: Box<Lz4StreamHc>,
    dict_stream_hc: Box<Lz4StreamHc>,
    /// Owns the dict bytes so the pointer passed to lz4 remains valid.
    _dict: Vec<u8>,
}

impl StreamHC {
    /// Create a `StreamHC` strategy, pre-loading `dict` into a dedicated HC stream.
    ///
    /// Pass an empty slice for `dict` to compress without a dictionary.
    pub fn new(c_level: i32, dict: &[u8]) -> io::Result<Self> {
        let stream_hc = Lz4StreamHc::create()
            .ok_or_else(|| io::Error::other("Lz4StreamHc::create failed"))?;
        let mut dict_stream_hc = Lz4StreamHc::create().ok_or_else(|| {
            io::Error::other("Lz4StreamHc::create (dict) failed")
        })?;
        let dict_copy: Vec<u8> = dict.to_vec();
        // Initialise the dict stream at the target level before loading the dictionary.
        reset_stream_hc_fast(&mut dict_stream_hc, c_level);
        if !dict_copy.is_empty() {
            unsafe {
                load_dict_hc(
                    &mut dict_stream_hc,
                    dict_copy.as_ptr(),
                    dict_copy.len() as i32,
                );
            }
        }
        Ok(StreamHC {
            c_level,
            stream_hc,
            dict_stream_hc,
            _dict: dict_copy,
        })
    }
}

unsafe impl Send for StreamHC {}
unsafe impl Sync for StreamHC {}

impl CompressionStrategy for StreamHC {
    fn compress_block(&mut self, src: &[u8], dst: &mut Vec<u8>) -> io::Result<usize> {
        ensure_dst_capacity(src.len(), dst);
        // Reset the HC stream before each block, discarding cross-block history
        // and re-applying the compression level.
        reset_stream_hc_fast(&mut self.stream_hc, self.c_level);
        // Only attach a dict stream that was prepared by load_dict_hc;
        // an uninitialised dict stream must not be passed here.
        let dict_ptr = if self._dict.is_empty() {
            None
        } else {
            Some(&*self.dict_stream_hc as *const Lz4StreamHc)
        };
        unsafe {
            attach_hc_dictionary(&mut self.stream_hc, dict_ptr);
        }

        let written = unsafe {
            compress_hc_continue(
                &mut self.stream_hc,
                src.as_ptr(),
                dst.as_mut_ptr(),
                src.len() as i32,
                dst.len() as i32,
            )
        };
        if written == 0 {
            Err(io::Error::other(
                "compress_hc_continue returned 0",
            ))
        } else {
            Ok(written as usize)
        }
    }
}

// ── Factory functions ─────────────────────────────────────────────────────────

/// Build a no-dict compression strategy for the given compression level.
///
/// - `c_level < LZ4HC_CLEVEL_MIN (2)` → [`NoStreamFast`]
/// - `c_level ≥ LZ4HC_CLEVEL_MIN`     → [`NoStreamHC`]
///
/// `_src_size` and `_block_size` are accepted for API compatibility with the
/// benchmark loop; strategy selection does not depend on block geometry.
pub fn build_compression_parameters(
    c_level: i32,
    _src_size: usize,
    _block_size: usize,
) -> Box<dyn CompressionStrategy> {
    if c_level < LZ4HC_CLEVEL_MIN {
        Box::new(NoStreamFast::new(c_level))
    } else {
        Box::new(NoStreamHC::new(c_level))
    }
}

/// Build a dict-aware compression strategy for the given level and dictionary.
///
/// - `c_level < LZ4HC_CLEVEL_MIN (2)` → [`StreamFast`]
/// - `c_level ≥ LZ4HC_CLEVEL_MIN`     → [`StreamHC`]
///
/// Pass an empty slice for `dict` to skip dictionary preloading while still
/// using the streaming code path.
pub fn build_compression_parameters_with_dict(
    c_level: i32,
    dict: &[u8],
) -> io::Result<Box<dyn CompressionStrategy>> {
    if c_level < LZ4HC_CLEVEL_MIN {
        Ok(Box::new(StreamFast::new(c_level, dict)?))
    } else {
        Ok(Box::new(StreamHC::new(c_level, dict)?))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::decompress_safe;

    const SAMPLE: &[u8] = b"hello world hello world hello world hello world \
          this is a test of lz4 block compression round-trip!";

    fn lz4_decompress(compressed: &[u8], original_len: usize) -> Vec<u8> {
        let mut out = vec![0u8; original_len];
        let n = decompress_safe(compressed, &mut out).expect("decompress_safe failed");
        assert_eq!(n, original_len);
        out
    }

    #[test]
    fn no_stream_fast_roundtrip() {
        let mut strategy = NoStreamFast::new(1);
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn no_stream_fast_negative_level_roundtrip() {
        // Negative c_level: acceleration = -c_level + 1
        let mut strategy = NoStreamFast::new(-5);
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn no_stream_hc_roundtrip() {
        let mut strategy = NoStreamHC::new(9);
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn no_stream_hc_min_level_roundtrip() {
        let mut strategy = NoStreamHC::new(LZ4HC_CLEVEL_MIN);
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn stream_fast_no_dict_roundtrip() {
        let mut strategy = StreamFast::new(1, b"").unwrap();
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn stream_fast_with_dict_roundtrip() {
        let dict = b"hello world ";
        let mut strategy = StreamFast::new(1, dict).unwrap();
        let mut dst = Vec::new();
        // compress_block should succeed even with a dict attached
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        assert!(n > 0);
    }

    #[test]
    fn stream_hc_no_dict_roundtrip() {
        let mut strategy = StreamHC::new(9, b"").unwrap();
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn stream_hc_with_dict_roundtrip() {
        let dict = b"hello world ";
        let mut strategy = StreamHC::new(9, dict).unwrap();
        let mut dst = Vec::new();
        let n = strategy.compress_block(SAMPLE, &mut dst).unwrap();
        assert!(n > 0);
    }

    #[test]
    fn build_compression_parameters_selects_fast() {
        // c_level 1 < LZ4HC_CLEVEL_MIN=2 → NoStreamFast
        let mut s = build_compression_parameters(1, 65536, 65536);
        let mut dst = Vec::new();
        let n = s.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn build_compression_parameters_selects_hc() {
        // c_level 9 ≥ LZ4HC_CLEVEL_MIN=2 → NoStreamHC
        let mut s = build_compression_parameters(9, 65536, 65536);
        let mut dst = Vec::new();
        let n = s.compress_block(SAMPLE, &mut dst).unwrap();
        let recovered = lz4_decompress(&dst[..n], SAMPLE.len());
        assert_eq!(recovered.as_slice(), SAMPLE);
    }

    #[test]
    fn build_with_dict_selects_stream_fast() {
        let dict = b"hello world ";
        let mut s = build_compression_parameters_with_dict(1, dict).unwrap();
        let mut dst = Vec::new();
        s.compress_block(SAMPLE, &mut dst).unwrap();
    }

    #[test]
    fn build_with_dict_selects_stream_hc() {
        let dict = b"hello world ";
        let mut s = build_compression_parameters_with_dict(9, dict).unwrap();
        let mut dst = Vec::new();
        s.compress_block(SAMPLE, &mut dst).unwrap();
    }
}
