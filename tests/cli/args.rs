// Integration tests for task-034: cli/args.rs — Argument Parsing Loop (Chunk 6)
//
// Verifies parity with lz4cli.c lines 442–703:
//   - Short and long option parsing
//   - Aggregated short flags (-9fv style)
//   - Non-option (positional) argument handling
//   - Legacy lz4c command handling
//   - end-of-options `--` sentinel
//   - Error paths (bad usage)
//   - Integration with CliInit from task-033

use lz4::cli::args::parse_args_from;
use lz4::cli::constants::{display_level, set_display_level, set_lz4c_legacy_commands};
use lz4::cli::init::detect_alias;
use lz4::cli::op_mode::OpMode;
use lz4::hc::types::LZ4HC_CLEVEL_MAX;
use lz4::io::file_io::{NUL_MARK, STDIN_MARK, STDOUT_MARK};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Parse with the plain "lz4" init (no alias)
fn parse(argv: &[&str]) -> lz4::cli::args::ParsedArgs {
    let init = detect_alias("lz4");
    parse_args_from(init, "lz4", &args(argv)).expect("parse should succeed")
}

/// Parse expecting an error
fn parse_err(argv: &[&str]) -> String {
    let init = detect_alias("lz4");
    parse_args_from(init, "lz4", &args(argv))
        .expect_err("expected parse error")
        .to_string()
}

fn reset_globals() {
    set_display_level(2);
    set_lz4c_legacy_commands(false);
}

// ─────────────────────────────────────────────────────────────────────────────
// Empty / no-args — default state (lz4cli.c lines 388–441 initial values)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn no_args_default_op_mode_is_auto() {
    // With no arguments the mode is not changed from init default (OpMode::Auto).
    let p = parse(&[]);
    assert_eq!(p.op_mode, OpMode::Auto);
}

#[test]
fn no_args_c_level_default() {
    // C: cLevel starts as LZ4_CLEVEL_DEFAULT (1 when env var absent)
    std::env::remove_var("LZ4_CLEVEL");
    let p = parse(&[]);
    assert_eq!(p.c_level, 1);
}

#[test]
fn no_args_c_level_last_sentinel() {
    // cLevelLast is initialised to -10000 (a sentinel meaning "not set")
    let p = parse(&[]);
    assert_eq!(p.c_level_last, -10_000);
}

#[test]
fn no_args_legacy_format_false() {
    let p = parse(&[]);
    assert!(!p.legacy_format);
}

#[test]
fn no_args_force_overwrite_false() {
    let p = parse(&[]);
    assert!(!p.force_overwrite);
}

#[test]
fn no_args_main_pause_false() {
    let p = parse(&[]);
    assert!(!p.main_pause);
}

#[test]
fn no_args_multiple_inputs_false() {
    let p = parse(&[]);
    assert!(!p.multiple_inputs);
}

#[test]
fn no_args_exit_early_false() {
    let p = parse(&[]);
    assert!(!p.exit_early);
}

// ─────────────────────────────────────────────────────────────────────────────
// Numeric compression levels (lz4cli.c lines 527–534)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn short_level_0() {
    // -0 is valid (fastest)
    let p = parse(&["-0"]);
    assert_eq!(p.c_level, 0);
}

#[test]
fn short_level_1() {
    let p = parse(&["-1"]);
    assert_eq!(p.c_level, 1);
}

#[test]
fn short_level_9() {
    let p = parse(&["-9"]);
    assert_eq!(p.c_level, 9);
}

#[test]
fn short_level_12() {
    // LZ4HC_CLEVEL_MAX
    let p = parse(&["-12"]);
    assert_eq!(p.c_level, 12);
}

#[test]
fn best_flag_sets_max_level() {
    let p = parse(&["--best"]);
    assert_eq!(p.c_level, LZ4HC_CLEVEL_MAX);
}

#[test]
fn fast_no_arg_sets_minus_1() {
    // --fast without =N defaults to -1 (lz4cli.c lines 501–508)
    let p = parse(&["--fast"]);
    assert_eq!(p.c_level, -1);
}

#[test]
fn fast_equals_1() {
    let p = parse(&["--fast=1"]);
    assert_eq!(p.c_level, -1);
}

#[test]
fn fast_equals_10() {
    let p = parse(&["--fast=10"]);
    assert_eq!(p.c_level, -10);
}

#[test]
fn fast_zero_is_error() {
    // --fast=0 is explicitly rejected (lz4cli.c line 499: assert(fastLevel != 0))
    let e = parse_err(&["--fast=0"]);
    assert!(
        e.contains("bad usage"),
        "expected bad usage error, got: {e}"
    );
}

#[test]
fn fast_with_extra_chars_is_error() {
    let e = parse_err(&["--fast=3x"]);
    assert!(
        e.contains("bad usage"),
        "expected bad usage error, got: {e}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation modes (lz4cli.c lines 461–480, 549–592)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn long_compress_flag() {
    let p = parse(&["--compress"]);
    assert_eq!(p.op_mode, OpMode::Compress);
}

#[test]
fn short_z_compress_flag() {
    let p = parse(&["-z"]);
    assert_eq!(p.op_mode, OpMode::Compress);
}

#[test]
fn long_decompress_flag() {
    let p = parse(&["--decompress"]);
    assert_eq!(p.op_mode, OpMode::Decompress);
}

#[test]
fn long_uncompress_alias() {
    let p = parse(&["--uncompress"]);
    assert_eq!(p.op_mode, OpMode::Decompress);
}

#[test]
fn short_d_decompress_flag() {
    let p = parse(&["-d"]);
    assert_eq!(p.op_mode, OpMode::Decompress);
}

#[test]
fn long_test_flag() {
    let p = parse(&["--test"]);
    assert_eq!(p.op_mode, OpMode::Test);
}

#[test]
fn short_t_test_flag() {
    let p = parse(&["-t"]);
    assert_eq!(p.op_mode, OpMode::Test);
}

#[test]
fn list_mode_enables_multiple_inputs() {
    // --list also enables multiple_inputs (lz4cli.c line 480)
    let p = parse(&["--list"]);
    assert_eq!(p.op_mode, OpMode::List);
    assert!(p.multiple_inputs);
}

#[test]
fn short_b_benchmark_mode() {
    // -b sets op_mode=Bench and multiple_inputs=true (lz4cli.c lines 647–648)
    let p = parse(&["-b"]);
    assert_eq!(p.op_mode, OpMode::Bench);
    assert!(p.multiple_inputs);
}

#[test]
fn decompress_does_not_override_bench_mode() {
    // C: if (mode != om_bench) mode = om_decompress (lz4cli.c line 580)
    let p = parse(&["-b", "-d"]);
    // op_mode stays Bench even when -d is seen afterward
    assert_eq!(p.op_mode, OpMode::Bench);
}

#[test]
fn decompress_sets_bench_config_decode_only() {
    // -d always sets decode_only in bench_config (lz4cli.c line 582)
    let p = parse(&["-d"]);
    assert!(p.bench_config.decode_only);
}

// ─────────────────────────────────────────────────────────────────────────────
// Force stdout / pass-through  (lz4cli.c lines 585–589)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn short_c_force_stdout() {
    // -c → force_stdout=true, output=stdoutmark, pass_through=true
    let p = parse(&["-c"]);
    assert!(p.force_stdout);
    assert_eq!(p.output_filename.as_deref(), Some(STDOUT_MARK));
    assert!(p.prefs.pass_through);
}

#[test]
fn long_stdout_flag() {
    // --stdout (lz4cli.c line 474)
    let p = parse(&["--stdout"]);
    assert!(p.force_stdout);
    assert_eq!(p.output_filename.as_deref(), Some(STDOUT_MARK));
}

#[test]
fn long_to_stdout_alias() {
    // --to-stdout is the same as --stdout (lz4cli.c line 474)
    let p = parse(&["--to-stdout"]);
    assert!(p.force_stdout);
    assert_eq!(p.output_filename.as_deref(), Some(STDOUT_MARK));
}

// ─────────────────────────────────────────────────────────────────────────────
// Force overwrite / keep (lz4cli.c lines 595, 604)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn short_f_force_overwrite() {
    let p = parse(&["-f"]);
    assert!(p.force_overwrite);
    assert!(p.prefs.overwrite);
}

#[test]
fn long_force_sets_prefs_overwrite() {
    let p = parse(&["--force"]);
    assert!(p.prefs.overwrite);
}

#[test]
fn long_no_force_clears_prefs_overwrite() {
    let p = parse(&["--no-force"]);
    assert!(!p.prefs.overwrite);
}

#[test]
fn short_k_keep_src_file() {
    // -k → do not remove source file (lz4cli.c line 604)
    let p = parse(&["-k"]);
    assert!(!p.prefs.remove_src_file);
}

#[test]
fn long_keep_flag() {
    let p = parse(&["--keep"]);
    assert!(!p.prefs.remove_src_file);
}

#[test]
fn long_rm_flag_removes_src() {
    // --rm → remove source file (lz4cli.c: --rm)
    let p = parse(&["--rm"]);
    assert!(p.prefs.remove_src_file);
}

// ─────────────────────────────────────────────────────────────────────────────
// Verbose / quiet (lz4cli.c lines 598–603)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn short_v_increases_display_level() {
    reset_globals();
    let before = display_level();
    let init = detect_alias("lz4");
    let _ = parse_args_from(init, "lz4", &args(&["-v"])).unwrap();
    assert!(display_level() > before);
    set_display_level(before); // restore
}

#[test]
fn long_verbose_increases_display_level() {
    reset_globals();
    let before = display_level();
    let init = detect_alias("lz4");
    let _ = parse_args_from(init, "lz4", &args(&["--verbose"])).unwrap();
    assert!(display_level() > before);
    set_display_level(before);
}

#[test]
fn short_q_decreases_display_level() {
    set_display_level(3);
    let before = display_level();
    let init = detect_alias("lz4");
    let _ = parse_args_from(init, "lz4", &args(&["-q"])).unwrap();
    assert!(display_level() < before);
    set_display_level(2); // restore
}

#[test]
fn long_quiet_decreases_display_level() {
    set_display_level(3);
    let before = display_level();
    let init = detect_alias("lz4");
    let _ = parse_args_from(init, "lz4", &args(&["--quiet"])).unwrap();
    assert!(display_level() < before);
    set_display_level(2);
}

#[test]
fn quiet_at_zero_does_not_underflow() {
    // C: if (displayLevel) displayLevel-- — must not go below 0
    set_display_level(0);
    let init = detect_alias("lz4");
    let _ = parse_args_from(init, "lz4", &args(&["-q"])).unwrap();
    assert_eq!(display_level(), 0);
    set_display_level(2);
}

// ─────────────────────────────────────────────────────────────────────────────
// Version / help → exit_early (lz4cli.c lines 483, 538–540)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn long_version_sets_exit_early() {
    let p = parse(&["--version"]);
    assert!(p.exit_early);
}

#[test]
fn short_v_version_sets_exit_early() {
    let p = parse(&["-V"]);
    assert!(p.exit_early);
}

#[test]
fn long_help_sets_exit_early() {
    let p = parse(&["--help"]);
    assert!(p.exit_early);
}

#[test]
fn short_h_help_sets_exit_early() {
    let p = parse(&["-h"]);
    assert!(p.exit_early);
}

#[test]
fn short_hc_long_help_sets_exit_early() {
    // -H is long help (lz4cli.c line 540)
    let p = parse(&["-H"]);
    assert!(p.exit_early);
}

#[test]
fn exit_early_stops_further_parsing() {
    // Flags after --version are not processed (break out of loop)
    let p = parse(&["--version", "--compress"]);
    assert!(p.exit_early);
    // op_mode should remain Auto (not set to Compress) because we broke out.
    assert_eq!(p.op_mode, OpMode::Auto);
}

// ─────────────────────────────────────────────────────────────────────────────
// Frame / checksum flags (lz4cli.c lines 469–477)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn long_frame_crc_enables_stream_checksum() {
    let p = parse(&["--frame-crc"]);
    assert!(p.prefs.stream_checksum);
    assert!(!p.bench_config.skip_checksums);
}

#[test]
fn long_no_frame_crc_disables_stream_checksum() {
    let p = parse(&["--no-frame-crc"]);
    assert!(!p.prefs.stream_checksum);
    assert!(p.bench_config.skip_checksums);
}

#[test]
fn long_no_crc_disables_both_checksums() {
    // --no-crc → stream_checksum=false AND block_checksum=false (lz4cli.c line 472)
    let p = parse(&["--no-crc"]);
    assert!(!p.prefs.stream_checksum);
    assert!(!p.prefs.block_checksum);
    assert!(p.bench_config.skip_checksums);
}

#[test]
fn long_content_size_flag() {
    let p = parse(&["--content-size"]);
    assert!(p.prefs.content_size_flag);
}

#[test]
fn long_no_content_size_flag() {
    let p = parse(&["--no-content-size"]);
    assert!(!p.prefs.content_size_flag);
}

#[test]
fn long_favor_dec_speed() {
    let p = parse(&["--favor-decSpeed"]);
    assert!(p.prefs.favor_dec_speed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Sparse file support (lz4cli.c lines 482–486)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn long_sparse_sets_sparse_2() {
    // --sparse → LZ4IO_setSparseFile(prefs, 2) — force sparse on
    let p = parse(&["--sparse"]);
    assert_eq!(p.prefs.sparse_file_support, 2);
}

#[test]
fn long_no_sparse_sets_sparse_0() {
    // --no-sparse → LZ4IO_setSparseFile(prefs, 0) — sparse off
    let p = parse(&["--no-sparse"]);
    assert_eq!(p.prefs.sparse_file_support, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Block properties -B option (lz4cli.c lines 607–644)
// ─────────────────────────────────────────────────────────────────────────────

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
fn block_linked_mode() {
    // -BD → BlockMode::Linked (not block independent)
    let p = parse(&["-BD"]);
    assert!(!p.prefs.block_independence);
}

#[test]
fn block_independent_mode() {
    // -BI → BlockMode::Independent
    let p = parse(&["-BI"]);
    assert!(p.prefs.block_independence);
}

#[test]
fn block_checksum_flag() {
    // -BX → block checksum on
    let p = parse(&["-BX"]);
    assert!(p.prefs.block_checksum);
}

#[test]
fn block_combined_flags() {
    // -BDIX → linked + independent (last wins per C loop) + checksum
    let p = parse(&["-BDX"]);
    // BD: linked; BX: checksum
    assert!(!p.prefs.block_independence);
    assert!(p.prefs.block_checksum);
}

#[test]
fn block_size_id_under_4_is_error() {
    // B < 4 is explicitly rejected (lz4cli.c line 622)
    let e = parse_err(&["-B3"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

#[test]
fn block_size_id_8_to_31_is_error() {
    // 8 ≤ B < 32 is rejected when B > 7 (must be ≥ 32 bytes when treated as raw size)
    let e = parse_err(&["-B31"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

#[test]
fn block_size_raw_32_is_accepted() {
    // B=32 bytes is the minimum accepted raw block size (lz4cli.c line 634)
    let p = parse(&["-B32"]);
    assert_eq!(p.block_size, 32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Thread count (lz4cli.c lines 552–557)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn threads_long_equals_syntax() {
    let p = parse(&["--threads=4"]);
    assert_eq!(p.nb_workers, 4);
}

#[test]
fn threads_long_space_syntax() {
    let p = parse(&["--threads", "8"]);
    assert_eq!(p.nb_workers, 8);
}

#[test]
fn threads_short_inline() {
    // -T4 → nb_workers=4
    let p = parse(&["-T4"]);
    assert_eq!(p.nb_workers, 4);
}

#[test]
fn threads_short_separate() {
    // -T 2 → nb_workers=2 (separate argument)
    let p = parse(&["-T", "2"]);
    assert_eq!(p.nb_workers, 2);
}

#[test]
fn threads_long_non_numeric_is_error() {
    let e = parse_err(&["--threads=abc"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

#[test]
fn threads_short_missing_value_is_error() {
    // -T with no following argument
    let e = parse_err(&["-T"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Dictionary file (lz4cli.c lines 559–573)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn dictionary_inline_syntax() {
    let p = parse(&["-Dpath/to/dict"]);
    assert_eq!(p.dictionary_filename.as_deref(), Some("path/to/dict"));
}

#[test]
fn dictionary_separate_syntax() {
    let p = parse(&["-D", "path/to/dict"]);
    assert_eq!(p.dictionary_filename.as_deref(), Some("path/to/dict"));
}

#[test]
fn dictionary_missing_path_is_error() {
    // -D with no following argument
    let e = parse_err(&["-D"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Legacy format -l (lz4cli.c line 576)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn legacy_format_flag() {
    // -l → legacy_format=true, block_size=8MiB (LEGACY_BLOCK_SIZE)
    let p = parse(&["-l"]);
    assert!(p.legacy_format);
    assert_eq!(p.block_size, 8 * (1 << 20));
    assert_eq!(p.prefs.block_size, 8 * (1 << 20));
}

// ─────────────────────────────────────────────────────────────────────────────
// Benchmark-related flags
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn bench_e_level_range() {
    // -e<N> → c_level_last=N (lz4cli.c lines 542–546)
    let p = parse(&["-e9"]);
    assert_eq!(p.c_level_last, 9);
}

#[test]
fn bench_e_without_digit_is_error() {
    let e = parse_err(&["-e"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

#[test]
fn bench_separately_flag() {
    // -S → bench_separately (lz4cli.c line 651)
    let p = parse(&["-S"]);
    assert!(p.bench_config.bench_separately);
}

#[test]
fn bench_iterations_flag() {
    // -i<N> → bench_config.nb_seconds (lz4cli.c lines 664–671)
    let p = parse(&["-i5"]);
    assert_eq!(p.bench_config.nb_seconds, 5);
}

#[test]
fn bench_iterations_without_digit_is_error() {
    let e = parse_err(&["-i"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Pause (hidden -p flag, lz4cli.c line 675)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn main_pause_flag() {
    let p = parse(&["-p"]);
    assert!(p.main_pause);
}

// ─────────────────────────────────────────────────────────────────────────────
// Multiple inputs / recursive  (lz4cli.c lines 654–660)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn short_m_enables_multiple_inputs() {
    let p = parse(&["-m"]);
    assert!(p.multiple_inputs);
}

#[test]
fn long_multiple_flag() {
    let p = parse(&["--multiple"]);
    assert!(p.multiple_inputs);
}

#[test]
fn short_r_enables_multiple_inputs() {
    // -r also sets multiple_inputs (lz4cli.c line 658: fall-through to -m)
    let p = parse(&["-r"]);
    assert!(p.multiple_inputs);
}

#[test]
fn multiple_inputs_collects_filenames() {
    let p = parse(&["-m", "a.txt", "b.txt", "c.txt"]);
    assert_eq!(p.in_file_names, vec!["a.txt", "b.txt", "c.txt"]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Positional (non-option) argument handling (lz4cli.c lines 698–702)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn first_positional_is_input_filename() {
    let p = parse(&["input.lz4"]);
    assert_eq!(p.input_filename.as_deref(), Some("input.lz4"));
}

#[test]
fn second_positional_is_output_filename() {
    let p = parse(&["input.lz4", "output.txt"]);
    assert_eq!(p.input_filename.as_deref(), Some("input.lz4"));
    assert_eq!(p.output_filename.as_deref(), Some("output.txt"));
}

#[test]
fn null_output_name_translated_to_nul_mark() {
    // "null" output path → NUL_MARK (lz4cli.c: if (!strcmp(output_filename, nullOutput)))
    let p = parse(&["input.lz4", "null"]);
    assert_eq!(p.output_filename.as_deref(), Some(NUL_MARK));
}

#[test]
fn third_positional_without_force_is_error() {
    // Three non-option args without -f is a fatal error (lz4cli.c line 700)
    let e = parse_err(&["in.lz4", "out.txt", "extra.txt"]);
    assert!(
        e.contains("bad usage") || e.contains("won't be used"),
        "expected error message, got: {e}"
    );
}

#[test]
fn third_positional_with_force_is_warning_not_error() {
    // With -f the third arg is silently dropped (lz4cli.c line 699: only a warning)
    let p = parse(&["-f", "in.lz4", "out.txt", "extra.txt"]);
    assert_eq!(p.input_filename.as_deref(), Some("in.lz4"));
    assert_eq!(p.output_filename.as_deref(), Some("out.txt"));
}

#[test]
fn dash_alone_sets_stdin_mark() {
    // `-` alone → STDIN_MARK for input (lz4cli.c: stdinmark)
    let p = parse(&["-"]);
    assert_eq!(p.input_filename.as_deref(), Some(STDIN_MARK));
}

#[test]
fn dash_as_second_arg_sets_stdout_mark() {
    // Second `-` → STDOUT_MARK for output (lz4cli.c: stdoutmark)
    let p = parse(&["input.lz4", "-"]);
    assert_eq!(p.output_filename.as_deref(), Some(STDOUT_MARK));
}

// ─────────────────────────────────────────────────────────────────────────────
// End-of-options `--` sentinel (lz4cli.c logic: all_arguments_are_files)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn end_of_options_sentinel_treats_rest_as_files() {
    // `--` followed by `-not-a-flag` → the dash-arg is treated as a file name
    let p = parse(&["--", "-not-a-flag"]);
    assert_eq!(p.input_filename.as_deref(), Some("-not-a-flag"));
}

#[test]
fn end_of_options_sentinel_with_multiple_inputs() {
    let p = parse(&["-m", "--", "a.txt", "--not-flag"]);
    assert!(p.in_file_names.contains(&"--not-flag".to_string()));
}

// ─────────────────────────────────────────────────────────────────────────────
// Aggregated short flags  (lz4cli.c: outer while → inner while pointer++/argv)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn aggregated_9fv() {
    // `-9fv` — compression level 9, force, verbose
    reset_globals();
    let before = display_level();
    let init = detect_alias("lz4");
    let p = parse_args_from(init, "lz4", &args(&["-9fv"])).unwrap();
    assert_eq!(p.c_level, 9);
    assert!(p.force_overwrite);
    assert!(display_level() > before);
    set_display_level(before);
}

#[test]
fn aggregated_zfk() {
    // `-zfk` — compress, force, keep
    let p = parse(&["-zfk"]);
    assert_eq!(p.op_mode, OpMode::Compress);
    assert!(p.force_overwrite);
    assert!(!p.prefs.remove_src_file);
}

#[test]
fn aggregated_dt() {
    // `-dt` is unrelated flags: decompress then test — last wins for op_mode
    let p = parse(&["-dt"]);
    // -d sets Decompress, -t sets Test; -t comes after
    assert_eq!(p.op_mode, OpMode::Test);
}

// ─────────────────────────────────────────────────────────────────────────────
// Legacy lz4c commands (lz4cli.c lines 519–526) — requires lz4c_legacy=true
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4c_legacy_c0_sets_level_0() {
    let init = detect_alias("lz4c");
    let p = parse_args_from(init, "lz4c", &args(&["-c0"])).unwrap();
    assert_eq!(p.c_level, 0);
}

#[test]
fn lz4c_legacy_c1_sets_level_9() {
    let init = detect_alias("lz4c");
    let p = parse_args_from(init, "lz4c", &args(&["-c1"])).unwrap();
    assert_eq!(p.c_level, 9);
}

#[test]
fn lz4c_legacy_c2_sets_level_12() {
    let init = detect_alias("lz4c");
    let p = parse_args_from(init, "lz4c", &args(&["-c2"])).unwrap();
    assert_eq!(p.c_level, 12);
}

#[test]
fn lz4c_legacy_hc_sets_level_12() {
    let init = detect_alias("lz4c");
    let p = parse_args_from(init, "lz4c", &args(&["-hc"])).unwrap();
    assert_eq!(p.c_level, 12);
}

#[test]
fn lz4c_legacy_y_enables_overwrite() {
    let init = detect_alias("lz4c");
    let p = parse_args_from(init, "lz4c", &args(&["-y"])).unwrap();
    assert!(p.prefs.overwrite);
}

#[test]
fn non_lz4c_binary_ignores_c0_legacy_flag() {
    // -c0 for a plain lz4 binary: 'c' is force-stdout, '0' is level 0
    let p = parse(&["-c0"]);
    assert!(p.force_stdout); // -c handled
    assert_eq!(p.c_level, 0); // 0 handled as numeric level
}

// ─────────────────────────────────────────────────────────────────────────────
// Error paths — unknown / malformed options
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn unknown_long_option_is_error() {
    let e = parse_err(&["--not-a-real-option"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

#[test]
fn unknown_short_option_is_error() {
    // 'X' (uppercase) is not a recognised short flag
    let e = parse_err(&["-X"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

#[test]
fn threads_long_with_option_value_is_error() {
    // --threads followed by an option (not a number) is an error
    let e = parse_err(&["--threads", "--compress"]);
    assert!(e.contains("bad usage"), "expected bad usage, got: {e}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration: lz4cat alias + argument parsing  (parity concern from task-033)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lz4cat_init_preserves_force_stdout_through_parse() {
    // lz4cat sets force_stdout=true in init; parse_args_from must preserve it
    let init = detect_alias("lz4cat");
    let p = parse_args_from(init, "lz4cat", &args(&[])).unwrap();
    assert!(p.force_stdout);
    assert_eq!(p.output_filename.as_deref(), Some(STDOUT_MARK));
}

#[test]
fn lz4cat_init_preserves_op_mode_decompress() {
    let init = detect_alias("lz4cat");
    let p = parse_args_from(init, "lz4cat", &args(&[])).unwrap();
    assert_eq!(p.op_mode, OpMode::Decompress);
}

#[test]
fn lz4cat_init_preserves_multiple_inputs() {
    let init = detect_alias("lz4cat");
    let p = parse_args_from(init, "lz4cat", &args(&["file1.lz4", "file2.lz4"])).unwrap();
    assert!(p.multiple_inputs);
    assert_eq!(p.in_file_names, vec!["file1.lz4", "file2.lz4"]);
}

#[test]
fn unlz4_init_op_mode_decompress_preserved() {
    let init = detect_alias("unlz4");
    let p = parse_args_from(init, "unlz4", &args(&[])).unwrap();
    assert_eq!(p.op_mode, OpMode::Decompress);
}

// ─────────────────────────────────────────────────────────────────────────────
// exe_name propagated to ParsedArgs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn exe_name_preserved_in_parsed_args() {
    let init = detect_alias("lz4");
    let p = parse_args_from(init, "my-lz4", &args(&[])).unwrap();
    assert_eq!(p.exe_name, "my-lz4");
}

// ─────────────────────────────────────────────────────────────────────────────
// Empty string argument is silently skipped (lz4cli.c: if (!argument[0]) continue)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn empty_string_arg_is_skipped() {
    // An empty argv element must not panic or error; it is skipped per C line ~449
    let p = parse(&[""]);
    assert_eq!(p.op_mode, OpMode::Auto);
    assert!(p.input_filename.is_none());
}
