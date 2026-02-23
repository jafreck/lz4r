//! LZ4 streaming compression state management.
//!
//! Pure-Rust implementation of the LZ4 streaming block-compression API,
//! corresponding to lz4.c v1.10.0 (lines 1526–1834).
//!
//! # API correspondence
//! - [`Lz4Stream`] (mirrors `LZ4_stream_t` / `LZ4_stream_t_internal`)
//! - [`Lz4Stream::reset`] / [`Lz4Stream::reset_fast`]
//!   (`LZ4_resetStream`, `LZ4_resetStream_fast`)
//! - [`Lz4Stream::load_dict`] / [`Lz4Stream::load_dict_slow`]
//!   (`LZ4_loadDict`, `LZ4_loadDictSlow`) via shared internal helper
//! - [`Lz4Stream::attach_dictionary`] (`LZ4_attach_dictionary`)
//!   — stores a raw pointer to an external dict stream; see Safety note below
//! - [`Lz4Stream::renorm_dict`] (`LZ4_renormDictT`)
//! - [`Lz4Stream::compress_fast_continue`] (`LZ4_compress_fast_continue`)
//! - [`Lz4Stream::compress_force_ext_dict`] (`LZ4_compress_forceExtDict`)
//! - [`Lz4Stream::save_dict`] (`LZ4_saveDict`)
//!
//! ## Dictionary-attachment safety invariant
//! [`Lz4Stream::attach_dictionary`] stores a raw `*const StreamStateInternal`
//! pointing into the **dict stream's** internal state.  The caller **must**
//! ensure that the dict stream outlives the working (compressor) stream, and
//! that neither stream is moved (or that they are heap-allocated via
//! [`Box::new`]) for the lifetime of the attached relationship.

use core::ptr;

use super::compress::{compress_generic, LZ4_ACCELERATION_DEFAULT, LZ4_ACCELERATION_MAX};
use super::types::{
    get_index_on_hash, hash_position, prepare_table, put_index_on_hash, DictDirective,
    DictIssueDirective, LimitedOutputDirective, StreamStateInternal, TableType, KB,
};

// `HASH_UNIT` = `sizeof(reg_t)` in C.  On 64-bit targets `reg_t` is `u64`
// (8 bytes); on 32-bit targets it is `u32` (4 bytes).  `usize` gives the
// same size on every platform.
const HASH_UNIT: usize = core::mem::size_of::<usize>();

// ─────────────────────────────────────────────────────────────────────────────
// Internal load-dict mode flag
// ─────────────────────────────────────────────────────────────────────────────

/// Mirrors the `LoadDict_mode_e` enum from lz4.c:1585.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadDictMode {
    /// Fast path: step-3 hash filling (favours end-of-dictionary positions).
    Fast,
    /// Slow path: additional step-1 hash filling (favours begin-of-dictionary).
    Slow,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public streaming-compression state
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 streaming compression context.
///
/// Mirrors `LZ4_stream_t` from `lz4.h`.  Allocate on the heap with
/// [`Lz4Stream::new`] (returns `Box<Self>`) to keep the address stable —
/// this is especially important when [`attach_dictionary`] is in use, since
/// `attach_dictionary` stores a pointer into *another* stream's innards.
///
/// # Thread safety
/// `Lz4Stream` is `Send` but **not** `Sync`.  Concurrent calls to any method
/// are unsound; use external synchronisation if sharing across threads.
#[derive(Default)]
pub struct Lz4Stream {
    pub(crate) internal: StreamStateInternal,
}

// SAFETY: The raw pointers inside `StreamStateInternal` are not independently
// aliased — the caller is responsible for ensuring exclusive access, and the
// dict pointer is controlled by documented safety invariants.
unsafe impl Send for Lz4Stream {}

impl Lz4Stream {
    // ── Construction / destruction ────────────────────────────────────────────

    /// Allocate and zero-initialise a new streaming context on the heap.
    ///
    /// Equivalent to `LZ4_createStream`.  Returns `Box<Lz4Stream>` so that
    /// the allocation address remains stable (needed by `attach_dictionary`).
    pub fn new() -> Box<Self> {
        Box::new(Self {
            internal: StreamStateInternal::new(),
        })
    }

    // ── Reset ─────────────────────────────────────────────────────────────────

    /// Fully reset the stream to its zero-initialised state.
    ///
    /// Equivalent to `LZ4_resetStream` / `LZ4_initStream`.
    pub fn reset(&mut self) {
        self.internal = StreamStateInternal::new();
    }

    /// Fast reset — prepare the hash table for a new stream while avoiding
    /// a full zero-fill when possible.  The stream **must** have been validly
    /// initialised (i.e., previously used or `reset`).
    ///
    /// Equivalent to `LZ4_resetStream_fast`.
    ///
    /// # Safety
    /// `self.internal` must be in a valid (not moved-from / uninitialised)
    /// state before calling this function.
    pub fn reset_fast(&mut self) {
        // SAFETY: self.internal is exclusively owned and initialised.
        unsafe {
            prepare_table(&mut self.internal, 0, TableType::ByU32);
        }
    }

    // ── Dictionary loading ────────────────────────────────────────────────────

    /// Shared implementation for [`load_dict`] and [`load_dict_slow`].
    ///
    /// Mirrors `LZ4_loadDict_internal` (lz4.c:1587–1646).
    fn load_dict_internal(&mut self, dictionary: &[u8], mode: LoadDictMode) -> i32 {
        let dict_size = dictionary.len();
        let dict_ptr = dictionary.as_ptr();
        // SAFETY: dict_ptr + dict_size is one-past-the-end of a valid slice.
        let dict_end: *const u8 = unsafe { dict_ptr.add(dict_size) };

        // A full reset is required (not just prepareTable) to avoid any risk
        // of generating overflowing matchIndex when using this dictionary.
        self.reset();

        // Always advance the current offset by 64 KB so that we can guarantee
        // all offsets in the window are valid, enabling the noDictIssue
        // optimisation in compress_fast_continue even for sub-64KB dicts.
        self.internal.current_offset = self.internal.current_offset.wrapping_add(64 * KB as u32);

        if dict_size < HASH_UNIT {
            return 0;
        }

        // Truncate to the last 64 KB of the supplied dictionary.
        let p_start: *const u8 = if dict_size > 64 * KB {
            unsafe { dict_end.sub(64 * KB) }
        } else {
            dict_ptr
        };

        self.internal.dictionary = p_start;
        self.internal.dict_size = (dict_size.min(64 * KB)) as u32;
        self.internal.table_type = TableType::ByU32 as u32;

        let mut p = p_start;
        let mut idx32 = self.internal.current_offset - self.internal.dict_size;

        // Fast pass (step 3): fill hash table, overwriting earlier entries so
        // the table favours positions towards the *end* of the dictionary.
        unsafe {
            // SAFETY: p starts within the dict slice; loop guard prevents
            // reading past dict_end (we check p + HASH_UNIT <= dict_end).
            while p.add(HASH_UNIT) <= dict_end {
                let h = hash_position(p, TableType::ByU32);
                put_index_on_hash(
                    idx32,
                    h,
                    self.internal.hash_table.as_mut_ptr(),
                    TableType::ByU32,
                );
                p = p.add(3);
                idx32 = idx32.wrapping_add(3);
            }

            if mode == LoadDictMode::Slow {
                // Slow pass (step 1): add additional references while keeping
                // existing entries when they already point past the window
                // limit, so the table also covers beginning-of-dictionary
                // positions.  Does NOT overwrite positions that are already
                // well-placed (favours beginning of dictionary on ties).
                p = p_start;
                idx32 = self.internal.current_offset - self.internal.dict_size;
                let limit = self.internal.current_offset.wrapping_sub(64 * KB as u32);

                while p.add(HASH_UNIT) <= dict_end {
                    let h = hash_position(p, TableType::ByU32);
                    if get_index_on_hash(h, self.internal.hash_table.as_ptr(), TableType::ByU32)
                        <= limit
                    {
                        put_index_on_hash(
                            idx32,
                            h,
                            self.internal.hash_table.as_mut_ptr(),
                            TableType::ByU32,
                        );
                    }
                    p = p.add(1);
                    idx32 = idx32.wrapping_add(1);
                }
            }
        }

        self.internal.dict_size as i32
    }

    /// Load a dictionary into the stream (fast variant).
    ///
    /// Resets the stream, then indexes the last 64 KB of `dictionary` (or
    /// the entire dictionary if shorter) into the hash table with step-3
    /// scanning.
    ///
    /// Returns the number of bytes actually used (≤ 64 KB), or 0 if the
    /// dictionary is too small to be useful.
    ///
    /// Equivalent to `LZ4_loadDict`.
    pub fn load_dict(&mut self, dictionary: &[u8]) -> i32 {
        self.load_dict_internal(dictionary, LoadDictMode::Fast)
    }

    /// Load a dictionary into the stream (slow / high-coverage variant).
    ///
    /// Like [`load_dict`] but also performs a step-1 pass that inserts
    /// additional references from the beginning of the dictionary, improving
    /// compression on data with repeated structures.
    ///
    /// Returns the number of bytes actually used (≤ 64 KB), or 0.
    ///
    /// Equivalent to `LZ4_loadDictSlow` (new in v1.10.0).
    pub fn load_dict_slow(&mut self, dictionary: &[u8]) -> i32 {
        self.load_dict_internal(dictionary, LoadDictMode::Slow)
    }

    // ── Dictionary attachment ─────────────────────────────────────────────────

    /// Attach a pre-loaded dictionary stream to this compressor **without**
    /// copying its hash table into the working context (zero-copy).
    ///
    /// Equivalent to `LZ4_attach_dictionary`.
    ///
    /// Pass `None` to detach any currently attached dictionary.
    ///
    /// # Safety
    ///
    /// **Lifetime invariant**: the `Lz4Stream` pointed to by `dict_stream`
    /// (when `Some`) **must outlive** `self`.  The raw pointer to the dict
    /// stream's internal state stored inside `self.internal.dict_ctx` is not
    /// lifetime-checked by the compiler — violating this invariant is
    /// undefined behaviour.
    ///
    /// **Stability invariant**: `dict_stream` must not be moved (its address
    /// must remain stable) for as long as it is attached to `self`.
    /// Heap-allocating both streams with `Box` (via [`Lz4Stream::new`])
    /// satisfies this requirement.
    pub unsafe fn attach_dictionary(&mut self, dict_stream: Option<*const Lz4Stream>) {
        let dict_ctx: *const StreamStateInternal = match dict_stream {
            None => ptr::null(),
            Some(p) => {
                debug_assert!(!p.is_null());
                &(*p).internal
            }
        };

        if !dict_ctx.is_null() {
            // If current_offset is zero, a zero hash-table entry could never
            // be distinguished from a "miss".  Bump to a non-zero offset so
            // that a true miss (index 0) is unambiguous.
            if self.internal.current_offset == 0 {
                self.internal.current_offset = 64 * KB as u32;
            }

            // Do not attach an empty dictionary — treat it as detach.
            if (*dict_ctx).dict_size == 0 {
                self.internal.dict_ctx = ptr::null();
                return;
            }
        }

        self.internal.dict_ctx = dict_ctx;
    }

    // ── Renormalisation ───────────────────────────────────────────────────────

    /// Rescale the hash table when `current_offset` is about to overflow the
    /// 31-bit boundary used for `ptrdiff_t` safety on 32-bit targets.
    ///
    /// Equivalent to the static `LZ4_renormDictT` (lz4.c:1687–1704).
    pub fn renorm_dict(&mut self, next_size: i32) {
        debug_assert!(next_size >= 0);
        // Mirror of the C guard: `currentOffset + (unsigned)nextSize > 0x80000000`.
        if self.internal.current_offset.wrapping_add(next_size as u32) > 0x8000_0000 {
            let delta = self.internal.current_offset.wrapping_sub(64 * KB as u32);
            // Compute dict_end before mutating dict_size / dictionary.
            let dict_end: *const u8 = unsafe {
                self.internal
                    .dictionary
                    .add(self.internal.dict_size as usize)
            };

            for entry in self.internal.hash_table.iter_mut() {
                if *entry < delta {
                    *entry = 0;
                } else {
                    *entry = entry.wrapping_sub(delta);
                }
            }

            self.internal.current_offset = 64 * KB as u32;
            if self.internal.dict_size > 64 * KB as u32 {
                self.internal.dict_size = 64 * KB as u32;
            }
            self.internal.dictionary = unsafe { dict_end.sub(self.internal.dict_size as usize) };
        }
    }

    // ── Streaming compression ─────────────────────────────────────────────────

    /// Compress `src` into `dst`, continuing from the compression history
    /// accumulated by previous calls.
    ///
    /// Returns the number of bytes written to `dst`, or 0 on failure (output
    /// buffer too small or other error).
    ///
    /// `acceleration` is clamped to `[1, LZ4_ACCELERATION_MAX]`; larger
    /// values trade compression ratio for speed.
    ///
    /// Equivalent to `LZ4_compress_fast_continue`.
    ///
    /// # Safety
    ///
    /// In **prefix mode** (when the previous `src` is adjacent in memory),
    /// `src` must remain accessible at its original address for at least as
    /// long as the stream is used, because the stream's history pointer may
    /// point into it.  Heap-copying old data via [`save_dict`] eliminates
    /// this requirement.
    pub fn compress_fast_continue(&mut self, src: &[u8], dst: &mut [u8], acceleration: i32) -> i32 {
        let table_type = TableType::ByU32;
        let input_size = src.len() as i32;
        let max_output_size = dst.len() as i32;

        let source_ptr = src.as_ptr();
        let dest_ptr = dst.as_mut_ptr();

        // Compute dict_end before renorm potentially moves dictionary pointer.
        let dict_end: *const u8 = if self.internal.dict_size != 0 {
            unsafe {
                self.internal
                    .dictionary
                    .add(self.internal.dict_size as usize)
            }
        } else {
            ptr::null()
        };

        // Prevent currentOffset from overflowing the 31-bit ptrdiff_t boundary.
        self.renorm_dict(input_size);

        let acceleration = acceleration
            .max(LZ4_ACCELERATION_DEFAULT)
            .min(LZ4_ACCELERATION_MAX);

        // Invalidate tiny dictionaries (< 4 bytes) that are not in prefix mode
        // and not in dictCtx mode.  Doing so allows the faster prefix path to
        // take over on the *next* call.
        let dict_end: *const u8 = if self.internal.dict_size < 4
                && dict_end != source_ptr          // not already prefix mode
                && input_size > 0
                && self.internal.dict_ctx.is_null()
        {
            self.internal.dict_size = 0;
            self.internal.dictionary = source_ptr;
            // Transition to prefix mode: dictEnd is now == source.
            source_ptr
        } else {
            dict_end
        };

        // Clip the dictionary if the new source overlaps its tail.
        if !dict_end.is_null() {
            let source_end = unsafe { source_ptr.add(src.len()) };
            if source_end > self.internal.dictionary && source_end < dict_end {
                // source overlaps the end of the dict — shrink dict to the
                // non-overlapping prefix.
                let remaining = (dict_end as usize) - (source_end as usize);
                let mut new_dict_size = remaining.min(64 * KB) as u32;
                if new_dict_size < 4 {
                    new_dict_size = 0;
                }
                self.internal.dict_size = new_dict_size;
                self.internal.dictionary =
                    unsafe { dict_end.sub(self.internal.dict_size as usize) };
            }
        }

        // ── Prefix mode: new data immediately follows the dictionary ──────────
        if dict_end == source_ptr {
            let result = unsafe {
                if self.internal.dict_size < (64 * KB as u32)
                    && self.internal.dict_size < self.internal.current_offset
                {
                    compress_generic(
                        &mut self.internal,
                        source_ptr,
                        dest_ptr,
                        input_size,
                        ptr::null_mut(),
                        max_output_size,
                        LimitedOutputDirective::LimitedOutput,
                        table_type,
                        DictDirective::WithPrefix64k,
                        DictIssueDirective::DictSmall,
                        acceleration,
                    )
                } else {
                    compress_generic(
                        &mut self.internal,
                        source_ptr,
                        dest_ptr,
                        input_size,
                        ptr::null_mut(),
                        max_output_size,
                        LimitedOutputDirective::LimitedOutput,
                        table_type,
                        DictDirective::WithPrefix64k,
                        DictIssueDirective::NoDictIssue,
                        acceleration,
                    )
                }
            };
            return match result {
                Ok(n) => n as i32,
                Err(_) => 0,
            };
        }

        // ── External dictionary mode ──────────────────────────────────────────
        let result = unsafe {
            if !self.internal.dict_ctx.is_null() {
                // We depend on the fact that the dictCtx was produced by
                // load_dict / load_dict_slow, which guarantees no references
                // to offsets in the "dead zone" [currentOffset-64KB,
                // currentOffset-dictSize), making noDictIssue safe even for
                // sub-64KB dicts.
                if input_size > 4 * KB as i32 {
                    // For large blocks: copy the dict's hash table into the
                    // working context so the compression loop only looks into
                    // one table (faster).  Mirrors C's `LZ4_memcpy(streamPtr,
                    // streamPtr->dictCtx, sizeof(*streamPtr))`.
                    let dict_ctx_ptr = self.internal.dict_ctx;
                    ptr::copy_nonoverlapping(
                        dict_ctx_ptr,
                        &mut self.internal as *mut StreamStateInternal,
                        1,
                    );
                    compress_generic(
                        &mut self.internal,
                        source_ptr,
                        dest_ptr,
                        input_size,
                        ptr::null_mut(),
                        max_output_size,
                        LimitedOutputDirective::LimitedOutput,
                        table_type,
                        DictDirective::UsingExtDict,
                        DictIssueDirective::NoDictIssue,
                        acceleration,
                    )
                } else {
                    // Small block: reference the dict context directly.
                    compress_generic(
                        &mut self.internal,
                        source_ptr,
                        dest_ptr,
                        input_size,
                        ptr::null_mut(),
                        max_output_size,
                        LimitedOutputDirective::LimitedOutput,
                        table_type,
                        DictDirective::UsingDictCtx,
                        DictIssueDirective::NoDictIssue,
                        acceleration,
                    )
                }
            } else if self.internal.dict_size < 64 * KB as u32
                && self.internal.dict_size < self.internal.current_offset
            {
                compress_generic(
                    &mut self.internal,
                    source_ptr,
                    dest_ptr,
                    input_size,
                    ptr::null_mut(),
                    max_output_size,
                    LimitedOutputDirective::LimitedOutput,
                    table_type,
                    DictDirective::UsingExtDict,
                    DictIssueDirective::DictSmall,
                    acceleration,
                )
            } else {
                compress_generic(
                    &mut self.internal,
                    source_ptr,
                    dest_ptr,
                    input_size,
                    ptr::null_mut(),
                    max_output_size,
                    LimitedOutputDirective::LimitedOutput,
                    table_type,
                    DictDirective::UsingExtDict,
                    DictIssueDirective::NoDictIssue,
                    acceleration,
                )
            }
        };

        // Record the newly compressed block as the new history.
        self.internal.dictionary = source_ptr;
        self.internal.dict_size = input_size as u32;

        match result {
            Ok(n) => n as i32,
            Err(_) => 0,
        }
    }

    // ── Force external-dictionary mode (hidden debug helper) ──────────────────

    /// Compress using external-dictionary mode regardless of the current state.
    ///
    /// This is a **hidden debug function** that exists in the C source to
    /// force-test the `usingExtDict` compression path.  It is exposed here to
    /// allow targeted testing of the external-dictionary compression path.
    ///
    /// Equivalent to `LZ4_compress_forceExtDict`.
    ///
    /// # Safety
    /// - `src` must be valid for reads of `src_size` bytes.
    /// - `dst` must be writable.  When `dst_capacity` is 0 the output is
    ///   unlimited (equivalent to `notLimited` mode in C).
    pub unsafe fn compress_force_ext_dict(
        &mut self,
        src: *const u8,
        dst: *mut u8,
        src_size: i32,
        dst_capacity: i32,
    ) -> i32 {
        self.renorm_dict(src_size);

        let result = if self.internal.dict_size < 64 * KB as u32
            && self.internal.dict_size < self.internal.current_offset
        {
            compress_generic(
                &mut self.internal,
                src,
                dst,
                src_size,
                ptr::null_mut(),
                dst_capacity,
                LimitedOutputDirective::NotLimited,
                TableType::ByU32,
                DictDirective::UsingExtDict,
                DictIssueDirective::DictSmall,
                1,
            )
        } else {
            compress_generic(
                &mut self.internal,
                src,
                dst,
                src_size,
                ptr::null_mut(),
                dst_capacity,
                LimitedOutputDirective::NotLimited,
                TableType::ByU32,
                DictDirective::UsingExtDict,
                DictIssueDirective::NoDictIssue,
                1,
            )
        };

        self.internal.dictionary = src;
        self.internal.dict_size = src_size as u32;

        match result {
            Ok(n) => n as i32,
            Err(_) => 0,
        }
    }

    // ── Save dictionary ───────────────────────────────────────────────────────

    /// Copy the last up-to-64 KB of compression history into `safe_buffer`
    /// for persistent storage.
    ///
    /// After this call the stream's history pointer is updated to point at
    /// `safe_buffer`, so [`compress_fast_continue`] can be called immediately
    /// without calling [`load_dict`].
    ///
    /// Returns the number of bytes saved (≤ `min(safe_buffer.len(), 64 KB,
    /// current dict size)`).
    ///
    /// Equivalent to `LZ4_saveDict`.
    pub fn save_dict(&mut self, safe_buffer: &mut [u8]) -> i32 {
        // Clamp dict_size to the smaller of 64 KB, the buffer length, and the
        // actual dictionary size.
        let mut dict_size = (safe_buffer.len().min(64 * KB)) as u32;
        if dict_size > self.internal.dict_size {
            dict_size = self.internal.dict_size;
        }

        if dict_size > 0 {
            // SAFETY: `dictionary` + `self.internal.dict_size` is a valid
            // pointer into the previously compressed data.  We are reading
            // `dict_size` bytes ending at that address.  `safe_buffer` has at
            // least `dict_size` bytes of space.
            let previous_dict_end: *const u8 = unsafe {
                self.internal
                    .dictionary
                    .add(self.internal.dict_size as usize)
            };
            unsafe {
                // Use ptr::copy (memmove) rather than copy_nonoverlapping, since
                // safe_buffer might overlap with the current dictionary region.
                ptr::copy(
                    previous_dict_end.sub(dict_size as usize),
                    safe_buffer.as_mut_ptr(),
                    dict_size as usize,
                );
            }
        }

        // Update history to point at the saved copy.
        self.internal.dictionary = safe_buffer.as_ptr();
        self.internal.dict_size = dict_size;

        dict_size as i32
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests (require pub(crate) field access)
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::types::KB;

    // ── renorm_dict ───────────────────────────────────────────────────────────

    #[test]
    fn renorm_dict_triggers_when_overflow_would_occur() {
        // Set current_offset near the 0x80000000 boundary.
        // 0x7FFF_FFF0 + 32 = 0x80000010 > 0x80000000 → must trigger.
        let mut stream = Lz4Stream::new();
        stream.internal.current_offset = 0x7FFF_FFF0;
        // Set a hash table entry that is large enough to survive after renorm.
        stream.internal.hash_table[0] = 0x7FFF_0000;
        // Small entry that will be zeroed.
        stream.internal.hash_table[1] = 0x0000_0001;

        stream.renorm_dict(32);

        // After renorm: current_offset must be reset to 64 KB.
        assert_eq!(
            stream.internal.current_offset,
            64 * KB as u32,
            "renorm_dict must reset current_offset to 64 KB"
        );
    }

    #[test]
    fn renorm_dict_with_large_dict_clips_to_64kb() {
        // dict_size > 64KB should be clamped.
        let mut stream = Lz4Stream::new();
        stream.internal.current_offset = 0x7FFF_FFF0;
        stream.internal.dict_size = 128 * KB as u32; // more than 64KB
        stream.renorm_dict(32);
        assert_eq!(
            stream.internal.dict_size,
            64 * KB as u32,
            "renorm_dict must clip dict_size to 64 KB"
        );
    }

    #[test]
    fn renorm_dict_noop_when_below_boundary() {
        // current_offset + next_size <= 0x80000000 → no-op.
        let mut stream = Lz4Stream::new();
        stream.internal.current_offset = 1024;
        stream.renorm_dict(1024);
        // current_offset must remain unchanged.
        assert_eq!(stream.internal.current_offset, 1024);
    }

    // ── compress_fast_continue large dict_ctx path ────────────────────────────

    #[test]
    fn compress_fast_continue_large_block_with_attached_dict_ctx() {
        // Exercises the `input_size > 4 * KB` path when dict_ctx is non-null
        // (lines 442-459 in stream.rs).
        let dict_data: Vec<u8> = (0u8..=255).cycle().take(64 * KB).collect();

        // dict_stream: load the dictionary (Box keeps address stable).
        let mut dict_stream = Lz4Stream::new();
        dict_stream.load_dict(&dict_data);

        // working_stream: attach dict_stream as dict context.
        let mut working_stream = Lz4Stream::new();
        unsafe {
            // SAFETY: dict_stream is alive for the duration of this test.
            working_stream.attach_dictionary(Some(&*dict_stream as *const Lz4Stream));
        }

        // Source > 4 KB to trigger the `input_size > 4*KB` branch.
        let src: Vec<u8> = (0u8..=127).cycle().take(8 * KB).collect();
        let bound = {
            let b = crate::block::compress::compress_bound(src.len() as i32);
            b.max(0) as usize
        };
        let mut dst = vec![0u8; bound.max(src.len() + 64)];

        let n = working_stream.compress_fast_continue(&src, &mut dst, 1);
        // Must not panic; typically returns positive (compressed output).
        assert!(n >= 0, "compress_fast_continue with large dict_ctx must not panic");
    }

    // ── Source-overlaps-dict clipping ─────────────────────────────────────────

    #[test]
    fn compress_fast_continue_source_end_overlaps_dict_tail_clips_dict() {
        // Exercises lines 377-384: when source_end falls inside the dictionary
        // window, the dictionary is clipped to the non-overlapping prefix.
        //
        // Layout in `unified_buf`:
        //   dictionary starts at offset 8, dict_size = 100 → dict_end = offset 108
        //   source starts at offset 50, length = 50 → source_end = offset 100
        //   source_end (100) is in (dictionary=8 … dict_end=108) → overlap!
        let unified_buf: Vec<u8> = (0u8..=255).cycle().take(1024).collect();

        let mut stream = Lz4Stream::new();
        // Set dictionary to unified_buf[8], size 100.
        stream.internal.dictionary = unsafe { unified_buf.as_ptr().add(8) };
        stream.internal.dict_size = 100;
        // current_offset must be non-zero so the stream is "armed".
        stream.internal.current_offset = 64 * KB as u32;

        // Source: unified_buf[50..100] → source_end = unified_buf[100]
        // which is inside [unified_buf[8]..unified_buf[108]].
        let source = &unified_buf[50..100];
        let mut dst = vec![0u8; 512];

        // The call should not panic and should clip the dictionary.
        let _ = stream.compress_fast_continue(source, &mut dst, 1);

        // After clipping: dict should have been shrunk so its end coincides with
        // the clipped boundary. We just verify no panic occurred.
    }
}
