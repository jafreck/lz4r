//! Help and usage text for the `lz4` CLI.
//!
//! Provides functions that write brief usage, advanced options, and long-form
//! help to stderr, along with error-exit and interactive-pause helpers used by
//! the argument parser.

use std::io::{self, Write};

use crate::cli::constants::{display_level, lz4c_legacy_commands, LZ4_EXTENSION};

/// Maximum HC compression level (12), corresponding to the `--best` flag.
const LZ4HC_CLEVEL_MAX: i32 = 12;

/// Default worker-thread count; 0 means auto-detect at runtime.
const LZ4_NBWORKERS_DEFAULT: i32 = 0;

/// Default block size ID used in the LZ4 frame format (7 → 4 MiB blocks).
const LZ4_BLOCKSIZEID_DEFAULT: i32 = 7;

/// Sentinel string the CLI treats as a request to read from standard input.
const STDINMARK: &str = "stdin";

/// Sentinel string the CLI treats as a request to write to standard output.
const STDOUTMARK: &str = "stdout";

/// Sentinel string that discards all output (useful for integrity testing).
const NULL_OUTPUT: &str = "null";

/// Print `msg` to stderr and exit with code 1.
///
/// The message is suppressed when the global display level is below 1
/// (i.e. when `-qq` has been passed).
pub fn error_out(msg: &str) -> ! {
    if display_level() >= 1 {
        eprintln!("{} ", msg);
    }
    std::process::exit(1);
}

/// Print a brief usage summary to stderr.
pub fn print_usage(program: &str) {
    eprintln!("Usage : ");
    eprintln!("      {} [arg] [input] [output] ", program);
    eprintln!();
    eprintln!("input   : a filename ");
    eprintln!(
        "          with no FILE, or when FILE is - or {}, read standard input",
        STDINMARK
    );
    eprintln!("Arguments : ");
    eprintln!(" -1     : fast compression (default) ");
    eprintln!(" -{:2}    : slowest compression level ", LZ4HC_CLEVEL_MAX);
    eprintln!(
        " -T#    : use # threads for compression (default:{}==auto) ",
        LZ4_NBWORKERS_DEFAULT
    );
    eprintln!(
        " -d     : decompression (default for {} extension)",
        LZ4_EXTENSION
    );
    eprintln!(" -f     : overwrite output without prompting ");
    eprintln!(" -k     : preserve source files(s)  (default) ");
    eprintln!("--rm    : remove source file(s) after successful de/compression ");
    eprintln!(" -h/-H  : display help/long help and exit ");
}

/// Print the welcome banner followed by brief usage and advanced options to stderr.
///
/// The banner includes the library version (derived from the compile-time version
/// number), pointer width, and multi-threading capability.  Legacy `lz4c`
/// arguments are appended when [`lz4c_legacy_commands`] returns `true`.
pub fn print_usage_advanced(program: &str) {
    // Derive the version string from the compile-time integer so it always
    // matches the linked library without a separate runtime call.
    let bits = (std::mem::size_of::<*const ()>() * 8) as u32;
    let mt = crate::cli::constants::IO_MT;
    let ver_num = crate::LZ4_VERSION_NUMBER;
    let ver_str = format!(
        "{}.{}.{}",
        ver_num / 10000,
        (ver_num / 100) % 100,
        ver_num % 100
    );
    eprintln!(
        "*** {} v{} {}-bit {}, by {} ***",
        crate::cli::constants::COMPRESSOR_NAME,
        ver_str,
        bits,
        mt,
        crate::cli::constants::AUTHOR
    );

    print_usage(program);

    eprintln!();
    eprintln!("Advanced arguments :");
    eprintln!(" -V     : display Version number and exit ");
    eprintln!(" -v     : verbose mode ");
    eprintln!(" -q     : suppress warnings; specify twice to suppress errors too");
    eprintln!(" -c     : force write to standard output, even if it is the console");
    eprintln!(" -t     : test compressed file integrity");
    eprintln!(" -m     : multiple input files (implies automatic output filenames)");
    #[cfg(feature = "recursive")]
    eprintln!(" -r     : operate recursively on directories (sets also -m) ");
    eprintln!(" -l     : compress using Legacy format (Linux kernel compression)");
    eprintln!(" -z     : force compression ");
    eprintln!(" -D FILE: use FILE as dictionary (compression & decompression)");
    eprintln!(" -B#    : cut file into blocks of size # bytes [32+] ");
    eprintln!(
        "                     or predefined block size [4-7] (default: {}) ",
        LZ4_BLOCKSIZEID_DEFAULT
    );
    eprintln!(" -BI    : Block Independence (default) ");
    eprintln!(" -BD    : Block dependency (improves compression ratio) ");
    eprintln!(" -BX    : enable block checksum (default:disabled) ");
    eprintln!("--no-frame-crc : disable stream checksum (default:enabled) ");
    eprintln!("--content-size : compressed frame includes original size (default:not present)");
    eprintln!("--list FILE : lists information about .lz4 files (useful for files compressed with --content-size flag)");
    eprintln!("--[no-]sparse  : sparse mode (default:enabled on file, disabled on stdout)");
    eprintln!("--favor-decSpeed: compressed files decompress faster, but are less compressed ");
    eprintln!(
        "--fast[=#]: switch to ultra fast compression level (default: {})",
        1
    );
    eprintln!("--best  : same as -{}", LZ4HC_CLEVEL_MAX);
    eprintln!("Benchmark arguments : ");
    eprintln!(" -b#    : benchmark file(s), using # compression level (default : 1) ");
    eprintln!(" -e#    : test all compression levels from -bX to # (default : 1)");
    eprintln!(" -i#    : minimum evaluation time in seconds (default : 3s) ");

    // Legacy arguments are only shown when the binary is invoked as `lz4c`.
    if lz4c_legacy_commands() {
        eprintln!("Legacy arguments : ");
        eprintln!(" -c0    : fast compression ");
        eprintln!(" -c1    : high compression ");
        eprintln!(" -c2,-hc: very high compression ");
        eprintln!(" -y     : overwrite output without prompting ");
    }
}

/// Print the full long-form help to stderr.
///
/// Includes everything from [`print_usage_advanced`] plus detailed explanations
/// of output-naming rules, compression levels, console safety, pipe mode, and
/// argument aggregation.
pub fn print_long_help(program: &str) {
    print_usage_advanced(program);

    eprintln!();
    eprintln!("****************************");
    eprintln!("***** Advanced comment *****");
    eprintln!("****************************");
    eprintln!();
    eprintln!("Which values can [output] have ? ");
    eprintln!("---------------------------------");
    eprintln!("[output] : a filename ");
    eprintln!(
        "          '{}', or '-' for standard output (pipe mode)",
        STDOUTMARK
    );
    eprintln!("          '{}' to discard output (test mode) ", NULL_OUTPUT);
    eprintln!("[output] can be left empty. In this case, it receives the following value :");
    eprintln!("          - if stdout is not the console, then [output] = stdout ");
    eprintln!("          - if stdout is console : ");
    eprintln!(
        "               + for compression, output to filename{} ",
        LZ4_EXTENSION
    );
    eprintln!(
        "               + for decompression, output to filename without '{}'",
        LZ4_EXTENSION
    );
    eprintln!(
        "                    > if input filename has no '{}' extension : error ",
        LZ4_EXTENSION
    );
    eprintln!();
    eprintln!("Compression levels : ");
    eprintln!("---------------------");
    eprintln!("-0 ... -2  => Fast compression, all identical");
    eprintln!(
        "-3 ... -{} => High compression; higher number == more compression but slower",
        LZ4HC_CLEVEL_MAX
    );
    eprintln!();
    eprintln!("stdin, stdout and the console : ");
    eprintln!("--------------------------------");
    eprintln!("To protect the console from binary flooding (bad argument mistake)");
    eprintln!(
        "{} will refuse to read from console, or write to console ",
        program
    );
    eprintln!("except if '-c' command is specified, to force output to console ");
    eprintln!();
    eprintln!("Simple example :");
    eprintln!("----------------");
    eprintln!("1 : compress 'filename' fast, using default output name 'filename.lz4'");
    eprintln!("          {} filename", program);
    eprintln!();
    eprintln!("Short arguments can be aggregated. For example :");
    eprintln!("----------------------------------");
    eprintln!("2 : compress 'filename' in high compression mode, overwrite output if exists");
    eprintln!("          {} -9 -f filename ", program);
    eprintln!("    is equivalent to :");
    eprintln!("          {} -9f filename ", program);
    eprintln!();
    eprintln!("{} can be used in 'pure pipe mode'. For example :", program);
    eprintln!("-------------------------------------");
    eprintln!("3 : compress data stream from 'generator', send result to 'consumer'");
    eprintln!("          generator | {} | consumer ", program);

    // When running as `lz4c`, warn that legacy flags take precedence over modern ones.
    if lz4c_legacy_commands() {
        eprintln!();
        eprintln!("***** Warning  ***** ");
        eprintln!("Legacy arguments take precedence. Therefore : ");
        eprintln!("--------------------------------- ");
        eprintln!("          {} -hc filename ", program);
        eprintln!("means 'compress filename in high compression mode' ");
        eprintln!("It is not equivalent to : ");
        eprintln!("          {} -h -c filename ", program);
        eprintln!("which displays help text and exits ");
    }
}

/// Print "Incorrect parameters" to stderr, show brief usage, and exit with code 1.
///
/// Both the message and the usage text are suppressed when the display level
/// is below 1 (i.e. when `-qq` has been passed).
pub fn print_bad_usage(program: &str) -> ! {
    if display_level() >= 1 {
        eprintln!("Incorrect parameters");
        print_usage(program);
    }
    std::process::exit(1);
}

/// Print a prompt to stderr and block until the user presses Enter.
///
/// Reads exactly one byte from stdin via `libc::getchar` so that only the
/// newline is consumed, leaving any remaining buffered input intact.
pub fn wait_enter() {
    eprintln!("Press enter to continue...");
    let _ = io::stderr().flush();
    // Read a single byte so buffered input beyond the newline is not discarded.
    unsafe { libc::getchar() };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirm that [`print_usage`] runs to completion without panicking.
    ///
    /// Stderr capture requires out-of-process infrastructure; full output
    /// verification is covered by CLI integration tests.
    #[test]
    fn print_usage_does_not_panic() {
        print_usage("lz4");
    }

    #[test]
    fn print_usage_advanced_does_not_panic() {
        print_usage_advanced("lz4");
    }

    #[test]
    fn print_long_help_does_not_panic() {
        print_long_help("lz4");
    }

    #[test]
    fn constants_are_sensible() {
        assert_eq!(LZ4HC_CLEVEL_MAX, 12);
        assert_eq!(LZ4_BLOCKSIZEID_DEFAULT, 7);
        assert_eq!(STDINMARK, "stdin");
        assert_eq!(STDOUTMARK, "stdout");
        assert_eq!(NULL_OUTPUT, "null");
    }
}
