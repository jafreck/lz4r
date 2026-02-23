// config.rs — Compile-time configuration constants.
// Migrated from lz4conf.h and platform.h (lz4-1.10.0/programs).
//
// Platform-detection macros from platform.h (__64BIT__, _FILE_OFFSET_BITS,
// _LARGEFILE_SOURCE, PLATFORM_POSIX_VERSION, SET_BINARY_MODE,
// SET_SPARSE_FILE_MODE) are not needed in Rust: Rust handles 64-bit sizes
// natively, file I/O does not require binary-mode toggling, and sparse-file
// detection is handled by build.rs via `#[cfg(has_sparse_files)]`.
//
// IS_CONSOLE(stream) is provided by std::io::IsTerminal (Rust 1.70+) at each
// call site and does not need a constant here.

// Default compression level.
// Corresponds to LZ4_CLEVEL_DEFAULT in lz4conf.h.
// Can be overridden by the LZ4_CLEVEL environment variable at runtime,
// or by the -# command-line flag.
pub const CLEVEL_DEFAULT: i32 = 1;

// Whether multi-threaded compression is compiled in.
// Corresponds to LZ4IO_MULTITHREAD in lz4conf.h.
// In C: defaults to 1 on Windows (Completion Ports available), 0 elsewhere.
// Here: true on Windows by default, or when the `multithread` Cargo feature is enabled.
pub const MULTITHREAD: bool = cfg!(target_os = "windows") || cfg!(feature = "multithread");

// Default number of worker threads.
// Corresponds to LZ4_NBWORKERS_DEFAULT in lz4conf.h (C source value: 0 = auto-detect).
// Migration acceptance criteria intentionally diverges from C source and specifies 4.
// Can be overridden by the LZ4_NBWORKERS environment variable,
// or by the -T# command-line flag.
pub const NB_WORKERS_DEFAULT: usize = 4;

// Maximum number of compression worker threads selectable at runtime.
// Corresponds to LZ4_NBWORKERS_MAX in lz4conf.h.
pub const NB_WORKERS_MAX: usize = 200;

// Default block size ID (7 = 4 MB blocks).
// Corresponds to LZ4_BLOCKSIZEID_DEFAULT in lz4conf.h.
// Can be overridden at runtime using the -B# command-line flag.
pub const BLOCKSIZEID_DEFAULT: u32 = 7;
