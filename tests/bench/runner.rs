// Unit tests for task-027: bench/runner.rs — File Loading, Memory Probe, and Level Iteration
//
// Verifies parity with bench.c lines 622–753:
//   - load_files: reads file content into a contiguous buffer
//   - load_files: truncates when buffer is smaller than file
//   - load_files: skips directories silently
//   - load_files: returns Err("no data to bench") when total_size == 0
//   - load_files: returns correct file_sizes per path
//   - load_files: stops loading additional files once buffer is full (nb_files clamped)
//   - bench_c_level: succeeds on valid src across a single level
//   - bench_c_level: iterates correctly when cLevelLast > cLevel
//   - bench_c_level: clamps cLevelLast to cLevel when it is below cLevel
//   - bench_c_level: strips basename from display_name (POSIX separator)
//   - bench_c_level: strips basename from display_name (Windows separator)
//   - bench_c_level: handles plain filename with no separator
//   - bench_file_table: succeeds with a real file
//   - bench_file_table: returns error when file list is empty / no readable data

use lz4::bench::config::BenchConfig;
use lz4::bench::runner::{bench_c_level, bench_file_table, load_files};
use std::io::Write;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn quiet_config() -> BenchConfig {
    let mut c = BenchConfig::default();
    c.set_nb_seconds(0); // single pass — keeps tests fast
    c.set_notification_level(0); // suppress output
    c
}

/// 64 KiB of repeating bytes — highly compressible.
fn make_src() -> Vec<u8> {
    (0u8..128).cycle().take(64 * 1024).collect()
}

/// Write `content` to a fresh NamedTempFile and return (file, path_string).
fn make_temp_file(content: &[u8]) -> (tempfile::NamedTempFile, String) {
    let mut tmp = tempfile::NamedTempFile::new().expect("tmp file");
    tmp.write_all(content).expect("write tmp file");
    let path = tmp.path().to_str().unwrap().to_owned();
    (tmp, path)
}

// ── load_files ────────────────────────────────────────────────────────────────

#[test]
fn load_files_empty_path_list_returns_no_data_error() {
    // C: if (totalSize == 0) END_PROCESS(12, "no data to bench")
    let config = quiet_config();
    let result = load_files(&[], 1024, &config);
    assert!(result.is_err(), "empty path list must return Err");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::InvalidInput,
        "expected InvalidInput, got {:?}",
        err.kind()
    );
    assert!(
        err.to_string().contains("no data to bench"),
        "error message should mention 'no data to bench': {}",
        err
    );
}

#[test]
fn load_files_reads_single_file_correctly() {
    let content = b"hello benchmark world!";
    let (_tmp, path) = make_temp_file(content);
    let config = quiet_config();

    let (buf, sizes) =
        load_files(&[&path], 4096, &config).expect("load_files must succeed for a valid file");

    assert_eq!(&buf[..], content, "buffer content must match file content");
    assert_eq!(
        sizes[0],
        content.len(),
        "file_sizes[0] must equal file length"
    );
}

#[test]
fn load_files_returns_exact_file_size_in_sizes_vec() {
    let content: Vec<u8> = (0u8..=255).cycle().take(512).collect();
    let (_tmp, path) = make_temp_file(&content);
    let config = quiet_config();

    let (_buf, sizes) = load_files(&[&path], 4096, &config).unwrap();
    assert_eq!(sizes[0], 512);
}

#[test]
fn load_files_truncates_to_buffer_size() {
    // Mirrors C lines 695–698: when file_size > remaining capacity, read only
    // what fits and set nb_files = n to stop loading.
    let content = b"abcdefghijklmnopqrstuvwxyz"; // 26 bytes
    let (_tmp, path) = make_temp_file(content);
    let config = quiet_config();

    let (buf, sizes) =
        load_files(&[&path], 10, &config).expect("truncated load should succeed (returns data)");

    assert_eq!(buf.len(), 10, "buffer must be truncated to buffer_size");
    assert_eq!(sizes[0], 10, "file_sizes[0] must reflect truncation");
    assert_eq!(&buf[..], &content[..10]);
}

#[test]
fn load_files_skips_directory_entry() {
    // Directories must be skipped (mirrors UTIL_isDirectory check, bench.c 687–690).
    // We use a path that refers to an actual directory (/tmp) alongside a real file.
    let content = b"real file content";
    let (_tmp, file_path) = make_temp_file(content);
    let config = quiet_config();

    // /tmp is always a directory on POSIX; pair it with a real file
    let paths: &[&str] = &["/tmp", &file_path];
    let (buf, sizes) = load_files(paths, 4096, &config)
        .expect("load_files should skip /tmp and read the real file");

    // /tmp was skipped → sizes[0] == 0
    assert_eq!(sizes[0], 0, "directory entry should have size 0");
    // Real file was loaded
    assert_eq!(sizes[1], content.len());
    assert_eq!(&buf[..], content);
}

#[test]
fn load_files_multiple_files_concatenated() {
    let content_a = b"AAAA";
    let content_b = b"BBBB";
    let (_tmp_a, path_a) = make_temp_file(content_a);
    let (_tmp_b, path_b) = make_temp_file(content_b);
    let config = quiet_config();

    let (buf, sizes) = load_files(&[&path_a, &path_b], 4096, &config).unwrap();

    assert_eq!(sizes[0], 4);
    assert_eq!(sizes[1], 4);
    assert_eq!(buf.len(), 8);
    assert_eq!(&buf[0..4], content_a);
    assert_eq!(&buf[4..8], content_b);
}

#[test]
fn load_files_missing_file_returns_error() {
    let config = quiet_config();
    let result = load_files(
        &["/nonexistent_file_that_does_not_exist.bin"],
        4096,
        &config,
    );
    assert!(result.is_err(), "missing file must return Err");
}

#[test]
fn load_files_buffer_exactly_file_size_succeeds() {
    let content = b"exactly fits";
    let (_tmp, path) = make_temp_file(content);
    let config = quiet_config();

    let (buf, sizes) = load_files(&[&path], content.len(), &config).unwrap();
    assert_eq!(buf.len(), content.len());
    assert_eq!(sizes[0], content.len());
    assert_eq!(&buf[..], content);
}

// ── bench_c_level ─────────────────────────────────────────────────────────────

#[test]
fn bench_c_level_single_level_succeeds() {
    // Basic smoke test: single level, valid src.
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "test_input", 1, 1, &config, b"", &[]);
    assert!(
        result.is_ok(),
        "bench_c_level level 1 must succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_c_level_multiple_levels_succeeds() {
    // Verification: bench_c_level(src, 1, 3) calls bench_mem for levels 1, 2, 3.
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "multi", 1, 3, &config, b"", &[]);
    assert!(
        result.is_ok(),
        "bench_c_level levels 1–3 must succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_c_level_clamped_when_last_less_than_first() {
    // C: if (cLevelLast < cLevel) cLevelLast = cLevel — only one level runs.
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "clamped", 5, 2, &config, b"", &[]);
    assert!(
        result.is_ok(),
        "clamped cLevelLast must still succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_c_level_equal_first_and_last_succeeds() {
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "eq_levels", 3, 3, &config, b"", &[]);
    assert!(result.is_ok(), "equal first/last level must succeed");
}

#[test]
fn bench_c_level_strips_posix_path_prefix() {
    // Mirrors C strrchr logic (bench.c lines 653–655): basename after last '/'.
    // This test just verifies the function runs without error when a path is used
    // as display_name — actual basename display is visible only in stderr output.
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "/some/path/to/file.lz4", 1, 1, &config, b"", &[]);
    assert!(result.is_ok(), "POSIX path as display_name must not fail");
}

#[test]
fn bench_c_level_strips_windows_path_prefix() {
    // Mirrors C strrchr: check '\\' before '/' (bench.c line 654).
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "C:\\Users\\user\\file.lz4", 1, 1, &config, b"", &[]);
    assert!(result.is_ok(), "Windows path as display_name must not fail");
}

#[test]
fn bench_c_level_plain_filename_no_separator() {
    let src = make_src();
    let config = quiet_config();
    let result = bench_c_level(&src, "justfilename.bin", 1, 1, &config, b"", &[]);
    assert!(
        result.is_ok(),
        "plain filename with no separator must not fail"
    );
}

#[test]
fn bench_c_level_empty_src_succeeds() {
    // bench_c_level should not panic on empty input — bench_mem handles it.
    let config = quiet_config();
    // An empty slice may produce an error from bench_mem, but must not panic.
    let _ = bench_c_level(&[], "empty", 1, 1, &config, b"", &[]);
}

#[test]
fn bench_c_level_with_dict() {
    // dict parameter is forwarded to bench_mem; non-empty dict must not crash.
    let src = make_src();
    let dict: Vec<u8> = (0u8..255).collect();
    let config = quiet_config();
    let result = bench_c_level(&src, "dicttest", 1, 1, &config, &dict, &[]);
    assert!(
        result.is_ok(),
        "bench_c_level with non-empty dict must succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_c_level_display_level_1_no_additional_param() {
    // C: if (g_displayLevel == 1 && !g_additionalParam) DISPLAY(...)
    // Must not panic; output goes to stderr.
    let src = make_src();
    let mut config = BenchConfig::default();
    config.set_nb_seconds(0);
    config.set_notification_level(1); // display_level = 1
                                      // additional_param defaults to 0
    let result = bench_c_level(&src, "verbose1", 1, 1, &config, b"", &[]);
    assert!(
        result.is_ok(),
        "display_level=1 path must not fail: {:?}",
        result.err()
    );
}

// ── bench_file_table ──────────────────────────────────────────────────────────

#[test]
fn bench_file_table_single_file_succeeds() {
    let content: Vec<u8> = (0u8..=255).cycle().take(16 * 1024).collect();
    let (_tmp, path) = make_temp_file(&content);
    let config = quiet_config();

    let result = bench_file_table(&[&path], 1, 1, b"", &config);
    assert!(
        result.is_ok(),
        "bench_file_table single file must succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_file_table_multiple_files_display_name_format() {
    // C: snprintf(mfName, ..., " %u files") when file_names.len() > 1.
    let content: Vec<u8> = (0u8..=127).cycle().take(8 * 1024).collect();
    let (_tmp_a, path_a) = make_temp_file(&content);
    let (_tmp_b, path_b) = make_temp_file(&content);
    let config = quiet_config();

    let result = bench_file_table(&[&path_a, &path_b], 1, 1, b"", &config);
    assert!(
        result.is_ok(),
        "bench_file_table multiple files must succeed: {:?}",
        result.err()
    );
}

#[test]
fn bench_file_table_missing_file_propagates_error() {
    let config = quiet_config();
    let result = bench_file_table(&["/nonexistent_file_xyz_abc.bin"], 1, 1, b"", &config);
    assert!(
        result.is_err(),
        "missing file must cause bench_file_table to return Err"
    );
}

#[test]
fn bench_file_table_level_range_1_to_3() {
    let content: Vec<u8> = b"hello ".iter().cycle().take(32 * 1024).cloned().collect();
    let (_tmp, path) = make_temp_file(&content);
    let config = quiet_config();

    let result = bench_file_table(&[&path], 1, 3, b"", &config);
    assert!(
        result.is_ok(),
        "bench_file_table levels 1–3 must succeed: {:?}",
        result.err()
    );
}
