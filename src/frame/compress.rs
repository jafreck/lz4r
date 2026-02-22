//! LZ4 Frame streaming compression.
//!
//! Translated from lz4frame.c v1.10.0, lines 419–1244.
//!
//! # Coverage
//! - cctx lifecycle: [`lz4f_create_compression_context`],
//!   [`lz4f_free_compression_context`], [`Drop`] for [`Lz4FCCtx`]
//! - Stream initialisation: [`lz4f_init_stream`]
//! - Frame header write: [`lz4f_compress_begin_internal`], and all
//!   `compress_begin*` variants
//! - Block dispatch: [`CompressMode`] enum replaces the C `compressFunc_t`
//!   function pointer; dispatched inside [`lz4f_make_block`]
//! - Streaming update: [`lz4f_compress_update_impl`], [`lz4f_compress_update`],
//!   [`lz4f_uncompressed_update`], [`lz4f_flush`], [`lz4f_compress_end`]
//! - One-shot: [`lz4f_compress_frame_using_cdict`], [`lz4f_compress_frame`]
//!
//! # goto elimination
//! The three `goto _end` cleanup patterns in `LZ4F_compressFrame` and
//! `LZ4F_compressFrame_usingCDict` are eliminated via Rust's RAII / `?`
//! operator.  The temporary cctx in `lz4f_compress_frame` is a local
//! `Box<Lz4FCCtx>` dropped at the end of the function regardless of success.

use crate::block::compress::compress_fast_ext_state_fast_reset;
use crate::block::stream::Lz4Stream;
use crate::frame::cdict::Lz4FCDict;
use crate::frame::header::{
    lz4f_compress_bound_internal, lz4f_compress_frame_bound, lz4f_get_block_size,
    lz4f_header_checksum, lz4f_optimal_bsid, write_le32, write_le64,
};
use crate::frame::types::{
    BlockChecksum, BlockCompressMode, BlockMode, BlockSizeId, ContentChecksum, CtxType,
    Lz4FError, Lz4FCCtx, Preferences, BF_SIZE, BH_SIZE, MAX_FH_SIZE,
};
use crate::hc::api::{
    attach_hc_dictionary, compress_hc_continue, compress_hc_ext_state_fast_reset,
    favor_decompression_speed, init_stream_hc, load_dict_hc, reset_stream_hc_fast,
    save_dict_hc, set_compression_level as hc_set_compression_level, Lz4StreamHc,
};
use crate::hc::types::LZ4HC_CLEVEL_MIN;
use crate::xxhash::{xxh32_oneshot, Xxh32State};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 frame magic number (lz4frame.h:280).
pub const LZ4F_MAGIC_NUMBER: u32 = 0x184D_2204;

/// API version guarded by `LZ4F_createCompressionContext`.
pub const LZ4F_VERSION: u32 = 100;

/// Platform pointer size in bytes (used to store raw Box pointers in Vec<u8>).
const PTR_BYTES: usize = core::mem::size_of::<*mut ()>();

/// 64 KiB — dictionary window size in linked-block mode.
const KB64: usize = 64 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// CompressOptions (LZ4F_compressOptions_t in lz4frame.h:202-208)
// ─────────────────────────────────────────────────────────────────────────────

/// Per-call options for streaming compression.
///
/// Corresponds to `LZ4F_compressOptions_t` in lz4frame.h.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompressOptions {
    /// When `true`, the `src` buffer is guaranteed to remain accessible for
    /// all future calls in the same session.  The implementation may then skip
    /// copying input data into its internal staging buffer (linked-block mode).
    ///
    /// Equivalent to the `stableSrc` field (lz4frame.h:204).
    pub stable_src: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressMode — replaces compressFunc_t function pointer (lz4frame.c:876)
// ─────────────────────────────────────────────────────────────────────────────

/// Block compression dispatch mode.
///
/// Replaces the C `compressFunc_t` function pointer.  Selected once per
/// `compress_update_impl` call by [`select_compress_mode`] and then used
/// inside [`lz4f_make_block`].
///
/// Mirrors `LZ4F_selectCompression` and the five static compress helper
/// functions (lz4frame.c:952–962).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompressMode {
    /// `LZ4F_compressBlock` — fast, each block independently reset.
    FastIndependent,
    /// `LZ4F_compressBlock_continue` — fast, blocks share linked history.
    FastLinked,
    /// `LZ4F_compressBlockHC` — HC, each block independently reset.
    HcIndependent,
    /// `LZ4F_compressBlockHC_continue` — HC, blocks share linked history.
    HcLinked,
    /// `LZ4F_doNotCompressBlock` — store input verbatim (uncompressed flag).
    Uncompressed,
}

/// Select the block compression mode from frame preferences.
///
/// Mirrors `LZ4F_selectCompression` (lz4frame.c:952–962).
#[inline]
fn select_compress_mode(
    block_mode: BlockMode,
    level: i32,
    compress_mode: BlockCompressMode,
) -> CompressMode {
    if compress_mode == BlockCompressMode::Uncompressed {
        return CompressMode::Uncompressed;
    }
    if level < LZ4HC_CLEVEL_MIN {
        if block_mode == BlockMode::Independent {
            CompressMode::FastIndependent
        } else {
            CompressMode::FastLinked
        }
    } else if block_mode == BlockMode::Independent {
        CompressMode::HcIndependent
    } else {
        CompressMode::HcLinked
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Inner context raw-pointer management helpers
// ─────────────────────────────────────────────────────────────────────────────
//
// `Lz4FCCtx::lz4_ctx` stores exactly PTR_BYTES (== sizeof(*mut ())) bytes
// holding the raw pointer of a heap-allocated `Lz4Stream` or `Lz4StreamHc`.
// `lz4_ctx_alloc` tracks what type was Box-allocated (0=none, 1=Fast, 2=HC).
// `lz4_ctx_type` tracks the currently initialised type.

fn read_inner_ptr(cctx: &Lz4FCCtx) -> usize {
    match cctx.lz4_ctx.as_ref() {
        Some(v) if v.len() >= PTR_BYTES => {
            usize::from_ne_bytes(v[..PTR_BYTES].try_into().unwrap())
        }
        _ => 0,
    }
}

fn write_inner_ptr(cctx: &mut Lz4FCCtx, ptr: usize) {
    let bytes = ptr.to_ne_bytes();
    match cctx.lz4_ctx.as_mut() {
        Some(v) if v.len() >= PTR_BYTES => {
            v[..PTR_BYTES].copy_from_slice(&bytes);
        }
        _ => {
            cctx.lz4_ctx = Some(bytes.to_vec());
        }
    }
}

/// Free the inner LZ4/HC context stored in `cctx.lz4_ctx`.
///
/// # Safety
/// Must only be called once per allocation cycle.  Caller must ensure no
/// outstanding references to the inner context exist.
unsafe fn free_inner_ctx(cctx: &mut Lz4FCCtx) {
    let ptr = read_inner_ptr(cctx);
    if ptr != 0 {
        match cctx.lz4_ctx_alloc {
            1 => drop(Box::from_raw(ptr as *mut Lz4Stream)),
            2 => drop(Box::from_raw(ptr as *mut Lz4StreamHc)),
            _ => {}
        }
    }
    cctx.lz4_ctx = None;
    cctx.lz4_ctx_alloc = 0;
    cctx.lz4_ctx_type = CtxType::None;
}

/// Get a raw mutable pointer to the inner fast (LZ4) context.
///
/// # Safety
/// `cctx.lz4_ctx_alloc` must equal 1 and the stored pointer must be non-null.
#[inline]
unsafe fn fast_ctx_ptr(cctx: &Lz4FCCtx) -> *mut Lz4Stream {
    read_inner_ptr(cctx) as *mut Lz4Stream
}

/// Get a raw mutable pointer to the inner HC context.
///
/// # Safety
/// `cctx.lz4_ctx_alloc` must equal 2 and the stored pointer must be non-null.
#[inline]
unsafe fn hc_ctx_ptr(cctx: &Lz4FCCtx) -> *mut Lz4StreamHc {
    read_inner_ptr(cctx) as *mut Lz4StreamHc
}

/// Get the attached CDict pointer (or null).
///
/// # Safety
/// The returned pointer is valid as long as the external CDict lives.
#[inline]
unsafe fn cdict_ref(cctx: &Lz4FCCtx) -> *const Lz4FCDict {
    cctx.cdict_ptr as *const Lz4FCDict
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_initStream (lz4frame.c:648–674)
// ─────────────────────────────────────────────────────────────────────────────

/// Prepare the inner LZ4/HC stream for a new block.
///
/// For fast streams (level < `LZ4HC_CLEVEL_MIN`): resets and optionally
/// attaches the CDict's fast context when `cdict` is non-null OR when
/// `block_mode` is linked.  In the independent/no-dict case the one-shot
/// API performs its own reset, so no action is needed here.
///
/// For HC streams: always fast-resets and optionally attaches the HC dict.
///
/// Mirrors `LZ4F_initStream` (lz4frame.c:648–674).
///
/// # Safety
/// `ctx_ptr` must be a valid, exclusively accessible pointer to the inner
/// stream; `cdict` (if non-null) must outlive this call.
unsafe fn lz4f_init_stream(
    ctx_ptr: usize,        // raw pointer to Lz4Stream or Lz4StreamHc
    cdict: *const Lz4FCDict,
    level: i32,
    block_mode: BlockMode,
) {
    if level < LZ4HC_CLEVEL_MIN {
        let stream = &mut *(ctx_ptr as *mut Lz4Stream);
        if !cdict.is_null() || block_mode == BlockMode::Linked {
            stream.reset_fast();
            if !cdict.is_null() {
                // attach the CDict's fast context
                let dict_stream: *const Lz4Stream = &*(*cdict).fast_ctx;
                stream.attach_dictionary(Some(dict_stream));
            }
        }
        // else: one-shot API resets internally — nothing to do here
    } else {
        let stream = &mut *(ctx_ptr as *mut Lz4StreamHc);
        reset_stream_hc_fast(stream, level);
        if !cdict.is_null() {
            let hc_dict: *const Lz4StreamHc = &*(*cdict).hc_ctx;
            attach_hc_dictionary(stream, Some(hc_dict));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Local save-dict helper (lz4frame.c:964–970)
// ─────────────────────────────────────────────────────────────────────────────

/// Copy up to 64 KB of compression history into the start of `cctx.tmp_buf`.
///
/// Returns the number of bytes actually saved.  Called after compressing a
/// block from src (not tmp) to preserve the dictionary for linked-block mode.
///
/// Mirrors `LZ4F_localSaveDict` (lz4frame.c:964–970).
///
/// # Safety
/// Inner context pointer in `cctx.lz4_ctx` must be valid.
unsafe fn local_save_dict(cctx: &mut Lz4FCCtx) -> i32 {
    let buf_len = cctx.tmp_buf.len().min(KB64) as i32;
    let buf_ptr = cctx.tmp_buf.as_mut_ptr();
    if cctx.prefs.compression_level < LZ4HC_CLEVEL_MIN {
        let stream = &mut *fast_ctx_ptr(cctx);
        let slice = core::slice::from_raw_parts_mut(buf_ptr, buf_len as usize);
        stream.save_dict(slice)
    } else {
        let stream = &mut *hc_ctx_ptr(cctx);
        save_dict_hc(stream, buf_ptr, buf_len)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_makeBlock (lz4frame.c:879–908)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress one block and write the LZ4 block header (+ optional checksum).
///
/// `dst` must have capacity ≥ `BH_SIZE + src.len() + (if crc { BF_SIZE } else { 0 })`.
///
/// Returns the number of bytes written to `dst`.
///
/// Mirrors `LZ4F_makeBlock` (lz4frame.c:879–908) with the `compressFunc_t`
/// replaced by [`CompressMode`].
///
/// # Safety
/// `cctx.lz4_ctx` must hold a valid inner context appropriate for `mode`.
unsafe fn lz4f_make_block(
    dst: &mut [u8],
    src: &[u8],
    mode: CompressMode,
    cctx: &mut Lz4FCCtx,
    crc_flag: bool,
) -> usize {
    let src_size = src.len();
    let level = cctx.prefs.compression_level;
    let ctx_ptr = read_inner_ptr(cctx);
    let cdict = cdict_ref(cctx);

    // Compression destination: dst[BH_SIZE..] with capacity srcSize-1.
    // If compressed output >= srcSize, we fall back to uncompressed storage.
    // Mirrors: compress(lz4ctx, src, cSizePtr+BHSize, srcSize, srcSize-1, level, cdict)
    let compress_dst: *mut u8 = dst.as_mut_ptr().add(BH_SIZE);
    let compress_cap: i32 = if src_size > 0 { (src_size - 1) as i32 } else { 0 };

    // Attempt compression — returns 0 when not compressible or on error.
    let c_size: usize = match mode {
        CompressMode::Uncompressed => 0,

        CompressMode::FastIndependent => {
            // init_stream first (resets + attaches dict if present),
            // then create the &mut reference — avoids two simultaneous &mut to same memory.
            lz4f_init_stream(ctx_ptr, cdict, level, BlockMode::Independent);
            let stream = &mut *(ctx_ptr as *mut Lz4Stream);
            let result: i32 = if !cdict.is_null() {
                // Dict attached: use continue API (stream was reset+attached above).
                let dst_slice =
                    core::slice::from_raw_parts_mut(compress_dst, src_size.saturating_sub(1));
                stream.compress_fast_continue(src, dst_slice, accel(level))
            } else {
                // No dict: one-shot fast-reset compress.
                match compress_fast_ext_state_fast_reset(
                    &mut stream.internal,
                    src.as_ptr(),
                    src_size as i32,
                    compress_dst,
                    compress_cap,
                    accel(level),
                ) {
                    Ok(n) => n as i32,
                    Err(_) => 0,
                }
            };
            result.max(0) as usize
        }

        CompressMode::FastLinked => {
            // Linked: stream was initialised once at frame start; just continue.
            let stream = &mut *(ctx_ptr as *mut Lz4Stream);
            let dst_slice =
                core::slice::from_raw_parts_mut(compress_dst, src_size.saturating_sub(1));
            stream.compress_fast_continue(src, dst_slice, accel(level)).max(0) as usize
        }

        CompressMode::HcIndependent => {
            // HC independent: init per-block then compress.
            lz4f_init_stream(ctx_ptr, cdict, level, BlockMode::Independent);
            let stream = &mut *(ctx_ptr as *mut Lz4StreamHc);
            let result: i32 = if !cdict.is_null() {
                compress_hc_continue(stream, src.as_ptr(), compress_dst, src_size as i32, compress_cap)
            } else {
                compress_hc_ext_state_fast_reset(
                    stream,
                    src.as_ptr(),
                    compress_dst,
                    src_size as i32,
                    compress_cap,
                    level,
                )
            };
            result.max(0) as usize
        }

        CompressMode::HcLinked => {
            // HC linked: stream was initialised once at frame start; just continue.
            let stream = &mut *(ctx_ptr as *mut Lz4StreamHc);
            compress_hc_continue(stream, src.as_ptr(), compress_dst, src_size as i32, compress_cap)
                .max(0) as usize
        }
    };

    // Decide: compressed or uncompressed block?
    let final_c_size: usize;
    if c_size == 0 || c_size >= src_size {
        // Not compressible — store raw
        final_c_size = src_size;
        write_le32(dst, 0, final_c_size as u32 | crate::frame::types::LZ4F_BLOCKUNCOMPRESSED_FLAG);
        dst[BH_SIZE..BH_SIZE + src_size].copy_from_slice(src);
    } else {
        final_c_size = c_size;
        write_le32(dst, 0, final_c_size as u32);
    }

    // Optional per-block checksum (XXH32 of the stored block data).
    if crc_flag {
        let crc = xxh32_oneshot(&dst[BH_SIZE..BH_SIZE + final_c_size], 0);
        write_le32(dst, BH_SIZE + final_c_size, crc);
    }

    BH_SIZE + final_c_size + if crc_flag { BF_SIZE } else { 0 }
}

/// Compute fast-path LZ4 acceleration from compression level.
/// Negative levels map to positive acceleration (mirrors C `level < 0 ? -level+1 : 1`).
#[inline]
fn accel(level: i32) -> i32 {
    if level < 0 {
        -level + 1
    } else {
        1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FCCtx lifecycle
// ─────────────────────────────────────────────────────────────────────────────

impl Lz4FCCtx {
    /// Create a zeroed compression context.
    ///
    /// Mirrors `LZ4F_createCompressionContext_advanced` (lz4frame.c:595–607).
    pub fn new(version: u32) -> Box<Self> {
        Box::new(Lz4FCCtx {
            cmem: Default::default(),
            prefs: Preferences::default(),
            version,
            c_stage: 0,
            max_block_size: 0,
            max_buffer_size: 0,
            tmp_buf: Vec::new(),
            tmp_in_offset: 0,
            tmp_in_size: 0,
            total_in_size: 0,
            xxh: Xxh32State::new(0),
            lz4_ctx: None,
            lz4_ctx_alloc: 0,
            lz4_ctx_type: CtxType::None,
            block_compress_mode: BlockCompressMode::Compressed,
            cdict_ptr: 0,
        })
    }
}

impl Drop for Lz4FCCtx {
    /// Free the inner LZ4/HC context.
    ///
    /// Mirrors `LZ4F_freeCompressionContext` (lz4frame.c:629–637).
    fn drop(&mut self) {
        // SAFETY: We own the context and this is called at most once.
        unsafe { free_inner_ctx(self) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public context lifecycle API
// ─────────────────────────────────────────────────────────────────────────────

/// Allocate a new LZ4F compression context.
///
/// Mirrors `LZ4F_createCompressionContext` (lz4frame.c:617–627).
///
/// Returns `Err(Lz4FError::AllocationFailed)` if `version != LZ4F_VERSION`.
pub fn lz4f_create_compression_context(
    version: u32,
) -> Result<Box<Lz4FCCtx>, Lz4FError> {
    if version != LZ4F_VERSION {
        return Err(Lz4FError::AllocationFailed);
    }
    Ok(Lz4FCCtx::new(version))
}

/// Free a compression context.
///
/// Accepts `Box<Lz4FCCtx>`; all cleanup is handled by `Drop`.
/// Mirrors `LZ4F_freeCompressionContext` (lz4frame.c:629–637).
#[inline]
pub fn lz4f_free_compression_context(_cctx: Box<Lz4FCCtx>) {
    // dropping the Box calls Lz4FCCtx::drop which frees inner context
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_compressBegin_internal (lz4frame.c:690–813)
// ─────────────────────────────────────────────────────────────────────────────

/// Write the LZ4 frame header and initialise a compression session.
///
/// Only one of `dict_buffer` or `cdict` should be non-null/non-zero (the C
/// assert is `assert(cdict == NULL || dictBuffer == NULL)`).
///
/// Returns the number of bytes written to `dst`.
///
/// Mirrors `LZ4F_compressBegin_internal` (lz4frame.c:690–813).
pub fn lz4f_compress_begin_internal(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    dict_buffer: Option<&[u8]>,
    cdict: Option<*const Lz4FCDict>,
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError> {
    if dst.len() < MAX_FH_SIZE {
        return Err(Lz4FError::DstMaxSizeTooSmall);
    }

    let prefs_val = prefs.copied().unwrap_or_default();
    cctx.prefs = prefs_val;

    // ── Inner context management ──────────────────────────────────────────────
    let ctx_type_id: u16 = if cctx.prefs.compression_level < LZ4HC_CLEVEL_MIN {
        1
    } else {
        2
    };

    // Determine whether we need to (re-)allocate the inner context.
    // C: `if (allocatedSize < requiredSize)` — in C, HC > Fast in bytes.
    // In Rust we track by type-ID; 2 (HC) > 1 (Fast) > 0 (None).
    if cctx.lz4_ctx_alloc < ctx_type_id {
        // Free old (if any) and allocate the correct type.
        // SAFETY: We are about to overwrite lz4_ctx so no aliasing.
        unsafe { free_inner_ctx(cctx) };
        let raw_ptr: usize = if ctx_type_id == 1 {
            let stream = Lz4Stream::new();
            Box::into_raw(stream) as usize
        } else {
            let stream = Lz4StreamHc::create().ok_or(Lz4FError::AllocationFailed)?;
            Box::into_raw(stream) as usize
        };
        write_inner_ptr(cctx, raw_ptr);
        cctx.lz4_ctx_alloc = ctx_type_id;
        cctx.lz4_ctx_type = if ctx_type_id == 1 { CtxType::Fast } else { CtxType::Hc };
    } else if cctx.lz4_ctx_type as u16 != ctx_type_id {
        // Already have enough space, just re-initialise to the correct type.
        let ptr = read_inner_ptr(cctx);
        if ctx_type_id == 1 {
            // SAFETY: lz4_ctx_alloc >= 1, ptr points to valid Lz4Stream bytes.
            unsafe {
                let stream = &mut *(ptr as *mut Lz4Stream);
                stream.reset();
            }
        } else {
            // SAFETY: lz4_ctx_alloc >= 2, ptr points to valid Lz4StreamHc.
            unsafe {
                let stream = &mut *(ptr as *mut Lz4StreamHc);
                init_stream_hc(stream);
                set_hc_level(stream, cctx.prefs.compression_level);
            }
        }
        cctx.lz4_ctx_type = if ctx_type_id == 1 { CtxType::Fast } else { CtxType::Hc };
    }

    // ── Buffer management ─────────────────────────────────────────────────────
    if cctx.prefs.frame_info.block_size_id == BlockSizeId::Default {
        cctx.prefs.frame_info.block_size_id = BlockSizeId::Max64Kb;
    }
    cctx.max_block_size =
        lz4f_get_block_size(cctx.prefs.frame_info.block_size_id).unwrap_or(KB64);

    let required_buff_size: usize = if prefs_val.auto_flush {
        if cctx.prefs.frame_info.block_mode == BlockMode::Linked {
            KB64
        } else {
            0
        }
    } else {
        cctx.max_block_size
            + if cctx.prefs.frame_info.block_mode == BlockMode::Linked {
                128 * 1024
            } else {
                0
            }
    };

    if cctx.max_buffer_size < required_buff_size {
        cctx.tmp_buf = vec![0u8; required_buff_size];
        cctx.max_buffer_size = required_buff_size;
    }
    cctx.tmp_in_offset = 0;
    cctx.tmp_in_size = 0;
    cctx.xxh = Xxh32State::new(0);

    // ── Attach cdict / init stream ────────────────────────────────────────────
    let cdict_raw: *const Lz4FCDict = cdict.unwrap_or(core::ptr::null());
    cctx.cdict_ptr = cdict_raw as usize;

    let ctx_ptr = read_inner_ptr(cctx);
    if cctx.prefs.frame_info.block_mode == BlockMode::Linked {
        // Frame-level init for linked blocks only; independent blocks init per-block.
        unsafe {
            lz4f_init_stream(ctx_ptr, cdict_raw, cctx.prefs.compression_level, BlockMode::Linked);
        }
    }
    if cctx.prefs.compression_level >= LZ4HC_CLEVEL_MIN {
        unsafe {
            let stream = &mut *(ctx_ptr as *mut Lz4StreamHc);
            favor_decompression_speed(stream, prefs_val.favor_dec_speed);
        }
    }

    // Load raw dict buffer (only when no CDict is provided).
    if let Some(dict) = dict_buffer {
        if !dict.is_empty() {
            if dict.len() > i32::MAX as usize {
                return Err(Lz4FError::ParameterInvalid);
            }
            unsafe {
                if ctx_type_id == 1 {
                    let stream = &mut *(ctx_ptr as *mut Lz4Stream);
                    stream.load_dict(dict);
                } else {
                    let stream = &mut *(ctx_ptr as *mut Lz4StreamHc);
                    load_dict_hc(stream, dict.as_ptr(), dict.len() as i32);
                }
            }
        }
    }

    // ── Write frame header ────────────────────────────────────────────────────
    let mut pos: usize = 0;

    // Magic number (4 bytes)
    write_le32(dst, pos, LZ4F_MAGIC_NUMBER);
    pos += 4;

    let header_start = pos;

    // FLG byte
    let fi = &cctx.prefs.frame_info;
    let flg: u8 = (1u8 << 6) // Version = 01
        | ((fi.block_mode as u8 & 1) << 5)
        | ((fi.block_checksum_flag as u8 & 1) << 4)
        | (if fi.content_size > 0 { 1u8 } else { 0 } << 3)
        | ((fi.content_checksum_flag as u8 & 1) << 2)
        | (if fi.dict_id > 0 { 1u8 } else { 0 });
    dst[pos] = flg;
    pos += 1;

    // BD byte
    let bd: u8 = (fi.block_size_id as u8 & 7) << 4;
    dst[pos] = bd;
    pos += 1;

    // Optional content size (8 bytes LE)
    if fi.content_size > 0 {
        write_le64(dst, pos, fi.content_size);
        pos += 8;
        cctx.total_in_size = 0;
    }

    // Optional dictionary ID (4 bytes LE)
    if fi.dict_id > 0 {
        write_le32(dst, pos, fi.dict_id);
        pos += 4;
    }

    // Header checksum byte (XXH32 of FLG..dictID, byte [1])
    let hc = lz4f_header_checksum(&dst[header_start..pos]);
    dst[pos] = hc;
    pos += 1;

    cctx.c_stage = 1; // header written; ready to accept blocks
    Ok(pos)
}

// ─────────────────────────────────────────────────────────────────────────────
// compressBegin variants (lz4frame.c:815–859)
// ─────────────────────────────────────────────────────────────────────────────

/// Begin a new compression frame with default preferences.
///
/// Mirrors `LZ4F_compressBegin` (lz4frame.c:815–822).
pub fn lz4f_compress_begin(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError> {
    lz4f_compress_begin_internal(cctx, dst, None, None, prefs)
}

/// Begin using a raw dictionary buffer (applied once, not per block).
///
/// Note: like the C implementation, this applies the dictionary once rather
/// than per block in independent-block mode.  For per-block dict reuse,
/// prefer [`lz4f_compress_begin_using_cdict`].
///
/// Mirrors `LZ4F_compressBegin_usingDictOnce` / `LZ4F_compressBegin_usingDict`
/// (lz4frame.c:828–849).
pub fn lz4f_compress_begin_using_dict(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    dict: &[u8],
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError> {
    lz4f_compress_begin_internal(cctx, dst, Some(dict), None, prefs)
}

/// Begin using a pre-digested [`Lz4FCDict`].
///
/// Mirrors `LZ4F_compressBegin_usingCDict` (lz4frame.c:851–859).
///
/// # Safety
/// `cdict` must remain valid and unmodified for the entire session
/// (until `lz4f_compress_end` or the cctx is freed).
pub unsafe fn lz4f_compress_begin_using_cdict(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    cdict: *const Lz4FCDict,
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError> {
    let cdict_opt = if cdict.is_null() { None } else { Some(cdict) };
    lz4f_compress_begin_internal(cctx, dst, None, cdict_opt, prefs)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_compressBound (lz4frame.c:862–873)
// ─────────────────────────────────────────────────────────────────────────────

/// Worst-case output buffer size for a streaming `compress_update` call.
///
/// Mirrors `LZ4F_compressBound` (lz4frame.c:862–873).
pub fn lz4f_compress_bound(src_size: usize, prefs: Option<&Preferences>) -> usize {
    let default_prefs = Preferences::default();
    let prefs = prefs.unwrap_or(&default_prefs);
    let already_buffered = if prefs.auto_flush {
        0
    } else {
        usize::MAX // mirrors (size_t)-1 in C
    };
    lz4f_compress_bound_internal(src_size, prefs, already_buffered)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_compressUpdateImpl (lz4frame.c:989–1105)
// ─────────────────────────────────────────────────────────────────────────────

/// Core streaming compression: buffer input and emit complete blocks.
///
/// Called by both [`lz4f_compress_update`] (compressed blocks) and
/// [`lz4f_uncompressed_update`] (verbatim blocks).
///
/// Mirrors `LZ4F_compressUpdateImpl` (lz4frame.c:989–1105).
pub fn lz4f_compress_update_impl(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    src: &[u8],
    opts: Option<&CompressOptions>,
    block_compression: BlockCompressMode,
) -> Result<usize, Lz4FError> {
    if cctx.c_stage != 1 {
        return Err(Lz4FError::CompressionStateUninitialized);
    }
    // Capacity checks
    if dst.len()
        < lz4f_compress_bound_internal(src.len(), &cctx.prefs, cctx.tmp_in_size)
    {
        return Err(Lz4FError::DstMaxSizeTooSmall);
    }
    if block_compression == BlockCompressMode::Uncompressed && dst.len() < src.len() {
        return Err(Lz4FError::DstMaxSizeTooSmall);
    }

    let opts = opts.copied().unwrap_or_default();
    let block_size = cctx.max_block_size;
    let mut dst_pos: usize = 0;

    // Flush any pending data if the compression mode changed between calls.
    if cctx.block_compress_mode != block_compression {
        let flush_size = lz4f_flush_impl(cctx, dst, None)?;
        dst_pos += flush_size;
        cctx.block_compress_mode = block_compression;
    }

    let compress_mode = select_compress_mode(
        cctx.prefs.frame_info.block_mode,
        cctx.prefs.compression_level,
        block_compression,
    );
    let crc_flag = cctx.prefs.frame_info.block_checksum_flag == BlockChecksum::Enabled;
    let block_mode = cctx.prefs.frame_info.block_mode;

    let mut src_pos: usize = 0;
    let mut last_block_status = 0u8; // 0=notDone, 1=fromTmpBuffer, 2=fromSrcBuffer

    // ── Step 1: drain the internal staging buffer ─────────────────────────────
    if cctx.tmp_in_size > 0 {
        let size_to_copy = block_size - cctx.tmp_in_size;
        if size_to_copy > src.len() - src_pos {
            // Not enough new data to fill one block — just append to staging.
            let append = &src[src_pos..];
            let dst_off = cctx.tmp_in_offset + cctx.tmp_in_size;
            cctx.tmp_buf[dst_off..dst_off + append.len()].copy_from_slice(append);
            src_pos = src.len();
            cctx.tmp_in_size += append.len();
        } else {
            // Complete the staging block and compress it.
            last_block_status = 1; // fromTmpBuffer
            let fill_slice = &src[src_pos..src_pos + size_to_copy];
            let tmp_dst = cctx.tmp_in_offset + cctx.tmp_in_size;
            cctx.tmp_buf[tmp_dst..tmp_dst + size_to_copy].copy_from_slice(fill_slice);
            src_pos += size_to_copy;

            let tmp_in_off = cctx.tmp_in_offset;
            // SAFETY: inner ctx is valid; tmp_buf and dst are separate allocations.
            let written = unsafe {
                // Build a temporary slice view of the staging block.
                let src_slice = core::slice::from_raw_parts(
                    cctx.tmp_buf.as_ptr().add(tmp_in_off),
                    block_size,
                );
                lz4f_make_block(
                    &mut dst[dst_pos..],
                    src_slice,
                    compress_mode,
                    cctx,
                    crc_flag,
                )
            };
            dst_pos += written;

            if block_mode == BlockMode::Linked {
                cctx.tmp_in_offset += block_size;
            }
            cctx.tmp_in_size = 0;
        }
    }

    // ── Step 2: compress full blocks directly from src ────────────────────────
    while src.len() - src_pos >= block_size {
        last_block_status = 2; // fromSrcBuffer
        let block = &src[src_pos..src_pos + block_size];
        let written =
            // SAFETY: inner ctx valid; src and dst are separate.
            unsafe { lz4f_make_block(&mut dst[dst_pos..], block, compress_mode, cctx, crc_flag) };
        dst_pos += written;
        src_pos += block_size;
    }

    // ── Step 3: auto-flush remaining (< blockSize) bytes from src ─────────────
    if cctx.prefs.auto_flush && src_pos < src.len() {
        last_block_status = 2; // fromSrcBuffer
        let rem = &src[src_pos..];
        let written =
            unsafe { lz4f_make_block(&mut dst[dst_pos..], rem, compress_mode, cctx, crc_flag) };
        dst_pos += written;
        src_pos = src.len();
    }

    // ── Step 4: preserve dictionary for linked-block mode ─────────────────────
    if block_mode == BlockMode::Linked && last_block_status == 2 {
        // Compressed mode only (linked + uncompressed is unsupported per C assert).
        if opts.stable_src {
            // Src remains valid — point tmpIn back to start of tmpBuf.
            cctx.tmp_in_offset = 0;
        } else {
            let real_dict_size = unsafe { local_save_dict(cctx) };
            debug_assert!(real_dict_size >= 0 && real_dict_size as usize <= KB64);
            cctx.tmp_in_offset = real_dict_size as usize;
        }
    }

    // ── Step 5: keep tmpIn within tmpBuf bounds (non-autoFlush linked mode) ───
    if !cctx.prefs.auto_flush {
        let tmp_in_end = cctx.tmp_in_offset + block_size;
        if tmp_in_end > cctx.max_buffer_size {
            let real_dict_size = unsafe { local_save_dict(cctx) };
            cctx.tmp_in_offset = real_dict_size as usize;
            debug_assert!(cctx.tmp_in_offset + block_size <= cctx.max_buffer_size);
        }
    }

    // ── Step 6: buffer any remaining src bytes in staging area ────────────────
    if src_pos < src.len() {
        let rem = &src[src_pos..];
        let dst_off = cctx.tmp_in_offset + cctx.tmp_in_size;
        cctx.tmp_buf[dst_off..dst_off + rem.len()].copy_from_slice(rem);
        cctx.tmp_in_size += rem.len();
    }

    // ── Content checksum update ───────────────────────────────────────────────
    if cctx.prefs.frame_info.content_checksum_flag == ContentChecksum::Enabled {
        cctx.xxh.update(src);
    }

    cctx.total_in_size += src.len() as u64;
    Ok(dst_pos)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_compressUpdate / LZ4F_uncompressedUpdate (lz4frame.c:1107–1148)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `src` into `dst` and buffer any remainder.
///
/// May return 0 bytes written when all input was buffered.
/// `dst` must be large enough per `lz4f_compress_bound`.
///
/// Mirrors `LZ4F_compressUpdate` (lz4frame.c:1119–1128).
pub fn lz4f_compress_update(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    src: &[u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError> {
    lz4f_compress_update_impl(cctx, dst, src, opts, BlockCompressMode::Compressed)
}

/// Compress `src` into `dst` with blocks stored verbatim (uncompressed).
///
/// Only valid when `block_mode == Independent`.
/// `dst.len()` must be ≥ `src.len()`.
///
/// Mirrors `LZ4F_uncompressedUpdate` (lz4frame.c:1139–1148).
pub fn lz4f_uncompressed_update(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    src: &[u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError> {
    lz4f_compress_update_impl(cctx, dst, src, opts, BlockCompressMode::Uncompressed)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_flush (lz4frame.c:1159–1194)
// ─────────────────────────────────────────────────────────────────────────────

/// Internal flush — compress the internal staging buffer immediately.
fn lz4f_flush_impl(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    _opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError> {
    if cctx.tmp_in_size == 0 {
        return Ok(0); // nothing buffered
    }
    if cctx.c_stage != 1 {
        return Err(Lz4FError::CompressionStateUninitialized);
    }
    let min_dst = cctx.tmp_in_size + BH_SIZE + BF_SIZE;
    if dst.len() < min_dst {
        return Err(Lz4FError::DstMaxSizeTooSmall);
    }

    let compress_mode = select_compress_mode(
        cctx.prefs.frame_info.block_mode,
        cctx.prefs.compression_level,
        cctx.block_compress_mode,
    );
    let crc_flag = cctx.prefs.frame_info.block_checksum_flag == BlockChecksum::Enabled;
    let tmp_in_off = cctx.tmp_in_offset;
    let tmp_in_sz = cctx.tmp_in_size;

    let written = unsafe {
        let src_slice =
            core::slice::from_raw_parts(cctx.tmp_buf.as_ptr().add(tmp_in_off), tmp_in_sz);
        lz4f_make_block(dst, src_slice, compress_mode, cctx, crc_flag)
    };

    if cctx.prefs.frame_info.block_mode == BlockMode::Linked {
        cctx.tmp_in_offset += cctx.tmp_in_size;
    }
    cctx.tmp_in_size = 0;

    // Keep tmpIn within bounds (linked mode only).
    if cctx.tmp_in_offset + cctx.max_block_size > cctx.max_buffer_size {
        let real_dict_size = unsafe { local_save_dict(cctx) };
        cctx.tmp_in_offset = real_dict_size as usize;
    }

    Ok(written)
}

/// Flush buffered data to `dst` immediately.
///
/// Returns 0 if there was no buffered data.
///
/// Mirrors `LZ4F_flush` (lz4frame.c:1159–1194).
pub fn lz4f_flush(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError> {
    lz4f_flush_impl(cctx, dst, opts)
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_compressEnd (lz4frame.c:1206–1241)
// ─────────────────────────────────────────────────────────────────────────────

/// Flush remaining data, write the end-mark, and optional content checksum.
///
/// After a successful call, `cctx` may be reused with another
/// `lz4f_compress_begin*` call.
///
/// Returns the number of bytes written (at minimum 4 for the end-mark).
///
/// Mirrors `LZ4F_compressEnd` (lz4frame.c:1206–1241).
pub fn lz4f_compress_end(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError> {
    let flush_size = lz4f_flush_impl(cctx, dst, opts)?;
    let mut pos = flush_size;

    if dst.len() - pos < 4 {
        return Err(Lz4FError::DstMaxSizeTooSmall);
    }
    // End-mark: 4 zero bytes
    write_le32(dst, pos, 0);
    pos += 4;

    // Optional content checksum
    if cctx.prefs.frame_info.content_checksum_flag == ContentChecksum::Enabled {
        if dst.len() - pos < 4 {
            return Err(Lz4FError::DstMaxSizeTooSmall);
        }
        let xxh = cctx.xxh.digest();
        write_le32(dst, pos, xxh);
        pos += 4;
    }

    cctx.c_stage = 0; // context is re-usable

    // Verify content size if it was declared in the frame header.
    if cctx.prefs.frame_info.content_size != 0
        && cctx.prefs.frame_info.content_size != cctx.total_in_size
    {
        return Err(Lz4FError::FrameSizeWrong);
    }

    Ok(pos)
}

// ─────────────────────────────────────────────────────────────────────────────
// One-shot compression (lz4frame.c:419–524)
// ─────────────────────────────────────────────────────────────────────────────

/// Compress `src` into a complete LZ4 frame in one call, using a CDict.
///
/// `dst` must be large enough per `lz4f_compress_frame_bound`.
/// `cdict` may be null (no dictionary).
///
/// Mirrors `LZ4F_compressFrame_usingCDict` (lz4frame.c:428–474).
pub fn lz4f_compress_frame_using_cdict(
    cctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    src: &[u8],
    cdict: *const Lz4FCDict,
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError> {
    let mut local_prefs = prefs.copied().unwrap_or_default();

    // Auto-correct content size if the caller declared non-zero.
    if local_prefs.frame_info.content_size != 0 {
        local_prefs.frame_info.content_size = src.len() as u64;
    }

    // Select smallest block-size ID that fits the source.
    local_prefs.frame_info.block_size_id =
        lz4f_optimal_bsid(local_prefs.frame_info.block_size_id, src.len());

    // Force autoFlush: one-shot always flushes every update.
    local_prefs.auto_flush = true;

    // Single block → no need for linked history.
    if src.len() <= lz4f_get_block_size(local_prefs.frame_info.block_size_id).unwrap_or(KB64) {
        local_prefs.frame_info.block_mode = BlockMode::Independent;
    }

    let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&local_prefs));
    if dst.len() < frame_bound {
        return Err(Lz4FError::DstMaxSizeTooSmall);
    }

    let opts = CompressOptions { stable_src: true };

    // Write header
    let cdict_opt = if cdict.is_null() { None } else { Some(cdict) };
    let header_size =
        lz4f_compress_begin_internal(cctx, dst, None, cdict_opt, Some(&local_prefs))?;
    let mut pos = header_size;

    // Compress
    let c_size = lz4f_compress_update(cctx, &mut dst[pos..], src, Some(&opts))?;
    pos += c_size;

    // Finalize
    let tail_size = lz4f_compress_end(cctx, &mut dst[pos..], Some(&opts))?;
    pos += tail_size;

    Ok(pos)
}

/// Compress `src` into a complete LZ4 frame in one call.
///
/// Creates a temporary compression context internally; no external cctx needed.
///
/// Mirrors `LZ4F_compressFrame` (lz4frame.c:484–524).
/// The C `goto _end` cleanup is replaced by Rust's RAII drop at function exit.
pub fn lz4f_compress_frame(
    dst: &mut [u8],
    src: &[u8],
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError> {
    // Heap-allocate a temporary context; Drop handles cleanup automatically.
    let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
    lz4f_compress_frame_using_cdict(&mut cctx, dst, src, core::ptr::null(), prefs)
    // `cctx` is dropped here → inner LZ4 ctx freed via Drop impl
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helper: set HC compression level without resetting stream
// ─────────────────────────────────────────────────────────────────────────────

fn set_hc_level(stream: &mut Lz4StreamHc, level: i32) {
    hc_set_compression_level(stream, level);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::types::{BlockSizeId, FrameInfo};

    // ── Frame header magic ────────────────────────────────────────────────────

    /// Parity: magic number appears at byte offset 0 of any compressed frame.
    #[test]
    fn compress_frame_magic_number() {
        let src = b"hello world";
        let frame_bound = lz4f_compress_frame_bound(src.len(), None);
        let mut dst = vec![0u8; frame_bound];
        let written = lz4f_compress_frame(&mut dst, src, None).expect("compress_frame failed");
        assert!(written >= 4, "must write at least magic number");
        let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
        assert_eq!(magic, LZ4F_MAGIC_NUMBER, "magic number must be 0x184D2204");
    }

    /// Parity: frame magic number constant matches C LZ4F_MAGICNUMBER.
    #[test]
    fn magic_constant() {
        assert_eq!(LZ4F_MAGIC_NUMBER, 0x184D_2204u32);
    }

    // ── One-shot round trip ───────────────────────────────────────────────────

    /// compress_frame produces a non-trivial, non-empty output for non-empty input.
    #[test]
    fn compress_frame_nonempty() {
        let src: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let frame_bound = lz4f_compress_frame_bound(src.len(), None);
        let mut dst = vec![0u8; frame_bound];
        let written = lz4f_compress_frame(&mut dst, &src, None).expect("compress_frame");
        assert!(written > 4, "compressed output must contain more than magic");
    }

    /// compress_frame on empty input produces a minimal valid frame.
    #[test]
    fn compress_frame_empty_src() {
        let frame_bound = lz4f_compress_frame_bound(0, None);
        let mut dst = vec![0u8; frame_bound];
        let written = lz4f_compress_frame(&mut dst, &[], None).expect("empty compress_frame");
        // Minimum: magic(4) + FLG(1) + BD(1) + HC(1) + endMark(4) = 11 bytes
        assert!(written >= 11);
        let magic = u32::from_le_bytes(dst[..4].try_into().unwrap());
        assert_eq!(magic, LZ4F_MAGIC_NUMBER);
    }

    /// Content checksum flag is respected.
    #[test]
    fn compress_frame_with_content_checksum() {
        let src = b"content checksum test payload aaaa bbbb cccc dddd";
        let prefs = Preferences {
            frame_info: FrameInfo {
                content_checksum_flag: ContentChecksum::Enabled,
                ..Default::default()
            },
            ..Default::default()
        };
        let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
        let mut dst = vec![0u8; frame_bound];
        let written =
            lz4f_compress_frame(&mut dst, src, Some(&prefs)).expect("checksum compress_frame");
        assert!(written > 4);
        // Frame with content checksum is 4 bytes longer than without.
        let prefs_no_csum = Preferences::default();
        let frame_bound2 = lz4f_compress_frame_bound(src.len(), Some(&prefs_no_csum));
        let mut dst2 = vec![0u8; frame_bound2];
        let written2 = lz4f_compress_frame(&mut dst2, src, Some(&prefs_no_csum))
            .expect("no-checksum compress_frame");
        assert_eq!(written, written2 + 4);
    }

    // ── Streaming compress ────────────────────────────────────────────────────

    /// Streaming compress (begin → update × N → end) produces a valid frame.
    #[test]
    fn streaming_compress_valid_frame() {
        let src: Vec<u8> = b"the quick brown fox jumps over the lazy dog"
            .iter()
            .cycle()
            .take(8192)
            .copied()
            .collect();
        let prefs = Preferences {
            frame_info: FrameInfo {
                block_size_id: BlockSizeId::Max64Kb,
                content_checksum_flag: ContentChecksum::Enabled,
                ..Default::default()
            },
            auto_flush: true,
            ..Default::default()
        };

        let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
        let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
        let mut out = vec![0u8; frame_bound];
        let mut pos = 0;

        pos += lz4f_compress_begin(&mut cctx, &mut out[pos..], Some(&prefs)).expect("begin");

        // Feed in 1 KB chunks
        for chunk in src.chunks(1024) {
            let opts = CompressOptions { stable_src: false };
            pos +=
                lz4f_compress_update(&mut cctx, &mut out[pos..], chunk, Some(&opts))
                    .expect("update");
        }

        pos += lz4f_compress_end(&mut cctx, &mut out[pos..], None).expect("end");

        // Frame must start with magic and have written some data.
        assert!(pos > 0, "streaming must produce output");
        let magic = u32::from_le_bytes(out[..4].try_into().unwrap());
        assert_eq!(magic, LZ4F_MAGIC_NUMBER, "streaming frame must start with magic");
    }

    /// Streaming with stable_src produces byte-identical output to one-shot.
    #[test]
    fn streaming_stable_src_matches_one_shot() {
        let src: Vec<u8> = b"aaaaaaaabbbbbbbbccccccccdddddddd"
            .iter()
            .cycle()
            .take(256)
            .copied()
            .collect();
        let prefs = Preferences {
            frame_info: FrameInfo {
                // Force independent blocks so history doesn't carry over.
                block_mode: BlockMode::Independent,
                content_checksum_flag: ContentChecksum::Enabled,
                ..Default::default()
            },
            auto_flush: true,
            ..Default::default()
        };

        // One-shot
        let frame_bound = lz4f_compress_frame_bound(src.len(), Some(&prefs));
        let mut one_shot = vec![0u8; frame_bound];
        let one_shot_len =
            lz4f_compress_frame(&mut one_shot, &src, Some(&prefs)).expect("one-shot");

        // Streaming (whole src in one update = equivalent to one-shot)
        let mut cctx = Lz4FCCtx::new(LZ4F_VERSION);
        let mut streaming = vec![0u8; frame_bound];
        let mut pos = 0;
        let opts = CompressOptions { stable_src: true };
        pos +=
            lz4f_compress_begin(&mut cctx, &mut streaming[pos..], Some(&prefs)).expect("begin");
        pos += lz4f_compress_update(&mut cctx, &mut streaming[pos..], &src, Some(&opts))
            .expect("update");
        pos += lz4f_compress_end(&mut cctx, &mut streaming[pos..], Some(&opts)).expect("end");

        assert_eq!(pos, one_shot_len);
        assert_eq!(&streaming[..pos], &one_shot[..one_shot_len]);
    }

    // ── Context lifecycle ─────────────────────────────────────────────────────

    /// lz4f_create_compression_context rejects wrong version.
    #[test]
    fn create_ctx_wrong_version() {
        assert!(lz4f_create_compression_context(99).is_err());
    }

    /// lz4f_create_compression_context accepts correct version.
    #[test]
    fn create_ctx_correct_version() {
        let ctx = lz4f_create_compression_context(LZ4F_VERSION);
        assert!(ctx.is_ok());
    }

    /// lz4f_free_compression_context drops without panic.
    #[test]
    fn free_ctx_no_panic() {
        let ctx = lz4f_create_compression_context(LZ4F_VERSION).unwrap();
        lz4f_free_compression_context(ctx);
    }

    // ── select_compress_mode ──────────────────────────────────────────────────

    #[test]
    fn select_compress_mode_uncompressed() {
        assert_eq!(
            select_compress_mode(BlockMode::Linked, 0, BlockCompressMode::Uncompressed),
            CompressMode::Uncompressed
        );
    }

    #[test]
    fn select_compress_mode_fast_independent() {
        assert_eq!(
            select_compress_mode(BlockMode::Independent, 0, BlockCompressMode::Compressed),
            CompressMode::FastIndependent
        );
    }

    #[test]
    fn select_compress_mode_fast_linked() {
        assert_eq!(
            select_compress_mode(BlockMode::Linked, 0, BlockCompressMode::Compressed),
            CompressMode::FastLinked
        );
    }

    #[test]
    fn select_compress_mode_hc_independent() {
        assert_eq!(
            select_compress_mode(
                BlockMode::Independent,
                LZ4HC_CLEVEL_MIN,
                BlockCompressMode::Compressed
            ),
            CompressMode::HcIndependent
        );
    }

    #[test]
    fn select_compress_mode_hc_linked() {
        assert_eq!(
            select_compress_mode(BlockMode::Linked, LZ4HC_CLEVEL_MIN, BlockCompressMode::Compressed),
            CompressMode::HcLinked
        );
    }
}
