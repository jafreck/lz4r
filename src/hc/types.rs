//! HC compression types, level table, hash functions, and context initialisation.
//!
//! Translated from lz4hc.c v1.10.0, lines 71–260:
//!   - `lz4hc_strat_e` enum → `HcStrategy`
//!   - `cParams_t` → `CParams`
//!   - `k_clTable[13]` compression-level parameter table
//!   - `LZ4HC_getCLevelParams` → `get_clevel_params`
//!   - Hash functions: `hash_ptr`, `mid_hash4_ptr`, `mid_hash7`, `mid_hash8_ptr`,
//!     `read64`, `read_le64`
//!   - `LZ4HC_NbCommonBytes32` → `nb_common_bytes32`
//!   - `LZ4HC_count` (forward match) → `hc_count` (delegates to `block::types::count`)
//!   - `LZ4HC_countBack` → `count_back`
//!   - `LZ4HC_CCtx_internal` → `HcCCtxInternal`
//!   - `LZ4HC_clearTables` → `clear_tables`
//!   - `LZ4HC_init_internal` → `init_internal`

use crate::block::types as bt;

// ─────────────────────────────────────────────────────────────────────────────
// Compression-level constants (lz4hc.h:47–50)
// ─────────────────────────────────────────────────────────────────────────────

pub const LZ4HC_CLEVEL_MIN: i32 = 2;
pub const LZ4HC_CLEVEL_DEFAULT: i32 = 9;
pub const LZ4HC_CLEVEL_OPT_MIN: i32 = 10;
pub const LZ4HC_CLEVEL_MAX: i32 = 12;

// ─────────────────────────────────────────────────────────────────────────────
// HC hash-table sizing (lz4hc.h:222–228)
// ─────────────────────────────────────────────────────────────────────────────

pub const LZ4HC_DICTIONARY_LOGSIZE: u32 = 16;
/// Chain table length: one entry per slot in the 64 KB dictionary window.
pub const LZ4HC_MAXD: usize = 1 << LZ4HC_DICTIONARY_LOGSIZE; // 65536
pub const LZ4HC_MAXD_MASK: usize = LZ4HC_MAXD - 1;           // 65535

pub const LZ4HC_HASH_LOG: u32 = 15;
/// Hash table entries (HC uses a 15-bit log → 32768 u32 slots).
pub const LZ4HC_HASHTABLESIZE: usize = 1 << LZ4HC_HASH_LOG; // 32768
pub const LZ4HC_HASH_MASK: u32 = (LZ4HC_HASHTABLESIZE - 1) as u32;

pub const LZ4HC_HASHSIZE: usize = 4; // bytes hashed by LZ4HC_hashPtr

// ─────────────────────────────────────────────────────────────────────────────
// LZ4MID hash-table sizing (lz4hc.c:141–143)
// ─────────────────────────────────────────────────────────────────────────────

pub const LZ4MID_HASHSIZE: usize = 8;
pub const LZ4MID_HASHLOG: u32 = LZ4HC_HASH_LOG - 1; // 14
pub const LZ4MID_HASHTABLESIZE: usize = 1 << LZ4MID_HASHLOG; // 16384

// ─────────────────────────────────────────────────────────────────────────────
// Other constants (lz4hc.c:76–77)
// ─────────────────────────────────────────────────────────────────────────────

/// Max match length representable in the HC optimal parser.
/// Equivalent to `(ML_MASK - 1) + MINMATCH` in C.
pub const OPTIMAL_ML: i32 = (bt::ML_MASK - 1) as i32 + bt::MINMATCH as i32; // 18

/// Lookahead window size for the optimal parser.
pub const LZ4_OPT_NUM: usize = 1 << 12; // 4096

// ─────────────────────────────────────────────────────────────────────────────
// Enums (lz4hc.c:72, 86)
// ─────────────────────────────────────────────────────────────────────────────

/// Internal directive for dictionary-context mode (lz4hc.c:72).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DictCtxDirective {
    NoDictCtx,
    UsingDictCtxHc,
}

/// Compression strategy selected by the compression level (lz4hc.c:86).
///
/// Maps to C `lz4hc_strat_e { lz4mid, lz4hc, lz4opt }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum HcStrategy {
    /// Medium-speed dual-hash strategy (levels 0–2).
    Lz4Mid = 0,
    /// Hash-chain strategy (levels 3–9).
    Lz4Hc = 1,
    /// Optimal-parser strategy (levels 10–12).
    Lz4Opt = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// Compression parameters (lz4hc.c:87–91)
// ─────────────────────────────────────────────────────────────────────────────

/// Per-level compression parameters.  Mirrors C `cParams_t`.
#[derive(Clone, Copy, Debug)]
pub struct CParams {
    pub strat: HcStrategy,
    pub nb_searches: u32,
    pub target_length: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Compression-level table (lz4hc.c:92–106)
//
// Struct initialiser order in C: { strat, nbSearches, targetLength }
// Entry [12]: targetLength = LZ4_OPT_NUM (4096)
// ─────────────────────────────────────────────────────────────────────────────

/// Level → compression parameter table. Index is the compression level (0–12).
///
/// Matches `k_clTable[LZ4HC_CLEVEL_MAX + 1]` in lz4hc.c exactly.
pub static K_CL_TABLE: [CParams; (LZ4HC_CLEVEL_MAX + 1) as usize] = [
    CParams { strat: HcStrategy::Lz4Mid, nb_searches:     2, target_length:              16 }, /* 0, unused */
    CParams { strat: HcStrategy::Lz4Mid, nb_searches:     2, target_length:              16 }, /* 1, unused */
    CParams { strat: HcStrategy::Lz4Mid, nb_searches:     2, target_length:              16 }, /* 2 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:     4, target_length:              16 }, /* 3 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:     8, target_length:              16 }, /* 4 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:    16, target_length:              16 }, /* 5 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:    32, target_length:              16 }, /* 6 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:    64, target_length:              16 }, /* 7 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:   128, target_length:              16 }, /* 8 */
    CParams { strat: HcStrategy::Lz4Hc,  nb_searches:   256, target_length:              16 }, /* 9 */
    CParams { strat: HcStrategy::Lz4Opt, nb_searches:    96, target_length:              64 }, /* 10 == LZ4HC_CLEVEL_OPT_MIN */
    CParams { strat: HcStrategy::Lz4Opt, nb_searches:   512, target_length:             128 }, /* 11 */
    CParams { strat: HcStrategy::Lz4Opt, nb_searches: 16384, target_length: LZ4_OPT_NUM as u32 }, /* 12 == LZ4HC_CLEVEL_MAX */
];

/// Return the compression parameters for a given compression level.
///
/// Mirrors `LZ4HC_getCLevelParams`.  Levels < 1 are clamped to
/// `LZ4HC_CLEVEL_DEFAULT`; levels > `LZ4HC_CLEVEL_MAX` are clamped to
/// `LZ4HC_CLEVEL_MAX`.
#[inline]
pub fn get_clevel_params(mut c_level: i32) -> CParams {
    if c_level < 1 {
        c_level = LZ4HC_CLEVEL_DEFAULT;
    }
    c_level = c_level.min(LZ4HC_CLEVEL_MAX);
    K_CL_TABLE[c_level as usize]
}

// ─────────────────────────────────────────────────────────────────────────────
// 64-bit read helpers (lz4hc.c:126–163)
// ─────────────────────────────────────────────────────────────────────────────

/// Read a native-endian `u64` from an unaligned pointer.
///
/// Equivalent to `LZ4_read64` (portable / safe `memcpy` path).
///
/// # Safety
/// `ptr` must be valid for reads of at least 8 bytes.
#[inline(always)]
pub unsafe fn read64(ptr: *const u8) -> u64 {
    core::ptr::read_unaligned(ptr as *const u64)
}

/// Read a little-endian `u64` from an unaligned pointer.
///
/// On little-endian hosts this is identical to `read64`; on big-endian the
/// bytes are assembled in LE order.  Equivalent to `LZ4_readLE64`.
///
/// # Safety
/// `ptr` must be valid for reads of at least 8 bytes.
#[inline(always)]
pub unsafe fn read_le64(ptr: *const u8) -> u64 {
    #[cfg(target_endian = "little")]
    {
        read64(ptr)
    }
    #[cfg(not(target_endian = "little"))]
    {
        (*ptr) as u64
            | ((*ptr.add(1)) as u64) << 8
            | ((*ptr.add(2)) as u64) << 16
            | ((*ptr.add(3)) as u64) << 24
            | ((*ptr.add(4)) as u64) << 32
            | ((*ptr.add(5)) as u64) << 40
            | ((*ptr.add(6)) as u64) << 48
            | ((*ptr.add(7)) as u64) << 56
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HC hash functions (lz4hc.c:120–151)
// ─────────────────────────────────────────────────────────────────────────────

/// 4-byte Knuth-multiplicative hash for the HC algorithm.
///
/// `HASH_FUNCTION(i) = (i * 2654435761) >> (MINMATCH*8 - LZ4HC_HASH_LOG)`
///                   = `(i * 2654435761) >> 17`
///
/// Equivalent to `LZ4HC_hashPtr`.
///
/// # Safety
/// `ptr` must be valid for reads of at least 4 bytes.
#[inline(always)]
pub unsafe fn hash_ptr(ptr: *const u8) -> u32 {
    bt::read32(ptr).wrapping_mul(2_654_435_761u32) >> (bt::MINMATCH as u32 * 8 - LZ4HC_HASH_LOG)
}

/// 4-byte hash for the LZ4MID strategy.
///
/// `(v * 2654435761) >> (32 - LZ4MID_HASHLOG)`
///
/// Equivalent to `LZ4MID_hash4`.
#[inline(always)]
pub fn mid_hash4(v: u32) -> u32 {
    v.wrapping_mul(2_654_435_761u32) >> (32 - LZ4MID_HASHLOG)
}

/// 4-byte hash for the LZ4MID strategy, reading from a pointer.
///
/// Equivalent to `LZ4MID_hash4Ptr`.
///
/// # Safety
/// `ptr` must be valid for reads of at least 4 bytes.
#[inline(always)]
pub unsafe fn mid_hash4_ptr(ptr: *const u8) -> u32 {
    mid_hash4(bt::read32(ptr))
}

/// 7-byte (lower 56-bit) hash for the LZ4MID strategy.
///
/// Input `v` is assumed to have been read as little-endian.
/// Equivalent to `LZ4MID_hash7`.
#[inline(always)]
pub fn mid_hash7(v: u64) -> u32 {
    // Hash the lower 56 bits: shift left 8 to discard the top byte, then apply prime
    ((v << (64 - 56)).wrapping_mul(58_295_818_150_454_627u64) >> (64 - LZ4MID_HASHLOG)) as u32
}

/// 8-byte hash for the LZ4MID strategy, reading from a pointer using LE order.
///
/// Equivalent to `LZ4MID_hash8Ptr`.
///
/// # Safety
/// `ptr` must be valid for reads of at least 8 bytes.
#[inline(always)]
pub unsafe fn mid_hash8_ptr(ptr: *const u8) -> u32 {
    mid_hash7(read_le64(ptr))
}

// ─────────────────────────────────────────────────────────────────────────────
// Match-length counting (lz4hc.c:167–225)
// ─────────────────────────────────────────────────────────────────────────────

/// Count the number of common (matching) leading bytes in a 32-bit XOR
/// difference value, using platform-appropriate bit instructions.
///
/// On little-endian: leading zeros → trailing zeros (LSB = first byte).
/// On big-endian:    leading zeros directly.
///
/// Equivalent to `LZ4HC_NbCommonBytes32`.  `val` **must** be non-zero.
#[inline(always)]
pub fn nb_common_bytes32(val: u32) -> u32 {
    debug_assert!(val != 0);
    #[cfg(target_endian = "little")]
    {
        val.trailing_zeros() >> 3
    }
    #[cfg(not(target_endian = "little"))]
    {
        val.leading_zeros() >> 3
    }
}

/// Count how many bytes at `ip` and `match_ptr` match, stopping at
/// `p_in_limit`.
///
/// This is the HC forward match-length counter — a thin alias for
/// `block::types::count` (equivalent to `LZ4_count`, which `lz4hc.c` gains
/// via `#include "lz4.c"`).
///
/// Returns the number of matching bytes (0 if the first bytes differ).
///
/// # Safety
/// - `ip ≤ p_in_limit`.
/// - Both `ip` and `match_ptr` must be readable for at least
///   `p_in_limit - ip` bytes.
#[inline(always)]
pub unsafe fn hc_count(ip: *const u8, match_ptr: *const u8, p_in_limit: *const u8) -> u32 {
    bt::count(ip, match_ptr, p_in_limit)
}

/// Extend a match backwards from `ip`/`match`, stopping at `i_min`/`m_min`.
///
/// Returns a **negative** value: the number of common bytes before the
/// current position.
///
/// Equivalent to `LZ4HC_countBack`.
///
/// # Safety
/// All four pointers must be valid and within the same allocation.
/// `ip >= i_min` and `match >= m_min`.
#[inline(always)]
pub unsafe fn count_back(
    ip: *const u8,
    match_ptr: *const u8,
    i_min: *const u8,
    m_min: *const u8,
) -> i32 {
    let mut back: i32 = 0;
    // min is the maximum we can step back (negative value)
    let min = {
        let di = i_min.offset_from(ip) as i32;   // ≤ 0
        let dm = m_min.offset_from(match_ptr) as i32; // ≤ 0
        di.max(dm) // whichever is less negative → the binding limit
    };
    debug_assert!(min <= 0);

    // 4-byte step: consume 4 common bytes at a time
    while (back - min) > 3 {
        let vi = bt::read32(ip.offset(back as isize - 4));
        let vm = bt::read32(match_ptr.offset(back as isize - 4));
        let v = vi ^ vm;
        if v != 0 {
            // Scanning backward — count common bytes from the high address end.
            // On little-endian the high address byte is in the MSBs, so use
            // leading_zeros; on big-endian it is in the LSBs, so use
            // trailing_zeros.  This is the opposite of nb_common_bytes32.
            #[cfg(target_endian = "little")]
            let common_high = v.leading_zeros() >> 3;
            #[cfg(not(target_endian = "little"))]
            let common_high = v.trailing_zeros() >> 3;
            return back - common_high as i32;
        }
        back -= 4;
    }

    // byte-by-byte tail
    while back > min && *ip.offset(back as isize - 1) == *match_ptr.offset(back as isize - 1) {
        back -= 1;
    }

    back
}

// ─────────────────────────────────────────────────────────────────────────────
// HC compression context (lz4hc.h:234–250)
// ─────────────────────────────────────────────────────────────────────────────

/// Internal HC compression context.  Mirrors `LZ4HC_CCtx_internal`.
///
/// Contains raw pointers to the **caller-owned** input data; callers must
/// ensure input remains alive for the duration of any operation.
#[repr(C)]
pub struct HcCCtxInternal {
    /// Position → most-recent offset hash table (HC 4-byte hash).
    pub hash_table: [u32; LZ4HC_HASHTABLESIZE],
    /// 16-bit delta chain table, indexed by `pos & LZ4HC_MAXD_MASK`.
    pub chain_table: [u16; LZ4HC_MAXD],
    /// One past the end of the current input window (not owned).
    pub end: *const u8,
    /// Start of the current prefix window; offsets are relative to this.
    pub prefix_start: *const u8,
    /// Start of the external dictionary (alternate reference for extDict mode).
    pub dict_start: *const u8,
    /// Positions below this value need the external dictionary.
    pub dict_limit: u32,
    /// Positions below this value have no history at all.
    pub low_limit: u32,
    /// Next position index from which to continue dictionary updates.
    pub next_to_update: u32,
    pub compression_level: i16,
    /// Non-zero → prefer decompression speed over compression ratio.
    pub favor_dec_speed: i8,
    /// Non-zero → stream must be fully reset before next use.
    pub dirty: i8,
    /// Attached dictionary context (may be null).
    pub dict_ctx: *const HcCCtxInternal,
}

// SAFETY: HC compression is single-threaded or externally synchronised; raw
// pointers inside the struct are not independently aliased.
unsafe impl Send for HcCCtxInternal {}

impl HcCCtxInternal {
    /// Create a zero-initialised context (all pointers null, tables zeroed).
    pub const fn new() -> Self {
        Self {
            hash_table: [0u32; LZ4HC_HASHTABLESIZE],
            // chain_table initial value must be 0xFF bytes (see clear_tables),
            // but `const fn` cannot call memset, so callers must invoke
            // `clear_tables` or `init_internal` before first use.
            chain_table: [0u16; LZ4HC_MAXD],
            end: core::ptr::null(),
            prefix_start: core::ptr::null(),
            dict_start: core::ptr::null(),
            dict_limit: 0,
            low_limit: 0,
            next_to_update: 0,
            compression_level: 0,
            favor_dec_speed: 0,
            dirty: 0,
            dict_ctx: core::ptr::null(),
        }
    }
}

impl Default for HcCCtxInternal {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Context initialisation (lz4hc.c:236–259)
// ─────────────────────────────────────────────────────────────────────────────

/// Zero the hash table and fill the chain table with 0xFF bytes.
///
/// Equivalent to `LZ4HC_clearTables`.
pub fn clear_tables(hc4: &mut HcCCtxInternal) {
    hc4.hash_table.fill(0u32);
    hc4.chain_table.fill(0xFFFFu16); // MEM_INIT(chainTable, 0xFF, …)
}

/// Initialise (or re-initialise) an HC context for a new input block.
///
/// Computes a `newStartingOffset` that accounts for the prior buffer size and
/// `dictLimit`.  If the offset would exceed 1 GB the tables are cleared first
/// to prevent stale back-references.  Then 64 KB is added to avoid
/// underflow at the start of the window.
///
/// Equivalent to `LZ4HC_init_internal`.
///
/// # Safety
/// `hc4.end` and `hc4.prefix_start` must both be valid (or both null for a
/// freshly zeroed context).  `start` must be a valid pointer for the
/// duration of all subsequent operations on `hc4`.
pub unsafe fn init_internal(hc4: &mut HcCCtxInternal, start: *const u8) {
    let buffer_size = hc4.end.offset_from(hc4.prefix_start) as usize;
    let mut new_starting_offset: usize = buffer_size + hc4.dict_limit as usize;
    // Overflow check: if this exceeded usize we cannot trust the value.
    debug_assert!(new_starting_offset >= buffer_size);

    if new_starting_offset > (1usize << 30) {
        // 1 GB exceeded — clear tables and reset to 0
        clear_tables(hc4);
        new_starting_offset = 0;
    }
    new_starting_offset += 64 * 1024; // 64 KB guard at the bottom of the window

    hc4.next_to_update = new_starting_offset as u32;
    hc4.prefix_start = start;
    hc4.end = start;
    hc4.dict_start = start;
    hc4.dict_limit = new_starting_offset as u32;
    hc4.low_limit = new_starting_offset as u32;
}
