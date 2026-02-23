//! I/O preferences, display globals, and timing helpers for the `io` subsystem.
//!
//! This module defines:
//!
//! - [`Prefs`] — a plain value type holding all tunable compression and
//!   decompression parameters (block size, checksum policy, worker count, etc.).
//! - [`DISPLAY_LEVEL`] / [`set_notification_level`] — an atomic global controlling
//!   how much diagnostic output the library emits to stderr.
//! - [`display_level`] — a conditional stderr printer keyed on that level.
//! - [`cpu_load_sec`] / [`final_time_display`] — platform-specific CPU-time
//!   accounting used to report compression and decompression throughput.
//! - Assorted numeric constants (magic numbers, buffer sizes, and SI units).

use std::sync::atomic::{AtomicI32, Ordering};

use crate::timefn::{clock_span_ns, DurationNs, TimeT};

// ---------------------------------------------------------------------------
// Numeric constants
// ---------------------------------------------------------------------------
pub const KB: usize = 1 << 10;
pub const MB: usize = 1 << 20;
pub const GB: usize = 1 << 30;

// ---------------------------------------------------------------------------
// Magic numbers
// ---------------------------------------------------------------------------
pub const MAGICNUMBER_SIZE: usize = 4;
pub const LZ4IO_MAGICNUMBER: u32 = 0x184D2204;
pub const LZ4IO_SKIPPABLE0: u32 = 0x184D2A50;
pub const LZ4IO_SKIPPABLEMASK: u32 = 0xFFFF_FFF0;
pub const LEGACY_MAGICNUMBER: u32 = 0x184C2102;

// ---------------------------------------------------------------------------
// Buffer-size and format constants
// ---------------------------------------------------------------------------
pub const CACHELINE: usize = 64;
pub const LEGACY_BLOCKSIZE: usize = 8 * MB;
pub const MIN_STREAM_BUFSIZE: usize = 192 * KB;
pub const LZ4IO_BLOCKSIZEID_DEFAULT: u32 = 7;
pub const LZ4_MAX_DICT_SIZE: usize = 64 * KB;

// ---------------------------------------------------------------------------
// Display / notification globals
// ---------------------------------------------------------------------------

/// Global notification level. 0 = silent, 1 = errors only, 2 = results +
/// warnings, 3 = progress, 4+ = verbose.
pub static DISPLAY_LEVEL: AtomicI32 = AtomicI32::new(0);

/// Refresh interval for progress updates (200 ms expressed as nanoseconds).
pub const REFRESH_RATE_NS: DurationNs = 200_000_000;

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Writes `msg` to stderr if the current notification level is ≥ `level`.
/// Flushes stderr unconditionally when the level is ≥ 4 to ensure progress
/// output is visible in real time.
#[inline]
pub fn display_level(level: i32, msg: &str) {
    if DISPLAY_LEVEL.load(Ordering::Relaxed) >= level {
        eprint!("{}", msg);
        if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 4 {
            // flush — best-effort; ignore errors
            use std::io::Write;
            let _ = std::io::stderr().flush();
        }
    }
}

// ---------------------------------------------------------------------------
// CPU-load helper
// ---------------------------------------------------------------------------

/// Returns the seconds of CPU time consumed since `cpu_start`.
///
/// On POSIX platforms reads `clock()` and divides by `CLOCKS_PER_SEC`
/// (1 000 000 on SUSv2/macOS).  On Windows reads kernel + user time from
/// `GetProcessTimes` in 100-nanosecond intervals and converts to seconds.
pub fn cpu_load_sec(cpu_start: libc::clock_t) -> f64 {
    #[cfg(not(target_os = "windows"))]
    {
        extern "C" {
            fn clock() -> libc::clock_t;
        }
        // CLOCKS_PER_SEC is 1_000_000 on POSIX (SUSv2) and macOS.
        const CLOCKS_PER_SEC: libc::clock_t = 1_000_000;
        let elapsed = unsafe { clock() } - cpu_start;
        elapsed as f64 / CLOCKS_PER_SEC as f64
    }
    #[cfg(target_os = "windows")]
    {
        // On Windows, ignore cpu_start and read from GetProcessTimes.
        // kernel_time and user_time are in 100-nanosecond intervals.
        use std::mem::MaybeUninit;
        unsafe {
            let process = winapi::um::processthreadsapi::GetCurrentProcess();
            let mut creation = MaybeUninit::uninit();
            let mut exit = MaybeUninit::uninit();
            let mut kernel = MaybeUninit::uninit();
            let mut user = MaybeUninit::uninit();
            winapi::um::processthreadsapi::GetProcessTimes(
                process,
                creation.as_mut_ptr(),
                exit.as_mut_ptr(),
                kernel.as_mut_ptr(),
                user.as_mut_ptr(),
            );
            let k = kernel.assume_init();
            let u = user.assume_init();
            // The high 32-bit word should never be set for realistic process
            // lifetimes; assert in debug builds to catch unexpected overflow.
            debug_assert_eq!(k.dwHighDateTime, 0, "kernel time dwHighDateTime unexpected non-zero");
            debug_assert_eq!(u.dwHighDateTime, 0, "user time dwHighDateTime unexpected non-zero");
            ((k.dwLowDateTime as f64) + (u.dwLowDateTime as f64)) * 100.0 / 1_000_000_000.0
        }
    }
}

// ---------------------------------------------------------------------------
// Final timing display
// ---------------------------------------------------------------------------

/// Prints a `"Done in … s ==> … MiB/s (cpu load: …%)"` line to stderr at
/// notification level 3.
///
/// `time_start` is a wall-clock snapshot taken before the operation began;
/// `cpu_start` is the corresponding `clock()` value; `size` is the number
/// of bytes processed.
pub fn final_time_display(time_start: TimeT, cpu_start: libc::clock_t, size: u64) {
    #[cfg(feature = "multithread")]
    {
        if !crate::timefn::support_mt_measurements() {
            display_level(
                5,
                "time measurements not compatible with multithreading \n",
            );
            return;
        }
    }
    let duration_ns = clock_span_ns(time_start);
    // Avoid division by zero: if duration is 0, treat it as 1 ns.
    let seconds = (duration_ns.max(1)) as f64 / 1_000_000_000.0_f64;
    let cpu_load_s = cpu_load_sec(cpu_start);
    let msg = format!(
        "Done in {:.2} s ==> {:.2} MiB/s  (cpu load : {:.0}%)\n",
        seconds,
        (size as f64) / seconds / 1024.0 / 1024.0,
        (cpu_load_s / seconds) * 100.0,
    );
    display_level(3, &msg);
}

// ---------------------------------------------------------------------------
// Block mode enum
// ---------------------------------------------------------------------------

/// Whether LZ4 blocks are linked (sharing a 64 KB sliding window) or
/// compressed independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockMode {
    /// Blocks share a 64 KB dictionary window with the preceding block.
    Linked = 0,
    /// Each block is compressed with no dependency on preceding blocks.
    Independent = 1,
}

// ---------------------------------------------------------------------------
// Preferences struct
// ---------------------------------------------------------------------------

/// All tunable parameters for LZ4 compression and decompression.
///
/// `Prefs` is a plain value type. Callers create one with [`Prefs::default`]
/// and apply setters as needed before passing it to the I/O routines.
#[derive(Clone, Debug)]
pub struct Prefs {
    /// Pass compressed data through without decompressing. Default: false.
    pub pass_through: bool,
    /// Overwrite existing destination files without prompting. Default: true.
    pub overwrite: bool,
    /// Test mode — decompress but discard output. Default: false.
    pub test_mode: bool,
    /// LZ4F block-size ID (4–7 corresponding to 64 KB – 4 MB). Default: 7.
    pub block_size_id: u32,
    /// Actual block size in bytes (0 = derive from block_size_id at use time). Default: 0.
    pub block_size: usize,
    /// Append a per-block xxHash32 checksum. Default: false.
    pub block_checksum: bool,
    /// Append a whole-stream xxHash32 checksum. Default: true.
    pub stream_checksum: bool,
    /// Compress blocks independently (`true`) or linked (`false`). Default: true.
    pub block_independence: bool,
    /// Sparse-file write support: 0 = off, 1 = auto, 2 = forced. Default: 1.
    pub sparse_file_support: i32,
    /// Embed uncompressed content size in the frame header. Default: false.
    pub content_size_flag: bool,
    /// Use a compression/decompression dictionary. Derived from `dictionary_filename`.
    pub use_dictionary: bool,
    /// Favour decompression speed over compression ratio (HC levels only). Default: false.
    pub favor_dec_speed: bool,
    /// Path to the dictionary file, if any.
    pub dictionary_filename: Option<String>,
    /// Remove source file after successful compression/decompression. Default: false.
    pub remove_src_file: bool,
    /// Number of worker threads for multi-threaded compression. Default: auto-detected.
    pub nb_workers: i32,
}

// ---------------------------------------------------------------------------
// Default worker-count calculation
// ---------------------------------------------------------------------------

/// Returns the default number of compression worker threads.
///
/// When the `multithread` feature is enabled, detects the number of physical
/// cores and reserves a fraction (`1 + cores >> 3`) for other system work,
/// returning at least 1.  Without the feature, always returns 1.
pub fn default_nb_workers() -> i32 {
    #[cfg(feature = "multithread")]
    {
        let nb_cores = num_cpus::get_physical() as i32;
        let spared = 1 + ((nb_cores as u32) >> 3) as i32;
        if nb_cores <= spared {
            1
        } else {
            nb_cores - spared
        }
    }
    #[cfg(not(feature = "multithread"))]
    {
        1
    }
}

// ---------------------------------------------------------------------------
// Default implementation
// ---------------------------------------------------------------------------

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            pass_through: false,
            overwrite: true,
            test_mode: false,
            block_size_id: LZ4IO_BLOCKSIZEID_DEFAULT,
            block_size: 0,
            block_checksum: false,
            stream_checksum: true,
            block_independence: true,
            sparse_file_support: 1,
            content_size_flag: false,
            use_dictionary: false,
            favor_dec_speed: false,
            dictionary_filename: None,
            remove_src_file: false,
            nb_workers: default_nb_workers(),
        }
    }
}

// ---------------------------------------------------------------------------
// Preference setters
// ---------------------------------------------------------------------------

impl Prefs {
    /// Creates a new `Prefs` with all defaults applied.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the number of worker threads, clamped to `[1, NB_WORKERS_MAX]`.
    /// Returns the actual value stored.
    pub fn set_nb_workers(&mut self, nb_workers: i32) -> i32 {
        let clamped = nb_workers
            .max(1)
            .min(crate::config::NB_WORKERS_MAX as i32);
        self.nb_workers = clamped;
        clamped
    }

    /// Sets the dictionary file path. Passing `None` clears the dictionary.
    /// Returns `true` if a dictionary is now active.
    pub fn set_dictionary_filename(&mut self, filename: Option<&str>) -> bool {
        self.dictionary_filename = filename.map(|s| s.to_owned());
        self.use_dictionary = self.dictionary_filename.is_some();
        self.use_dictionary
    }

    /// Enables or disables pass-through mode. Returns the new value.
    pub fn set_pass_through(&mut self, yes: bool) -> bool {
        self.pass_through = yes;
        yes
    }

    /// Enables or disables destination-file overwrite. Returns the new value.
    pub fn set_overwrite(&mut self, yes: bool) -> bool {
        self.overwrite = yes;
        yes
    }

    /// Enables or disables test mode (decompress and discard output). Returns the new value.
    pub fn set_test_mode(&mut self, yes: bool) -> bool {
        self.test_mode = yes;
        yes
    }

    /// Sets the block-size ID (4–7). Returns the corresponding block size in
    /// bytes, or 0 if the ID is out of range.
    ///
    /// Block-size table: 4 → 64 KB, 5 → 256 KB, 6 → 1 MB, 7 → 4 MB.
    pub fn set_block_size_id(&mut self, bsid: u32) -> usize {
        const BLOCK_SIZE_TABLE: [usize; 4] = [64 * KB, 256 * KB, MB, 4 * MB];
        const MIN_BSID: u32 = 4;
        const MAX_BSID: u32 = 7;
        if bsid < MIN_BSID || bsid > MAX_BSID {
            return 0;
        }
        self.block_size_id = bsid;
        self.block_size = BLOCK_SIZE_TABLE[(bsid - MIN_BSID) as usize];
        self.block_size
    }

    /// Sets the block size in bytes, clamped to `[32, 4 MB]`.
    /// Also derives and stores the closest standard block-size ID (4–7).
    /// Returns the clamped block size.
    pub fn set_block_size(&mut self, block_size: usize) -> usize {
        const MIN_BLOCK_SIZE: usize = 32;
        const MAX_BLOCK_SIZE: usize = 4 * MB;
        let block_size = block_size.max(MIN_BLOCK_SIZE).min(MAX_BLOCK_SIZE);
        self.block_size = block_size;
        // Count bit-pair positions to find the closest standard block-size ID.
        let mut bsid: u32 = 0;
        let mut bs = block_size - 1;
        while { bs >>= 2; bs != 0 } {
            bsid += 1;
        }
        if bsid < 7 {
            bsid = 7;
        }
        self.block_size_id = bsid - 3;
        block_size
    }

    /// Sets block linking mode. Returns `true` if blocks are now independent.
    pub fn set_block_mode(&mut self, mode: BlockMode) -> bool {
        self.block_independence = mode == BlockMode::Independent;
        self.block_independence
    }

    /// Enables or disables per-block checksums. Returns the new value.
    pub fn set_block_checksum_mode(&mut self, enable: bool) -> bool {
        self.block_checksum = enable;
        enable
    }

    /// Enables or disables the whole-stream checksum. Returns the new value.
    pub fn set_stream_checksum_mode(&mut self, enable: bool) -> bool {
        self.stream_checksum = enable;
        enable
    }

    /// Enables or disables forced sparse-file mode.
    /// Returns the internal sparse-file mode value: 0 = disabled, 2 = forced on.
    pub fn set_sparse_file(&mut self, enable: bool) -> i32 {
        self.sparse_file_support = if enable { 2 } else { 0 };
        self.sparse_file_support
    }

    /// Enables or disables embedding the uncompressed content size in the frame
    /// header. Returns the new value.
    pub fn set_content_size(&mut self, enable: bool) -> bool {
        self.content_size_flag = enable;
        enable
    }

    /// Enables or disables favour-decompression-speed mode.
    ///
    /// When enabled, the HC compressor optimises for decompression throughput
    /// at the cost of compression ratio. Has no effect at non-HC levels.
    pub fn favor_dec_speed(&mut self, favor: bool) {
        self.favor_dec_speed = favor;
    }

    /// Enables or disables removal of the source file after successful processing.
    pub fn set_remove_src_file(&mut self, flag: bool) {
        self.remove_src_file = flag;
    }
}

// ---------------------------------------------------------------------------
// Global notification-level setter
// ---------------------------------------------------------------------------

/// Sets the global notification level and returns the value stored.
pub fn set_notification_level(level: i32) -> i32 {
    DISPLAY_LEVEL.store(level, Ordering::Relaxed);
    level
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefs_fields() {
        let p = Prefs::default();
        assert!(!p.pass_through);
        assert!(p.overwrite);
        assert!(!p.test_mode);
        assert_eq!(p.block_size_id, LZ4IO_BLOCKSIZEID_DEFAULT);
        assert_eq!(p.block_size, 0);
        assert!(!p.block_checksum);
        assert!(p.stream_checksum);
        assert!(p.block_independence);
        assert_eq!(p.sparse_file_support, 1);
        assert!(!p.content_size_flag);
        assert!(!p.use_dictionary);
        assert!(!p.favor_dec_speed);
        assert!(p.dictionary_filename.is_none());
        assert!(!p.remove_src_file);
        assert!(p.nb_workers >= 1);
    }

    #[test]
    fn set_nb_workers_clamps() {
        let mut p = Prefs::default();
        assert_eq!(p.set_nb_workers(0), 1);
        assert_eq!(p.set_nb_workers(1000), crate::config::NB_WORKERS_MAX as i32);
        assert_eq!(p.set_nb_workers(4), 4);
    }

    #[test]
    fn set_block_size_id_valid() {
        let mut p = Prefs::default();
        assert_eq!(p.set_block_size_id(4), 64 * KB);
        assert_eq!(p.set_block_size_id(5), 256 * KB);
        assert_eq!(p.set_block_size_id(6), MB);
        assert_eq!(p.set_block_size_id(7), 4 * MB);
    }

    #[test]
    fn set_block_size_id_invalid() {
        let mut p = Prefs::default();
        assert_eq!(p.set_block_size_id(3), 0);
        assert_eq!(p.set_block_size_id(8), 0);
    }

    #[test]
    fn set_block_size_clamps() {
        let mut p = Prefs::default();
        let s = p.set_block_size(10); // below min → 32
        assert_eq!(s, 32);
        let s = p.set_block_size(100 * MB); // above max → 4 MB
        assert_eq!(s, 4 * MB);
    }

    #[test]
    fn set_sparse_file_returns_two_when_enabled() {
        let mut p = Prefs::default();
        assert_eq!(p.set_sparse_file(true), 2);
        assert_eq!(p.set_sparse_file(false), 0);
    }

    #[test]
    fn set_dictionary_filename() {
        let mut p = Prefs::default();
        assert!(p.set_dictionary_filename(Some("dict.lz4")));
        assert!(p.use_dictionary);
        assert_eq!(p.dictionary_filename.as_deref(), Some("dict.lz4"));
        p.set_dictionary_filename(None);
        assert!(!p.use_dictionary);
    }

    #[test]
    fn set_notification_level_updates_global() {
        set_notification_level(3);
        assert_eq!(DISPLAY_LEVEL.load(Ordering::Relaxed), 3);
        set_notification_level(0);
    }

    #[test]
    fn set_block_mode_independent() {
        let mut p = Prefs::default();
        assert!(p.set_block_mode(BlockMode::Independent));
        assert!(!p.set_block_mode(BlockMode::Linked));
    }
}
