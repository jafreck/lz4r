//! LZ4 block constants, memory helpers, copy primitives, and hash-table types.
//!
//! Translated from lz4.c v1.10.0, lines 239–740:
//!   - Common constants (MINMATCH, WILDCOPYLENGTH, …)
//!   - Memory read/write helpers (unaligned, portable)
//!   - Wildcard-copy and offset-copy primitives
//!   - `INC32TABLE` / `DEC64TABLE` lookup arrays
//!   - `nb_common_bytes` and `count` (match-length helpers)
//!   - Hash-table types and operations (`hash4`, `hash5`, put/get/prepare)

use core::ptr;

// ── Platform-sizing note ──────────────────────────────────────────────────────
// `reg_t` in C is `u64` on x86_64 and `size_t` otherwise.  In Rust we use
// `usize`, which is pointer-width on every platform — equivalent behaviour.

// ─────────────────────────────────────────────────────────────────────────────
// Constants — "Common Constants" (lz4.c:239-264)
// ─────────────────────────────────────────────────────────────────────────────

/// Minimum match length encoded in an LZ4 block.
pub const MINMATCH: usize = 4;

/// Wildcard-copy granularity (helpers may write up to this many bytes past the
/// logical end of the destination).
pub const WILDCOPYLENGTH: usize = 8;

/// Last N bytes of the input are always emitted as literals.
/// See doc/lz4_Block_format.md#parsing-restrictions.
pub const LASTLITERALS: usize = 5;

/// Minimum bytes needed at the input tail to attempt a new match.
/// See doc/lz4_Block_format.md#parsing-restrictions.
pub const MFLIMIT: usize = 12;

/// Ensure 2 × WILDCOPYLENGTH can be written without overflowing the output buffer.
pub const MATCH_SAFEGUARD_DISTANCE: usize = 2 * WILDCOPYLENGTH - MINMATCH;

/// Minimum distance from the end of the safe fast-loop region.
pub const FASTLOOP_SAFE_DISTANCE: usize = 64;

/// Minimum input length that may produce any match at all.
pub const LZ4_MIN_LENGTH: usize = MFLIMIT + 1;

pub const KB: usize = 1 << 10;
pub const MB: usize = 1 << 20;
pub const GB: usize = 1 << 30;

/// Maximum back-reference distance supported by the LZ4 format.
pub const LZ4_DISTANCE_ABSOLUTE_MAX: u32 = 65_535;
/// Maximum back-reference distance used in this build (≤ LZ4_DISTANCE_ABSOLUTE_MAX).
pub const LZ4_DISTANCE_MAX: u32 = LZ4_DISTANCE_ABSOLUTE_MAX;

pub const ML_BITS: u32 = 4;
pub const ML_MASK: u32 = (1u32 << ML_BITS) - 1;
pub const RUN_BITS: u32 = 8 - ML_BITS;
pub const RUN_MASK: u32 = (1u32 << RUN_BITS) - 1;

// ─────────────────────────────────────────────────────────────────────────────
// Hash-table sizing constants (lz4.h:695-697, default LZ4_MEMORY_USAGE = 14)
// ─────────────────────────────────────────────────────────────────────────────

/// Log₂ of hash-table memory (bytes).  Default: 14 → 16 KiB table.
pub const LZ4_MEMORY_USAGE: u32 = 14;
/// Hash log: number of bits kept from each hash value.
pub const LZ4_HASHLOG: u32 = LZ4_MEMORY_USAGE - 2; // = 12
/// Hash-table size in bytes.
pub const LZ4_HASHTABLESIZE: usize = 1 << 14; // 1 << LZ4_MEMORY_USAGE
/// Number of u32 entries in the hash table.
pub const LZ4_HASH_SIZE_U32: usize = 1 << 12; // 1 << LZ4_HASHLOG

// ─────────────────────────────────────────────────────────────────────────────
// Local constants (lz4.c:709-711)
// ─────────────────────────────────────────────────────────────────────────────

/// Inputs within this size can use 16-bit (byU16) hash offsets.
pub const LZ4_64KLIMIT: usize = (64 * KB) + (MFLIMIT - 1);

/// Higher → faster on incompressible data at the cost of compression ratio.
pub const LZ4_SKIP_TRIGGER: u32 = 6;

// ─────────────────────────────────────────────────────────────────────────────
// Directives and table-type enumerations (lz4.c:328-332, 717, 742-743)
// ─────────────────────────────────────────────────────────────────────────────

/// Controls whether output is capped / must fill the destination buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum LimitedOutputDirective {
    /// No limit — compress as much as possible.
    NotLimited = 0,
    /// Stop as soon as the output buffer is full.
    LimitedOutput = 1,
    /// Fill the output buffer completely (`LZ4_compress_destSize` mode).
    FillOutput = 2,
}

/// Describes what kind of data lives in the hash table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum TableType {
    /// Table was zeroed out; no prior content recorded.
    ClearedTable = 0,
    /// Table stores raw `*const u8` pointers (32-bit external-dict mode only).
    ByPtr = 1,
    /// Table stores 32-bit absolute offsets (standard streaming mode).
    ByU32 = 2,
    /// Table stores 16-bit absolute offsets (small-input / byU16 mode).
    ByU16 = 3,
}

impl From<u32> for TableType {
    fn from(v: u32) -> Self {
        match v {
            1 => TableType::ByPtr,
            2 => TableType::ByU32,
            3 => TableType::ByU16,
            _ => TableType::ClearedTable,
        }
    }
}

/// Describes how previous content is accessible for back-references.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum DictDirective {
    /// No preceding content.
    NoDict = 0,
    /// Preceding 64 KB immediately before the current input in memory.
    WithPrefix64k = 1,
    /// Preceding content at an arbitrary memory location (`dictionary` field).
    UsingExtDict = 2,
    /// Preceding content is described by a separate context (`dict_ctx` field).
    UsingDictCtx = 3,
}

/// Whether the dictionary is smaller than a full 64 KB window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum DictIssueDirective {
    NoDictIssue = 0,
    DictSmall = 1,
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal stream state (mirrors LZ4_stream_t_internal from lz4.h:719-726)
// ─────────────────────────────────────────────────────────────────────────────

/// Internal compression state.  Mirrors `LZ4_stream_t_internal`.
///
/// The `hash_table` field stores `u32` values in `ByU32` mode, `u16` pairs
/// packed into `u32` slots in `ByU16` mode, and — on 32-bit targets only —
/// raw `*const u8` pointer values in `ByPtr` mode.
#[repr(C)]
pub struct StreamStateInternal {
    pub hash_table: [u32; LZ4_HASH_SIZE_U32],
    pub dictionary: *const u8,
    pub dict_ctx: *const StreamStateInternal,
    pub current_offset: u32,
    pub table_type: u32,
    pub dict_size: u32,
}

// SAFETY: Compression is driven by the caller under single-threaded
// or externally-synchronized access; raw pointers in the struct are not
// independently aliased.
unsafe impl Send for StreamStateInternal {}

impl StreamStateInternal {
    /// Create a zero-initialized state equivalent to `LZ4_initStream`.
    pub const fn new() -> Self {
        Self {
            hash_table: [0u32; LZ4_HASH_SIZE_U32],
            dictionary: ptr::null(),
            dict_ctx: ptr::null(),
            current_offset: 0,
            table_type: TableType::ClearedTable as u32,
            dict_size: 0,
        }
    }
}

impl Default for StreamStateInternal {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Memory read/write helpers (lz4.c:402-461)
//
// All helpers use `ptr::read_unaligned` / `ptr::write_unaligned` — the
// portable `memcpy`-based path from the C source — so they are safe even when
// the pointer is not naturally aligned.
// ─────────────────────────────────────────────────────────────────────────────

/// Read a native-endian `u16` from an unaligned pointer.
///
/// # Safety
/// `ptr` must be valid for reads of at least 2 bytes.
#[inline(always)]
pub unsafe fn read16(ptr: *const u8) -> u16 {
    core::ptr::read_unaligned(ptr as *const u16)
}

/// Read a native-endian `u32` from an unaligned pointer.
///
/// # Safety
/// `ptr` must be valid for reads of at least 4 bytes.
#[inline(always)]
pub unsafe fn read32(ptr: *const u8) -> u32 {
    core::ptr::read_unaligned(ptr as *const u32)
}

/// Read a `usize`-wide native-endian word from an unaligned pointer.
///
/// Corresponds to `LZ4_read_ARCH` — pointer-width on every platform.
///
/// # Safety
/// `ptr` must be valid for reads of at least `size_of::<usize>()` bytes.
#[inline(always)]
pub unsafe fn read_arch(ptr: *const u8) -> usize {
    core::ptr::read_unaligned(ptr as *const usize)
}

/// Write a native-endian `u16` to an unaligned pointer.
///
/// # Safety
/// `ptr` must be valid for writes of at least 2 bytes.
#[inline(always)]
pub unsafe fn write16(ptr: *mut u8, value: u16) {
    core::ptr::write_unaligned(ptr as *mut u16, value);
}

/// Write a native-endian `u32` to an unaligned pointer.
///
/// # Safety
/// `ptr` must be valid for writes of at least 4 bytes.
#[inline(always)]
pub unsafe fn write32(ptr: *mut u8, value: u32) {
    core::ptr::write_unaligned(ptr as *mut u32, value);
}

/// Read a little-endian `u16` from an unaligned pointer.
///
/// On little-endian targets this is identical to `read16`; on big-endian the
/// two bytes are byte-swapped to produce the correct LE interpretation.
///
/// # Safety
/// `ptr` must be valid for reads of at least 2 bytes.
#[inline(always)]
pub unsafe fn read_le16(ptr: *const u8) -> u16 {
    #[cfg(target_endian = "little")]
    {
        read16(ptr)
    }
    #[cfg(not(target_endian = "little"))]
    {
        (*ptr) as u16 | ((*ptr.add(1)) as u16) << 8
    }
}

/// Read a little-endian `u32` from an unaligned pointer.
///
/// Corresponds to `LZ4_readLE32` (conditionally compiled in C under
/// `LZ4_STATIC_LINKING_ONLY_ENDIANNESS_INDEPENDENT_OUTPUT`).
///
/// # Safety
/// `ptr` must be valid for reads of at least 4 bytes.
#[inline(always)]
pub unsafe fn read_le32(ptr: *const u8) -> u32 {
    #[cfg(target_endian = "little")]
    {
        read32(ptr)
    }
    #[cfg(not(target_endian = "little"))]
    {
        (*ptr) as u32
            | ((*ptr.add(1)) as u32) << 8
            | ((*ptr.add(2)) as u32) << 16
            | ((*ptr.add(3)) as u32) << 24
    }
}

/// Write a little-endian `u16` to an unaligned pointer.
///
/// # Safety
/// `ptr` must be valid for writes of at least 2 bytes.
#[inline(always)]
pub unsafe fn write_le16(ptr: *mut u8, value: u16) {
    #[cfg(target_endian = "little")]
    {
        write16(ptr, value);
    }
    #[cfg(not(target_endian = "little"))]
    {
        *ptr = value as u8;
        *ptr.add(1) = (value >> 8) as u8;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lookup tables (lz4.c:474-475)
// ─────────────────────────────────────────────────────────────────────────────

/// Advance the source pointer for small overlapping-copy offsets.
/// Exact values from C: `{0, 1, 2, 1, 0, 4, 4, 4}`.
pub static INC32TABLE: [u32; 8] = [0, 1, 2, 1, 0, 4, 4, 4];

/// Back up the source pointer for small overlapping-copy offsets.
/// Exact values from C: `{0, 0, 0, -1, -4, 1, 2, 3}`.
pub static DEC64TABLE: [i32; 8] = [0, 0, 0, -1, -4, 1, 2, 3];

// ─────────────────────────────────────────────────────────────────────────────
// Wildcard copy and offset copy primitives (lz4.c:463-572)
// ─────────────────────────────────────────────────────────────────────────────

/// Customized memcpy that may write up to **8 bytes past** `dst_end`.
///
/// Equivalent to `LZ4_wildCopy8`.
///
/// # Safety
/// - `src` and `dst` must be non-null, correctly aligned for byte access.
/// - The output buffer must have at least `(dst_end - dst) + 8` bytes of
///   allocated space to absorb the overwrite.
/// - `src` must be readable for the same total number of bytes.
#[inline(always)]
pub unsafe fn wild_copy8(mut dst: *mut u8, mut src: *const u8, dst_end: *mut u8) {
    loop {
        core::ptr::copy_nonoverlapping(src, dst, 8);
        dst = dst.add(8);
        src = src.add(8);
        if dst >= dst_end {
            break;
        }
    }
}

/// Customized memcpy that may write up to **32 bytes past** `dst_end`.
///
/// Copies two 16-byte chunks per iteration so it is compatible with offsets
/// ≥ 16.  Equivalent to `LZ4_wildCopy32`.
///
/// # Safety
/// Same as `wild_copy8` but with a 32-byte overwrite margin.
#[inline(always)]
pub unsafe fn wild_copy32(mut dst: *mut u8, mut src: *const u8, dst_end: *mut u8) {
    loop {
        core::ptr::copy_nonoverlapping(src, dst, 16);
        core::ptr::copy_nonoverlapping(src.add(16), dst.add(16), 16);
        dst = dst.add(32);
        src = src.add(32);
        if dst >= dst_end {
            break;
        }
    }
}

/// Base helper for back-references with `offset < 8` (overlapping patterns).
///
/// Corresponds to `LZ4_memcpy_using_offset_base`.
///
/// # Safety
/// - `src_ptr + offset == dst_ptr` (strict back-reference invariant).
/// - The output buffer must have at least 12 bytes of margin beyond `dst_end`.
/// - All pointers must be in bounds for their respective accesses.
#[inline(always)]
pub unsafe fn memcpy_using_offset_base(
    mut dst: *mut u8,
    mut src: *const u8,
    dst_end: *mut u8,
    offset: usize,
) {
    debug_assert!(src.add(offset) == dst);
    if offset < 8 {
        // Write 0 first to silence potential uninitialized-memory tools when offset==0.
        write32(dst, 0);
        *dst = *src;
        *dst.add(1) = *src.add(1);
        *dst.add(2) = *src.add(2);
        *dst.add(3) = *src.add(3);
        src = src.add(INC32TABLE[offset] as usize);
        core::ptr::copy_nonoverlapping(src, dst.add(4), 4);
        src = src.offset(-(DEC64TABLE[offset] as isize));
        dst = dst.add(8);
    } else {
        core::ptr::copy_nonoverlapping(src, dst, 8);
        dst = dst.add(8);
        src = src.add(8);
    }
    wild_copy8(dst, src, dst_end);
}

/// Overlapping copy for match expansion, with special fast paths for small
/// offsets (1, 2, 4) that replicate a short byte pattern.
///
/// Corresponds to `LZ4_memcpy_using_offset`.
///
/// Preconditions:
/// - `dst_end >= dst + MINMATCH` (at least 4 bytes to copy).
/// - At least 12 bytes of write space available beyond `dst_end`.
///
/// # Safety
/// Raw pointer arithmetic; callers must ensure valid ranges and write margin.
#[inline(always)]
pub unsafe fn memcpy_using_offset(
    mut dst: *mut u8,
    src: *const u8,
    dst_end: *mut u8,
    offset: usize,
) {
    let mut v = [0u8; 8];
    match offset {
        1 => {
            let b = *src;
            v = [b, b, b, b, b, b, b, b];
        }
        2 => {
            // v = [s0, s1, s0, s1, s0, s1, s0, s1]
            core::ptr::copy_nonoverlapping(src, v.as_mut_ptr(), 2);
            core::ptr::copy_nonoverlapping(src, v.as_mut_ptr().add(2), 2);
            // v[4..8] comes from v[0..4] — non-overlapping, safe
            core::ptr::copy_nonoverlapping(v.as_ptr(), v.as_mut_ptr().add(4), 4);
        }
        4 => {
            core::ptr::copy_nonoverlapping(src, v.as_mut_ptr(), 4);
            core::ptr::copy_nonoverlapping(src, v.as_mut_ptr().add(4), 4);
        }
        _ => {
            memcpy_using_offset_base(dst, src, dst_end, offset);
            return;
        }
    }
    // Write the 8-byte pattern and repeat until dst_end is reached.
    core::ptr::copy_nonoverlapping(v.as_ptr(), dst, 8);
    dst = dst.add(8);
    while dst < dst_end {
        core::ptr::copy_nonoverlapping(v.as_ptr(), dst, 8);
        dst = dst.add(8);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Common-byte counting (lz4.c:579-703)
// ─────────────────────────────────────────────────────────────────────────────

/// Count the number of low-order zero bytes in `val` (little-endian host) or
/// high-order zero bytes (big-endian host).
///
/// Equivalent to `LZ4_NbCommonBytes`.  `val` **must** be non-zero.
///
/// On little-endian: `val.trailing_zeros() >> 3`.
/// On big-endian:    `val.leading_zeros()  >> 3`.
#[cfg(target_pointer_width = "64")]
#[inline(always)]
pub fn nb_common_bytes(val: usize) -> u32 {
    debug_assert!(val != 0);
    #[cfg(target_endian = "little")]
    {
        (val as u64).trailing_zeros() >> 3
    }
    #[cfg(not(target_endian = "little"))]
    {
        (val as u64).leading_zeros() >> 3
    }
}

#[cfg(target_pointer_width = "32")]
#[inline(always)]
pub fn nb_common_bytes(val: usize) -> u32 {
    debug_assert!(val != 0);
    #[cfg(target_endian = "little")]
    {
        (val as u32).trailing_zeros() >> 3
    }
    #[cfg(not(target_endian = "little"))]
    {
        (val as u32).leading_zeros() >> 3
    }
}

/// Count how many bytes match between `p_in` and `p_match`, stopping at
/// `p_in_limit`.  Equivalent to `LZ4_count`.
///
/// Returns the number of matching bytes (0 if the first bytes differ).
///
/// # Safety
/// - `p_in ≤ p_in_limit`.
/// - Both `p_in` and `p_match` must be readable for at least
///   `p_in_limit - p_in` bytes.
/// - All three pointers must reside within the same allocation.
#[inline(always)]
pub unsafe fn count(mut p_in: *const u8, mut p_match: *const u8, p_in_limit: *const u8) -> u32 {
    let p_start = p_in;
    let step = core::mem::size_of::<usize>(); // == STEPSIZE in C

    // Fast path: consume one word at a time.
    if p_in < p_in_limit.sub(step - 1) {
        let diff = read_arch(p_match) ^ read_arch(p_in);
        if diff == 0 {
            p_in = p_in.add(step);
            p_match = p_match.add(step);
        } else {
            return nb_common_bytes(diff);
        }
    }

    while p_in < p_in_limit.sub(step - 1) {
        let diff = read_arch(p_match) ^ read_arch(p_in);
        if diff == 0 {
            p_in = p_in.add(step);
            p_match = p_match.add(step);
            continue;
        }
        p_in = p_in.add(nb_common_bytes(diff) as usize);
        return p_in.offset_from(p_start) as u32;
    }

    // Tail: handle remaining bytes one size at a time.
    if step == 8 && p_in < p_in_limit.sub(3) && read32(p_match) == read32(p_in) {
        p_in = p_in.add(4);
        p_match = p_match.add(4);
    }
    if p_in < p_in_limit.sub(1) && read16(p_match) == read16(p_in) {
        p_in = p_in.add(2);
        p_match = p_match.add(2);
    }
    if p_in < p_in_limit && *p_match == *p_in {
        p_in = p_in.add(1);
    }
    p_in.offset_from(p_start) as u32
}

// ─────────────────────────────────────────────────────────────────────────────
// Hash functions (lz4.c:777-806)
// ─────────────────────────────────────────────────────────────────────────────

/// 4-byte Knuth-multiplicative hash for a match candidate.
///
/// `table_type` selects the hash-log width:
/// - `ByU16` → `LZ4_HASHLOG + 1` bits (8192 entries, 16-bit table).
/// - others  → `LZ4_HASHLOG` bits    (4096 entries, 32-bit table).
///
/// Equivalent to `LZ4_hash4`.
#[inline(always)]
pub fn hash4(sequence: u32, table_type: TableType) -> u32 {
    let hash_log = if table_type == TableType::ByU16 {
        LZ4_HASHLOG + 1
    } else {
        LZ4_HASHLOG
    };
    // MINMATCH * 8 == 32; shift removes the low (32 - hash_log) bits.
    sequence.wrapping_mul(2_654_435_761u32) >> (32 - hash_log)
}

/// 5-byte hash for a match candidate (preferred on 64-bit targets).
///
/// Uses a different prime for little-endian vs big-endian to extract the most
/// significant byte from the 5-byte window via shifting.
///
/// Equivalent to `LZ4_hash5`.
#[inline(always)]
pub fn hash5(sequence: u64, table_type: TableType) -> u32 {
    let hash_log = if table_type == TableType::ByU16 {
        LZ4_HASHLOG + 1
    } else {
        LZ4_HASHLOG
    };
    #[cfg(target_endian = "little")]
    {
        const PRIME5: u64 = 889_523_592_379;
        (((sequence << 24).wrapping_mul(PRIME5)) >> (64 - hash_log)) as u32
    }
    #[cfg(not(target_endian = "little"))]
    {
        const PRIME8: u64 = 11_400_714_785_074_694_791;
        (((sequence >> 24).wrapping_mul(PRIME8)) >> (64 - hash_log)) as u32
    }
}

/// Compute the hash of the byte sequence starting at `p`.
///
/// On 64-bit targets and when `table_type != ByU16`, uses `hash5` (5-byte
/// window); otherwise falls back to `hash4` (4-byte window).
///
/// Equivalent to `LZ4_hashPosition`.
///
/// # Safety
/// `p` must be valid for reads of at least `size_of::<usize>()` bytes.
#[inline(always)]
pub unsafe fn hash_position(p: *const u8, table_type: TableType) -> u32 {
    #[cfg(target_pointer_width = "64")]
    if table_type != TableType::ByU16 {
        return hash5(read_arch(p) as u64, table_type);
    }
    hash4(read32(p), table_type)
}

// ─────────────────────────────────────────────────────────────────────────────
// Hash-table put / get operations (lz4.c:808-881)
// ─────────────────────────────────────────────────────────────────────────────

/// Zero out hash slot `h` in the table (equivalent to `LZ4_clearHash`).
///
/// # Safety
/// `table_base` must point to an appropriately-typed and sized hash table.
/// `h` must be within bounds for `table_type`.
#[inline(always)]
pub unsafe fn clear_hash(h: u32, table_base: *mut u32, table_type: TableType) {
    match table_type {
        TableType::ByPtr => {
            let tbl = table_base as *mut *const u8;
            *tbl.add(h as usize) = ptr::null();
        }
        TableType::ByU32 => {
            *table_base.add(h as usize) = 0;
        }
        TableType::ByU16 => {
            let tbl = table_base as *mut u16;
            *tbl.add(h as usize) = 0;
        }
        TableType::ClearedTable => {
            debug_assert!(false, "clear_hash called on ClearedTable");
        }
    }
}

/// Store index `idx` at hash slot `h` in a `ByU32` or `ByU16` table.
///
/// Equivalent to `LZ4_putIndexOnHash`.
///
/// # Safety
/// - `table_base` must be valid for the given `table_type` and slot `h`.
/// - Must not be called with `ByPtr` or `ClearedTable`.
#[inline(always)]
pub unsafe fn put_index_on_hash(idx: u32, h: u32, table_base: *mut u32, table_type: TableType) {
    match table_type {
        TableType::ByU32 => {
            *table_base.add(h as usize) = idx;
        }
        TableType::ByU16 => {
            debug_assert!(idx < 65536, "put_index_on_hash: idx overflows u16");
            let tbl = table_base as *mut u16;
            *tbl.add(h as usize) = idx as u16;
        }
        _ => {
            debug_assert!(false, "put_index_on_hash: invalid table type");
        }
    }
}

/// Store a raw pointer `p` at hash slot `h` in a `ByPtr` table.
///
/// Equivalent to `LZ4_putPositionOnHash`.
///
/// # Safety
/// - `table_base` must be a valid `*mut *const u8` pointer array of sufficient size.
/// - `h` must be within bounds.
/// - This is only used on 32-bit targets; on 64-bit targets `ByPtr` mode is
///   not employed by the standard compression path.
#[inline(always)]
pub unsafe fn put_position_on_hash(
    p: *const u8,
    h: u32,
    table_base: *mut *const u8,
    _table_type: TableType,
) {
    *table_base.add(h as usize) = p;
}

/// Hash `p` and store a raw pointer at that slot in a `ByPtr` hash table.
///
/// Equivalent to `LZ4_putPosition` (byPtr overload).
///
/// # Safety
/// See `put_position_on_hash` and `hash_position`.
#[inline(always)]
pub unsafe fn put_position(p: *const u8, table_base: *mut *const u8, table_type: TableType) {
    let h = hash_position(p, table_type);
    put_position_on_hash(p, h, table_base, table_type);
}

/// Retrieve the index stored at hash slot `h` from a `ByU32` or `ByU16` table.
///
/// Equivalent to `LZ4_getIndexOnHash`.
///
/// # Safety
/// - `table_base` must be valid for `table_type` and slot `h`.
/// - `table_type` must be `ByU32` or `ByU16`.
#[inline(always)]
pub unsafe fn get_index_on_hash(h: u32, table_base: *const u32, table_type: TableType) -> u32 {
    match table_type {
        TableType::ByU32 => {
            debug_assert!((h as usize) < (1 << (LZ4_MEMORY_USAGE - 2)));
            *table_base.add(h as usize)
        }
        TableType::ByU16 => {
            debug_assert!((h as usize) < (1 << (LZ4_MEMORY_USAGE - 1)));
            let tbl = table_base as *const u16;
            *tbl.add(h as usize) as u32
        }
        _ => {
            debug_assert!(false, "get_index_on_hash: invalid table type");
            0
        }
    }
}

/// Retrieve a raw pointer from a `ByPtr` hash table at slot `h`.
///
/// Equivalent to `LZ4_getPositionOnHash`.
///
/// # Safety
/// - `table_base` must be a valid `*const *const u8` pointer array.
/// - `h` must be within bounds.
#[inline(always)]
pub unsafe fn get_position_on_hash(
    h: u32,
    table_base: *const *const u8,
    _table_type: TableType,
) -> *const u8 {
    *table_base.add(h as usize)
}

/// Hash `p` and retrieve a raw pointer from a `ByPtr` hash table.
///
/// Equivalent to `LZ4_getPosition`.
///
/// # Safety
/// See `get_position_on_hash` and `hash_position`.
#[inline(always)]
pub unsafe fn get_position(
    p: *const u8,
    table_base: *const *const u8,
    table_type: TableType,
) -> *const u8 {
    let h = hash_position(p, table_type);
    get_position_on_hash(h, table_base, table_type)
}

// ─────────────────────────────────────────────────────────────────────────────
// Hash-table preparation (lz4.c:883-922)
// ─────────────────────────────────────────────────────────────────────────────

/// Prepare the hash table in `cctx` for a new compression job.
///
/// Resets the table when the table type changes, the offset counter wraps, or
/// the input is large enough that stale entries would interfere with
/// back-reference validity.  Mirrors `LZ4_prepareTable`.
///
/// # Safety
/// `cctx` must be a valid, exclusively-accessed pointer to a
/// `StreamStateInternal`.  `input_size` must be non-negative.
pub unsafe fn prepare_table(
    cctx: *mut StreamStateInternal,
    input_size: i32,
    table_type: TableType,
) {
    let ctx = &mut *cctx;
    let current_type = TableType::from(ctx.table_type);

    if current_type != TableType::ClearedTable {
        debug_assert!(input_size >= 0);

        // Determine whether a reset is necessary.
        let need_reset = current_type != table_type
            || (table_type == TableType::ByU16
                && ctx.current_offset.wrapping_add(input_size as u32) >= 0xFFFF)
            || (table_type == TableType::ByU32 && ctx.current_offset > GB as u32)
            || table_type == TableType::ByPtr
            || input_size >= (4 * KB as i32);

        if need_reset {
            // Zero the hash table (equivalent to MEM_INIT(hashTable, 0, LZ4_HASHTABLESIZE)).
            ptr::write_bytes(ctx.hash_table.as_mut_ptr(), 0, LZ4_HASH_SIZE_U32);
            ctx.current_offset = 0;
            ctx.table_type = TableType::ClearedTable as u32;
        }
    }

    // Adding a 64 KB gap ensures all old entries are > LZ4_DISTANCE_MAX back,
    // which is faster than compressing without a gap — but only when the offset
    // is already non-zero (offset == 0 is an even faster special case).
    if ctx.current_offset != 0 && table_type == TableType::ByU32 {
        ctx.current_offset = ctx.current_offset.wrapping_add(64 * KB as u32);
    }

    // Clear history.
    ctx.dict_ctx = ptr::null();
    ctx.dictionary = ptr::null();
    ctx.dict_size = 0;
}
