//! LZ4 Frame format types, constants, and error handling.
//!
//! Translated from lz4frame.c v1.10.0, lines 234–330 and related
//! portions of lz4frame.h / lz4frame_static.h.
//!
//! Covers:
//! - Frame format constants (`LZ4F_BLOCKUNCOMPRESSED_FLAG`, `BHSize`, `BFSize`, etc.)
//! - Public enums from lz4frame.h: `BlockSizeId`, `BlockMode`, `ContentChecksum`, etc.
//! - `FrameInfo` / `Preferences` structs (lz4frame.h:175-198)
//! - Internal enums: `BlockCompressMode` (`LZ4F_BlockCompressMode_e`),
//!   `CtxType` (`LZ4F_CtxType_e`) — lz4frame.c:262-263
//! - `Lz4FCCtx` struct (`LZ4F_cctx_s`) — lz4frame.c:265-283
//! - `DecompressStage` enum (`dStage_t`) — lz4frame.c:1248-1258
//! - `Lz4FError` enum with `Display` + `Error` impls (LZ4F_errorStrings[]) — lz4frame.c:286-316

use core::fmt;
use crate::xxhash::Xxh32State;

// ─────────────────────────────────────────────────────────────────────────────
// API version (lz4frame.h:256)
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 Frame API version — used to guard context creation compatibility.
/// Equivalent to `LZ4F_VERSION` (100) in lz4frame.h.
pub const LZ4F_VERSION: u32 = 100;

// ─────────────────────────────────────────────────────────────────────────────
// Frame format constants (lz4frame.c:234–256, lz4frame.h:280-290)
// ─────────────────────────────────────────────────────────────────────────────

/// High bit of block header: indicates the block data is stored uncompressed.
/// Equivalent to `LZ4F_BLOCKUNCOMPRESSED_FLAG` (0x80000000U) in lz4frame.c.
pub const LZ4F_BLOCKUNCOMPRESSED_FLAG: u32 = 0x8000_0000;

/// Block header size in bytes (holds block data length + compressed flag bit).
/// Equivalent to `BHSize` / `LZ4F_BLOCK_HEADER_SIZE` = 4.
pub const BH_SIZE: usize = 4;

/// Block footer (checksum) size in bytes, present when block checksums are enabled.
/// Equivalent to `BFSize` / `LZ4F_BLOCK_CHECKSUM_SIZE` = 4.
pub const BF_SIZE: usize = 4;

/// Minimum LZ4 frame header size in bytes.
/// Equivalent to `minFHSize` / `LZ4F_HEADER_SIZE_MIN` = 7.
pub const MIN_FH_SIZE: usize = 7;

/// Maximum LZ4 frame header size in bytes.
/// Equivalent to `maxFHSize` / `LZ4F_HEADER_SIZE_MAX` = 19.
pub const MAX_FH_SIZE: usize = 19;

// ─────────────────────────────────────────────────────────────────────────────
// Public enums from lz4frame.h (frame parameters)
// ─────────────────────────────────────────────────────────────────────────────

/// Block size identifier determining the maximum LZ4 block size within a frame.
/// Corresponds to `LZ4F_blockSizeID_t` in lz4frame.h:123-133.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum BlockSizeId {
    /// Default: equivalent to `Max64Kb` when not explicitly set.
    #[default]
    Default = 0,
    Max64Kb = 4,
    Max256Kb = 5,
    Max1Mb = 6,
    Max4Mb = 7,
}

/// Block linking mode: linked blocks share history, independent blocks do not.
/// Corresponds to `LZ4F_blockMode_t` in lz4frame.h:138-143.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum BlockMode {
    /// Blocks share history (better compression, default).
    #[default]
    Linked = 0,
    /// Each block is compressed independently (wider compatibility).
    Independent = 1,
}

/// Whether a 32-bit content checksum (XXH32) is appended after the last block.
/// Corresponds to `LZ4F_contentChecksum_t` in lz4frame.h:145-150.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum ContentChecksum {
    #[default]
    Disabled = 0,
    Enabled = 1,
}

/// Whether a 32-bit checksum follows each compressed block.
/// Corresponds to `LZ4F_blockChecksum_t` in lz4frame.h:152-155.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum BlockChecksum {
    #[default]
    Disabled = 0,
    Enabled = 1,
}

/// Frame type: standard LZ4 frame or skippable frame.
/// Corresponds to `LZ4F_frameType_t` in lz4frame.h:157-161.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum FrameType {
    #[default]
    Frame = 0,
    SkippableFrame = 1,
}

// ─────────────────────────────────────────────────────────────────────────────
// FrameInfo and Preferences structs (lz4frame.h:175-198)
// ─────────────────────────────────────────────────────────────────────────────

/// Decoded LZ4 frame header parameters (read or write).
/// Corresponds to `LZ4F_frameInfo_t` in lz4frame.h:175-183.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInfo {
    /// Maximum block size ID (determines buffer requirements).
    pub block_size_id: BlockSizeId,
    /// Linked or independent blocks.
    pub block_mode: BlockMode,
    /// Whether a content checksum is present at end of frame.
    pub content_checksum_flag: ContentChecksum,
    /// Read-only: frame type (standard or skippable).
    pub frame_type: FrameType,
    /// Uncompressed content size in bytes; 0 = unknown.
    pub content_size: u64,
    /// Dictionary ID hint; 0 = no dict ID provided.
    pub dict_id: u32,
    /// Whether a per-block checksum is present after each block.
    pub block_checksum_flag: BlockChecksum,
}

/// User preferences supplied to streaming compression.
/// Corresponds to `LZ4F_preferences_t` in lz4frame.h:192-198.
#[derive(Debug, Clone, Copy, Default)]
pub struct Preferences {
    /// Frame metadata fields.
    pub frame_info: FrameInfo,
    /// Compression level: 0 = fast; > 0 = HC (clamped at max); < 0 = fast acceleration.
    pub compression_level: i32,
    /// When `true`, flush after every `compress_update` call (reduces buffering).
    pub auto_flush: bool,
    /// When `true`, HC parser favors decompression speed over ratio (`>= OPT_MIN` only).
    pub favor_dec_speed: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Custom memory allocator (lz4frame.h:727-735)
// ─────────────────────────────────────────────────────────────────────────────

/// Custom memory allocator hooks.
/// Corresponds to `LZ4F_CustomMem` in lz4frame.h:730-735.
/// When `alloc_fn` / `free_fn` are `None`, stdlib allocator is used.
#[derive(Clone, Copy, Default)]
pub struct CustomMem {
    /// Custom allocation function (`customAlloc`); `None` = use stdlib.
    pub alloc_fn: Option<fn(opaque: *mut (), size: usize) -> *mut ()>,
    /// Optional zeroing allocation (`customCalloc`); `None` = alloc + memset.
    pub calloc_fn: Option<fn(opaque: *mut (), size: usize) -> *mut ()>,
    /// Custom free function (`customFree`); `None` = use stdlib.
    pub free_fn: Option<fn(opaque: *mut (), ptr: *mut ())>,
    /// Opaque state pointer passed to all hooks.
    pub opaque: *mut (),
}

// SAFETY: The C API treats CustomMem as a plain-data struct passed by value.
// We only expose it in unsafe contexts through compression/decompression contexts.
unsafe impl Send for CustomMem {}
unsafe impl Sync for CustomMem {}

impl fmt::Debug for CustomMem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomMem")
            .field("alloc_fn", &self.alloc_fn.map(|_| "<fn>"))
            .field("calloc_fn", &self.calloc_fn.map(|_| "<fn>"))
            .field("free_fn", &self.free_fn.map(|_| "<fn>"))
            .finish()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal enums (lz4frame.c:262-263)
// ─────────────────────────────────────────────────────────────────────────────

/// Whether a block is compressed or stored verbatim (uncompressed).
/// Corresponds to `LZ4F_BlockCompressMode_e` (`LZ4B_COMPRESSED` / `LZ4B_UNCOMPRESSED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockCompressMode {
    #[default]
    Compressed = 0,
    Uncompressed = 1,
}

/// Which internal LZ4 context type is currently allocated in a `Lz4FCCtx`.
/// Corresponds to `LZ4F_CtxType_e` (`ctxNone` / `ctxFast` / `ctxHC`) in lz4frame.c:263.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u16)]
pub enum CtxType {
    #[default]
    None = 0,
    Fast = 1,
    Hc = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4F_cctx_s → Lz4FCCtx (lz4frame.c:265-283)
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 Frame streaming compression context.
///
/// Corresponds to `LZ4F_cctx_s` / `LZ4F_cctx_t` in lz4frame.c:265-283.
///
/// Ownership notes:
/// - `tmp_buf` owns the internal staging buffer (`tmpBuff` + `tmpIn` window).
/// - `lz4_ctx` holds the boxed inner fast or HC compression context.
/// - `cdict` is a non-owning reference (the caller keeps the CDict alive).
pub struct Lz4FCCtx {
    /// Custom memory allocator hooks (C: `cmem`). Default = stdlib.
    pub cmem: CustomMem,
    /// User compression preferences (C: `prefs`).
    pub prefs: Preferences,
    /// API version used when this context was created (C: `version`).
    pub version: u32,
    /// Compression stage: 0 = uninitialized, 1 = ready to compress (C: `cStage`).
    pub c_stage: u32,
    /// Maximum input block size in bytes derived from `prefs.frame_info.block_size_id` (C: `maxBlockSize`).
    pub max_block_size: usize,
    /// Allocated size of `tmp_buf` in bytes (C: `maxBufferSize`).
    pub max_buffer_size: usize,
    /// Internal staging buffer: holds up to `blockSize` of input + compressed output area (C: `tmpBuff`/`tmpIn`).
    pub tmp_buf: Vec<u8>,
    /// Byte offset within `tmp_buf` where the current accumulation window starts (C: `tmpIn` pointer offset).
    pub tmp_in_offset: usize,
    /// Number of bytes buffered in the current accumulation window (C: `tmpInSize`).
    pub tmp_in_size: usize,
    /// Total uncompressed bytes consumed across all `compress_update` calls (C: `totalInSize`).
    pub total_in_size: u64,
    /// Running XXH32 state for the optional content checksum (C: `xxh`).
    pub xxh: Xxh32State,
    /// The inner LZ4 or LZ4-HC context, stored as a raw byte buffer (C: `lz4CtxPtr`).
    /// `None` when no context is allocated.
    pub lz4_ctx: Option<Vec<u8>>,
    /// Allocated context size class: 0 = none, 1 = fast ctx, 2 = HC ctx (C: `lz4CtxAlloc`).
    pub lz4_ctx_alloc: u16,
    /// Currently active context type: 0 = none, 1 = fast, 2 = HC (C: `lz4CtxType`).
    pub lz4_ctx_type: CtxType,
    /// Whether blocks are compressed or stored verbatim (C: `blockCompressMode`).
    pub block_compress_mode: BlockCompressMode,
    /// Non-owning raw pointer to a [`Lz4FCDict`](crate::frame::cdict::Lz4FCDict).
    /// 0 = no dictionary attached. Set by `compress_begin_using_cdict`.
    /// The CDict must outlive the active compression session.
    /// Equivalent to `cdict` in `LZ4F_cctx_s` (lz4frame.c:275).
    pub cdict_ptr: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// dStage_t → DecompressStage (lz4frame.c:1248-1258)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompression state-machine stages.
///
/// Maps 1-to-1 to the C `dStage_t` enum (same discriminant values) so that
/// numeric comparisons in the decompression loop (`dStage <= dstage_init`, etc.)
/// remain valid when translated to Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u32)]
pub enum DecompressStage {
    /// Try to decode the frame header from available input (C: `dstage_getFrameHeader` = 0).
    #[default]
    GetFrameHeader = 0,
    /// Buffer header bytes into `dctx.header[]` until enough bytes arrive (C: `dstage_storeFrameHeader` = 1).
    StoreFrameHeader = 1,
    /// Initialize decompressor from the decoded header; allocate buffers (C: `dstage_init` = 2).
    Init = 2,
    /// Read the 4-byte block header from input (C: `dstage_getBlockHeader` = 3).
    GetBlockHeader = 3,
    /// Buffer the block header when fewer than 4 bytes are available (C: `dstage_storeBlockHeader` = 4).
    StoreBlockHeader = 4,
    /// Uncompressed block — copy directly to output (C: `dstage_copyDirect` = 5).
    CopyDirect = 5,
    /// Verify or skip the optional per-block checksum (C: `dstage_getBlockChecksum` = 6).
    GetBlockChecksum = 6,
    /// Read compressed block data directly from input when sufficient bytes present (C: `dstage_getCBlock` = 7).
    GetCBlock = 7,
    /// Buffer partial compressed block data (C: `dstage_storeCBlock` = 8).
    StoreCBlock = 8,
    /// Flush buffered decompressed output to the caller's buffer (C: `dstage_flushOut` = 9).
    FlushOut = 9,
    /// Read the optional 4-byte content checksum (C: `dstage_getSuffix` = 10).
    GetSuffix = 10,
    /// Buffer the content checksum bytes (C: `dstage_storeSuffix` = 11).
    StoreSuffix = 11,
    /// Read the 4-byte skippable frame size field (C: `dstage_getSFrameSize` = 12).
    GetSFrameSize = 12,
    /// Buffer the skippable frame size field (C: `dstage_storeSFrameSize` = 13).
    StoreSFrameSize = 13,
    /// Skip N bytes of a skippable frame (C: `dstage_skipSkippable` = 14).
    SkipSkippable = 14,
}

// ─────────────────────────────────────────────────────────────────────────────
// Error codes (lz4frame.h:653-678, lz4frame.c:286-316)
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 Frame error codes.
///
/// Corresponds to the `LZ4F_errorCodes` enum generated by `LZ4F_LIST_ERRORS` in lz4frame.h.
/// Discriminant indices match the position in `LZ4F_errorStrings[]` so that
/// `error_name()` returns the identical strings as `LZ4F_getErrorName`.
///
/// Note: `ERROR_maxCode` is a sentinel boundary in C (not a real error) — it is
/// omitted here; the equivalent boundary is encoded in `lz4f_is_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lz4FError {
    /// Index 0 — no error.
    OkNoError,
    /// Index 1
    Generic,
    /// Index 2
    MaxBlockSizeInvalid,
    /// Index 3
    BlockModeInvalid,
    /// Index 4
    ParameterInvalid,
    /// Index 5
    CompressionLevelInvalid,
    /// Index 6
    HeaderVersionWrong,
    /// Index 7
    BlockChecksumInvalid,
    /// Index 8
    ReservedFlagSet,
    /// Index 9
    AllocationFailed,
    /// Index 10
    SrcSizeTooLarge,
    /// Index 11
    DstMaxSizeTooSmall,
    /// Index 12
    FrameHeaderIncomplete,
    /// Index 13
    FrameTypeUnknown,
    /// Index 14
    FrameSizeWrong,
    /// Index 15
    SrcPtrWrong,
    /// Index 16
    DecompressionFailed,
    /// Index 17
    HeaderChecksumInvalid,
    /// Index 18
    ContentChecksumInvalid,
    /// Index 19
    FrameDecodingAlreadyStarted,
    /// Index 20
    CompressionStateUninitialized,
    /// Index 21
    ParameterNull,
    /// Index 22
    IoWrite,
    /// Index 23
    IoRead,
}

impl Lz4FError {
    /// Human-readable name string, byte-for-byte identical to the C `LZ4F_errorStrings[]` array
    /// entries so that `LZ4F_getErrorName` parity tests pass.
    ///
    /// Equivalent to `LZ4F_getErrorName` for error values (lz4frame.c:298-303).
    pub fn error_name(&self) -> &'static str {
        match self {
            Lz4FError::OkNoError => "OK_NoError",
            Lz4FError::Generic => "ERROR_GENERIC",
            Lz4FError::MaxBlockSizeInvalid => "ERROR_maxBlockSize_invalid",
            Lz4FError::BlockModeInvalid => "ERROR_blockMode_invalid",
            Lz4FError::ParameterInvalid => "ERROR_parameter_invalid",
            Lz4FError::CompressionLevelInvalid => "ERROR_compressionLevel_invalid",
            Lz4FError::HeaderVersionWrong => "ERROR_headerVersion_wrong",
            Lz4FError::BlockChecksumInvalid => "ERROR_blockChecksum_invalid",
            Lz4FError::ReservedFlagSet => "ERROR_reservedFlag_set",
            Lz4FError::AllocationFailed => "ERROR_allocation_failed",
            Lz4FError::SrcSizeTooLarge => "ERROR_srcSize_tooLarge",
            Lz4FError::DstMaxSizeTooSmall => "ERROR_dstMaxSize_tooSmall",
            Lz4FError::FrameHeaderIncomplete => "ERROR_frameHeader_incomplete",
            Lz4FError::FrameTypeUnknown => "ERROR_frameType_unknown",
            Lz4FError::FrameSizeWrong => "ERROR_frameSize_wrong",
            Lz4FError::SrcPtrWrong => "ERROR_srcPtr_wrong",
            Lz4FError::DecompressionFailed => "ERROR_decompressionFailed",
            Lz4FError::HeaderChecksumInvalid => "ERROR_headerChecksum_invalid",
            Lz4FError::ContentChecksumInvalid => "ERROR_contentChecksum_invalid",
            Lz4FError::FrameDecodingAlreadyStarted => "ERROR_frameDecoding_alreadyStarted",
            Lz4FError::CompressionStateUninitialized => "ERROR_compressionState_uninitialized",
            Lz4FError::ParameterNull => "ERROR_parameter_null",
            Lz4FError::IoWrite => "ERROR_io_write",
            Lz4FError::IoRead => "ERROR_io_read",
        }
    }

    /// Converts the numeric index from `LZ4F_errorStrings[]` to an error variant.
    /// Returns `None` for out-of-range indices (including the sentinel `maxCode = 24`).
    pub fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Lz4FError::OkNoError),
            1 => Some(Lz4FError::Generic),
            2 => Some(Lz4FError::MaxBlockSizeInvalid),
            3 => Some(Lz4FError::BlockModeInvalid),
            4 => Some(Lz4FError::ParameterInvalid),
            5 => Some(Lz4FError::CompressionLevelInvalid),
            6 => Some(Lz4FError::HeaderVersionWrong),
            7 => Some(Lz4FError::BlockChecksumInvalid),
            8 => Some(Lz4FError::ReservedFlagSet),
            9 => Some(Lz4FError::AllocationFailed),
            10 => Some(Lz4FError::SrcSizeTooLarge),
            11 => Some(Lz4FError::DstMaxSizeTooSmall),
            12 => Some(Lz4FError::FrameHeaderIncomplete),
            13 => Some(Lz4FError::FrameTypeUnknown),
            14 => Some(Lz4FError::FrameSizeWrong),
            15 => Some(Lz4FError::SrcPtrWrong),
            16 => Some(Lz4FError::DecompressionFailed),
            17 => Some(Lz4FError::HeaderChecksumInvalid),
            18 => Some(Lz4FError::ContentChecksumInvalid),
            19 => Some(Lz4FError::FrameDecodingAlreadyStarted),
            20 => Some(Lz4FError::CompressionStateUninitialized),
            21 => Some(Lz4FError::ParameterNull),
            22 => Some(Lz4FError::IoWrite),
            23 => Some(Lz4FError::IoRead),
            _ => None,
        }
    }

    /// Convert a raw C-style `size_t` return value to an `Lz4FError`.
    ///
    /// The C API encodes errors as `(size_t)(-(ptrdiff_t)errorCode)`.
    /// Returns `None` when `code` does not represent an error (i.e., success).
    /// Mirrors `LZ4F_getErrorCode` / `LZ4F_isError` from lz4frame.c:305-316.
    pub fn from_raw(code: usize) -> Option<Self> {
        if !lz4f_is_error(code) {
            return None;
        }
        // Recover index: index = -(ptrdiff_t)code (two's complement negation)
        let idx = code.wrapping_neg();
        Self::from_index(idx).or(Some(Lz4FError::Generic))
    }

    /// Returns `true` if this variant represents an actual error (not `OkNoError`).
    #[inline]
    pub fn is_error(&self) -> bool {
        !matches!(self, Lz4FError::OkNoError)
    }
}

impl fmt::Display for Lz4FError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.error_name())
    }
}

impl std::error::Error for Lz4FError {}

// ─────────────────────────────────────────────────────────────────────────────
// Free functions mirroring LZ4F_isError / LZ4F_getErrorName (lz4frame.c:293-303)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` when a raw C-style `size_t` return value represents an error.
///
/// Mirrors `LZ4F_isError(code)` from lz4frame.c:293-296.
///
/// The C implementation: `code > (LZ4F_errorCode_t)(-LZ4F_ERROR_maxCode)`.
/// `LZ4F_ERROR_maxCode` = 24 (sentinel); `-24` cast to `usize` = `usize::MAX - 23`.
/// Therefore errors occupy the range `(usize::MAX - 23)..=usize::MAX`.
#[inline]
pub fn lz4f_is_error(code: usize) -> bool {
    code > usize::MAX - 23
}

/// Return the human-readable name for a raw C-style error code.
///
/// Mirrors `LZ4F_getErrorName(code)` from lz4frame.c:298-303.
/// Returns `"Unspecified error code"` for non-error values (C returns the same string).
pub fn lz4f_get_error_name(code: usize) -> &'static str {
    if !lz4f_is_error(code) {
        return "Unspecified error code";
    }
    let idx = code.wrapping_neg();
    Lz4FError::from_index(idx)
        .map(|e| e.error_name())
        .unwrap_or("Unspecified error code")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity: verify error variant count matches C LZ4F_errorStrings[] (indices 0..=23).
    #[test]
    fn error_variant_count() {
        // 24 real variants (indices 0-23); ERROR_maxCode sentinel at index 24 is excluded.
        for i in 0..24usize {
            assert!(
                Lz4FError::from_index(i).is_some(),
                "missing variant for index {i}"
            );
        }
        // maxCode sentinel must NOT map to a variant
        assert!(Lz4FError::from_index(24).is_none());
    }

    /// Parity: Display strings must exactly match C LZ4F_errorStrings[] entries.
    #[test]
    fn error_name_strings() {
        assert_eq!(Lz4FError::OkNoError.error_name(), "OK_NoError");
        assert_eq!(Lz4FError::Generic.error_name(), "ERROR_GENERIC");
        assert_eq!(Lz4FError::MaxBlockSizeInvalid.error_name(), "ERROR_maxBlockSize_invalid");
        assert_eq!(Lz4FError::BlockModeInvalid.error_name(), "ERROR_blockMode_invalid");
        assert_eq!(Lz4FError::ParameterInvalid.error_name(), "ERROR_parameter_invalid");
        assert_eq!(Lz4FError::CompressionLevelInvalid.error_name(), "ERROR_compressionLevel_invalid");
        assert_eq!(Lz4FError::HeaderVersionWrong.error_name(), "ERROR_headerVersion_wrong");
        assert_eq!(Lz4FError::BlockChecksumInvalid.error_name(), "ERROR_blockChecksum_invalid");
        assert_eq!(Lz4FError::ReservedFlagSet.error_name(), "ERROR_reservedFlag_set");
        assert_eq!(Lz4FError::AllocationFailed.error_name(), "ERROR_allocation_failed");
        assert_eq!(Lz4FError::SrcSizeTooLarge.error_name(), "ERROR_srcSize_tooLarge");
        assert_eq!(Lz4FError::DstMaxSizeTooSmall.error_name(), "ERROR_dstMaxSize_tooSmall");
        assert_eq!(Lz4FError::FrameHeaderIncomplete.error_name(), "ERROR_frameHeader_incomplete");
        assert_eq!(Lz4FError::FrameTypeUnknown.error_name(), "ERROR_frameType_unknown");
        assert_eq!(Lz4FError::FrameSizeWrong.error_name(), "ERROR_frameSize_wrong");
        assert_eq!(Lz4FError::SrcPtrWrong.error_name(), "ERROR_srcPtr_wrong");
        assert_eq!(Lz4FError::DecompressionFailed.error_name(), "ERROR_decompressionFailed");
        assert_eq!(Lz4FError::HeaderChecksumInvalid.error_name(), "ERROR_headerChecksum_invalid");
        assert_eq!(Lz4FError::ContentChecksumInvalid.error_name(), "ERROR_contentChecksum_invalid");
        assert_eq!(Lz4FError::FrameDecodingAlreadyStarted.error_name(), "ERROR_frameDecoding_alreadyStarted");
        assert_eq!(Lz4FError::CompressionStateUninitialized.error_name(), "ERROR_compressionState_uninitialized");
        assert_eq!(Lz4FError::ParameterNull.error_name(), "ERROR_parameter_null");
        assert_eq!(Lz4FError::IoWrite.error_name(), "ERROR_io_write");
        assert_eq!(Lz4FError::IoRead.error_name(), "ERROR_io_read");
    }

    /// Parity: lz4f_is_error matches C LZ4F_isError for boundary values.
    #[test]
    fn is_error_boundary() {
        // Largest non-error value: usize::MAX - 23
        assert!(!lz4f_is_error(usize::MAX - 23));
        // Smallest error value: usize::MAX - 22 (= -23 as usize = index 23 = IoRead)
        assert!(lz4f_is_error(usize::MAX - 22));
        assert!(lz4f_is_error(usize::MAX)); // -1 = index 1 = Generic
    }

    /// Parity: lz4f_get_error_name returns correct strings.
    #[test]
    fn get_error_name_parity() {
        // Non-error code returns sentinel string (same as C)
        assert_eq!(lz4f_get_error_name(0), "Unspecified error code");
        assert_eq!(lz4f_get_error_name(42), "Unspecified error code");
        // Error code for Generic (index 1): -(ptrdiff_t)1 as usize = usize::MAX
        assert_eq!(lz4f_get_error_name(usize::MAX), "ERROR_GENERIC");
        // Error code for IoRead (index 23): usize::MAX - 22
        assert_eq!(lz4f_get_error_name(usize::MAX - 22), "ERROR_io_read");
    }

    /// Verify DecompressStage discriminants match C dStage_t values.
    #[test]
    fn decompress_stage_discriminants() {
        assert_eq!(DecompressStage::GetFrameHeader as u32, 0);
        assert_eq!(DecompressStage::StoreFrameHeader as u32, 1);
        assert_eq!(DecompressStage::Init as u32, 2);
        assert_eq!(DecompressStage::GetBlockHeader as u32, 3);
        assert_eq!(DecompressStage::StoreBlockHeader as u32, 4);
        assert_eq!(DecompressStage::CopyDirect as u32, 5);
        assert_eq!(DecompressStage::GetBlockChecksum as u32, 6);
        assert_eq!(DecompressStage::GetCBlock as u32, 7);
        assert_eq!(DecompressStage::StoreCBlock as u32, 8);
        assert_eq!(DecompressStage::FlushOut as u32, 9);
        assert_eq!(DecompressStage::GetSuffix as u32, 10);
        assert_eq!(DecompressStage::StoreSuffix as u32, 11);
        assert_eq!(DecompressStage::GetSFrameSize as u32, 12);
        assert_eq!(DecompressStage::StoreSFrameSize as u32, 13);
        assert_eq!(DecompressStage::SkipSkippable as u32, 14);
    }
}
