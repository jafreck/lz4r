//! Public API surface for LZ4 file I/O operations.
//!
//! This module assembles the `lz4io` sub-modules and re-exports the symbols
//! consumed by the CLI and library users.  The organisation mirrors `lz4io.h`
//! from the LZ4 reference implementation.

pub mod compress_frame;
pub mod compress_legacy;
pub mod compress_mt;
pub mod decompress_dispatch;
pub mod decompress_frame;
pub mod decompress_legacy;
pub mod decompress_resources;
pub mod file_info;
pub mod file_io;
pub mod prefs;
pub mod sparse;

// ── Core type re-exports (lz4io.h public surface) ────────────────────────────
pub use file_info::CompressedFileInfo;
pub use prefs::Prefs;

// ── Special I/O sentinels (mirrors lz4io.h #defines) ─────────────────────────
pub use file_io::{NULL_OUTPUT, NUL_MARK, STDIN_MARK, STDOUT_MARK};

// ── Magic number constants ────────────────────────────────────────────────────
pub use prefs::{LEGACY_MAGICNUMBER, LZ4IO_MAGICNUMBER, LZ4IO_SKIPPABLE0, LZ4IO_SKIPPABLEMASK};

// ── Notification level (global, mirrors g_displayLevel) ──────────────────────
/// Set the global display/notification level. Mirrors `LZ4IO_setNotificationLevel`.
pub use prefs::set_notification_level;

// ── Worker count ──────────────────────────────────────────────────────────────
/// Returns the default number of compression workers. Mirrors `LZ4IO_defaultNbWorkers`.
pub use prefs::default_nb_workers;

// ── Compression public API (mirrors lz4io.h) ─────────────────────────────────
/// Compress a single file. Mirrors `LZ4IO_compressFilename`.
pub use compress_frame::compress_filename;

/// Compress multiple files with a given suffix. Mirrors `LZ4IO_compressMultipleFilenames`.
pub use compress_frame::compress_multiple_filenames;

// ── Legacy LZ4 frame format compression ──────────────────────────────────────────
/// Compress a single file using the legacy LZ4 frame format.
pub use compress_legacy::compress_filename_legacy;

/// Compress multiple files using the legacy LZ4 frame format.
pub use compress_legacy::compress_multiple_filenames_legacy;

// ── Decompression public API (mirrors lz4io.h) ───────────────────────────────
/// Decompress a single file. Mirrors `LZ4IO_decompressFilename`.
pub use decompress_dispatch::decompress_filename;

/// Decompress multiple files. Mirrors `LZ4IO_decompressMultipleFilenames`.
pub use decompress_dispatch::decompress_multiple_filenames;

// ── File info / --list (mirrors lz4io.h) ─────────────────────────────────────
/// Print `--list` metadata for compressed files. Mirrors `LZ4IO_displayCompressedFilesInfo`.
pub use file_info::display_compressed_files_info;
