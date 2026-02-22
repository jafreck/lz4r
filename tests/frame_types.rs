// Integration tests for task-015: LZ4 Frame types, constants, and error handling
//
// Tests verify behavioural parity with lz4frame.c / lz4frame.h v1.10.0:
//   - All exported constants match their C counterparts exactly
//   - Enum discriminants match C enum values
//   - Default trait implementations match C zero-initialization
//   - Lz4FError::error_name() strings are byte-identical to C LZ4F_errorStrings[]
//   - Lz4FError::from_index() covers all 24 real error codes (0-23); 24 returns None
//   - Lz4FError::from_raw() decodes C-style size_t error codes correctly
//   - Lz4FError::is_error() returns false for OkNoError, true for all others
//   - lz4f_is_error() boundary matches C LZ4F_isError logic
//   - lz4f_get_error_name() mirrors C LZ4F_getErrorName output
//   - DecompressStage discriminants match C dStage_t values and ordering holds
//   - FrameInfo / Preferences Default initialisation has zero fields

use lz4::frame::types::{
    lz4f_get_error_name, lz4f_is_error, BlockChecksum, BlockCompressMode, BlockMode, BlockSizeId,
    ContentChecksum, CtxType, DecompressStage, FrameInfo, FrameType, Lz4FError, Preferences,
    BF_SIZE, BH_SIZE, LZ4F_BLOCKUNCOMPRESSED_FLAG, LZ4F_VERSION, MAX_FH_SIZE, MIN_FH_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constants — exact values from lz4frame.h / lz4frame.c
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn constant_lz4f_version() {
    // LZ4F_VERSION == 100 (lz4frame.h:256)
    assert_eq!(LZ4F_VERSION, 100u32);
}

#[test]
fn constant_blockuncompressed_flag() {
    // LZ4F_BLOCKUNCOMPRESSED_FLAG == 0x80000000 (lz4frame.c:234)
    assert_eq!(LZ4F_BLOCKUNCOMPRESSED_FLAG, 0x8000_0000u32);
    // High bit of a u32 set
    assert_eq!(LZ4F_BLOCKUNCOMPRESSED_FLAG, 1u32 << 31);
}

#[test]
fn constant_bh_size() {
    // BHSize == 4 (lz4frame.c: LZ4F_BLOCK_HEADER_SIZE)
    assert_eq!(BH_SIZE, 4usize);
}

#[test]
fn constant_bf_size() {
    // BFSize == 4 (lz4frame.c: LZ4F_BLOCK_CHECKSUM_SIZE)
    assert_eq!(BF_SIZE, 4usize);
}

#[test]
fn constant_min_fh_size() {
    // minFHSize == 7 (lz4frame.c: LZ4F_HEADER_SIZE_MIN)
    assert_eq!(MIN_FH_SIZE, 7usize);
}

#[test]
fn constant_max_fh_size() {
    // maxFHSize == 19 (lz4frame.c: LZ4F_HEADER_SIZE_MAX)
    assert_eq!(MAX_FH_SIZE, 19usize);
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockSizeId — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_size_id_discriminants() {
    // Corresponds to LZ4F_blockSizeID_t in lz4frame.h:123-133
    assert_eq!(BlockSizeId::Default as u32, 0);
    assert_eq!(BlockSizeId::Max64Kb as u32, 4);
    assert_eq!(BlockSizeId::Max256Kb as u32, 5);
    assert_eq!(BlockSizeId::Max1Mb as u32, 6);
    assert_eq!(BlockSizeId::Max4Mb as u32, 7);
}

#[test]
fn block_size_id_default_is_zero() {
    let d: BlockSizeId = Default::default();
    assert_eq!(d, BlockSizeId::Default);
    assert_eq!(d as u32, 0);
}

#[test]
fn block_size_id_clone_copy() {
    let a = BlockSizeId::Max1Mb;
    let b = a; // Copy
    let c = a.clone();
    assert_eq!(a, b);
    assert_eq!(a, c);
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockMode — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_mode_discriminants() {
    // Corresponds to LZ4F_blockMode_t in lz4frame.h:138-143
    assert_eq!(BlockMode::Linked as u32, 0);
    assert_eq!(BlockMode::Independent as u32, 1);
}

#[test]
fn block_mode_default_is_linked() {
    let d: BlockMode = Default::default();
    assert_eq!(d, BlockMode::Linked);
}

// ─────────────────────────────────────────────────────────────────────────────
// ContentChecksum — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn content_checksum_discriminants() {
    // Corresponds to LZ4F_contentChecksum_t in lz4frame.h:145-150
    assert_eq!(ContentChecksum::Disabled as u32, 0);
    assert_eq!(ContentChecksum::Enabled as u32, 1);
}

#[test]
fn content_checksum_default_is_disabled() {
    let d: ContentChecksum = Default::default();
    assert_eq!(d, ContentChecksum::Disabled);
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockChecksum — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_checksum_discriminants() {
    // Corresponds to LZ4F_blockChecksum_t in lz4frame.h:152-155
    assert_eq!(BlockChecksum::Disabled as u32, 0);
    assert_eq!(BlockChecksum::Enabled as u32, 1);
}

#[test]
fn block_checksum_default_is_disabled() {
    let d: BlockChecksum = Default::default();
    assert_eq!(d, BlockChecksum::Disabled);
}

// ─────────────────────────────────────────────────────────────────────────────
// FrameType — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn frame_type_discriminants() {
    // Corresponds to LZ4F_frameType_t in lz4frame.h:157-161
    assert_eq!(FrameType::Frame as u32, 0);
    assert_eq!(FrameType::SkippableFrame as u32, 1);
}

#[test]
fn frame_type_default_is_frame() {
    let d: FrameType = Default::default();
    assert_eq!(d, FrameType::Frame);
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockCompressMode — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn block_compress_mode_discriminants() {
    // Corresponds to LZ4F_BlockCompressMode_e in lz4frame.c:262
    assert_eq!(BlockCompressMode::Compressed as isize, 0);
    assert_eq!(BlockCompressMode::Uncompressed as isize, 1);
}

#[test]
fn block_compress_mode_default_is_compressed() {
    let d: BlockCompressMode = Default::default();
    assert_eq!(d, BlockCompressMode::Compressed);
}

// ─────────────────────────────────────────────────────────────────────────────
// CtxType — discriminants and Default
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ctx_type_discriminants() {
    // Corresponds to LZ4F_CtxType_e in lz4frame.c:263
    assert_eq!(CtxType::None as u16, 0);
    assert_eq!(CtxType::Fast as u16, 1);
    assert_eq!(CtxType::Hc as u16, 2);
}

#[test]
fn ctx_type_default_is_none() {
    let d: CtxType = Default::default();
    assert_eq!(d, CtxType::None);
}

// ─────────────────────────────────────────────────────────────────────────────
// FrameInfo — Default initialisation (zero fields, matching C zero-init)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn frame_info_default_fields() {
    let fi: FrameInfo = Default::default();
    assert_eq!(fi.block_size_id, BlockSizeId::Default);
    assert_eq!(fi.block_mode, BlockMode::Linked);
    assert_eq!(fi.content_checksum_flag, ContentChecksum::Disabled);
    assert_eq!(fi.frame_type, FrameType::Frame);
    assert_eq!(fi.content_size, 0u64);
    assert_eq!(fi.dict_id, 0u32);
    assert_eq!(fi.block_checksum_flag, BlockChecksum::Disabled);
}

#[test]
fn frame_info_is_copy() {
    let a: FrameInfo = Default::default();
    let b = a; // Copy trait
    assert_eq!(a.content_size, b.content_size);
    assert_eq!(a.dict_id, b.dict_id);
}

// ─────────────────────────────────────────────────────────────────────────────
// Preferences — Default initialisation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn preferences_default_fields() {
    let p: Preferences = Default::default();
    assert_eq!(p.compression_level, 0i32);
    assert!(!p.auto_flush);
    assert!(!p.favor_dec_speed);
    // Nested frame_info should also be zero
    assert_eq!(p.frame_info.content_size, 0u64);
    assert_eq!(p.frame_info.dict_id, 0u32);
}

#[test]
fn preferences_is_copy() {
    let a: Preferences = Default::default();
    let b = a; // Copy
    assert_eq!(a.compression_level, b.compression_level);
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressStage — discriminants (lz4frame.c:1248-1258) and ordering
// ─────────────────────────────────────────────────────────────────────────────

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

#[test]
fn decompress_stage_default_is_get_frame_header() {
    let d: DecompressStage = Default::default();
    assert_eq!(d, DecompressStage::GetFrameHeader);
}

#[test]
fn decompress_stage_ordering_le_init() {
    // C code uses comparisons like `dStage <= dstage_init` (== Init == 2)
    // All stages 0-2 must compare ≤ Init; stages 3+ must compare > Init.
    assert!(DecompressStage::GetFrameHeader <= DecompressStage::Init);
    assert!(DecompressStage::StoreFrameHeader <= DecompressStage::Init);
    assert!(DecompressStage::Init <= DecompressStage::Init);
    assert!(DecompressStage::GetBlockHeader > DecompressStage::Init);
    assert!(DecompressStage::SkipSkippable > DecompressStage::Init);
}

#[test]
fn decompress_stage_ordering_monotone() {
    // Each stage must be strictly less than the next
    let stages = [
        DecompressStage::GetFrameHeader,
        DecompressStage::StoreFrameHeader,
        DecompressStage::Init,
        DecompressStage::GetBlockHeader,
        DecompressStage::StoreBlockHeader,
        DecompressStage::CopyDirect,
        DecompressStage::GetBlockChecksum,
        DecompressStage::GetCBlock,
        DecompressStage::StoreCBlock,
        DecompressStage::FlushOut,
        DecompressStage::GetSuffix,
        DecompressStage::StoreSuffix,
        DecompressStage::GetSFrameSize,
        DecompressStage::StoreSFrameSize,
        DecompressStage::SkipSkippable,
    ];
    for w in stages.windows(2) {
        assert!(w[0] < w[1], "{:?} should be < {:?}", w[0], w[1]);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FError::error_name — byte-identical to C LZ4F_errorStrings[] (lz4frame.c:286-316)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn error_name_ok_no_error() {
    assert_eq!(Lz4FError::OkNoError.error_name(), "OK_NoError");
}

#[test]
fn error_name_generic() {
    assert_eq!(Lz4FError::Generic.error_name(), "ERROR_GENERIC");
}

#[test]
fn error_name_all_variants_parity() {
    // Table mirroring C LZ4F_errorStrings[] in order
    let expected: &[(&str, Lz4FError)] = &[
        ("OK_NoError", Lz4FError::OkNoError),
        ("ERROR_GENERIC", Lz4FError::Generic),
        ("ERROR_maxBlockSize_invalid", Lz4FError::MaxBlockSizeInvalid),
        ("ERROR_blockMode_invalid", Lz4FError::BlockModeInvalid),
        ("ERROR_parameter_invalid", Lz4FError::ParameterInvalid),
        ("ERROR_compressionLevel_invalid", Lz4FError::CompressionLevelInvalid),
        ("ERROR_headerVersion_wrong", Lz4FError::HeaderVersionWrong),
        ("ERROR_blockChecksum_invalid", Lz4FError::BlockChecksumInvalid),
        ("ERROR_reservedFlag_set", Lz4FError::ReservedFlagSet),
        ("ERROR_allocation_failed", Lz4FError::AllocationFailed),
        ("ERROR_srcSize_tooLarge", Lz4FError::SrcSizeTooLarge),
        ("ERROR_dstMaxSize_tooSmall", Lz4FError::DstMaxSizeTooSmall),
        ("ERROR_frameHeader_incomplete", Lz4FError::FrameHeaderIncomplete),
        ("ERROR_frameType_unknown", Lz4FError::FrameTypeUnknown),
        ("ERROR_frameSize_wrong", Lz4FError::FrameSizeWrong),
        ("ERROR_srcPtr_wrong", Lz4FError::SrcPtrWrong),
        ("ERROR_decompressionFailed", Lz4FError::DecompressionFailed),
        ("ERROR_headerChecksum_invalid", Lz4FError::HeaderChecksumInvalid),
        ("ERROR_contentChecksum_invalid", Lz4FError::ContentChecksumInvalid),
        ("ERROR_frameDecoding_alreadyStarted", Lz4FError::FrameDecodingAlreadyStarted),
        ("ERROR_compressionState_uninitialized", Lz4FError::CompressionStateUninitialized),
        ("ERROR_parameter_null", Lz4FError::ParameterNull),
        ("ERROR_io_write", Lz4FError::IoWrite),
        ("ERROR_io_read", Lz4FError::IoRead),
    ];
    for (s, variant) in expected {
        assert_eq!(variant.error_name(), *s, "mismatch for {:?}", variant);
    }
}

#[test]
fn error_display_matches_error_name() {
    // Display impl delegates to error_name(), so strings must match
    let variants = [
        Lz4FError::OkNoError,
        Lz4FError::Generic,
        Lz4FError::AllocationFailed,
        Lz4FError::IoRead,
    ];
    for v in &variants {
        assert_eq!(format!("{}", v), v.error_name());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FError::from_index — covers all 24 real codes; 24 returns None
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn from_index_all_real_codes_present() {
    for i in 0..24usize {
        assert!(
            Lz4FError::from_index(i).is_some(),
            "from_index({i}) should return Some"
        );
    }
}

#[test]
fn from_index_sentinel_24_is_none() {
    // ERROR_maxCode (= 24) is a C sentinel, not a real error
    assert!(Lz4FError::from_index(24).is_none());
}

#[test]
fn from_index_out_of_range_is_none() {
    assert!(Lz4FError::from_index(100).is_none());
    assert!(Lz4FError::from_index(usize::MAX).is_none());
}

#[test]
fn from_index_round_trips_with_error_name() {
    // from_index(i).error_name() must equal the C LZ4F_errorStrings[i] entry
    let strings = [
        "OK_NoError",
        "ERROR_GENERIC",
        "ERROR_maxBlockSize_invalid",
        "ERROR_blockMode_invalid",
        "ERROR_parameter_invalid",
        "ERROR_compressionLevel_invalid",
        "ERROR_headerVersion_wrong",
        "ERROR_blockChecksum_invalid",
        "ERROR_reservedFlag_set",
        "ERROR_allocation_failed",
        "ERROR_srcSize_tooLarge",
        "ERROR_dstMaxSize_tooSmall",
        "ERROR_frameHeader_incomplete",
        "ERROR_frameType_unknown",
        "ERROR_frameSize_wrong",
        "ERROR_srcPtr_wrong",
        "ERROR_decompressionFailed",
        "ERROR_headerChecksum_invalid",
        "ERROR_contentChecksum_invalid",
        "ERROR_frameDecoding_alreadyStarted",
        "ERROR_compressionState_uninitialized",
        "ERROR_parameter_null",
        "ERROR_io_write",
        "ERROR_io_read",
    ];
    for (i, &expected) in strings.iter().enumerate() {
        let name = Lz4FError::from_index(i).unwrap().error_name();
        assert_eq!(name, expected, "index {i}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FError::from_raw — decodes C-style size_t error codes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn from_raw_success_code_returns_none() {
    // Code 0 is a success (bytes written/read count)
    assert!(Lz4FError::from_raw(0).is_none());
    // Any value in [0, usize::MAX-23] is a success
    assert!(Lz4FError::from_raw(1000).is_none());
    assert!(Lz4FError::from_raw(usize::MAX - 23).is_none());
}

#[test]
fn from_raw_generic_is_index_1() {
    // -(ptrdiff_t)1 as usize == usize::MAX (two's complement)
    let result = Lz4FError::from_raw(usize::MAX);
    assert_eq!(result, Some(Lz4FError::Generic));
}

#[test]
fn from_raw_ok_no_error_index_0() {
    // -(ptrdiff_t)0 = 0, which is not > (usize::MAX - 23), so not an error
    // from_raw(0) should return None (success), not OkNoError
    assert!(Lz4FError::from_raw(0).is_none());
}

#[test]
fn from_raw_io_read_is_index_23() {
    // IoRead is at index 23; raw code = usize::MAX - 22 (= wrapping_neg of 23)
    let code = 23usize.wrapping_neg(); // == usize::MAX - 22
    let result = Lz4FError::from_raw(code);
    assert_eq!(result, Some(Lz4FError::IoRead));
}

#[test]
fn from_raw_out_of_known_range_is_none() {
    // Error range is only (usize::MAX-23, usize::MAX], so index 50 maps to
    // usize::MAX - 49 which is NOT in the error range → from_raw returns None.
    // The Generic fallback in from_raw is unreachable via the public error range.
    let code = 50usize.wrapping_neg(); // == usize::MAX - 49, below error threshold
    assert!(Lz4FError::from_raw(code).is_none());
    // Ensure the boundary holds: usize::MAX - 23 is the last non-error
    assert!(Lz4FError::from_raw(usize::MAX - 23).is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FError::is_error — false for OkNoError, true for everything else
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn is_error_ok_no_error_is_false() {
    assert!(!Lz4FError::OkNoError.is_error());
}

#[test]
fn is_error_all_error_variants_are_true() {
    let errors = [
        Lz4FError::Generic,
        Lz4FError::MaxBlockSizeInvalid,
        Lz4FError::BlockModeInvalid,
        Lz4FError::ParameterInvalid,
        Lz4FError::CompressionLevelInvalid,
        Lz4FError::HeaderVersionWrong,
        Lz4FError::BlockChecksumInvalid,
        Lz4FError::ReservedFlagSet,
        Lz4FError::AllocationFailed,
        Lz4FError::SrcSizeTooLarge,
        Lz4FError::DstMaxSizeTooSmall,
        Lz4FError::FrameHeaderIncomplete,
        Lz4FError::FrameTypeUnknown,
        Lz4FError::FrameSizeWrong,
        Lz4FError::SrcPtrWrong,
        Lz4FError::DecompressionFailed,
        Lz4FError::HeaderChecksumInvalid,
        Lz4FError::ContentChecksumInvalid,
        Lz4FError::FrameDecodingAlreadyStarted,
        Lz4FError::CompressionStateUninitialized,
        Lz4FError::ParameterNull,
        Lz4FError::IoWrite,
        Lz4FError::IoRead,
    ];
    for e in &errors {
        assert!(e.is_error(), "{:?} should be an error", e);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_is_error — boundary matches C LZ4F_isError (lz4frame.c:293-296)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn is_error_zero_is_not_error() {
    assert!(!lz4f_is_error(0));
}

#[test]
fn is_error_large_success_is_not_error() {
    // usize::MAX - 23 is the last non-error value
    assert!(!lz4f_is_error(usize::MAX - 23));
}

#[test]
fn is_error_boundary_first_error() {
    // usize::MAX - 22 is the first error value (index 23 = IoRead)
    assert!(lz4f_is_error(usize::MAX - 22));
}

#[test]
fn is_error_usize_max_is_error() {
    // usize::MAX = -(ptrdiff_t)1 = index 1 = Generic
    assert!(lz4f_is_error(usize::MAX));
}

#[test]
fn is_error_monotone_range() {
    // All values in [usize::MAX - 22, usize::MAX] must be errors
    for i in 0..=22usize {
        let code = usize::MAX - i;
        assert!(lz4f_is_error(code), "code {} should be an error", code);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_get_error_name — mirrors C LZ4F_getErrorName (lz4frame.c:298-303)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn get_error_name_non_error_returns_unspecified() {
    // Non-error codes produce "Unspecified error code" (same as C)
    assert_eq!(lz4f_get_error_name(0), "Unspecified error code");
    assert_eq!(lz4f_get_error_name(42), "Unspecified error code");
    assert_eq!(lz4f_get_error_name(usize::MAX - 23), "Unspecified error code");
}

#[test]
fn get_error_name_generic() {
    // Index 1 = Generic: raw code = usize::MAX
    assert_eq!(lz4f_get_error_name(usize::MAX), "ERROR_GENERIC");
}

#[test]
fn get_error_name_io_read() {
    // Index 23 = IoRead: raw code = usize::MAX - 22
    assert_eq!(lz4f_get_error_name(usize::MAX - 22), "ERROR_io_read");
}

#[test]
fn get_error_name_io_write() {
    // Index 22 = IoWrite: raw code = usize::MAX - 21
    assert_eq!(lz4f_get_error_name(usize::MAX - 21), "ERROR_io_write");
}

#[test]
fn get_error_name_allocation_failed() {
    // Index 9 = AllocationFailed: raw code = usize::MAX - 8
    assert_eq!(lz4f_get_error_name(usize::MAX - 8), "ERROR_allocation_failed");
}

#[test]
fn get_error_name_out_of_range_error_returns_unspecified() {
    // Error code with index > 23 (i.e., raw code < usize::MAX - 22)
    // but still > (usize::MAX - 23) is impossible given the boundary.
    // An out-of-range index like 50: raw = 50usize.wrapping_neg()
    // That falls in the error range but index 50 is unknown → "Unspecified error code"
    let code = 50usize.wrapping_neg();
    assert_eq!(lz4f_get_error_name(code), "Unspecified error code");
}
