//! Binary entry point for the `lz4` command-line tool.
//!
//! Handles post-parse validation, recursive directory expansion, automatic
//! output filename resolution, and operation dispatch (compress, decompress,
//! list, benchmark).  Corresponds to the post-argument-parsing section of
//! `main()` in `lz4cli.c` (LZ4 v1.10.0, lines 704–893).
//!
//! # Control flow
//!
//! 1. [`detect_alias`] inspects `argv[0]` to infer an initial mode
//!    (e.g. `unlz4` implies decompress).
//! 2. [`parse_args`] processes all flags and builds a [`ParsedArgs`] value.
//! 3. [`run`] dispatches to the appropriate I/O operation and returns an exit code.
//!
//! All heap allocations are released by Rust’s RAII; there is no explicit
//! `free` or `goto _cleanup`.

use std::io::IsTerminal;

use lz4::cli::args::{parse_args, ParsedArgs};
use lz4::cli::constants::{display_level, set_display_level, LZ4_EXTENSION};
use lz4::cli::help::wait_enter;
use lz4::cli::init::detect_alias;
use lz4::cli::op_mode::{determine_op_mode, OpMode};
use lz4::config::MULTITHREAD;
use lz4::io::{
    compress_filename, compress_filename_legacy, compress_multiple_filenames,
    compress_multiple_filenames_legacy, decompress_filename, decompress_multiple_filenames,
    display_compressed_files_info, set_notification_level, STDIN_MARK, STDOUT_MARK,
};

// ── Post-parse dispatch and cleanup (lz4cli.c lines 704-887) ─────────────────

/// Execute the operation selected by argument parsing.
///
/// Corresponds to the post-argument-parsing section of C `main()` (lz4cli.c lines 704–887).
/// All resources are released automatically via Rust's RAII drop.
///
/// Returns the process exit code (0 = success, non-zero = error).
fn run(args: ParsedArgs) -> i32 {
    // Unpack all relevant fields from ParsedArgs.
    let mut prefs          = args.prefs;
    let mut op_mode        = args.op_mode;
    let c_level            = args.c_level;
    let c_level_last       = args.c_level_last;
    let legacy_format      = args.legacy_format;
    let force_stdout       = args.force_stdout;
    let main_pause         = args.main_pause;
    let mut multiple_inputs = args.multiple_inputs;
    let nb_workers         = args.nb_workers;
    let mut input_filename : Option<String> = args.input_filename;
    let mut output_filename: Option<String> = args.output_filename;
    let dictionary_filename                 = args.dictionary_filename;
    let mut in_file_names  : Vec<String>    = args.in_file_names;
    let block_size         = args.block_size;
    let mut bench_config   = args.bench_config;
    let exe_name           = args.exe_name;

    // feature-gated field
    #[cfg(feature = "recursive")]
    let recursive = args.recursive;

    // Mirrors dynNameSpace in C — keeps the auto-generated output filename alive
    // until end of function (freed automatically on drop).
    let mut _output_filename_storage: Option<String> = None;

    // ── Verbosity info (lz4cli.c lines 704–722) ────────────────────────────
    // Platform compile-time info at high verbosity levels.
    // POSIX_C_SOURCE and similar are not meaningful in Rust; log build type instead.
    lz4::displaylevel!(
        3,
        "*** LZ4 v{} {}-bit {}, by {} ***\n",
        lz4::LZ4_VERSION_STRING,
        (std::mem::size_of::<*const ()>() * 8),
        lz4::cli::constants::IO_MT,
        lz4::cli::constants::AUTHOR
    );

    // ── MT worker count warning (lz4cli.c lines 723–726) ──────────────────
    // #if !LZ4IO_MULTITHREAD: warn when nb_workers > 1 but MT is disabled.
    if !MULTITHREAD && nb_workers > 1 {
        lz4::displaylevel!(
            2,
            "warning: this executable doesn't support multithreading \n"
        );
    }

    // ── Block size info (lz4cli.c lines 727–728) ───────────────────────────
    if op_mode == OpMode::Compress || op_mode == OpMode::Bench {
        lz4::displaylevel!(4, "Blocks size : {} KB\n", block_size >> 10);
    }

    // ── Multiple inputs: set input_filename from first entry (lines 730–738) ─
    if multiple_inputs {
        if let Some(first) = in_file_names.first() {
            input_filename = Some(first.clone());
        }
        // Recursive directory expansion (UTIL_HAS_CREATEFILELIST gate).
        #[cfg(feature = "recursive")]
        if recursive {
            use std::path::Path;
            let paths: Vec<&Path> = in_file_names.iter().map(|s| Path::new(s.as_str())).collect();
            match lz4::util::create_file_list(&paths) {
                Ok(list) => {
                    for (u, p) in list.iter().enumerate() {
                        lz4::displaylevel!(4, "{} {}\n", u, p.display());
                    }
                    in_file_names = list
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                }
                Err(e) => {
                    eprintln!("lz4: {}", e);
                    return 1;
                }
            }
        }
    }

    // ── Dictionary file setup (lz4cli.c lines 741–748) ────────────────────
    if let Some(ref dict) = dictionary_filename {
        if dict.as_str() == STDIN_MARK && std::io::stdin().is_terminal() {
            lz4::displaylevel!(1, "refusing to read from a console\n");
            std::process::exit(1);
        }
        prefs.set_dictionary_filename(Some(dict.as_str()));
    }

    // ── Bench mode dispatch ──────────────────────────────────────────────────
    if op_mode == OpMode::Bench {
        bench_config.set_notification_level(display_level());
        let file_refs: Vec<&str> = in_file_names.iter().map(|s| s.as_str()).collect();
        let result = lz4::bench::bench_files(
            &file_refs,
            c_level,
            c_level_last,
            dictionary_filename.as_deref(),
            &bench_config,
        );
        if main_pause {
            wait_enter();
        }
        return if result.is_ok() { 0 } else { 1 };
    }

    // ── Test mode setup (lz4cli.c lines 758–762) ───────────────────────────
    if op_mode == OpMode::Test {
        prefs.set_test_mode(true);
        output_filename = Some(lz4::io::NUL_MARK.to_owned());
        op_mode = OpMode::Decompress;
    }

    // ── Default input filename to stdin (lz4cli.c lines 764–768) ──────────
    let input_filename: String = input_filename.unwrap_or_else(|| STDIN_MARK.to_owned());

    // ── Refuse stdin from console (lz4cli.c lines 770–774) ────────────────
    if input_filename == STDIN_MARK && std::io::stdin().is_terminal() {
        lz4::displaylevel!(1, "refusing to read from a console\n");
        std::process::exit(1);
    }

    // ── Auto stdout when reading stdin (lz4cli.c lines 776–779) ──────────
    if input_filename == STDIN_MARK && output_filename.is_none() {
        output_filename = Some(STDOUT_MARK.to_owned());
    }

    // ── Auto output filename determination (lz4cli.c lines 781–808) ───────
    // Only when no output_filename is set and not in multiple-input mode.
    if output_filename.is_none() && !multiple_inputs {
        if op_mode == OpMode::Auto {
            op_mode = determine_op_mode(&input_filename);
        }
        if op_mode == OpMode::Compress {
            let out = format!("{}{}", input_filename, LZ4_EXTENSION);
            lz4::displaylevel!(2, "Compressed filename will be : {} \n", out);
            _output_filename_storage = Some(out.clone());
            output_filename = Some(out);
        } else if op_mode == OpMode::Decompress {
            // Strip .lz4 suffix (mirrors C dynNameSpace logic at lines 796–806).
            if let Some(base) = input_filename.strip_suffix(LZ4_EXTENSION) {
                lz4::displaylevel!(2, "Decoding file {} \n", base);
                _output_filename_storage = Some(base.to_owned());
                output_filename = Some(base.to_owned());
            } else {
                lz4::displaylevel!(1, "Cannot determine an output filename \n");
                lz4::cli::help::print_usage(&exe_name);
                std::process::exit(1);
            }
        }
    }

    // ── List mode: add input_filename to file list (lz4cli.c lines 810–813) ─
    if op_mode == OpMode::List {
        if !multiple_inputs {
            in_file_names.push(input_filename.clone());
        }
    } else if !multiple_inputs {
        // C: assert(output_filename != NULL) — already guaranteed by the logic above;
        // the output_filename == None case is handled by the dummy sentinel below.
    }

    // When output_filename is still None (only in multiple-input compress/decompress),
    // substitute the C dummy sentinel (mirrors C line 813).
    let output_filename: String =
        output_filename.unwrap_or_else(|| "*\\dummy^!//".to_owned());

    // ── Refuse console output (lz4cli.c lines 815–820) ────────────────────
    if output_filename == STDOUT_MARK
        && op_mode != OpMode::List
        && std::io::stdout().is_terminal()
        && !force_stdout
    {
        lz4::displaylevel!(1, "refusing to write to console without -c \n");
        std::process::exit(1);
    }

    // ── Display level downgrade (lz4cli.c lines 821–824) ──────────────────
    if output_filename == STDOUT_MARK && display_level() == 2 {
        set_display_level(1);
    }
    if multiple_inputs && display_level() == 2 {
        set_display_level(1);
    }

    // ── Auto-determine mode from extension (lz4cli.c lines 826–829) ───────
    if op_mode == OpMode::Auto {
        op_mode = determine_op_mode(&input_filename);
    }

    // ── Set IO notification level (lz4cli.c lines 831–832) ────────────────
    set_notification_level(display_level() as i32);
    if in_file_names.is_empty() {
        multiple_inputs = false;
    }

    // ── Operation dispatch (lz4cli.c lines 833–887) ────────────────────────
    let operation_result: i32 = if op_mode == OpMode::Decompress {
        // -- Decompress (lz4cli.c lines 833–845) --
        if multiple_inputs {
            let dec_extension: &str = if output_filename == STDOUT_MARK {
                STDOUT_MARK
            } else if output_filename == lz4::io::NUL_MARK {
                lz4::io::NUL_MARK
            } else {
                LZ4_EXTENSION
            };
            let srcs: Vec<&str> = in_file_names.iter().map(|s| s.as_str()).collect();
            match decompress_multiple_filenames(&srcs, dec_extension, &prefs) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        } else {
            match decompress_filename(&input_filename, &output_filename, &prefs) {
                Ok(_) => 0,
                Err(_) => 1,
            }
        }
    } else if op_mode == OpMode::List {
        // -- List (lz4cli.c line 847) --
        let srcs: Vec<&str> = in_file_names.iter().map(|s| s.as_str()).collect();
        match display_compressed_files_info(&srcs) {
            Ok(()) => 0,
            Err(_) => 1,
        }
    } else {
        // -- Compress (default; lz4cli.c lines 848–887) --

        // MT worker count adjustment (#if LZ4IO_MULTITHREAD block, lines 849–866).
        #[cfg(feature = "multithread")]
        {
            let mut nb = nb_workers;
            if nb != 1 {
                if nb == 0 {
                    nb = lz4::io::default_nb_workers() as usize;
                }
                let max = lz4::config::NB_WORKERS_MAX;
                if nb > max {
                    lz4::displaylevel!(
                        3,
                        "Requested {} threads too large => automatically reduced to {} \n",
                        nb,
                        max
                    );
                    nb = max;
                } else {
                    lz4::displaylevel!(3, "Using {} threads for compression \n", nb);
                }
            }
            prefs.set_nb_workers(nb as i32);
        }

        if legacy_format {
            // Legacy LZ4 frame format (lz4cli.c lines 868–877).
            lz4::displaylevel!(3, "! Generating LZ4 Legacy format (deprecated) ! \n");
            if multiple_inputs {
                let leg_ext: &str = if output_filename == STDOUT_MARK {
                    STDOUT_MARK
                } else {
                    LZ4_EXTENSION
                };
                let srcs: Vec<&str> = in_file_names.iter().map(|s| s.as_str()).collect();
                match compress_multiple_filenames_legacy(&srcs, leg_ext, c_level, &prefs) {
                    Ok(()) => 0,
                    Err(_) => 1,
                }
            } else {
                match compress_filename_legacy(&input_filename, &output_filename, c_level, &prefs) {
                    Ok(_) => 0,
                    Err(_) => 1,
                }
            }
        } else {
            // Standard LZ4 frame format (lz4cli.c lines 878–887).
            if multiple_inputs {
                let comp_ext: &str = if output_filename == STDOUT_MARK {
                    STDOUT_MARK
                } else {
                    LZ4_EXTENSION
                };
                let srcs: Vec<&str> = in_file_names.iter().map(|s| s.as_str()).collect();
                match compress_multiple_filenames(&srcs, comp_ext, c_level, &prefs) {
                    Ok(missed) => missed as i32,
                    Err(_) => 1,
                }
            } else {
                match compress_filename(&input_filename, &output_filename, c_level, &prefs) {
                    Ok(_) => 0,
                    Err(_) => 1,
                }
            }
        }
    };

    // ── _cleanup (lz4cli.c lines 888–893) ─────────────────────────────────
    // C: if (main_pause) waitEnter(); free(dynNameSpace); free(fileNamesBuf);
    //    LZ4IO_freePreferences(prefs); free((void*)inFileNames);
    // In Rust all heap allocations are freed automatically by Drop.
    if main_pause {
        wait_enter();
    }

    operation_result
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    // argv[0] → alias detection (lz4cli.c lines 412–439).
    let argv0 = std::env::args().next().unwrap_or_else(|| "lz4".to_owned());
    let init = detect_alias(&argv0);

    // Argument parsing loop (lz4cli.c lines 442–703).
    let args = match parse_args(init) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("lz4: {}", e);
            std::process::exit(1);
        }
    };

    // Help / version flags set exit_early; the caller should exit 0.
    if args.exit_early {
        std::process::exit(0);
    }

    // Post-parse dispatch and cleanup (lz4cli.c lines 704–893).
    let exit_code = run(args);
    std::process::exit(exit_code);
}
