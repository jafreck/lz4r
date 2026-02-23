// cli/help.rs — Rust port of lz4cli.c lines 104–260 (declarations #5, #7, #8, #9, #10, #11)
//
// Migrated from: lz4-src/lz4-1.10.0/programs/lz4cli.c
// Task: task-030 — Help Text Functions (Chunk 2)
//
// Functions:
//   errorOut         → pub fn error_out(msg: &str) -> !
//   usage            → pub fn print_usage(program: &str)
//   usage_advanced   → pub fn print_usage_advanced(program: &str)
//   usage_longhelp   → pub fn print_long_help(program: &str)
//   badusage         → pub fn print_bad_usage(program: &str) -> !
//   waitEnter        → pub fn wait_enter()

use std::io::{self, Write};

use crate::cli::constants::{display_level, lz4c_legacy_commands, LZ4_EXTENSION};

// ── Compile-time constants (from lz4hc.h and lz4conf.h) ───────────────────────
/// Maximum HC compression level — mirrors `LZ4HC_CLEVEL_MAX 12` in lz4hc.h.
const LZ4HC_CLEVEL_MAX: i32 = 12;

/// Default number of worker threads — mirrors `LZ4_NBWORKERS_DEFAULT 0` in lz4conf.h.
const LZ4_NBWORKERS_DEFAULT: i32 = 0;

/// Default block size ID — mirrors `LZ4_BLOCKSIZEID_DEFAULT 7` in lz4conf.h.
const LZ4_BLOCKSIZEID_DEFAULT: i32 = 7;

/// Standard-input mark — mirrors `stdinmark "stdin"` in lz4io.h.
const STDINMARK: &str = "stdin";

/// Standard-output mark — mirrors `stdoutmark "stdout"` in lz4io.h.
const STDOUTMARK: &str = "stdout";

/// Null output — mirrors `NULL_OUTPUT "null"` in lz4io.h.
const NULL_OUTPUT: &str = "null";

// ── errorOut (lz4cli.c lines 104–107) ─────────────────────────────────────────
/// Print `msg` to stderr (at display level 1) then exit with code 1.
/// Equivalent to C `static void errorOut(const char* msg)`.
pub fn error_out(msg: &str) -> ! {
    if display_level() >= 1 {
        eprintln!("{} ", msg);
    }
    std::process::exit(1);
}

// ── usage (lz4cli.c lines 124–141) ────────────────────────────────────────────
/// Print brief usage to stderr.
/// Equivalent to C `static int usage(const char* exeName)`.
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

// ── usage_advanced (lz4cli.c lines 143–185) ───────────────────────────────────
/// Print the welcome banner followed by brief usage and advanced options to stderr.
/// Equivalent to C `static int usage_advanced(const char* exeName)`.
///
/// The welcome message is formatted inline here; callers that need the full
/// formatted string can use `crate::cli::constants::WELCOME_MESSAGE_FMT`.
pub fn print_usage_advanced(program: &str) {
    // WELCOME_MESSAGE — mirrors `DISPLAY(WELCOME_MESSAGE)` at line 145
    // Use LZ4_versionNumber() to build the same version string that LZ4_versionString() returns,
    // matching the C source which calls LZ4_versionString() at runtime.
    let bits = (std::mem::size_of::<*const ()>() * 8) as u32;
    let mt = crate::cli::constants::IO_MT;
    let ver_num = unsafe { lz4_sys::LZ4_versionNumber() };
    let ver_str = format!("{}.{}.{}", ver_num / 10000, (ver_num / 100) % 100, ver_num % 100);
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
    eprintln!("--fast[=#]: switch to ultra fast compression level (default: {})", 1);
    eprintln!("--best  : same as -{}", LZ4HC_CLEVEL_MAX);
    eprintln!("Benchmark arguments : ");
    eprintln!(" -b#    : benchmark file(s), using # compression level (default : 1) ");
    eprintln!(" -e#    : test all compression levels from -bX to # (default : 1)");
    eprintln!(" -i#    : minimum evaluation time in seconds (default : 3s) ");

    // Legacy arguments — only shown when invoked as `lz4c` (mirrors lines 177–183)
    if lz4c_legacy_commands() {
        eprintln!("Legacy arguments : ");
        eprintln!(" -c0    : fast compression ");
        eprintln!(" -c1    : high compression ");
        eprintln!(" -c2,-hc: very high compression ");
        eprintln!(" -y     : overwrite output without prompting ");
    }
}

// ── usage_longhelp (lz4cli.c lines 187–246) ───────────────────────────────────
/// Print the full long-form help to stderr.
/// Equivalent to C `static int usage_longhelp(const char* exeName)`.
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
    eprintln!(
        "          '{}' to discard output (test mode) ",
        NULL_OUTPUT
    );
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
    eprintln!(
        "{} can be used in 'pure pipe mode'. For example :",
        program
    );
    eprintln!("-------------------------------------");
    eprintln!("3 : compress data stream from 'generator', send result to 'consumer'");
    eprintln!("          generator | {} | consumer ", program);

    // Legacy warning — mirrors lines 234–244
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

// ── badusage (lz4cli.c lines 248–253) ─────────────────────────────────────────
/// Print "Incorrect parameters" to stderr, optionally print brief usage, then exit 1.
/// Equivalent to C `static int badusage(const char* exeName)`.
pub fn print_bad_usage(program: &str) -> ! {
    if display_level() >= 1 {
        eprintln!("Incorrect parameters");
        print_usage(program);
    }
    std::process::exit(1);
}

// ── waitEnter (lz4cli.c lines 256–260) ────────────────────────────────────────
/// Print a prompt and wait for the user to press Enter.
/// Equivalent to C `static void waitEnter(void)`.
pub fn wait_enter() {
    eprint!("Press enter to continue...\n");
    let _ = io::stderr().flush();
    // Use getchar() to read exactly one character from stdin, matching the C source
    // behaviour of `(void)getchar()` — avoids consuming more buffered input than expected.
    unsafe { libc::getchar() };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the function exists and runs without panic (output goes to stderr).
    /// Acceptance criterion: print_usage produces non-empty text containing "Usage"
    /// and "-h/-H".
    #[test]
    fn print_usage_does_not_panic() {
        // We cannot easily capture stderr in a unit test without additional
        // infrastructure, so we just confirm the function completes without panic.
        // Integration / parity tests capture stderr via process invocation.
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
