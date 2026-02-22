//! LZ4 Frame decompression.
//!
//! Translated from lz4frame.c v1.10.0, lines 1244–2136.

use crate::block::decompress_api::decompress_safe_using_dict;
use crate::frame::header::{lz4f_get_block_size, lz4f_header_checksum, read_le32, read_le64};
use crate::frame::types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, CustomMem, DecompressStage,
    FrameInfo, FrameType, Lz4FError, LZ4F_VERSION, BF_SIZE, BH_SIZE, MAX_FH_SIZE, MIN_FH_SIZE,
};
use crate::xxhash::{xxh32_oneshot, Xxh32State};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const LZ4F_MAGICNUMBER: u32 = 0x184D_2204;
const LZ4F_MAGIC_SKIPPABLE_START: u32 = 0x184D_2A50;
const LZ4F_MIN_SIZE_TO_KNOW_HEADER_LENGTH: usize = 5;
const MAX_DICT_SIZE: usize = 64 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// DecompressOptions
// ─────────────────────────────────────────────────────────────────────────────

/// Options forwarded to [`lz4f_decompress`].
/// Corresponds to `LZ4F_decompressOptions_t` in lz4frame.h.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecompressOptions {
    /// Hint that destination buffer is stable between calls (not used in this impl).
    pub stable_dst: bool,
    /// Disable all checksum verification. Sticky once set for the frame lifetime.
    pub skip_checksums: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Lz4FDCtx struct
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 Frame decompression context.
/// Corresponds to `LZ4F_dctx_s` in lz4frame.c:1260.
pub struct Lz4FDCtx {
    pub cmem: CustomMem,
    pub frame_info: FrameInfo,
    pub version: u32,
    pub stage: DecompressStage,
    pub frame_remaining_size: u64,
    pub max_block_size: usize,
    pub max_buffer_size: usize,
    pub tmp_in: Vec<u8>,
    pub tmp_in_size: usize,
    pub tmp_in_target: usize,
    pub tmp_out_buffer: Vec<u8>,
    /// Offset of `tmpOut` within `tmp_out_buffer` (C: `tmpOut - tmpOutBuffer`).
    pub tmp_out_offset: usize,
    pub tmp_out_size: usize,
    pub tmp_out_start: usize,
    /// Rolling 64 KiB decompression dictionary. Replaces C\'s raw `dict` + `dictSize`.
    pub dict_bytes: Vec<u8>,
    pub xxh: Xxh32State,
    pub block_checksum: Xxh32State,
    /// Sticky: once `true`, checksums are skipped for the rest of the frame.
    pub skip_checksum: bool,
    /// Staging area for frame header bytes and per-block checksum bytes.
    pub header: [u8; MAX_FH_SIZE],
}

impl Lz4FDCtx {
    /// Create a zeroed decompression context.
    pub fn new(version: u32) -> Box<Self> {
        Box::new(Lz4FDCtx {
            cmem: CustomMem::default(),
            frame_info: FrameInfo::default(),
            version,
            stage: DecompressStage::GetFrameHeader,
            frame_remaining_size: 0,
            max_block_size: 0,
            max_buffer_size: 0,
            tmp_in: Vec::new(),
            tmp_in_size: 0,
            tmp_in_target: 0,
            tmp_out_buffer: Vec::new(),
            tmp_out_offset: 0,
            tmp_out_size: 0,
            tmp_out_start: 0,
            dict_bytes: Vec::new(),
            xxh: Xxh32State::new(0),
            block_checksum: Xxh32State::new(0),
            skip_checksum: false,
            header: [0u8; MAX_FH_SIZE],
        })
    }

    /// Append `new_bytes` to the rolling 64 KiB history dictionary.
    /// Equivalent to the copy-based part of `LZ4F_updateDict` (lz4frame.c:1527).
    fn update_dict(&mut self, new_bytes: &[u8]) {
        let n = new_bytes.len();
        if n == 0 {
            return;
        }
        if n >= MAX_DICT_SIZE {
            let src_start = n - MAX_DICT_SIZE;
            self.dict_bytes.resize(MAX_DICT_SIZE, 0);
            self.dict_bytes.copy_from_slice(&new_bytes[src_start..]);
        } else {
            let total = self.dict_bytes.len() + n;
            if total > MAX_DICT_SIZE {
                let drop = total - MAX_DICT_SIZE;
                self.dict_bytes.drain(..drop);
            }
            self.dict_bytes.extend_from_slice(new_bytes);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Context lifecycle (lz4frame.c:1284-1335)
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new LZ4 frame decompression context.
/// Equivalent to `LZ4F_createDecompressionContext` (lz4frame.c:1300).
pub fn lz4f_create_decompression_context(version: u32) -> Result<Box<Lz4FDCtx>, Lz4FError> {
    if version != LZ4F_VERSION {
        return Err(Lz4FError::HeaderVersionWrong);
    }
    Ok(Lz4FDCtx::new(version))
}

/// Release a decompression context (drops the Box).
/// Equivalent to `LZ4F_freeDecompressionContext` (lz4frame.c:1313).
pub fn lz4f_free_decompression_context(_dctx: Box<Lz4FDCtx>) {}

/// Reset context to initial state for a new frame.
/// Equivalent to `LZ4F_resetDecompressionContext` (lz4frame.c:1327).
pub fn lz4f_reset_decompression_context(dctx: &mut Lz4FDCtx) {
    dctx.stage = DecompressStage::GetFrameHeader;
    dctx.dict_bytes.clear();
    dctx.skip_checksum = false;
    dctx.frame_remaining_size = 0;
    dctx.frame_info = FrameInfo::default();
}

// ─────────────────────────────────────────────────────────────────────────────
// Frame header decoding (lz4frame.c:1346-1437)
// ─────────────────────────────────────────────────────────────────────────────

/// Decode the LZ4 frame header from `src`, updating `dctx` in place.
///
/// `from_header_buf` — `true` when `src` is the `dctx.header` staging buffer
/// (equivalent to C\'s `src == (void*)dctx->header` pointer comparison).
///
/// Returns number of bytes consumed, or an error.
/// Equivalent to `LZ4F_decodeHeader` (lz4frame.c:1346).
fn decode_header(dctx: &mut Lz4FDCtx, src: &[u8], from_header_buf: bool) -> Result<usize, Lz4FError> {
    if src.len() < MIN_FH_SIZE {
        return Err(Lz4FError::FrameHeaderIncomplete);
    }
    dctx.frame_info = FrameInfo::default();

    let magic = read_le32(src, 0);
    if (magic & 0xFFFF_FFF0) == LZ4F_MAGIC_SKIPPABLE_START {
        dctx.frame_info.frame_type = FrameType::SkippableFrame;
        if from_header_buf {
            dctx.tmp_in_size = src.len();
            dctx.tmp_in_target = 8;
            dctx.stage = DecompressStage::StoreSFrameSize;
            return Ok(src.len());
        } else {
            dctx.stage = DecompressStage::GetSFrameSize;
            return Ok(4);
        }
    }

    if magic != LZ4F_MAGICNUMBER {
        return Err(Lz4FError::FrameTypeUnknown);
    }
    dctx.frame_info.frame_type = FrameType::Frame;

    let flg = src[4] as u32;
    let version = (flg >> 6) & 0x3;
    let block_mode = (flg >> 5) & 0x1;
    let block_checksum_flag = (flg >> 4) & 0x1;
    let content_size_flag = (flg >> 3) & 0x1;
    let content_checksum_flag = (flg >> 2) & 0x1;
    let dict_id_flag = flg & 0x1;
    if ((flg >> 1) & 0x1) != 0 {
        return Err(Lz4FError::ReservedFlagSet);
    }
    if version != 1 {
        return Err(Lz4FError::HeaderVersionWrong);
    }

    let fh_size = MIN_FH_SIZE
        + if content_size_flag != 0 { 8 } else { 0 }
        + if dict_id_flag != 0 { 4 } else { 0 };

    if src.len() < fh_size {
        if !from_header_buf {
            dctx.header[..src.len()].copy_from_slice(src);
        }
        dctx.tmp_in_size = src.len();
        dctx.tmp_in_target = fh_size;
        dctx.stage = DecompressStage::StoreFrameHeader;
        return Ok(src.len());
    }

    let bd = src[5] as u32;
    let bsid_raw = (bd >> 4) & 0x7;
    if ((bd >> 7) & 0x1) != 0 {
        return Err(Lz4FError::ReservedFlagSet);
    }
    if bsid_raw < 4 {
        return Err(Lz4FError::MaxBlockSizeInvalid);
    }
    if (bd & 0x0F) != 0 {
        return Err(Lz4FError::ReservedFlagSet);
    }

    let hc = lz4f_header_checksum(&src[4..fh_size - 1]);
    if hc != src[fh_size - 1] {
        return Err(Lz4FError::HeaderChecksumInvalid);
    }

    let block_size_id = match bsid_raw {
        4 => BlockSizeId::Max64Kb,
        5 => BlockSizeId::Max256Kb,
        6 => BlockSizeId::Max1Mb,
        7 => BlockSizeId::Max4Mb,
        _ => return Err(Lz4FError::MaxBlockSizeInvalid),
    };

    dctx.frame_info.block_mode = if block_mode != 0 { BlockMode::Independent } else { BlockMode::Linked };
    dctx.frame_info.block_checksum_flag =
        if block_checksum_flag != 0 { BlockChecksum::Enabled } else { BlockChecksum::Disabled };
    dctx.frame_info.content_checksum_flag =
        if content_checksum_flag != 0 { ContentChecksum::Enabled } else { ContentChecksum::Disabled };
    dctx.frame_info.block_size_id = block_size_id;
    dctx.max_block_size = lz4f_get_block_size(block_size_id).unwrap_or(MAX_DICT_SIZE);

    if content_size_flag != 0 {
        let cs = read_le64(src, 6);
        dctx.frame_info.content_size = cs;
        dctx.frame_remaining_size = cs;
    }
    if dict_id_flag != 0 {
        dctx.frame_info.dict_id = read_le32(src, fh_size - 5);
    }

    dctx.stage = DecompressStage::Init;
    Ok(fh_size)
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_header_size (lz4frame.c:1440)
// ─────────────────────────────────────────────────────────────────────────────

/// Return the total byte length of the LZ4 frame header starting at `src`.
/// Equivalent to `LZ4F_headerSize` (lz4frame.c:1440).
pub fn lz4f_header_size(src: &[u8]) -> Result<usize, Lz4FError> {
    if src.len() < LZ4F_MIN_SIZE_TO_KNOW_HEADER_LENGTH {
        return Err(Lz4FError::FrameHeaderIncomplete);
    }
    let magic = read_le32(src, 0);
    if (magic & 0xFFFF_FFF0) == LZ4F_MAGIC_SKIPPABLE_START {
        return Ok(8);
    }
    if magic != LZ4F_MAGICNUMBER {
        return Err(Lz4FError::FrameTypeUnknown);
    }
    let flg = src[4] as u32;
    let csf = (flg >> 3) & 0x1;
    let dif = flg & 0x1;
    Ok(MIN_FH_SIZE + if csf != 0 { 8 } else { 0 } + if dif != 0 { 4 } else { 0 })
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_get_frame_info (lz4frame.c:1470)
// ─────────────────────────────────────────────────────────────────────────────

/// Extract frame parameters from `src` (or from an already-decoded context).
/// Returns `(frame_info, src_consumed, next_src_hint)`.
/// Equivalent to `LZ4F_getFrameInfo` (lz4frame.c:1470).
pub fn lz4f_get_frame_info(
    dctx: &mut Lz4FDCtx,
    src: &[u8],
) -> Result<(FrameInfo, usize, usize), Lz4FError> {
    if dctx.stage > DecompressStage::StoreFrameHeader {
        let (_, _, hint) = lz4f_decompress(dctx, None, &[], None)?;
        return Ok((dctx.frame_info, 0, hint));
    }
    if dctx.stage == DecompressStage::StoreFrameHeader {
        return Err(Lz4FError::FrameDecodingAlreadyStarted);
    }
    let h_size = lz4f_header_size(src)?;
    if src.len() < h_size {
        return Err(Lz4FError::FrameHeaderIncomplete);
    }
    let consumed = decode_header(dctx, &src[..h_size], false)?;
    Ok((dctx.frame_info, consumed, BH_SIZE))
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress — main streaming decompressor (lz4frame.c:1613-2116)
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming LZ4 frame decompressor.
///
/// Returns `Ok((src_consumed, dst_written, next_src_hint))`.
/// `next_src_hint` is 0 when the frame is fully decoded.
///
/// Equivalent to `LZ4F_decompress` (lz4frame.c:1613).
pub fn lz4f_decompress(
    dctx: &mut Lz4FDCtx,
    dst: Option<&mut [u8]>,
    src: &[u8],
    opts: Option<&DecompressOptions>,
) -> Result<(usize, usize, usize), Lz4FError> {
    if let Some(o) = opts {
        dctx.skip_checksum |= o.skip_checksums;
    }

    let src_len = src.len();
    let dst_len = dst.as_ref().map_or(0, |d| d.len());

    // Raw pointer to dst so we can re-borrow after mutable operations on dctx.
    // SAFETY: `dst_raw` is valid for `dst_len` bytes for the lifetime of `dst`.
    let dst_raw: *mut u8 = dst
        .as_ref()
        .map(|d| d.as_ptr() as *mut u8)
        .unwrap_or(core::ptr::null_mut());

    let mut src_pos: usize = 0;
    let mut dst_pos: usize = 0;
    let mut do_another = true;
    let mut next_hint: usize = 1;

    'sm: loop {
        if !do_another {
            break;
        }
        match dctx.stage {

            // ── GetFrameHeader ───────────────────────────────────────────────
            DecompressStage::GetFrameHeader => {
                let src_avail = src_len - src_pos;
                if src_avail >= MAX_FH_SIZE {
                    let h = decode_header(dctx, &src[src_pos..], false)?;
                    src_pos += h;
                } else {
                    dctx.tmp_in_size = 0;
                    if src_avail == 0 {
                        return Ok((0, 0, MIN_FH_SIZE));
                    }
                    dctx.tmp_in_target = MIN_FH_SIZE;
                    dctx.stage = DecompressStage::StoreFrameHeader;
                    // fall-through inline:
                    let copy = (dctx.tmp_in_target - dctx.tmp_in_size).min(src_len - src_pos);
                    dctx.header[dctx.tmp_in_size..dctx.tmp_in_size + copy]
                        .copy_from_slice(&src[src_pos..src_pos + copy]);
                    dctx.tmp_in_size += copy;
                    src_pos += copy;
                    if dctx.tmp_in_size < dctx.tmp_in_target {
                        next_hint = (dctx.tmp_in_target - dctx.tmp_in_size) + BH_SIZE;
                        do_another = false;
                        continue 'sm;
                    }
                    let tgt = dctx.tmp_in_target;
                    let hdr: Vec<u8> = dctx.header[..tgt].to_vec();
                    decode_header(dctx, &hdr, true)?;
                }
            }

            // ── StoreFrameHeader ─────────────────────────────────────────────
            DecompressStage::StoreFrameHeader => {
                let copy = (dctx.tmp_in_target - dctx.tmp_in_size).min(src_len - src_pos);
                dctx.header[dctx.tmp_in_size..dctx.tmp_in_size + copy]
                    .copy_from_slice(&src[src_pos..src_pos + copy]);
                dctx.tmp_in_size += copy;
                src_pos += copy;
                if dctx.tmp_in_size < dctx.tmp_in_target {
                    next_hint = (dctx.tmp_in_target - dctx.tmp_in_size) + BH_SIZE;
                    do_another = false;
                } else {
                    let tgt = dctx.tmp_in_target;
                    let hdr: Vec<u8> = dctx.header[..tgt].to_vec();
                    decode_header(dctx, &hdr, true)?;
                }
            }

            // ── Init ─────────────────────────────────────────────────────────
            DecompressStage::Init => {
                if dctx.frame_info.content_checksum_flag == ContentChecksum::Enabled {
                    dctx.xxh = Xxh32State::new(0);
                }
                let buf_needed = dctx.max_block_size
                    + if dctx.frame_info.block_mode == BlockMode::Linked { 128 * 1024 } else { 0 };
                if buf_needed > dctx.max_buffer_size {
                    dctx.max_buffer_size = 0;
                    dctx.tmp_in.resize(dctx.max_block_size + BF_SIZE, 0);
                    dctx.tmp_out_buffer.resize(buf_needed, 0);
                    dctx.max_buffer_size = buf_needed;
                }
                dctx.tmp_in_size = 0;
                dctx.tmp_in_target = 0;
                dctx.tmp_out_offset = 0;
                dctx.tmp_out_start = 0;
                dctx.tmp_out_size = 0;
                dctx.stage = DecompressStage::GetBlockHeader;
                continue 'sm;
            }

            // ── GetBlockHeader ───────────────────────────────────────────────
            DecompressStage::GetBlockHeader => {
                let src_avail = src_len - src_pos;
                let bh: [u8; BH_SIZE];
                if src_avail >= BH_SIZE {
                    bh = [src[src_pos], src[src_pos+1], src[src_pos+2], src[src_pos+3]];
                    src_pos += BH_SIZE;
                } else {
                    dctx.tmp_in_size = 0;
                    dctx.stage = DecompressStage::StoreBlockHeader;
                    let copy = BH_SIZE.min(src_avail);
                    dctx.tmp_in[..copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                    dctx.tmp_in_size = copy;
                    src_pos += copy;
                    if dctx.tmp_in_size < BH_SIZE {
                        next_hint = BH_SIZE - dctx.tmp_in_size;
                        do_another = false;
                        continue 'sm;
                    }
                    bh = [dctx.tmp_in[0], dctx.tmp_in[1], dctx.tmp_in[2], dctx.tmp_in[3]];
                }
                process_block_header(dctx, bh, &mut src_pos, src_len, dst_pos, dst_len, &mut next_hint, &mut do_another)?;
            }

            // ── StoreBlockHeader ─────────────────────────────────────────────
            DecompressStage::StoreBlockHeader => {
                let want = BH_SIZE - dctx.tmp_in_size;
                let copy = want.min(src_len - src_pos);
                let ts = dctx.tmp_in_size;
                dctx.tmp_in[ts..ts + copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                dctx.tmp_in_size += copy;
                src_pos += copy;
                if dctx.tmp_in_size < BH_SIZE {
                    next_hint = BH_SIZE - dctx.tmp_in_size;
                    do_another = false;
                    continue 'sm;
                }
                let bh = [dctx.tmp_in[0], dctx.tmp_in[1], dctx.tmp_in[2], dctx.tmp_in[3]];
                process_block_header(dctx, bh, &mut src_pos, src_len, dst_pos, dst_len, &mut next_hint, &mut do_another)?;
            }

            // ── CopyDirect (uncompressed block) ──────────────────────────────
            DecompressStage::CopyDirect => {
                let size_to_copy = if !dst_raw.is_null() {
                    let min_buf = (src_len - src_pos).min(dst_len - dst_pos);
                    dctx.tmp_in_target.min(min_buf)
                } else {
                    0
                };

                if size_to_copy > 0 {
                    // SAFETY: dst_raw is valid for dst_len bytes; bounds checked above.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            src.as_ptr().add(src_pos),
                            dst_raw.add(dst_pos),
                            size_to_copy,
                        );
                    }
                    if !dctx.skip_checksum {
                        if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled {
                            dctx.block_checksum.update(&src[src_pos..src_pos + size_to_copy]);
                        }
                        if dctx.frame_info.content_checksum_flag == ContentChecksum::Enabled {
                            dctx.xxh.update(&src[src_pos..src_pos + size_to_copy]);
                        }
                    }
                    if dctx.frame_info.content_size != 0 {
                        dctx.frame_remaining_size -= size_to_copy as u64;
                    }
                    if dctx.frame_info.block_mode == BlockMode::Linked {
                        dctx.update_dict(&src[src_pos..src_pos + size_to_copy]);
                    }
                    src_pos += size_to_copy;
                    dst_pos += size_to_copy;
                }

                if size_to_copy == dctx.tmp_in_target {
                    if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled {
                        dctx.tmp_in_size = 0;
                        dctx.stage = DecompressStage::GetBlockChecksum;
                    } else {
                        dctx.stage = DecompressStage::GetBlockHeader;
                    }
                } else {
                    dctx.tmp_in_target -= size_to_copy;
                    next_hint = dctx.tmp_in_target
                        + if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled { BF_SIZE } else { 0 }
                        + BH_SIZE;
                    do_another = false;
                }
            }

            // ── GetBlockChecksum ─────────────────────────────────────────────
            DecompressStage::GetBlockChecksum => {
                let crc4: [u8; 4];
                if (src_len - src_pos) >= 4 && dctx.tmp_in_size == 0 {
                    crc4 = [src[src_pos], src[src_pos+1], src[src_pos+2], src[src_pos+3]];
                    src_pos += 4;
                } else {
                    let still = 4 - dctx.tmp_in_size;
                    let copy = still.min(src_len - src_pos);
                    let ts = dctx.tmp_in_size;
                    dctx.header[ts..ts + copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                    dctx.tmp_in_size += copy;
                    src_pos += copy;
                    if dctx.tmp_in_size < 4 {
                        do_another = false;
                        continue 'sm;
                    }
                    crc4 = [dctx.header[0], dctx.header[1], dctx.header[2], dctx.header[3]];
                }
                if !dctx.skip_checksum {
                    let read_crc = u32::from_le_bytes(crc4);
                    let calc_crc = dctx.block_checksum.digest();
                    if read_crc != calc_crc {
                        return Err(Lz4FError::BlockChecksumInvalid);
                    }
                }
                dctx.stage = DecompressStage::GetBlockHeader;
            }

            // ── GetCBlock ────────────────────────────────────────────────────
            DecompressStage::GetCBlock => {
                if (src_len - src_pos) < dctx.tmp_in_target {
                    // Not enough input — switch to StoreCBlock and buffer inline
                    dctx.tmp_in_size = 0;
                    dctx.stage = DecompressStage::StoreCBlock;
                    let copy = dctx.tmp_in_target.min(src_len - src_pos);
                    dctx.tmp_in[..copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                    dctx.tmp_in_size = copy;
                    src_pos += copy;
                    if dctx.tmp_in_size < dctx.tmp_in_target {
                        let crc_extra = if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled { BF_SIZE } else { 0 };
                        next_hint = (dctx.tmp_in_target - dctx.tmp_in_size) + crc_extra + BH_SIZE;
                        do_another = false;
                        continue 'sm;
                    }
                    // Buffered enough — decode from tmp_in
                    let mut c_size = dctx.tmp_in_target;
                    // Verify block checksum if enabled
                    if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled {
                        c_size -= BF_SIZE;
                        let read_crc = u32::from_le_bytes([
                            dctx.tmp_in[c_size], dctx.tmp_in[c_size+1],
                            dctx.tmp_in[c_size+2], dctx.tmp_in[c_size+3],
                        ]);
                        let calc_crc = xxh32_oneshot(&dctx.tmp_in[..c_size], 0);
                        if !dctx.skip_checksum && read_crc != calc_crc {
                            return Err(Lz4FError::BlockChecksumInvalid);
                        }
                    }
                    let src_data: Vec<u8> = dctx.tmp_in[..c_size].to_vec();
                    decompress_and_dispatch(dctx, &src_data, &mut dst_pos, dst_len, dst_raw, &mut next_hint, &mut do_another)?;
                } else {
                    // Enough input — decode directly from src
                    let block_start = src_pos;
                    src_pos += dctx.tmp_in_target;
                    let mut c_size = dctx.tmp_in_target;
                    if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled {
                        c_size -= BF_SIZE;
                        let crc_off = block_start + c_size;
                        let read_crc = u32::from_le_bytes([
                            src[crc_off], src[crc_off+1], src[crc_off+2], src[crc_off+3],
                        ]);
                        let calc_crc = xxh32_oneshot(&src[block_start..block_start + c_size], 0);
                        if !dctx.skip_checksum && read_crc != calc_crc {
                            return Err(Lz4FError::BlockChecksumInvalid);
                        }
                    }
                    let src_data: Vec<u8> = src[block_start..block_start + c_size].to_vec();
                    decompress_and_dispatch(dctx, &src_data, &mut dst_pos, dst_len, dst_raw, &mut next_hint, &mut do_another)?;
                }
            }

            // ── StoreCBlock ──────────────────────────────────────────────────
            DecompressStage::StoreCBlock => {
                let want = dctx.tmp_in_target - dctx.tmp_in_size;
                let copy = want.min(src_len - src_pos);
                let ts = dctx.tmp_in_size;
                dctx.tmp_in[ts..ts + copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                dctx.tmp_in_size += copy;
                src_pos += copy;
                if dctx.tmp_in_size < dctx.tmp_in_target {
                    let crc_extra = if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled { BF_SIZE } else { 0 };
                    next_hint = (dctx.tmp_in_target - dctx.tmp_in_size) + crc_extra + BH_SIZE;
                    do_another = false;
                    continue 'sm;
                }
                let mut c_size = dctx.tmp_in_target;
                if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled {
                    c_size -= BF_SIZE;
                    let read_crc = u32::from_le_bytes([
                        dctx.tmp_in[c_size], dctx.tmp_in[c_size+1],
                        dctx.tmp_in[c_size+2], dctx.tmp_in[c_size+3],
                    ]);
                    let calc_crc = xxh32_oneshot(&dctx.tmp_in[..c_size], 0);
                    if !dctx.skip_checksum && read_crc != calc_crc {
                        return Err(Lz4FError::BlockChecksumInvalid);
                    }
                }
                let src_data: Vec<u8> = dctx.tmp_in[..c_size].to_vec();
                decompress_and_dispatch(dctx, &src_data, &mut dst_pos, dst_len, dst_raw, &mut next_hint, &mut do_another)?;
            }

            // ── FlushOut ─────────────────────────────────────────────────────
            DecompressStage::FlushOut => {
                if !dst_raw.is_null() {
                    let avail = dst_len - dst_pos;
                    let remaining = dctx.tmp_out_size - dctx.tmp_out_start;
                    let copy = remaining.min(avail);
                    let src_off = dctx.tmp_out_offset + dctx.tmp_out_start;
                    // SAFETY: dst_raw valid for dst_len bytes; src is tmp_out_buffer (owned).
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            dctx.tmp_out_buffer.as_ptr().add(src_off),
                            dst_raw.add(dst_pos),
                            copy,
                        );
                    }
                    // Mirror C's LZ4F_updateDict(withinTmp=1) in lz4frame.c:1969: update the
                    // rolling history window with the bytes just flushed from tmp_out_buffer.
                    // The clone releases the tmp_out_buffer borrow before the mutable call.
                    if dctx.frame_info.block_mode == BlockMode::Linked && copy > 0 {
                        let flushed = dctx.tmp_out_buffer[src_off..src_off + copy].to_vec();
                        dctx.update_dict(&flushed);
                    }
                    dctx.tmp_out_start += copy;
                    dst_pos += copy;
                }
                if dctx.tmp_out_start == dctx.tmp_out_size {
                    dctx.stage = DecompressStage::GetBlockHeader;
                } else {
                    do_another = false;
                    next_hint = BH_SIZE;
                }
            }

            // ── GetSuffix ────────────────────────────────────────────────────
            DecompressStage::GetSuffix => {
                if dctx.frame_remaining_size != 0 {
                    return Err(Lz4FError::FrameSizeWrong);
                }
                if dctx.frame_info.content_checksum_flag == ContentChecksum::Disabled {
                    next_hint = 0;
                    lz4f_reset_decompression_context(dctx);
                    do_another = false;
                    continue 'sm;
                }
                let src_avail = src_len - src_pos;
                if src_avail < 4 {
                    // fall through to StoreSuffix inline
                    dctx.tmp_in_size = 0;
                    dctx.stage = DecompressStage::StoreSuffix;
                    let copy = src_avail;
                    dctx.tmp_in[..copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                    dctx.tmp_in_size = copy;
                    src_pos += copy;
                    if dctx.tmp_in_size < 4 {
                        next_hint = 4 - dctx.tmp_in_size;
                        do_another = false;
                        continue 'sm;
                    }
                    let crc4 = [dctx.tmp_in[0], dctx.tmp_in[1], dctx.tmp_in[2], dctx.tmp_in[3]];
                    verify_content_checksum(dctx, crc4)?;
                } else {
                    let crc4 = [src[src_pos], src[src_pos+1], src[src_pos+2], src[src_pos+3]];
                    src_pos += 4;
                    verify_content_checksum(dctx, crc4)?;
                }
                next_hint = 0;
                lz4f_reset_decompression_context(dctx);
                do_another = false;
            }

            // ── StoreSuffix ──────────────────────────────────────────────────
            DecompressStage::StoreSuffix => {
                let want = 4 - dctx.tmp_in_size;
                let copy = want.min(src_len - src_pos);
                let ts = dctx.tmp_in_size;
                dctx.tmp_in[ts..ts + copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                dctx.tmp_in_size += copy;
                src_pos += copy;
                if dctx.tmp_in_size < 4 {
                    next_hint = 4 - dctx.tmp_in_size;
                    do_another = false;
                    continue 'sm;
                }
                let crc4 = [dctx.tmp_in[0], dctx.tmp_in[1], dctx.tmp_in[2], dctx.tmp_in[3]];
                verify_content_checksum(dctx, crc4)?;
                next_hint = 0;
                lz4f_reset_decompression_context(dctx);
                do_another = false;
            }

            // ── GetSFrameSize ────────────────────────────────────────────────
            DecompressStage::GetSFrameSize => {
                if (src_len - src_pos) >= 4 {
                    let sf = u32::from_le_bytes([src[src_pos], src[src_pos+1], src[src_pos+2], src[src_pos+3]]) as usize;
                    src_pos += 4;
                    dctx.frame_info.content_size = sf as u64;
                    dctx.tmp_in_target = sf;
                    dctx.stage = DecompressStage::SkipSkippable;
                } else {
                    // Stage into header buffer
                    dctx.tmp_in_size = 4; // magic bytes already "consumed" from src in decode_header
                    dctx.tmp_in_target = 8;
                    dctx.stage = DecompressStage::StoreSFrameSize;
                    let copy = (dctx.tmp_in_target - dctx.tmp_in_size).min(src_len - src_pos);
                    dctx.header[dctx.tmp_in_size..dctx.tmp_in_size + copy]
                        .copy_from_slice(&src[src_pos..src_pos + copy]);
                    dctx.tmp_in_size += copy;
                    src_pos += copy;
                    if dctx.tmp_in_size < dctx.tmp_in_target {
                        next_hint = dctx.tmp_in_target - dctx.tmp_in_size;
                        do_another = false;
                        continue 'sm;
                    }
                    let sf = u32::from_le_bytes([dctx.header[4], dctx.header[5], dctx.header[6], dctx.header[7]]) as usize;
                    dctx.frame_info.content_size = sf as u64;
                    dctx.tmp_in_target = sf;
                    dctx.stage = DecompressStage::SkipSkippable;
                }
            }

            // ── StoreSFrameSize ──────────────────────────────────────────────
            DecompressStage::StoreSFrameSize => {
                let copy = (dctx.tmp_in_target - dctx.tmp_in_size).min(src_len - src_pos);
                let ts = dctx.tmp_in_size;
                dctx.header[ts..ts + copy].copy_from_slice(&src[src_pos..src_pos + copy]);
                dctx.tmp_in_size += copy;
                src_pos += copy;
                if dctx.tmp_in_size < dctx.tmp_in_target {
                    next_hint = dctx.tmp_in_target - dctx.tmp_in_size;
                    do_another = false;
                    continue 'sm;
                }
                let sf = u32::from_le_bytes([dctx.header[4], dctx.header[5], dctx.header[6], dctx.header[7]]) as usize;
                dctx.frame_info.content_size = sf as u64;
                dctx.tmp_in_target = sf;
                dctx.stage = DecompressStage::SkipSkippable;
            }

            // ── SkipSkippable ────────────────────────────────────────────────
            DecompressStage::SkipSkippable => {
                let skip = dctx.tmp_in_target.min(src_len - src_pos);
                src_pos += skip;
                dctx.tmp_in_target -= skip;
                do_another = false;
                next_hint = dctx.tmp_in_target;
                if dctx.tmp_in_target == 0 {
                    lz4f_reset_decompression_context(dctx);
                }
            }
        }
    }

    // Post-loop note (mirrors C lz4frame.c:2081-2111):
    // In C, after the loop, history is explicitly preserved from the caller's dst
    // into tmpOutBuffer for linked-block frames when stable_dst=false. In this Rust
    // implementation dict_bytes is a separate allocation that is always disjoint from
    // the caller's dst buffer, so no post-loop copy is required:
    //   - For blocks decoded directly into dst: update_dict was called immediately
    //     after each block decode in decompress_and_dispatch.
    //   - For blocks decoded into tmp_out_buffer (tmpOut path): update_dict is called
    //     incrementally in FlushOut as bytes are flushed to dst.
    // In both cases dict_bytes retains its contents across calls regardless of what
    // the caller does with its dst buffer, satisfying the stable-history requirement.

    Ok((src_pos, dst_pos, next_hint))
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper: process block header bytes (shared by GetBlockHeader + StoreBlockHeader)
// ─────────────────────────────────────────────────────────────────────────────

fn process_block_header(
    dctx: &mut Lz4FDCtx,
    bh: [u8; BH_SIZE],
    src_pos: &mut usize,
    src_len: usize,
    dst_pos: usize,
    dst_len: usize,
    next_hint: &mut usize,
    do_another: &mut bool,
) -> Result<(), Lz4FError> {
    let block_header = u32::from_le_bytes(bh);
    let next_c_block_size = (block_header & 0x7FFF_FFFF) as usize;
    let crc_size = if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled { BF_SIZE } else { 0 };

    if block_header == 0 {
        dctx.stage = DecompressStage::GetSuffix;
        return Ok(());
    }
    if next_c_block_size > dctx.max_block_size {
        return Err(Lz4FError::MaxBlockSizeInvalid);
    }

    if (block_header & crate::frame::types::LZ4F_BLOCKUNCOMPRESSED_FLAG) != 0 {
        dctx.tmp_in_target = next_c_block_size;
        if dctx.frame_info.block_checksum_flag == BlockChecksum::Enabled {
            dctx.block_checksum = Xxh32State::new(0);
        }
        dctx.stage = DecompressStage::CopyDirect;
    } else {
        dctx.tmp_in_target = next_c_block_size + crc_size;
        dctx.stage = DecompressStage::GetCBlock;
        // If no dst space or no src remaining, stop
        if dst_pos == dst_len || *src_pos == src_len {
            *next_hint = BH_SIZE + next_c_block_size + crc_size;
            *do_another = false;
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper: decompress a block and write to dst or tmp_out_buffer
// ─────────────────────────────────────────────────────────────────────────────

/// Called from GetCBlock/StoreCBlock with the validated compressed block data.
/// Decides whether to decode directly into dst (if space allows) or into
/// `tmp_out_buffer`, sets `dctx.stage`, updates checksums and dict.
fn decompress_and_dispatch(
    dctx: &mut Lz4FDCtx,
    compressed: &[u8],
    dst_pos: &mut usize,
    dst_len: usize,
    dst_raw: *mut u8,
    _next_hint: &mut usize,
    _do_another: &mut bool,
) -> Result<(), Lz4FError> {
    let dst_avail = dst_len - *dst_pos;
    let dict_ptr: *const u8 = if dctx.dict_bytes.is_empty() {
        core::ptr::null()
    } else {
        dctx.dict_bytes.as_ptr()
    };
    let dict_len = dctx.dict_bytes.len();

    if !dst_raw.is_null() && dst_avail >= dctx.max_block_size {
        // Decode directly into caller\'s destination buffer.
        // SAFETY: dst_raw is valid for dst_avail bytes; dict_ptr valid for dict_len bytes.
        let decoded = unsafe {
            decompress_safe_using_dict(
                compressed.as_ptr(),
                dst_raw.add(*dst_pos),
                compressed.len(),
                dst_avail,
                dict_ptr,
                dict_len,
            )
            .map_err(|_| Lz4FError::DecompressionFailed)?
        };

        // Post-decode: update checksum and dict by reading back the decoded bytes.
        // SAFETY: we just wrote `decoded` bytes at dst_raw+*dst_pos; they are valid.
        if !dctx.skip_checksum && dctx.frame_info.content_checksum_flag == ContentChecksum::Enabled {
            let decoded_slice = unsafe {
                core::slice::from_raw_parts(dst_raw.add(*dst_pos) as *const u8, decoded)
            };
            dctx.xxh.update(decoded_slice);
        }
        if dctx.frame_info.content_size != 0 {
            dctx.frame_remaining_size -= decoded as u64;
        }
        if dctx.frame_info.block_mode == BlockMode::Linked {
            let decoded_slice = unsafe {
                core::slice::from_raw_parts(dst_raw.add(*dst_pos) as *const u8, decoded)
            };
            dctx.update_dict(decoded_slice);
        }
        *dst_pos += decoded;
        dctx.stage = DecompressStage::GetBlockHeader;
    } else {
        // Decode into tmp_out_buffer then flush to dst later.
        dctx.tmp_out_offset = 0;
        let cap = dctx.tmp_out_buffer.len();
        let tmp_ptr = dctx.tmp_out_buffer.as_mut_ptr();
        // SAFETY: tmp_ptr valid for cap bytes; dict_ptr valid for dict_len bytes.
        let decoded = unsafe {
            decompress_safe_using_dict(
                compressed.as_ptr(),
                tmp_ptr,
                compressed.len(),
                cap,
                dict_ptr,
                dict_len,
            )
            .map_err(|_| Lz4FError::DecompressionFailed)?
        };

        if !dctx.skip_checksum && dctx.frame_info.content_checksum_flag == ContentChecksum::Enabled {
            dctx.xxh.update(&dctx.tmp_out_buffer[..decoded]);
        }
        if dctx.frame_info.content_size != 0 {
            dctx.frame_remaining_size -= decoded as u64;
        }
        // Do NOT update dict here for the tmpOut path: the dictionary is updated
        // incrementally in FlushOut as data is flushed to the caller's buffer,
        // mirroring C's LZ4F_updateDict(withinTmp=1) semantics (lz4frame.c:1969).
        dctx.tmp_out_size = decoded;
        dctx.tmp_out_start = 0;
        dctx.stage = DecompressStage::FlushOut;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Content-checksum verification
// ─────────────────────────────────────────────────────────────────────────────

fn verify_content_checksum(dctx: &mut Lz4FDCtx, crc4: [u8; 4]) -> Result<(), Lz4FError> {
    if !dctx.skip_checksum {
        let read_crc = u32::from_le_bytes(crc4);
        let result_crc = dctx.xxh.digest();
        if read_crc != result_crc {
            return Err(Lz4FError::ContentChecksumInvalid);
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// lz4f_decompress_using_dict (lz4frame.c:2118)
// ─────────────────────────────────────────────────────────────────────────────

/// Decompress an LZ4 frame with a predefined dictionary.
/// The dictionary is loaded into the context before the first block decode.
/// Equivalent to `LZ4F_decompress_usingDict` (lz4frame.c:2118).
pub fn lz4f_decompress_using_dict(
    dctx: &mut Lz4FDCtx,
    dst: Option<&mut [u8]>,
    src: &[u8],
    dict: &[u8],
    opts: Option<&DecompressOptions>,
) -> Result<(usize, usize, usize), Lz4FError> {
    if dctx.stage <= DecompressStage::Init {
        dctx.dict_bytes.clear();
        dctx.dict_bytes.extend_from_slice(dict);
        if dctx.dict_bytes.len() > MAX_DICT_SIZE {
            let excess = dctx.dict_bytes.len() - MAX_DICT_SIZE;
            dctx.dict_bytes.drain(..excess);
        }
    }
    lz4f_decompress(dctx, dst, src, opts)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_free_context() {
        let dctx = lz4f_create_decompression_context(LZ4F_VERSION).unwrap();
        assert_eq!(dctx.stage, DecompressStage::GetFrameHeader);
        lz4f_free_decompression_context(dctx);
    }

    #[test]
    fn create_context_wrong_version() {
        assert!(lz4f_create_decompression_context(99).is_err());
        assert!(lz4f_create_decompression_context(101).is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
        dctx.skip_checksum = true;
        dctx.frame_remaining_size = 42;
        dctx.dict_bytes.extend_from_slice(b"hello");
        lz4f_reset_decompression_context(&mut dctx);
        assert_eq!(dctx.stage, DecompressStage::GetFrameHeader);
        assert!(!dctx.skip_checksum);
        assert_eq!(dctx.frame_remaining_size, 0);
        assert!(dctx.dict_bytes.is_empty());
    }

    #[test]
    fn header_size_skippable() {
        let mut buf = [0u8; 8];
        buf[..4].copy_from_slice(&0x184D_2A50u32.to_le_bytes());
        assert_eq!(lz4f_header_size(&buf), Ok(8));
    }

    #[test]
    fn header_size_standard_no_options() {
        let mut buf = [0u8; 7];
        buf[..4].copy_from_slice(&0x184D_2204u32.to_le_bytes());
        buf[4] = 0x60; // FLG: version=1, block_indep, no contentSize, no dictID
        buf[5] = 0x70; // BD: blockSizeID=7
        buf[6] = lz4f_header_checksum(&buf[4..6]);
        assert_eq!(lz4f_header_size(&buf), Ok(7));
    }

    #[test]
    fn header_size_insufficient_input() {
        assert!(lz4f_header_size(&[0u8; 3]).is_err());
    }

    #[test]
    fn decompress_empty_src() {
        let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
        let (sc, dw, hint) = lz4f_decompress(&mut dctx, None, &[], None).unwrap();
        assert_eq!(sc, 0);
        assert_eq!(dw, 0);
        assert_eq!(hint, MIN_FH_SIZE);
    }

    #[test]
    fn update_dict_small() {
        let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
        dctx.update_dict(b"hello");
        dctx.update_dict(b" world");
        assert_eq!(&dctx.dict_bytes, b"hello world");
    }

    #[test]
    fn update_dict_rolling_window() {
        let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
        let big = vec![0xAAu8; MAX_DICT_SIZE];
        dctx.update_dict(&big);
        assert_eq!(dctx.dict_bytes.len(), MAX_DICT_SIZE);
        let extra = vec![0xBBu8; 1024];
        dctx.update_dict(&extra);
        assert_eq!(dctx.dict_bytes.len(), MAX_DICT_SIZE);
        assert_eq!(&dctx.dict_bytes[MAX_DICT_SIZE - 1024..], &extra[..]);
    }

    #[test]
    fn update_dict_larger_than_max() {
        let mut dctx = Lz4FDCtx::new(LZ4F_VERSION);
        let data: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
        dctx.update_dict(&data);
        assert_eq!(dctx.dict_bytes.len(), MAX_DICT_SIZE);
        assert_eq!(&dctx.dict_bytes[..], &data[64 * 1024..]);
    }
}
