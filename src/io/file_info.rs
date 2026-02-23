//! File information display for the `--list` flag.
//!
//! Walks the frames of one or more LZ4-compressed files without decompressing
//! them and prints a summary table: frame count, frame type, block
//! configuration, compressed/uncompressed sizes, and compression ratio.
//!
//! All three frame families are recognised: standard LZ4 frames
//! (`LZ4IO_MAGICNUMBER`), legacy frames, and skippable frames.
//!
//! Entry point: [`display_compressed_files_info`].

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::Ordering;

use crate::frame::types::{
    BlockChecksum, BlockMode, BlockSizeId, ContentChecksum, FrameInfo as NativeFrameInfo,
    FrameType as NativeFrameType,
};
use crate::frame::{lz4f_create_decompression_context, lz4f_get_frame_info, lz4f_header_size};

use crate::io::file_io::STDIN_MARK;
use crate::io::prefs::{
    DISPLAY_LEVEL, LEGACY_MAGICNUMBER, LZ4IO_MAGICNUMBER, LZ4IO_SKIPPABLE0, LZ4IO_SKIPPABLEMASK,
    MAGICNUMBER_SIZE, MB,
};

// ---------------------------------------------------------------------------
// LZ4 frame format constants (lz4frame.h)
// ---------------------------------------------------------------------------

/// Minimum LZ4 frame header size in bytes (includes 4-byte magic number).
const LZ4F_HEADER_SIZE_MIN: usize = 7;

/// Maximum LZ4 frame header size in bytes.
const LZ4F_HEADER_SIZE_MAX: usize = 19;

/// Size of a block header field in bytes.
const LZ4F_BLOCK_HEADER_SIZE: usize = 4;

/// Size of an optional per-block checksum in bytes.
const LZ4F_BLOCK_CHECKSUM_SIZE: usize = 4;

/// Size of the content checksum appended after the end mark.
const LZ4F_CONTENT_CHECKSUM_SIZE: usize = 4;

/// Legacy block header size (equals `MAGICNUMBER_SIZE` per C static assert).
const LEGACY_BLOCK_HEADER_SIZE: usize = 4;

/// Maximum block payload size for legacy frames (8 MiB).
const LEGACY_BLOCK_SIZE_MAX: usize = 8 * MB;

/// lz4frame library version passed to `lz4f_create_decompression_context`.
const LZ4F_VERSION: u32 = 100;

// ---------------------------------------------------------------------------
// FrameType
// ---------------------------------------------------------------------------

/// Classifies the type of a compressed frame encountered during `--list` scanning.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameType {
    Lz4Frame = 0,
    LegacyFrame,
    SkippableFrame,
}

impl FrameType {
    /// Human-readable label used in the printed summary table.
    fn name(self) -> &'static str {
        match self {
            FrameType::Lz4Frame => "LZ4Frame",
            FrameType::LegacyFrame => "LegacyFrame",
            FrameType::SkippableFrame => "SkippableFrame",
        }
    }
}

// ---------------------------------------------------------------------------
// FrameInfo
// ---------------------------------------------------------------------------

/// Combines native frame metadata with the local [`FrameType`] classifier.
struct FrameInfo {
    lz4_frame_info: NativeFrameInfo,
    frame_type: FrameType,
}

impl FrameInfo {
    /// Returns a zeroed instance: content size 0, standard LZ4 frame type, default block settings.
    fn new() -> Self {
        FrameInfo {
            lz4_frame_info: NativeFrameInfo {
                block_size_id: BlockSizeId::Max64Kb,
                block_mode: BlockMode::Linked,
                content_checksum_flag: ContentChecksum::Disabled,
                frame_type: NativeFrameType::Frame,
                content_size: 0,
                dict_id: 0,
                block_checksum_flag: BlockChecksum::Disabled,
            },
            frame_type: FrameType::Lz4Frame,
        }
    }
}

// ---------------------------------------------------------------------------
// CompressedFileInfo
// ---------------------------------------------------------------------------

/// Accumulated metadata about all frames in a single compressed file.
///
/// Populated by [`get_compressed_file_info`] as it walks the frame stream,
/// then consumed by [`display_compressed_files_info`] to format the table row.
pub struct CompressedFileInfo {
    /// Display name (basename of the file path).
    pub file_name: String,
    /// Total compressed file size in bytes.
    pub file_size: u64,
    /// Number of frames found.
    pub frame_count: u64,
    /// Summary of the last frame (content_size accumulates across all frames).
    frame_summary: FrameInfo,
    /// `true` if all frames share the same frame type.
    pub eq_frame_types: bool,
    /// `true` if all lz4-format frames share the same block size/mode.
    pub eq_block_types: bool,
    /// `true` if every frame reported a content size.
    pub all_content_size: bool,
}

impl CompressedFileInfo {
    /// Returns a zeroed, default-initialised instance.
    fn new() -> Self {
        CompressedFileInfo {
            file_name: String::new(),
            file_size: 0,
            frame_count: 0,
            frame_summary: FrameInfo::new(),
            eq_frame_types: true,
            eq_block_types: true,
            all_content_size: true,
        }
    }
}

// ---------------------------------------------------------------------------
// InfoResult
// ---------------------------------------------------------------------------

#[derive(PartialEq, Eq, Debug)]
enum InfoResult {
    Ok,
    FormatNotKnown,
    NotAFile,
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Reads four bytes from `buf` as a little-endian `u32`.
#[inline]
fn read_le32(buf: &[u8]) -> u32 {
    u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
}

/// Returns `true` if `magic` is in the LZ4 skippable-frame range.
#[inline]
fn is_skippable_magic_number(magic: u32) -> bool {
    (magic & LZ4IO_SKIPPABLEMASK) == LZ4IO_SKIPPABLE0
}

// ---------------------------------------------------------------------------
// skip_blocks_data
// ---------------------------------------------------------------------------

/// Reads block headers and seeks past block payloads for a single standard LZ4 frame.
///
/// Returns the total byte count of all blocks (headers + payloads + optional
/// per-block checksums + optional content checksum), or `0` on I/O error.
/// The file cursor must be positioned immediately after the frame header on entry.
fn skip_blocks_data(file: &mut fs::File, block_checksum: bool, content_checksum: bool) -> u64 {
    let mut buf = [0u8; LZ4F_BLOCK_HEADER_SIZE];
    let mut total: u64 = 0;
    loop {
        match file.read_exact(&mut buf) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return total,
            Err(_) => return 0,
            Ok(_) => {}
        }
        total += LZ4F_BLOCK_HEADER_SIZE as u64;

        let next_cblock_size = (read_le32(&buf) & 0x7FFF_FFFF) as u64;
        let next_block = next_cblock_size
            + if block_checksum {
                LZ4F_BLOCK_CHECKSUM_SIZE as u64
            } else {
                0
            };

        if next_cblock_size == 0 {
            // Reached EndMark
            if content_checksum {
                if file
                    .seek(SeekFrom::Current(LZ4F_CONTENT_CHECKSUM_SIZE as i64))
                    .is_err()
                {
                    return 0;
                }
                total += LZ4F_CONTENT_CHECKSUM_SIZE as u64;
            }
            break;
        }

        total += next_block;
        if file.seek(SeekFrom::Current(next_block as i64)).is_err() {
            return 0;
        }
    }
    total
}

// ---------------------------------------------------------------------------
// skip_legacy_blocks_data
// ---------------------------------------------------------------------------

/// Sentinel value returned by [`skip_legacy_blocks_data`] on I/O or format error.
const LEGACY_FRAME_UNDECODABLE: u64 = u64::MAX;

/// Reads legacy block headers and seeks past block payloads.
///
/// Returns the total byte count of all blocks (4-byte headers + payloads),
/// or [`LEGACY_FRAME_UNDECODABLE`] on I/O or format error.
/// The file cursor must be positioned immediately after the legacy magic number on entry.
fn skip_legacy_blocks_data(file: &mut fs::File) -> u64 {
    let mut buf = [0u8; LEGACY_BLOCK_HEADER_SIZE];
    let mut total: u64 = 0;
    loop {
        // Detect EOF before reading: try one byte first
        let first = file.read(&mut buf[..1]);
        match first {
            Ok(0) => return total, // clean EOF
            Ok(1) => {}
            Ok(_) => unreachable!(),
            Err(_) => return LEGACY_FRAME_UNDECODABLE,
        }
        // Read remaining 3 bytes
        match file.read_exact(&mut buf[1..4]) {
            Ok(()) => {}
            Err(_) => return LEGACY_FRAME_UNDECODABLE,
        }

        let next_cblock_size = read_le32(&buf);

        // If this looks like the start of a new frame, rewind and stop
        if next_cblock_size == LEGACY_MAGICNUMBER
            || next_cblock_size == LZ4IO_MAGICNUMBER
            || is_skippable_magic_number(next_cblock_size)
        {
            if file
                .seek(SeekFrom::Current(-(LEGACY_BLOCK_HEADER_SIZE as i64)))
                .is_err()
            {
                eprintln!("Error 37 : impossible to skip backward");
                std::process::exit(37);
            }
            break;
        }

        if next_cblock_size as usize > LEGACY_BLOCK_SIZE_MAX {
            if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 4 {
                eprintln!("Error : block in legacy frame is too large");
            }
            return LEGACY_FRAME_UNDECODABLE;
        }

        total += (LEGACY_BLOCK_HEADER_SIZE as u64) + (next_cblock_size as u64);
        if file
            .seek(SeekFrom::Current(next_cblock_size as i64))
            .is_err()
        {
            return LEGACY_FRAME_UNDECODABLE;
        }
    }
    total
}

// ---------------------------------------------------------------------------
// block_type_id
// ---------------------------------------------------------------------------

/// Returns a compact block-type label such as `"B7I"` or `"B4D"`.
///
/// Format: `B` + block-size digit (`4`–`7`, mapping 64 KB–4 MB)
/// + `I` (independent blocks) or `D` (dependent/linked blocks).
pub fn block_type_id(size_id: &BlockSizeId, block_mode: &BlockMode) -> String {
    let id_digit = match size_id {
        BlockSizeId::Max64Kb | BlockSizeId::Default => b'4',
        BlockSizeId::Max256Kb => b'5',
        BlockSizeId::Max1Mb => b'6',
        BlockSizeId::Max4Mb => b'7',
    };
    let mode_char = match block_mode {
        BlockMode::Independent => b'I',
        BlockMode::Linked => b'D',
    };
    // SAFETY: all bytes are printable ASCII
    String::from_utf8(vec![b'B', id_digit, mode_char]).unwrap()
}

// ---------------------------------------------------------------------------
// to_human
// ---------------------------------------------------------------------------

/// Formats a byte count using the largest applicable binary SI prefix (K/M/G/T/P/E/Z/Y)
/// with two decimal places, e.g. `"3.14M"`.
fn to_human(mut size: f64) -> String {
    const UNITS: &[char] = &['\0', 'K', 'M', 'G', 'T', 'P', 'E', 'Z', 'Y'];
    let mut i = 0usize;
    while size >= 1024.0 && i + 1 < UNITS.len() {
        size /= 1024.0;
        i += 1;
    }
    if UNITS[i] == '\0' {
        format!("{:.2}", size)
    } else {
        format!("{:.2}{}", size, UNITS[i])
    }
}

// ---------------------------------------------------------------------------
// base_name
// ---------------------------------------------------------------------------

/// Returns the filename component after the last `/` or `\` path separator,
/// or the full string if no separator is present.
fn base_name(path: &str) -> &str {
    path.rfind('/')
        .or_else(|| path.rfind('\\'))
        .map(|pos| &path[pos + 1..])
        .unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Platform check for stdin being a regular file
// ---------------------------------------------------------------------------

/// Returns `true` if file descriptor 0 (stdin) refers to a regular file
/// rather than a pipe, device, or socket.
#[cfg(unix)]
fn is_stdin_regular_file() -> bool {
    use nix::sys::stat::fstat;
    use std::os::unix::io::RawFd;
    match fstat(0 as RawFd) {
        Ok(stat) => (stat.st_mode & 0o0170000) == 0o0100000,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_stdin_regular_file() -> bool {
    false
}

// ---------------------------------------------------------------------------
// get_compressed_file_info
// ---------------------------------------------------------------------------

/// Walks every frame in `path` without decompressing any data.
///
/// Populates `cfinfo` with aggregate statistics: frame count, file size,
/// frame-type and block-type consistency flags, and accumulated content size.
/// When `display_now` is `true`, a detail row is printed for each frame
/// (verbose `--list` mode).
fn get_compressed_file_info(
    cfinfo: &mut CompressedFileInfo,
    path: &str,
    display_now: bool,
) -> InfoResult {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
                eprintln!("{}: {}", path, e);
            }
            return InfoResult::NotAFile;
        }
    };

    cfinfo.file_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    let mut result = InfoResult::FormatNotKnown;
    // Buffer large enough for the maximum LZ4 frame header
    let mut buf = [0u8; LZ4F_HEADER_SIZE_MAX];

    'frame_loop: loop {
        // Read magic number; Ok(0) == clean EOF
        let n = match file.read(&mut buf[..MAGICNUMBER_SIZE]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        result = InfoResult::FormatNotKnown;

        if n != MAGICNUMBER_SIZE {
            // C: END_PROCESS(40, "Unrecognized header : Magic Number unreadable")
            eprintln!("Error 40 : Unrecognized header : Magic Number unreadable");
            std::process::exit(40);
        }

        let mut magic = read_le32(&buf[..4]);
        if is_skippable_magic_number(magic) {
            magic = LZ4IO_SKIPPABLE0; // fold all skippable magic numbers
        }

        let mut frame_info = FrameInfo::new();

        match magic {
            // ---------------------------------------------------------------
            LZ4IO_MAGICNUMBER => {
                if cfinfo.frame_summary.frame_type != FrameType::Lz4Frame {
                    cfinfo.eq_frame_types = false;
                }

                // Read LZ4F_HEADER_SIZE_MIN - MAGICNUMBER_SIZE = 3 more bytes
                {
                    let n2 = match file.read(&mut buf[MAGICNUMBER_SIZE..LZ4F_HEADER_SIZE_MIN]) {
                        Ok(n) => n,
                        Err(_) => {
                            eprintln!("Error 71 : Error reading {}", path);
                            std::process::exit(71);
                        }
                    };
                    if n2 == 0 {
                        eprintln!("Error 71 : Error reading {}", path);
                        std::process::exit(71);
                    }
                }

                // Determine full header size from the first LZ4F_HEADER_SIZE_MIN bytes
                let h_size_raw = match lz4f_header_size(&buf[..LZ4F_HEADER_SIZE_MIN]) {
                    Ok(n) => n,
                    Err(_) => break 'frame_loop,
                };
                let mut h_size = h_size_raw;

                // If the header is larger than what we've already read, fetch the rest.
                // Condition mirrors the C code exactly (LZ4F_HEADER_SIZE_MIN + MAGICNUMBER_SIZE).
                if h_size > LZ4F_HEADER_SIZE_MIN + MAGICNUMBER_SIZE {
                    let extra = h_size - LZ4F_HEADER_SIZE_MIN;
                    let end = LZ4F_HEADER_SIZE_MIN + extra;
                    let n3 = match file.read(&mut buf[LZ4F_HEADER_SIZE_MIN..end]) {
                        Ok(n) => n,
                        Err(_) => {
                            eprintln!("Error 72 : Error reading {}", path);
                            std::process::exit(72);
                        }
                    };
                    if n3 == 0 {
                        eprintln!("Error 72 : Error reading {}", path);
                        std::process::exit(72);
                    }
                }

                // Create a native decompression context and extract frame info.
                let mut dctx = match lz4f_create_decompression_context(LZ4F_VERSION) {
                    Ok(ctx) => ctx,
                    Err(_) => break 'frame_loop,
                };
                let (native_fi, consumed, _hint) =
                    match lz4f_get_frame_info(&mut dctx, &buf[..h_size]) {
                        Ok(t) => t,
                        Err(_) => break 'frame_loop,
                    };
                // dctx is dropped here; the decompression context owns no file state.
                frame_info.lz4_frame_info = native_fi;
                h_size = consumed; // update to actual bytes consumed by frame header

                // Check block-type consistency across frames
                if cfinfo.frame_count != 0 {
                    let prev = &cfinfo.frame_summary.lz4_frame_info;
                    let curr = &frame_info.lz4_frame_info;
                    let size_changed = prev.block_size_id != curr.block_size_id;
                    let mode_changed = prev.block_mode != curr.block_mode;
                    if size_changed || mode_changed {
                        cfinfo.eq_block_types = false;
                    }
                }

                // Skip block data; file cursor is now after the frame header
                let block_checksum = matches!(
                    frame_info.lz4_frame_info.block_checksum_flag,
                    BlockChecksum::Enabled
                );
                let content_checksum = matches!(
                    frame_info.lz4_frame_info.content_checksum_flag,
                    ContentChecksum::Enabled
                );
                let total_blocks_size =
                    skip_blocks_data(&mut file, block_checksum, content_checksum);

                if total_blocks_size != 0 {
                    let b_type = block_type_id(
                        &frame_info.lz4_frame_info.block_size_id,
                        &frame_info.lz4_frame_info.block_mode,
                    );
                    let checksum_str = if content_checksum { "XXH32" } else { "-" };
                    if display_now {
                        print!(
                            "    {:>6} {:>14} {:>5} {:>8}",
                            cfinfo.frame_count + 1,
                            frame_info.frame_type.name(),
                            b_type,
                            checksum_str
                        );
                    }

                    if frame_info.lz4_frame_info.content_size != 0 {
                        let compressed = total_blocks_size + h_size as u64;
                        let uncompressed = frame_info.lz4_frame_info.content_size;
                        let ratio = compressed as f64 / uncompressed as f64 * 100.0;
                        if display_now {
                            println!(" {:>20} {:>20} {:>9.2}%", compressed, uncompressed, ratio);
                        }
                        // Accumulate running content-size total into frame_info before
                        // moving it into cfinfo.frame_summary at the end of the loop.
                        frame_info.lz4_frame_info.content_size +=
                            cfinfo.frame_summary.lz4_frame_info.content_size;
                    } else {
                        if display_now {
                            println!(
                                " {:>20} {:>20} {:>9} ",
                                total_blocks_size + h_size as u64,
                                "-",
                                "-"
                            );
                        }
                        cfinfo.all_content_size = false;
                    }
                    result = InfoResult::Ok;
                }
            }

            // ---------------------------------------------------------------
            LEGACY_MAGICNUMBER => {
                frame_info.frame_type = FrameType::LegacyFrame;
                if cfinfo.frame_summary.frame_type != FrameType::LegacyFrame
                    && cfinfo.frame_count != 0
                {
                    cfinfo.eq_frame_types = false;
                }
                cfinfo.eq_block_types = false;
                cfinfo.all_content_size = false;

                let total_blocks_size = skip_legacy_blocks_data(&mut file);
                if total_blocks_size == LEGACY_FRAME_UNDECODABLE {
                    if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
                        eprintln!("Corrupted legacy frame");
                    }
                    result = InfoResult::FormatNotKnown;
                    break 'frame_loop;
                }
                if total_blocks_size != 0 {
                    if display_now {
                        println!(
                            "    {:>6} {:>14} {:>5} {:>8} {:>20} {:>20} {:>9}",
                            cfinfo.frame_count + 1,
                            frame_info.frame_type.name(),
                            "-",
                            "-",
                            total_blocks_size + 4, // +4 for the magic number bytes
                            "-",
                            "-"
                        );
                    }
                    result = InfoResult::Ok;
                }
            }

            // ---------------------------------------------------------------
            LZ4IO_SKIPPABLE0 => {
                frame_info.frame_type = FrameType::SkippableFrame;
                if cfinfo.frame_summary.frame_type != FrameType::SkippableFrame
                    && cfinfo.frame_count != 0
                {
                    cfinfo.eq_frame_types = false;
                }
                cfinfo.eq_block_types = false;
                cfinfo.all_content_size = false;

                // Read the 4-byte skippable frame size field
                let n = match file.read(&mut buf[..4]) {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("Error 42 : Stream error : skippable size unreadable");
                        std::process::exit(42);
                    }
                };
                if n != 4 {
                    eprintln!("Error 42 : Stream error : skippable size unreadable");
                    std::process::exit(42);
                }

                let size = read_le32(&buf[..4]);
                if file.seek(SeekFrom::Current(size as i64)).is_err() {
                    eprintln!("Error 43 : Stream error : cannot skip skippable area");
                    std::process::exit(43);
                }

                if display_now {
                    println!(
                        "    {:>6} {:>14} {:>5} {:>8} {:>20} {:>20} {:>9}",
                        cfinfo.frame_count + 1,
                        "SkippableFrame",
                        "-",
                        "-",
                        size + 8, // payload + magic (4) + size field (4)
                        "-",
                        "-"
                    );
                }
                result = InfoResult::Ok;
            }

            // ---------------------------------------------------------------
            _ => {
                if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 3 {
                    eprint!("Stream followed by undecodable data ");
                    if let Ok(pos) = file.stream_position() {
                        eprint!("at position {} ", pos);
                    }
                    eprintln!();
                }
                result = InfoResult::FormatNotKnown;
                break 'frame_loop;
            }
        } // match magic

        if result != InfoResult::Ok {
            break 'frame_loop;
        }
        cfinfo.frame_summary = frame_info;
        cfinfo.frame_count += 1;
    } // 'frame_loop

    result
}

// ---------------------------------------------------------------------------
// display_compressed_files_info
// ---------------------------------------------------------------------------

/// Prints a compressed-file summary table for the `--list` flag.
///
/// In non-verbose mode (`DISPLAY_LEVEL < 3`) a single summary row is printed
/// per file. In verbose mode (`DISPLAY_LEVEL >= 3`) per-frame detail rows are
/// printed first, followed by the summary.
///
/// Returns `Ok(())` if every file was processed successfully, or the first
/// `Err` encountered (unrecognised format or non-regular file).
pub fn display_compressed_files_info(paths: &[&str]) -> io::Result<()> {
    let display_level = DISPLAY_LEVEL.load(Ordering::Relaxed);

    if display_level < 3 {
        println!(
            "{:>10} {:>14} {:>5} {:>11} {:>13} {:>8}   {}",
            "Frames", "Type", "Block", "Compressed", "Uncompressed", "Ratio", "Filename"
        );
    }

    for (idx, &path) in paths.iter().enumerate() {
        let mut cfinfo = CompressedFileInfo::new();
        cfinfo.file_name = base_name(path).to_owned();

        // Verify it is a regular file (mirrors C's UTIL_isRegFile / UTIL_isRegFD check)
        let is_regular = if path == STDIN_MARK {
            is_stdin_regular_file()
        } else {
            fs::metadata(path)
                .map(|m| m.file_type().is_file())
                .unwrap_or(false)
        };
        if !is_regular {
            if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
                eprintln!("lz4: {} is not a regular file", path);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", path),
            ));
        }

        if display_level >= 3 {
            println!("{}({}/{})", cfinfo.file_name, idx + 1, paths.len());
            println!(
                "    {:>6} {:>14} {:>5} {:>8} {:>20} {:>20} {:>9}",
                "Frame", "Type", "Block", "Checksum", "Compressed", "Uncompressed", "Ratio"
            );
        }

        let op_result = get_compressed_file_info(&mut cfinfo, path, display_level >= 3);
        if op_result != InfoResult::Ok {
            if DISPLAY_LEVEL.load(Ordering::Relaxed) >= 1 {
                eprintln!("lz4: {}: File format not recognized", path);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: File format not recognized", path),
            ));
        }

        if display_level >= 3 {
            println!();
        }

        if display_level < 3 {
            let frame_type_str = if cfinfo.eq_frame_types {
                cfinfo.frame_summary.frame_type.name()
            } else {
                "-"
            };
            let block_type_str = if cfinfo.eq_block_types {
                block_type_id(
                    &cfinfo.frame_summary.lz4_frame_info.block_size_id,
                    &cfinfo.frame_summary.lz4_frame_info.block_mode,
                )
            } else {
                "-".to_owned()
            };
            let compressed_str = to_human(cfinfo.file_size as f64);
            let uncompressed_str = if cfinfo.all_content_size {
                to_human(cfinfo.frame_summary.lz4_frame_info.content_size as f64)
            } else {
                "-".to_owned()
            };

            print!(
                "{:>10} {:>14} {:>5} {:>11} {:>13} ",
                cfinfo.frame_count,
                frame_type_str,
                block_type_str,
                compressed_str,
                uncompressed_str,
            );

            if cfinfo.all_content_size && cfinfo.frame_summary.lz4_frame_info.content_size != 0 {
                let ratio = cfinfo.file_size as f64
                    / cfinfo.frame_summary.lz4_frame_info.content_size as f64
                    * 100.0;
                println!("{:>8.2}%  {} ", ratio, cfinfo.file_name);
            } else {
                println!("{:>8}   {}", "-", cfinfo.file_name);
            }
        }
    }

    Ok(())
}
