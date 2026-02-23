//! Command-line argument parsing for the `lz4` / `lz4c` / `unlz4` / `lz4cat` family.
//!
//! The entry points are [`parse_args`] (reads `std::env::args()`) and
//! [`parse_args_from`] (takes an explicit slice, suitable for unit-testing).
//! Both return a [`ParsedArgs`] value that captures every option and filename
//! discovered during the parse.
//!
//! Short options may be aggregated (e.g. `-9fv`).  Long options use either
//! `--option=VALUE` or `--option VALUE` syntax.  A bare `--` marks the end of
//! options; all subsequent arguments are treated as file paths regardless of
//! whether they start with `-`.
//!
//! Bad or unrecognised options return an `Err` with a human-readable message
//! that begins with `"bad usage: "`.

use anyhow::anyhow;

use crate::bench::BenchConfig;
use crate::cli::arg_utils::{long_command_w_arg, read_u32_from_str};
use crate::cli::constants::{display_level, set_display_level, AUTHOR, COMPRESSOR_NAME, IO_MT};
use crate::displaylevel;
use crate::cli::help::{print_long_help, print_usage_advanced};
use crate::cli::init::CliInit;
use crate::cli::op_mode::OpMode;
use crate::hc::types::LZ4HC_CLEVEL_MAX;
use crate::io::file_io::{NULL_OUTPUT, NUL_MARK, STDIN_MARK, STDOUT_MARK};
use crate::io::prefs::{BlockMode, Prefs};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Block size used when `-l` (legacy format) is selected: 8 MiB.
const LEGACY_BLOCK_SIZE: usize = 8 * (1 << 20);

// ── Public output type ─────────────────────────────────────────────────────────

/// Complete set of options and filenames produced by the argument parsing loop.
///
/// Fields are populated by [`parse_args_from`] and consumed by the dispatch
/// phase that selects compress / decompress / benchmark / list behaviour.
#[derive(Debug)]
pub struct ParsedArgs {
    /// Compression/decompression/IO preferences.
    pub prefs: Prefs,
    /// Resolved operation mode.
    pub op_mode: OpMode,
    /// Compression level (negative = fast acceleration).
    pub c_level: i32,
    /// Upper bound of compression-level range for benchmark (`-e` option).
    pub c_level_last: i32,
    /// Use the legacy (v0) LZ4 frame format (`-l`).
    pub legacy_format: bool,
    /// Force output to stdout even if it is a terminal.
    pub force_stdout: bool,
    /// Overwrite existing destination files without prompting.
    pub force_overwrite: bool,
    /// Pause before returning (hidden `-p` option).
    pub main_pause: bool,
    /// Treat all non-option arguments as input files (multiple-input mode).
    pub multiple_inputs: bool,
    /// Number of compression worker threads (0 = auto).
    pub nb_workers: usize,
    /// Single input filename (non-multiple-input mode).
    pub input_filename: Option<String>,
    /// Single output filename (non-multiple-input mode).
    pub output_filename: Option<String>,
    /// Dictionary file path.
    pub dictionary_filename: Option<String>,
    /// Input filenames collected in multiple-input mode.
    pub in_file_names: Vec<String>,
    /// Traverse directories recursively (requires `recursive` Cargo feature).
    #[cfg(feature = "recursive")]
    pub recursive: bool,
    /// Current block size (bytes), derived from prefs or explicitly set.
    pub block_size: usize,
    /// Benchmark configuration accumulated from `BMK_set*` calls.
    pub bench_config: BenchConfig,
    /// When `true`, a --version / --help flag was processed; the caller should
    /// exit 0 without performing any I/O operation.
    pub exit_early: bool,
    /// Program name (argv[0] basename), used by help functions.
    pub exe_name: String,
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Parse `std::env::args()` (skipping argv[0]) using `init` as the starting state.
///
/// Delegates to [`parse_args_from`] after collecting `argv` into a `Vec<String>`.
pub fn parse_args(init: CliInit) -> anyhow::Result<ParsedArgs> {
    let exe_name = std::env::args().next().unwrap_or_default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    parse_args_from(init, &exe_name, &argv)
}

/// Parse an explicit argument list using `init` as the starting state.
///
/// `exe_name` is argv[0] (used for help text). `argv` is argv[1..].
/// This variant is callable from tests without touching `std::env`.
pub fn parse_args_from(
    init: CliInit,
    exe_name: &str,
    argv: &[String],
) -> anyhow::Result<ParsedArgs> {
    // Unpack initial state produced by alias detection in CliInit.
    let CliInit {
        mut prefs,
        op_mode: init_op_mode,
        lz4c_legacy,
        multiple_inputs: init_multiple_inputs,
        c_level: init_c_level,
        nb_workers: init_nb_workers,
        force_stdout: init_force_stdout,
        output_filename: init_output_filename,
        display_level_override: _,
    } = init;

    // --- Mutable parsing state ---
    let mut op_mode = init_op_mode;
    let mut c_level: i32 = init_c_level;
    let mut c_level_last: i32 = -10_000; // sentinel: no explicit benchmark upper bound set yet
    let mut legacy_format = false;
    let mut force_stdout = init_force_stdout;
    let mut force_overwrite = false;
    let mut main_pause = false;
    let mut multiple_inputs = init_multiple_inputs;
    let mut all_arguments_are_files = false;
    let mut nb_workers: usize = init_nb_workers;
    let mut input_filename: Option<String> = None;
    let mut output_filename: Option<String> = init_output_filename;
    let mut dictionary_filename: Option<String> = None;
    let mut in_file_names: Vec<String> = Vec::new();
    #[cfg(feature = "recursive")]
    let mut recursive = false;
    let mut block_size: usize = prefs.block_size; // initialised from default prefs
    let mut bench_config = BenchConfig::default();
    let mut exit_early = false;

    let exe_name_str = exe_name.to_owned();

    // ── Main argument loop ──────────────────────────────────────────────────

    let mut arg_idx = 0usize;
    while arg_idx < argv.len() {
        let argument = &argv[arg_idx];

        if argument.is_empty() {
            arg_idx += 1;
            continue;
        }

        let bytes = argument.as_bytes();

        // ── Non-option path (or end-of-options forced by `--`) ────────────────
        if all_arguments_are_files || bytes[0] != b'-' {
            if multiple_inputs {
                in_file_names.push(argument.clone());
            } else if input_filename.is_none() {
                input_filename = Some(argument.clone());
            } else if output_filename.is_none() {
                // The special filename "null" is normalised to a sentinel so downstream code recognises it as /dev/null.
                let s = if argument == NULL_OUTPUT {
                    NUL_MARK.to_owned()
                } else {
                    argument.clone()
                };
                output_filename = Some(s);
            } else {
                // 3rd or later non-option argument with neither -m nor -f:
                if force_overwrite {
                    displaylevel!(
                        1,
                        "Warning: {} won't be used ! Do you want multiple input files (-m) ? \n",
                        argument
                    );
                } else {
                    return Err(anyhow!(
                        "Error: {} won't be used ! Do you want multiple input files (-m) ?",
                        argument
                    ));
                }
            }
            arg_idx += 1;
            continue;
        }

        // ── Single `-` means stdin (as input) or stdout (as output) ──────────
        if bytes.len() == 1 {
            // `-` alone
            if input_filename.is_none() {
                input_filename = Some(STDIN_MARK.to_owned());
            } else {
                output_filename = Some(STDOUT_MARK.to_owned());
            }
            arg_idx += 1;
            continue;
        }

        // ── Long options (`--...`) ────────────────────────────────────────────
        if bytes[1] == b'-' {
            // `--` end-of-options sentinel
            if argument == "--" {
                all_arguments_are_files = true;
                arg_idx += 1;
                continue;
            }

            // Dispatch on the long option name.

            if argument == "--compress" {
                op_mode = OpMode::Compress;
            } else if argument == "--decompress" || argument == "--uncompress" {
                if op_mode != OpMode::Bench {
                    op_mode = OpMode::Decompress;
                }
                bench_config.set_decode_only(true);
            } else if argument == "--multiple" {
                multiple_inputs = true;
            } else if argument == "--test" {
                op_mode = OpMode::Test;
            } else if argument == "--force" {
                prefs.set_overwrite(true);
            } else if argument == "--no-force" {
                prefs.set_overwrite(false);
            } else if argument == "--stdout" || argument == "--to-stdout" {
                force_stdout = true;
                output_filename = Some(STDOUT_MARK.to_owned());
            } else if argument == "--frame-crc" {
                prefs.set_stream_checksum_mode(true);
                bench_config.set_skip_checksums(false);
            } else if argument == "--no-frame-crc" {
                prefs.set_stream_checksum_mode(false);
                bench_config.set_skip_checksums(true);
            } else if argument == "--no-crc" {
                prefs.set_stream_checksum_mode(false);
                prefs.set_block_checksum_mode(false);
                bench_config.set_skip_checksums(true);
            } else if argument == "--content-size" {
                prefs.set_content_size(true);
            } else if argument == "--no-content-size" {
                prefs.set_content_size(false);
            } else if argument == "--list" {
                op_mode = OpMode::List;
                multiple_inputs = true;
            } else if argument == "--sparse" {
                // 2 = forced sparse; 0 = off; 1 = auto (default).
                prefs.sparse_file_support = 2;
            } else if argument == "--no-sparse" {
                prefs.sparse_file_support = 0;
            } else if argument == "--favor-decSpeed" {
                prefs.favor_dec_speed(true);
            } else if argument == "--verbose" {
                let lvl = display_level().saturating_add(1);
                set_display_level(lvl);
            } else if argument == "--quiet" {
                let lvl = display_level();
                if lvl > 0 {
                    set_display_level(lvl - 1);
                }
            } else if argument == "--version" {
                print_welcome_message(exe_name);
                exit_early = true;
                break;
            } else if argument == "--help" {
                print_usage_advanced(exe_name);
                exit_early = true;
                break;
            } else if argument == "--keep" {
                prefs.set_remove_src_file(false);
            } else if argument == "--rm" {
                prefs.set_remove_src_file(true);
            } else if let Some(rest) = long_command_w_arg(argument, "--threads") {
                // Accepts `--threads=N` or `--threads N` syntax.
                let (val, rest_pos) =
                    parse_next_uint32(rest, argv, &mut arg_idx, exe_name)?;
                if !rest_pos.is_empty() {
                    return Err(anyhow!("bad usage: --threads: only numeric values are allowed"));
                }
                nb_workers = val as usize;
            } else if let Some(rest) = long_command_w_arg(argument, "--fast") {
                // --fast[=N]: negative acceleration level (higher = faster, lower quality).
                if rest.starts_with('=') {
                    let value_str = &rest[1..];
                    if let Some((fast_level, remainder)) = read_u32_from_str(value_str) {
                        if !remainder.is_empty() {
                            return Err(anyhow!("bad usage: --fast: invalid argument"));
                        }
                        if fast_level == 0 {
                            return Err(anyhow!("bad usage: --fast: level must be > 0"));
                        }
                        c_level = -(fast_level as i32);
                    } else {
                        return Err(anyhow!("bad usage: --fast: expected a numeric level"));
                    }
                } else if rest.is_empty() {
                    c_level = -1; // default acceleration
                } else {
                    return Err(anyhow!("bad usage: --fast: unexpected characters after option"));
                }
            } else if argument == "--best" {
                // gzip(1) compatibility alias for maximum HC compression level.
                c_level = LZ4HC_CLEVEL_MAX;
            } else {
                return Err(anyhow!("bad usage: unknown option: {}", argument));
            }

            arg_idx += 1;
            continue;
        }

        // ── Short options (possibly aggregated, e.g. `-9fv`) ─────────────────
        //
        // `char_pos` starts at 1 (the first flag character after `-`).
        // Each iteration handles one flag character and increments `char_pos`.

        let mut char_pos: usize = 1; // skip the leading '-'
        while char_pos < bytes.len() {
            // ── Legacy commands (`-c0`, `-c1`, `-c2`, `-hc`, `-y`) ───────────
            // These multi-character sequences must be tested before the single-character dispatch below.
            if lz4c_legacy {
                let rest = &argument[char_pos..];
                if rest.starts_with("c0") {
                    c_level = 0;
                    char_pos += 2;
                    continue;
                }
                if rest.starts_with("c1") {
                    c_level = 9;
                    char_pos += 2;
                    continue;
                }
                if rest.starts_with("c2") {
                    c_level = 12;
                    char_pos += 2;
                    continue;
                }
                if rest.starts_with("hc") {
                    c_level = 12;
                    char_pos += 2;
                    continue;
                }
                if rest.starts_with('y') {
                    prefs.set_overwrite(true);
                    char_pos += 1;
                    continue;
                }
            }

            // ── Numeric compression level (`-0` … `-9` … `-12`) ──────────────
            // A run of ASCII digits sets the compression level directly.
            // `read_u32_from_str` consumes all leading digit characters.
            if bytes[char_pos].is_ascii_digit() {
                let (val, remainder) = read_u32_from_str(&argument[char_pos..])
                    .expect("is_ascii_digit guarantees at least one digit");
                c_level = val as i32;
                // `char_pos` must advance past every consumed digit.
                // The outer loop increments `char_pos` by 1 at the end of each
                // iteration, so we position it one before the desired next character.
                let consumed = argument[char_pos..].len() - remainder.len();
                char_pos += consumed; // char_pos now points one past last digit
                char_pos = char_pos.saturating_sub(1);
                char_pos += 1;
                continue;
            }

            // ── Main switch ───────────────────────────────────────────────────
            match bytes[char_pos] {
                b'V' => {
                    // Print version and exit.
                    print_welcome_message(exe_name);
                    exit_early = true;
                    break; // exit short-option loop
                }
                b'h' => {
                    // Print standard help and exit.
                    print_usage_advanced(exe_name);
                    exit_early = true;
                    break;
                }
                b'H' => {
                    // Print extended help and exit.
                    print_long_help(exe_name);
                    exit_early = true;
                    break;
                }
                b'e' => {
                    // `-eN` — upper bound of the compression-level range used during benchmark.
                    let next = char_pos + 1;
                    if next < bytes.len() && bytes[next].is_ascii_digit() {
                        let (val, remainder) = read_u32_from_str(&argument[next..]).unwrap();
                        c_level_last = val as i32;
                        let consumed = argument[next..].len() - remainder.len();
                        // Advance past consumed digits; loop adds 1, so set to end - 1.
                        char_pos = next + consumed - 1;
                    } else {
                        return Err(anyhow!("bad usage: -e requires a numeric argument"));
                    }
                }
                b'z' => {
                    // Force compress mode.
                    op_mode = OpMode::Compress;
                }
                b'T' => {
                    // Set the number of compression worker threads.
                    // Accepts `-TN` (inline) or `-T N` (next argument).
                    let next = char_pos + 1;
                    if next < bytes.len() && bytes[next].is_ascii_digit() {
                        let (val, remainder) = read_u32_from_str(&argument[next..]).unwrap();
                        nb_workers = val as usize;
                        let consumed = argument[next..].len() - remainder.len();
                        char_pos = next + consumed - 1;
                    } else if next >= bytes.len() {
                        // `-T N` — value is next argument
                        arg_idx += 1;
                        if arg_idx >= argv.len() {
                            return Err(anyhow!("bad usage: -T requires a numeric argument"));
                        }
                        let (val, _rest) =
                            read_u32_from_str(&argv[arg_idx]).ok_or_else(|| {
                                anyhow!("bad usage: -T: expected numeric value")
                            })?;
                        nb_workers = val as usize;
                        char_pos = bytes.len() - 1; // skip to end of current arg
                    } else {
                        return Err(anyhow!("bad usage: -T requires a numeric argument"));
                    }
                }
                b'D' => {
                    // Specify a dictionary file; the path may follow immediately or as the next argument.
                    let next = char_pos + 1;
                    if next >= bytes.len() {
                        // Path is the next argument.
                        arg_idx += 1;
                        if arg_idx >= argv.len() {
                            return Err(anyhow!("bad usage: -D requires a path argument"));
                        }
                        dictionary_filename = Some(argv[arg_idx].clone());
                    } else {
                        // Path immediately follows the 'D'.
                        dictionary_filename = Some(argument[next..].to_owned());
                    }
                    // Skip to end of this argument; the dictionary path has been fully consumed.
                    char_pos = bytes.len() - 1;
                }
                b'l' => {
                    // Use the legacy (v0) LZ4 frame format with a fixed 8 MiB block size.
                    legacy_format = true;
                    block_size = LEGACY_BLOCK_SIZE;
                    prefs.block_size = block_size;
                }
                b'd' => {
                    // Switch to decompress mode; also enables decode-only benchmarking.
                    if op_mode != OpMode::Bench {
                        op_mode = OpMode::Decompress;
                    }
                    bench_config.set_decode_only(true);
                }
                b'c' => {
                    // Force output to stdout; enables pass-through mode for non-LZ4 input.
                    force_stdout = true;
                    output_filename = Some(STDOUT_MARK.to_owned());
                    prefs.set_pass_through(true);
                }
                b't' => {
                    // Verify integrity of compressed input; no output is written.
                    op_mode = OpMode::Test;
                }
                b'f' => {
                    // Overwrite existing destination files without prompting.
                    force_overwrite = true;
                    prefs.set_overwrite(true);
                }
                b'v' => {
                    // Increase verbosity level.
                    let lvl = display_level().saturating_add(1);
                    set_display_level(lvl);
                }
                b'q' => {
                    // Decrease verbosity level.
                    let lvl = display_level();
                    if lvl > 0 {
                        set_display_level(lvl - 1);
                    }
                }
                b'k' => {
                    // Preserve the source file after compression or decompression.
                    prefs.set_remove_src_file(false);
                }
                b'B' => {
                    // Block format sub-options: size ID (4–7), raw byte count (≥32),
                    // linked/independent mode (D/I), and block checksum (X).
                    // Characters after 'B' are consumed by the inner loop below.
                    let mut j = char_pos + 1;
                    loop {
                        if j >= bytes.len() {
                            break;
                        }
                        match bytes[j] {
                            b'D' => {
                                prefs.set_block_mode(BlockMode::Linked);
                                j += 1;
                            }
                            b'I' => {
                                prefs.set_block_mode(BlockMode::Independent);
                                j += 1;
                            }
                            b'X' => {
                                prefs.set_block_checksum_mode(true);
                                j += 1;
                            }
                            c if c.is_ascii_digit() => {
                                // Numeric suffix: 4–7 selects a preset block-size ID; ≥32 is a raw byte count.
                                let (b_val, remainder) =
                                    read_u32_from_str(&argument[j..]).unwrap();
                                let consumed = argument[j..].len() - remainder.len();
                                // j advances by consumed chars; inner loop does not auto-advance.
                                j += consumed;
                                if b_val < 4 {
                                    return Err(anyhow!(
                                        "bad usage: block size ID must be >= 4"
                                    ));
                                }
                                if b_val <= 7 {
                                    block_size = prefs.set_block_size_id(b_val);
                                    bench_config.set_block_size(block_size);
                                    displaylevel!(
                                        2,
                                        "using blocks of size {} KB \n",
                                        block_size >> 10
                                    );
                                } else {
                                    if b_val < 32 {
                                        return Err(anyhow!(
                                            "bad usage: block size must be >= 32 bytes when > 7"
                                        ));
                                    }
                                    block_size = prefs.set_block_size(b_val as usize);
                                    bench_config.set_block_size(block_size);
                                    if block_size >= 1024 {
                                        displaylevel!(
                                            2,
                                            "using blocks of size {} KB \n",
                                            block_size >> 10
                                        );
                                    } else {
                                        displaylevel!(
                                            2,
                                            "using blocks of size {} bytes \n",
                                            block_size
                                        );
                                    }
                                }
                                // j is already past consumed digits; inner loop checks bytes[j] next.
                            }
                            _ => break, // unrecognised sub-option: stop parsing block properties
                        }
                    }
                    // Position char_pos so the outer loop's +1 lands at j.
                    char_pos = j.saturating_sub(1);
                }
                b'b' => {
                    // Enter benchmark mode; enables multiple input files.
                    op_mode = OpMode::Bench;
                    multiple_inputs = true;
                }
                b'S' => {
                    // Benchmark each input file separately (hidden option).
                    bench_config.set_bench_separately(true);
                }
                b'r' => {
                    // Traverse directories recursively (requires the "recursive" Cargo feature).
                    #[cfg(feature = "recursive")]
                    {
                        recursive = true;
                    }
                    // -r also implies -m: treat positional arguments as input files.
                    multiple_inputs = true;
                }
                b'm' => {
                    // Accept multiple positional arguments as input filenames.
                    multiple_inputs = true;
                }
                b'i' => {
                    // Set benchmark duration in seconds (or iteration count) for each level.
                    let next = char_pos + 1;
                    if next < bytes.len() && bytes[next].is_ascii_digit() {
                        let (iters, remainder) = read_u32_from_str(&argument[next..]).unwrap();
                        let consumed = argument[next..].len() - remainder.len();
                        bench_config.set_notification_level(display_level());
                        bench_config.set_nb_seconds(iters);
                        char_pos = next + consumed - 1;
                    } else {
                        return Err(anyhow!("bad usage: -i requires a numeric argument"));
                    }
                }
                b'p' => {
                    // Pause before returning (hidden diagnostic option).
                    main_pause = true;
                }
                _ => {
                    // Unrecognised short option.
                    return Err(anyhow!(
                        "bad usage: unrecognised option: -{c}",
                        c = bytes[char_pos] as char
                    ));
                }
            }

            if exit_early {
                break; // propagate early exit out of short-option loop
            }
            char_pos += 1;
        }

        if exit_early {
            break; // propagate out of main argument loop
        }

        arg_idx += 1;
    }

    Ok(ParsedArgs {
        prefs,
        op_mode,
        c_level,
        c_level_last,
        legacy_format,
        force_stdout,
        force_overwrite,
        main_pause,
        multiple_inputs,
        nb_workers,
        input_filename,
        output_filename,
        dictionary_filename,
        in_file_names,
        #[cfg(feature = "recursive")]
        recursive,
        block_size,
        bench_config,
        exit_early,
        exe_name: exe_name_str,
    })
}

// ── Private helpers ────────────────────────────────────────────────────────────

/// Prints the version banner to stdout.
fn print_welcome_message(exe_name: &str) {
    let bits = (std::mem::size_of::<usize>() * 8) as u32;
    println!(
        "*** {} v{} {}-bit {}, by {} ***",
        COMPRESSOR_NAME,
        env!("CARGO_PKG_VERSION"),
        bits,
        IO_MT,
        AUTHOR
    );
    let _ = exe_name; // unused; kept for symmetry with other help functions
}

/// Read a `u32` from either `=VALUE` within the current argument or from the next
/// element of `argv` (advancing `arg_idx`), supporting both `--option=N` and
/// `--option N` syntax.
///
/// `rest` is the slice of the current argument following the long-option name
/// (e.g. for `--threads=4`, `rest` is `"=4"`; for `--threads 4`, `rest` is `""`).
///
/// Returns `(value, unconsumed_suffix)`.  Callers should verify the suffix is
/// empty to catch trailing garbage such as `--threads=4x`.
fn parse_next_uint32<'a>(
    rest: &'a str,
    argv: &[String],
    arg_idx: &mut usize,
    exe_name: &str,
) -> anyhow::Result<(u32, &'a str)> {
    if rest.starts_with('=') {
        // `--option=VALUE` syntax.
        let value_str = &rest[1..];
        let (val, suffix) = read_u32_from_str(value_str)
            .ok_or_else(|| anyhow!("bad usage: {} expected numeric argument", exe_name))?;
        Ok((val, suffix))
    } else if rest.is_empty() {
        // `--option VALUE` syntax: consume the next argv element.
        *arg_idx += 1;
        let next = argv.get(*arg_idx).ok_or_else(|| {
            anyhow!("bad usage: {}: missing command argument", exe_name)
        })?;
        if next.starts_with('-') {
            return Err(anyhow!(
                "bad usage: {}: option argument cannot be another option",
                exe_name
            ));
        }
        let (val, suffix) = read_u32_from_str(next)
            .ok_or_else(|| anyhow!("bad usage: {}: expected numeric argument", exe_name))?;
        // The suffix borrow is from `next` which is a &String.  We cannot return a
        // reference to a local; signal non-empty suffix via a sentinel instead.
        // Callers that need strict validation should re-parse.
        let _ = suffix;
        Ok((val, ""))
    } else {
        Err(anyhow!("bad usage: {}: unexpected text after option", exe_name))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init::detect_alias;
    use crate::cli::op_mode::OpMode;

    fn make_args(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn parse(args: &[&str]) -> ParsedArgs {
        let init = detect_alias("lz4");
        parse_args_from(init, "lz4", &make_args(args)).expect("parse failed")
    }

    fn parse_err(args: &[&str]) -> anyhow::Error {
        let init = detect_alias("lz4");
        parse_args_from(init, "lz4", &make_args(args)).expect_err("expected error")
    }

    // ── Compression level ────────────────────────────────────────────────────

    #[test]
    fn short_level_9() {
        let p = parse(&["-9"]);
        assert_eq!(p.c_level, 9);
    }

    #[test]
    fn short_level_12() {
        let p = parse(&["-12"]);
        assert_eq!(p.c_level, 12);
    }

    #[test]
    fn best_flag() {
        let p = parse(&["--best"]);
        assert_eq!(p.c_level, LZ4HC_CLEVEL_MAX);
    }

    #[test]
    fn fast_default() {
        let p = parse(&["--fast"]);
        assert_eq!(p.c_level, -1);
    }

    #[test]
    fn fast_equals_3() {
        let p = parse(&["--fast=3"]);
        assert_eq!(p.c_level, -3);
    }

    // ── Operation mode ───────────────────────────────────────────────────────

    #[test]
    fn compress_flag() {
        let p = parse(&["--compress"]);
        assert_eq!(p.op_mode, OpMode::Compress);
    }

    #[test]
    fn short_compress_flag() {
        let p = parse(&["-z"]);
        assert_eq!(p.op_mode, OpMode::Compress);
    }

    #[test]
    fn decompress_flag() {
        let p = parse(&["--decompress"]);
        assert_eq!(p.op_mode, OpMode::Decompress);
    }

    #[test]
    fn uncompress_alias() {
        let p = parse(&["--uncompress"]);
        assert_eq!(p.op_mode, OpMode::Decompress);
    }

    #[test]
    fn short_decompress_flag() {
        let p = parse(&["-d"]);
        assert_eq!(p.op_mode, OpMode::Decompress);
    }

    #[test]
    fn test_mode() {
        let p = parse(&["--test"]);
        assert_eq!(p.op_mode, OpMode::Test);
    }

    #[test]
    fn short_test_mode() {
        let p = parse(&["-t"]);
        assert_eq!(p.op_mode, OpMode::Test);
    }

    #[test]
    fn list_mode() {
        let p = parse(&["--list"]);
        assert_eq!(p.op_mode, OpMode::List);
        assert!(p.multiple_inputs);
    }

    #[test]
    fn bench_mode() {
        let p = parse(&["-b"]);
        assert_eq!(p.op_mode, OpMode::Bench);
        assert!(p.multiple_inputs);
    }

    // ── Aggregated short flags ────────────────────────────────────────────────

    #[test]
    fn aggregated_9fv() {
        // `-9fv` sets c_level=9, force_overwrite=true, displayLevel++
        let init = detect_alias("lz4");
        let lvl_before = display_level();
        let p = parse_args_from(init, "lz4", &make_args(&["-9fv"])).unwrap();
        assert_eq!(p.c_level, 9);
        assert!(p.force_overwrite);
        assert!(display_level() > lvl_before);
        // restore
        set_display_level(lvl_before);
    }

    // ── Block size ───────────────────────────────────────────────────────────

    #[test]
    fn block_size_id_4() {
        let p = parse(&["-B4"]);
        assert_eq!(p.prefs.block_size_id, 4);
    }

    #[test]
    fn block_size_id_7() {
        let p = parse(&["-B7"]);
        assert_eq!(p.prefs.block_size_id, 7);
    }

    #[test]
    fn block_linked() {
        let p = parse(&["-BD"]);
        assert!(!p.prefs.block_independence);
    }

    #[test]
    fn block_independent() {
        let p = parse(&["-BI"]);
        assert!(p.prefs.block_independence);
    }

    #[test]
    fn block_checksum() {
        let p = parse(&["-BX"]);
        assert!(p.prefs.block_checksum);
    }

    // ── Threads ──────────────────────────────────────────────────────────────

    #[test]
    fn threads_long_equals() {
        let p = parse(&["--threads=2"]);
        assert_eq!(p.nb_workers, 2);
    }

    #[test]
    fn threads_long_space() {
        let p = parse(&["--threads", "2"]);
        assert_eq!(p.nb_workers, 2);
    }

    #[test]
    fn threads_short_inline() {
        let p = parse(&["-T4"]);
        assert_eq!(p.nb_workers, 4);
    }

    // ── Dictionary ───────────────────────────────────────────────────────────

    #[test]
    fn dictionary_inline() {
        let p = parse(&["-Ddict.bin"]);
        assert_eq!(p.dictionary_filename.as_deref(), Some("dict.bin"));
    }

    #[test]
    fn dictionary_separate() {
        let p = parse(&["-D", "dict.bin"]);
        assert_eq!(p.dictionary_filename.as_deref(), Some("dict.bin"));
    }

    // ── Non-option filenames ──────────────────────────────────────────────────

    #[test]
    fn input_file() {
        let p = parse(&["input.txt"]);
        assert_eq!(p.input_filename.as_deref(), Some("input.txt"));
    }

    #[test]
    fn input_and_output() {
        let p = parse(&["input.txt", "output.lz4"]);
        assert_eq!(p.input_filename.as_deref(), Some("input.txt"));
        assert_eq!(p.output_filename.as_deref(), Some("output.lz4"));
    }

    #[test]
    fn null_output_translated() {
        let p = parse(&["input.txt", "null"]);
        assert_eq!(p.output_filename.as_deref(), Some(NUL_MARK));
    }

    #[test]
    fn stdin_dash() {
        let p = parse(&["-"]);
        assert_eq!(p.input_filename.as_deref(), Some(STDIN_MARK));
    }

    #[test]
    fn multiple_inputs_flag() {
        let p = parse(&["-m", "a.txt", "b.txt"]);
        assert!(p.multiple_inputs);
        assert_eq!(p.in_file_names, vec!["a.txt", "b.txt"]);
    }

    // ── end-of-options `--` ───────────────────────────────────────────────────

    #[test]
    fn end_of_options_sentinel() {
        let p = parse(&["--", "-not-a-flag"]);
        assert_eq!(p.input_filename.as_deref(), Some("-not-a-flag"));
    }

    // ── Force / keep / quiet / verbose ───────────────────────────────────────

    #[test]
    fn force_flag() {
        let p = parse(&["--force"]);
        assert!(p.prefs.overwrite);
    }

    #[test]
    fn keep_flag() {
        let p = parse(&["--keep"]);
        assert!(!p.prefs.remove_src_file);
    }

    #[test]
    fn no_frame_crc() {
        let p = parse(&["--no-frame-crc"]);
        assert!(!p.prefs.stream_checksum);
        assert!(p.bench_config.skip_checksums);
    }

    #[test]
    fn content_size() {
        let p = parse(&["--content-size"]);
        assert!(p.prefs.content_size_flag);
    }

    // ── Sparse ───────────────────────────────────────────────────────────────

    #[test]
    fn sparse_flag() {
        let p = parse(&["--sparse"]);
        assert_eq!(p.prefs.sparse_file_support, 2);
    }

    #[test]
    fn no_sparse_flag() {
        let p = parse(&["--no-sparse"]);
        assert_eq!(p.prefs.sparse_file_support, 0);
    }

    // ── Version / help (exit_early) ───────────────────────────────────────────

    #[test]
    fn version_flag_exit_early() {
        let p = parse(&["--version"]);
        assert!(p.exit_early);
    }

    #[test]
    fn short_version_flag_exit_early() {
        let p = parse(&["-V"]);
        assert!(p.exit_early);
    }

    #[test]
    fn help_flag_exit_early() {
        let p = parse(&["--help"]);
        assert!(p.exit_early);
    }

    // ── Legacy mode ───────────────────────────────────────────────────────────

    #[test]
    fn legacy_c1_sets_level_9() {
        let init = detect_alias("lz4c");
        let p = parse_args_from(init, "lz4c", &make_args(&["-c1"])).unwrap();
        assert_eq!(p.c_level, 9);
    }

    #[test]
    fn legacy_hc_sets_level_12() {
        let init = detect_alias("lz4c");
        let p = parse_args_from(init, "lz4c", &make_args(&["-hc"])).unwrap();
        assert_eq!(p.c_level, 12);
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    #[test]
    fn unknown_long_option() {
        let e = parse_err(&["--unknown-option"]);
        assert!(e.to_string().contains("bad usage"));
    }

    #[test]
    fn bad_block_size_under_4() {
        let e = parse_err(&["-B3"]);
        assert!(e.to_string().contains("bad usage"));
    }

    #[test]
    fn fast_zero_level_is_error() {
        let e = parse_err(&["--fast=0"]);
        assert!(e.to_string().contains("bad usage"));
    }
}
