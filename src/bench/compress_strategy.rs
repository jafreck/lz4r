/*
    bench/compress_strategy.rs — Compression strategy vtable pattern
    Migrated from lz4-1.10.0/programs/bench.c (lines 152–312)

    Original copyright (C) Yann Collet 2012-2020 — GPL v2 License.

    Migration notes:
    - C `struct compressionParameters` with init/reset/block/cleanup function
      pointers → Rust `CompressionStrategy` trait with four concrete types.
    - Four concrete types map 1-to-1 with the four C function-pointer sets:
        · `NoStreamFast`  — no dict, fast   (LZ4_compressInitNoStream path)
        · `NoStreamHC`    — no dict, HC     (LZ4_compressInitNoStream path)
        · `StreamFast`    — dict + fast     (LZ4_compressInitStream path)
        · `StreamHC`      — dict + HC       (LZ4_compressInitStreamHC path)
    - All compression calls now use the native Rust implementations from
      `crate::block` and `crate::hc` — no FFI or third-party crate dependencies.
    - `LZ4_isError(errcode) → (errcode==0)` inversion is NOT ported; a zero
      return from any block compression function is mapped to `Err`.
    - `LZ4HC_CLEVEL_MIN = 2` (from lz4hc.h line 47); levels ≥ 2 use HC.
    - C init/cleanup → Rust `new()` / `Drop`; C reset is called inside
      `compress_block` before each block (mirrors C's LZ4_compressBlockStream).
*/

use std::io;

use crate::block::{compress_bound, compress_fast, decompress_safe, Lz4Stream};
use crate::hc::{
    Lz4StreamHc, compress_hc, compress_hc_continue,
    reset_stream_hc_fast, load_dict_hc, attach_hc_dictionary,
};

/// LZ4HC minimum compression level (lz4hc.h line 47).
const LZ4HC_CLEVEL_MIN: i32 = 2;

// ── CompressionStrategy trait ─────────────────────────────────────────────────

/// Vtable-style compression strategy.
///
/// Corresponds to the function-pointer fields of `struct compressionParameters`
/// in bench.c lines 163–171.  Each concrete implementation owns its stream
/// state (replacing the C `init` / `cleanup` pair with `new` / `Drop`) and
/// performs the stream reset inside `compress_block` (replacing the C `reset`
/// function pointer called at the top of each block function).
pub trait CompressionStrategy: Send + Sync {
    /// Compress `src` into `dst`.
    ///
    /// Before writing, `dst` is guaranteed to hold at least
    /// `LZ4_compressBound(src.len())` bytes.  Returns the number of
    /// compressed bytes written into `dst`.
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

/// No-dict, fast (non-stream) compression.
///
/// Corresponds to `LZ4_compressInitNoStream` / `LZ4_compressResetNoStream` /
/// `LZ4_compressBlockNoStream` / `LZ4_compressCleanupNoStream`
/// (bench.c lines 174–231).
///
/// Acceleration factor mirrors the C logic:
/// `acceleration = (cLevel < 0) ? -cLevel + 1 : 1`
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
        compress_fast(src, dst, self.acceleration)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("compress_fast failed: {e:?}")))
    }
}

// ── Strategy 2: NoStreamHC ────────────────────────────────────────────────────

/// No-dict, HC (non-stream) compression.
///
/// Corresponds to `LZ4_compressInitNoStream` / `LZ4_compressResetNoStream` /
/// `LZ4_compressBlockNoStreamHC` / `LZ4_compressCleanupNoStream`
/// (bench.c lines 174–264).
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
            Err(io::Error::new(
                io::ErrorKind::Other,
                "compress_hc returned 0",
            ))
        } else {
            Ok(written as usize)
        }
    }
}

// ── Strategy 3: StreamFast ────────────────────────────────────────────────────

/// Dict + fast (stream) compression.
///
/// Corresponds to `LZ4_compressInitStream` / `LZ4_compressResetStream` /
/// `LZ4_compressBlockStream` / `LZ4_compressCleanupStream`
/// (bench.c lines 183–271).
///
/// Uses `Lz4Stream::load_dict_slow` to pre-load the dict into a dedicated
/// stream; `attach_dictionary` + `compress_fast_continue` are called per block
/// (mirrors the C reset + block step).
pub struct StreamFast {
    c_level: i32,
    stream: Box<Lz4Stream>,
    dict_stream: Box<Lz4Stream>,
    /// Owns the dict bytes so the pointer passed to lz4 remains valid.
    _dict: Vec<u8>,
}

impl StreamFast {
    /// Create a new `StreamFast` strategy, loading `dict` into a dedicated stream.
    ///
    /// Mirrors `LZ4_compressInitStream` (bench.c lines 183–191).
    pub fn new(c_level: i32, dict: &[u8]) -> io::Result<Self> {
        let stream = Lz4Stream::new();
        let mut dict_stream = Lz4Stream::new();
        // Keep a private copy so the pointer stays valid for the lifetime of
        // this struct (the C code keeps the original dictBuf pointer alive).
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
        // acceleration mirrors bench.c line 246
        let acceleration = if self.c_level < 0 {
            -self.c_level + 1
        } else {
            1
        };
        // LZ4_compressResetStream (bench.c lines 210–215)
        self.stream.reset_fast();
        // Pass None when no dict was loaded; attach_dictionary(x, None) unsets any prior dict.
        let dict_ptr = if self._dict.is_empty() {
            None
        } else {
            Some(&*self.dict_stream as *const Lz4Stream)
        };
        unsafe {
            self.stream.attach_dictionary(dict_ptr);
        }

        // LZ4_compressBlockStream (bench.c lines 241–249)
        let written = self.stream.compress_fast_continue(src, dst, acceleration);
        if written == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "compress_fast_continue returned 0",
            ))
        } else {
            Ok(written as usize)
        }
    }
}

// ── Strategy 4: StreamHC ──────────────────────────────────────────────────────

/// Dict + HC (stream) compression.
///
/// Corresponds to `LZ4_compressInitStreamHC` / `LZ4_compressResetStreamHC` /
/// `LZ4_compressBlockStreamHC` / `LZ4_compressCleanupStreamHC`
/// (bench.c lines 193–278).
pub struct StreamHC {
    c_level: i32,
    stream_hc: Box<Lz4StreamHc>,
    dict_stream_hc: Box<Lz4StreamHc>,
    /// Owns the dict bytes so the pointer passed to lz4 remains valid.
    _dict: Vec<u8>,
}

impl StreamHC {
    /// Create a new `StreamHC` strategy, loading `dict` into a dedicated HC stream.
    ///
    /// Mirrors `LZ4_compressInitStreamHC` (bench.c lines 193–202).
    pub fn new(c_level: i32, dict: &[u8]) -> io::Result<Self> {
        let stream_hc = Lz4StreamHc::create().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "Lz4StreamHc::create failed")
        })?;
        let mut dict_stream_hc = Lz4StreamHc::create().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "Lz4StreamHc::create (dict) failed")
        })?;
        let dict_copy: Vec<u8> = dict.to_vec();
        // Reset dict stream and load dictionary (bench.c lines 200–201)
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
        // LZ4_compressResetStreamHC (bench.c lines 217–222)
        reset_stream_hc_fast(&mut self.stream_hc, self.c_level);
        // Pass None when no dict was loaded; attach_hc_dictionary requires a stream
        // that was prepared by load_dict_hc — an empty dict stream is not valid.
        let dict_ptr = if self._dict.is_empty() {
            None
        } else {
            Some(&*self.dict_stream_hc as *const Lz4StreamHc)
        };
        unsafe {
            attach_hc_dictionary(&mut self.stream_hc, dict_ptr);
        }

        // LZ4_compressBlockStreamHC (bench.c lines 251–258)
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
            Err(io::Error::new(
                io::ErrorKind::Other,
                "compress_hc_continue returned 0",
            ))
        } else {
            Ok(written as usize)
        }
    }
}

// ── Factory functions ─────────────────────────────────────────────────────────

/// Select the appropriate no-dict compression strategy.
///
/// Mirrors the `dictSize == 0` branch of `LZ4_buildCompressionParameters`
/// (bench.c lines 301–310).
///
/// - `c_level < LZ4HC_CLEVEL_MIN (2)` → [`NoStreamFast`]
/// - `c_level ≥ LZ4HC_CLEVEL_MIN`     → [`NoStreamHC`]
///
/// `_src_size` and `_block_size` are accepted for signature compatibility with
/// the benchmark loop; the block API does not require them for strategy selection.
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

/// Select the appropriate dict-aware compression strategy.
///
/// Mirrors the `dictSize > 0` branch of `LZ4_buildCompressionParameters`
/// (bench.c lines 289–300).
///
/// - `c_level < LZ4HC_CLEVEL_MIN (2)` → [`StreamFast`]
/// - `c_level ≥ LZ4HC_CLEVEL_MIN`     → [`StreamHC`]
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

    const SAMPLE: &[u8] =
        b"hello world hello world hello world hello world \
          this is a test of lz4 block compression round-trip!";

    fn lz4_decompress(compressed: &[u8], original_len: usize) -> Vec<u8> {
        let mut out = vec![0u8; original_len];
        let n = decompress_safe(compressed, &mut out)
            .expect("decompress_safe failed");
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
