//! Public LZ4 block decompression API.
//!
//! Implements the LZ4 block decompression functions from `lz4.c` v1.10.0
//! (lines 2448–2760):
//!
//!   - One-shot decompression: `decompress_safe`, `decompress_safe_partial`
//!   - Dictionary decompression: `decompress_safe_using_dict`,
//!     `decompress_safe_partial_using_dict`, `decompress_safe_force_ext_dict`,
//!     `decompress_safe_partial_force_ext_dict`
//!   - Prefix-mode helpers: `decompress_safe_with_prefix64k`,
//!     `decompress_safe_with_small_prefix`
//!   - Streaming decode context: [`Lz4StreamDecode`]
//!   - Streaming API: `decompress_safe_continue`
//!
//! # Not implemented
//!
//! The `LZ4_decompress_fast*` family is **not** included — it is deprecated
//! in the reference C implementation and inherently unsafe (no output-size bound).
//!
//! # Safety model
//!
//! Simple one-shot functions (`decompress_safe`, `decompress_safe_partial`)
//! are fully safe.  Streaming and dictionary functions that must compare raw
//! pointer addresses or track caller-managed ring-buffer positions are marked
//! `unsafe`; their contracts are documented inline.

use core::ptr;

use super::decompress_core::{decompress_generic, DecompressError};
use super::types::{DictDirective, KB};

// ─────────────────────────────────────────────────────────────────────────────
// Re-export
// ─────────────────────────────────────────────────────────────────────────────

pub use super::decompress_core::DecompressError as BlockDecompressError;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum supported input size for LZ4 compression/decompression.
/// Equivalent to C macro `LZ4_MAX_INPUT_SIZE` (2,113,929,216 bytes).
pub const LZ4_MAX_INPUT_SIZE: usize = 0x7E000000;

/// 64 KiB − 1: threshold below which a prefix is "small".
const KB64_MINUS1: usize = 64 * KB - 1;

// ─────────────────────────────────────────────────────────────────────────────
// Streaming decode context — mirrors LZ4_streamDecode_t_internal (lz4.h:758-763)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming decompression tracking context.
///
/// Equivalent to `LZ4_streamDecode_t` in C.  Stores raw pointers into
/// previously-decoded output buffers; the caller is responsible for keeping
/// those buffers alive and accessible for the lifetime of this context.
///
/// Create with [`Lz4StreamDecode::new`] and configure with
/// [`set_stream_decode`].
#[repr(C)]
pub struct Lz4StreamDecode {
    /// Pointer to the end of the external dictionary (may be null).
    pub(crate) external_dict: *const u8,
    /// Pointer to the byte just past the last decoded byte (end of prefix).
    pub(crate) prefix_end: *const u8,
    /// Size of the external dictionary in bytes.
    pub(crate) ext_dict_size: usize,
    /// Size of the current prefix (previously decoded data) in bytes.
    pub(crate) prefix_size: usize,
}

// SAFETY: `Lz4StreamDecode` is driven by the caller under single-threaded or
// externally-synchronised access.  The raw pointers stored here are not
// independently aliased from Rust's perspective.
unsafe impl Send for Lz4StreamDecode {}

impl Lz4StreamDecode {
    /// Allocate and zero-initialise a new streaming decompression context.
    ///
    /// Equivalent to `LZ4_createStreamDecode` (but stack-allocated).
    pub const fn new() -> Self {
        Self {
            external_dict: ptr::null(),
            prefix_end: ptr::null(),
            ext_dict_size: 0,
            prefix_size: 0,
        }
    }
}

impl Default for Lz4StreamDecode {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public — one-shot safe API (lines 2450-2465)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress a full LZ4 block with no dictionary.
///
/// Equivalent to `LZ4_decompress_safe`.
///
/// Returns the number of bytes written into `dst`, or
/// `Err(DecompressError::MalformedInput)` for invalid input.
pub fn decompress_safe(src: &[u8], dst: &mut [u8]) -> Result<usize, DecompressError> {
    // SAFETY: slices guarantee valid, non-overlapping memory regions.
    // low_prefix == dst.as_ptr() (no prior output prefix).
    unsafe {
        decompress_generic(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len(),
            dst.len(),
            false, // decode_full_block
            DictDirective::NoDict,
            dst.as_ptr() as *const u8, // low_prefix = start of dst
            ptr::null(),               // no external dictionary
            0,
        )
    }
}

/// Decompress up to `target_output_size` bytes from an LZ4 block.
///
/// Equivalent to `LZ4_decompress_safe_partial`.
///
/// `dst.len()` is the available capacity; `target_output_size` is the desired
/// number of decoded bytes.  At most `min(target_output_size, dst.len())`
/// bytes are written.
///
/// Returns the number of bytes written, or
/// `Err(DecompressError::MalformedInput)` on error.
pub fn decompress_safe_partial(
    src: &[u8],
    dst: &mut [u8],
    target_output_size: usize,
) -> Result<usize, DecompressError> {
    // C: dstCapacity = MIN(targetOutputSize, dstCapacity)
    let output_size = target_output_size.min(dst.len());

    // SAFETY: same as decompress_safe; partial_decoding = true.
    unsafe {
        decompress_generic(
            src.as_ptr(),
            dst.as_mut_ptr(),
            src.len(),
            output_size,
            true, // partial_decode
            DictDirective::NoDict,
            dst.as_ptr() as *const u8,
            ptr::null(),
            0,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal prefix/extDict helpers (lines 2478-2564)
//
// These mirror the C static helpers called by LZ4_decompress_safe_continue
// and LZ4_decompress_safe_usingDict.  They operate on raw pointers because
// they are dispatched from the streaming path where slices are not available.
// ─────────────────────────────────────────────────────────────────────────────

/// Full-block decode assuming 64 KiB of prefix immediately before `dst`.
///
/// Equivalent to C `LZ4_decompress_safe_withPrefix64k`.
///
/// # Safety
/// - `src_ptr` is valid for `src_size` bytes of reads.
/// - `dst_ptr` is valid for `max_output` bytes of writes.
/// - The 64 KiB of memory immediately before `dst_ptr` are readable and
///   contain the previously decoded data.
#[inline]
pub(crate) unsafe fn decompress_safe_with_prefix64k(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    max_output: usize,
) -> Result<usize, DecompressError> {
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        max_output,
        false,
        DictDirective::WithPrefix64k,
        dst_ptr.sub(64 * KB) as *const u8,
        ptr::null(),
        0,
    )
}

/// Partial decode with 64 KiB prefix immediately before `dst`.
///
/// # Safety
/// Same as [`decompress_safe_with_prefix64k`].
#[inline]
pub(crate) unsafe fn decompress_safe_partial_with_prefix64k(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    target_output_size: usize,
    dst_capacity: usize,
) -> Result<usize, DecompressError> {
    // C: dstCapacity = MIN(targetOutputSize, dstCapacity)
    let output_size = target_output_size.min(dst_capacity);
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        output_size,
        true,
        DictDirective::WithPrefix64k,
        dst_ptr.sub(64 * KB) as *const u8,
        ptr::null(),
        0,
    )
}

/// Full-block decode with a small prefix (`prefix_size` < 64 KiB) immediately
/// before `dst`.
///
/// Equivalent to C `LZ4_decompress_safe_withSmallPrefix`.
///
/// # Safety
/// - `src_ptr` valid for `src_size` reads; `dst_ptr` valid for `max_output` writes.
/// - `prefix_size` bytes immediately before `dst_ptr` are readable.
#[inline]
pub(crate) unsafe fn decompress_safe_with_small_prefix(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    max_output: usize,
    prefix_size: usize,
) -> Result<usize, DecompressError> {
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        max_output,
        false,
        DictDirective::NoDict,
        dst_ptr.sub(prefix_size) as *const u8,
        ptr::null(),
        0,
    )
}

/// Partial decode with a small prefix before `dst`.
///
/// # Safety
/// Same as [`decompress_safe_with_small_prefix`].
#[inline]
pub(crate) unsafe fn decompress_safe_partial_with_small_prefix(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    target_output_size: usize,
    dst_capacity: usize,
    prefix_size: usize,
) -> Result<usize, DecompressError> {
    let output_size = target_output_size.min(dst_capacity);
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        output_size,
        true,
        DictDirective::NoDict,
        dst_ptr.sub(prefix_size) as *const u8,
        ptr::null(),
        0,
    )
}

/// Full-block decode with an external dictionary at an arbitrary location.
///
/// Equivalent to C `LZ4_decompress_safe_forceExtDict`.
///
/// # Safety
/// - `src_ptr` valid for `src_size` reads; `dst_ptr` valid for `max_output` writes.
/// - `dict_start` valid for `dict_size` reads.
/// - Dict memory must not alias the output buffer.
pub unsafe fn decompress_safe_force_ext_dict(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    max_output: usize,
    dict_start: *const u8,
    dict_size: usize,
) -> Result<usize, DecompressError> {
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        max_output,
        false,
        DictDirective::UsingExtDict,
        dst_ptr as *const u8, // low_prefix = start of current output
        dict_start,
        dict_size,
    )
}

/// Partial decode with an external dictionary.
///
/// Equivalent to C `LZ4_decompress_safe_partial_forceExtDict`.
///
/// # Safety
/// Same as [`decompress_safe_force_ext_dict`].
pub unsafe fn decompress_safe_partial_force_ext_dict(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    target_output_size: usize,
    dst_capacity: usize,
    dict_start: *const u8,
    dict_size: usize,
) -> Result<usize, DecompressError> {
    let output_size = target_output_size.min(dst_capacity);
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        output_size,
        true,
        DictDirective::UsingExtDict,
        dst_ptr as *const u8,
        dict_start,
        dict_size,
    )
}

/// "Double dictionary" mode: prefix immediately before `dst` **and** an
/// external dictionary at an arbitrary location.
///
/// Used by `decompress_safe_continue` when the ring buffer wraps and there
/// is still an active external dictionary from a previous wrap.
///
/// Equivalent to C `LZ4_decompress_safe_doubleDict`.
///
/// # Safety
/// - `src_ptr` valid for `src_size` reads; `dst_ptr` valid for `max_output` writes.
/// - `prefix_size` bytes immediately before `dst_ptr` are readable.
/// - `dict_start` valid for `dict_size` reads.
#[inline]
pub(crate) unsafe fn decompress_safe_double_dict(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    max_output: usize,
    prefix_size: usize,
    dict_start: *const u8,
    dict_size: usize,
) -> Result<usize, DecompressError> {
    decompress_generic(
        src_ptr,
        dst_ptr,
        src_size,
        max_output,
        false,
        DictDirective::UsingExtDict,
        dst_ptr.sub(prefix_size) as *const u8,
        dict_start,
        dict_size,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming decode context management (lines 2575-2621)
// ─────────────────────────────────────────────────────────────────────────────

/// Configure a streaming decode context with a dictionary.
///
/// Equivalent to `LZ4_setStreamDecode`.
///
/// Call this before `decompress_safe_continue` to supply the dictionary used
/// during compression (or to reset the context for a new stream).  Passing
/// `dict = &[]` resets to no-dictionary mode.
///
/// Returns `true` on success (mirrors C returning `1`).
///
/// # Safety
/// `ctx` must not be concurrently accessed from another thread.
/// The `dict` slice must remain valid and unmodified for the lifetime of any
/// subsequent `decompress_safe_continue` calls that reference it.
pub unsafe fn set_stream_decode(ctx: &mut Lz4StreamDecode, dict: &[u8]) -> bool {
    ctx.prefix_size = dict.len();
    if !dict.is_empty() {
        // SAFETY: dict.as_ptr() + dict.len() stays within the dict allocation.
        ctx.prefix_end = dict.as_ptr().add(dict.len());
    } else {
        ctx.prefix_end = dict.as_ptr();
    }
    ctx.external_dict = ptr::null();
    ctx.ext_dict_size = 0;
    true
}

/// Return the minimum ring-buffer size for streaming decompression.
///
/// Equivalent to `LZ4_decoderRingBufferSize`.
///
/// `max_block_size` is the maximum compressed block size that will be fed to
/// `decompress_safe_continue`.  Returns `None` if `max_block_size` is invalid.
pub fn decoder_ring_buffer_size(max_block_size: usize) -> Option<usize> {
    if max_block_size > LZ4_MAX_INPUT_SIZE {
        return None;
    }
    let block = max_block_size.max(16);
    Some(65536 + 14 + block)
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming decompression — continue (lines 2630-2668)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress consecutive LZ4 blocks in streaming mode.
///
/// Equivalent to `LZ4_decompress_safe_continue`.
///
/// `ctx` tracks the relationship between previously-decoded data and the
/// current decode position.  On each call:
/// - `src_ptr` points to the next compressed block.
/// - `dst_ptr` points to where decoded output should be written.
/// - `src_size` is the byte length of the compressed block.
/// - `max_output` is the capacity at `dst_ptr`.
///
/// Returns the number of bytes written on success, or
/// `Err(DecompressError::MalformedInput)` on error.
///
/// # Safety
/// - `src_ptr` must be valid for `src_size` reads.
/// - `dst_ptr` must be valid for `max_output` writes.
/// - Previously-decoded output referenced by `ctx` must still be readable at
///   the same address (ring-buffer or linear-buffer contract).
/// - `ctx` must not be concurrently accessed.
pub unsafe fn decompress_safe_continue(
    ctx: &mut Lz4StreamDecode,
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    max_output: usize,
) -> Result<usize, DecompressError> {
    let result: usize;

    if ctx.prefix_size == 0 {
        // First call — no dictionary yet.
        debug_assert!(ctx.ext_dict_size == 0);
        let mut tmp_dst = core::slice::from_raw_parts_mut(dst_ptr, max_output);
        let tmp_src = core::slice::from_raw_parts(src_ptr, src_size);
        result = decompress_safe(tmp_src, tmp_dst)?;
        ctx.prefix_size = result;
        // SAFETY: dst_ptr + result stays within the caller's buffer.
        ctx.prefix_end = dst_ptr.add(result) as *const u8;
    } else if std::ptr::eq(ctx.prefix_end, dst_ptr) {
        // Rolling the current segment: new block is contiguous with previous.
        if ctx.prefix_size >= KB64_MINUS1 {
            result = decompress_safe_with_prefix64k(src_ptr, dst_ptr, src_size, max_output)?;
        } else if ctx.ext_dict_size == 0 {
            result = decompress_safe_with_small_prefix(
                src_ptr,
                dst_ptr,
                src_size,
                max_output,
                ctx.prefix_size,
            )?;
        } else {
            result = decompress_safe_double_dict(
                src_ptr,
                dst_ptr,
                src_size,
                max_output,
                ctx.prefix_size,
                ctx.external_dict,
                ctx.ext_dict_size,
            )?;
        }
        ctx.prefix_size += result;
        ctx.prefix_end = ctx.prefix_end.add(result);
    } else {
        // Buffer wrapped around, or caller switched to a different buffer.
        // Previous prefix becomes the new external dictionary.
        ctx.ext_dict_size = ctx.prefix_size;
        // SAFETY: prefix_end - ext_dict_size = start of previous prefix.
        ctx.external_dict = ctx.prefix_end.sub(ctx.ext_dict_size);
        result = decompress_safe_force_ext_dict(
            src_ptr,
            dst_ptr,
            src_size,
            max_output,
            ctx.external_dict,
            ctx.ext_dict_size,
        )?;
        ctx.prefix_size = result;
        ctx.prefix_end = dst_ptr.add(result) as *const u8;
    }

    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Stateless dictionary API (lines 2719-2747)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress an LZ4 block using an explicit dictionary.
///
/// Equivalent to `LZ4_decompress_safe_usingDict`.
///
/// If `dict_start + dict_size` is immediately adjacent to `dst_ptr` in
/// memory, the more-efficient prefix mode is used.  Otherwise the external-
/// dictionary path is taken.
///
/// # Safety
/// - `src_ptr` valid for `src_size` reads; `dst_ptr` valid for `max_output` writes.
/// - `dict_start` valid for `dict_size` reads.
pub unsafe fn decompress_safe_using_dict(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    max_output: usize,
    dict_start: *const u8,
    dict_size: usize,
) -> Result<usize, DecompressError> {
    if dict_size == 0 {
        let src = core::slice::from_raw_parts(src_ptr, src_size);
        let dst = core::slice::from_raw_parts_mut(dst_ptr, max_output);
        return decompress_safe(src, dst);
    }
    // Check if dictionary is immediately before the output buffer.
    // SAFETY: dict_start + dict_size is within the dict allocation.
    if std::ptr::eq(dict_start.add(dict_size), dst_ptr) {
        if dict_size >= KB64_MINUS1 {
            return decompress_safe_with_prefix64k(src_ptr, dst_ptr, src_size, max_output);
        }
        return decompress_safe_with_small_prefix(
            src_ptr, dst_ptr, src_size, max_output, dict_size,
        );
    }
    decompress_safe_force_ext_dict(
        src_ptr, dst_ptr, src_size, max_output, dict_start, dict_size,
    )
}

/// Partially decompress an LZ4 block using an explicit dictionary.
///
/// Equivalent to `LZ4_decompress_safe_partial_usingDict`.
///
/// # Safety
/// Same contracts as [`decompress_safe_using_dict`], plus:
/// - `dst_capacity` is the total available bytes at `dst_ptr`.
/// - `target_output_size` is the desired decompressed byte count.
pub unsafe fn decompress_safe_partial_using_dict(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    src_size: usize,
    target_output_size: usize,
    dst_capacity: usize,
    dict_start: *const u8,
    dict_size: usize,
) -> Result<usize, DecompressError> {
    if dict_size == 0 {
        let src = core::slice::from_raw_parts(src_ptr, src_size);
        let dst = core::slice::from_raw_parts_mut(dst_ptr, dst_capacity);
        return decompress_safe_partial(src, dst, target_output_size);
    }
    // Check if dictionary is immediately before the output buffer.
    // SAFETY: dict_start + dict_size is within the dict allocation.
    if std::ptr::eq(dict_start.add(dict_size), dst_ptr) {
        if dict_size >= KB64_MINUS1 {
            return decompress_safe_partial_with_prefix64k(
                src_ptr,
                dst_ptr,
                src_size,
                target_output_size,
                dst_capacity,
            );
        }
        return decompress_safe_partial_with_small_prefix(
            src_ptr,
            dst_ptr,
            src_size,
            target_output_size,
            dst_capacity,
            dict_size,
        );
    }
    decompress_safe_partial_force_ext_dict(
        src_ptr,
        dst_ptr,
        src_size,
        target_output_size,
        dst_capacity,
        dict_start,
        dict_size,
    )
}
